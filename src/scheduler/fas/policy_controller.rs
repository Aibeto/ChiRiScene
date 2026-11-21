//! policy_controller.rs: [struct] [init] [apply] [reset]

use crate::fas_types::ClusterProfile;
use crate::fluent_args;
use crate::i18n::t_with_args;
use crate::utils::FastWriter;
use log::{debug, warn};
use std::fs;
use std::time::{Duration, Instant};

/// 频率校验的最小等待：内核调频异步，过早读 scaling_cur_freq 得到的是旧档
const VERIFY_SETTLE: Duration = Duration::from_millis(200);

/// QoS 钳制长窗口：qos_clamped 持续超过该时长才打 warn
/// thermal cdev 降频恢复为秒级~分钟级，300s 已覆盖绝大多数单次热事件，超过视为 thermal 近乎永久钳死
const QOS_CLAMP_WARN_WINDOW: Duration = Duration::from_secs(300);

// [struct] 单 policy 频率控制器状态
pub struct PolicyController {
    pub max_writer: FastWriter,
    pub min_writer: FastWriter,
    pub available_freqs: Vec<u32>,
    cached_ratios: Vec<f32>,
    /// 最后一次 min/max 均写成功的锁频目标；写失败/半写时保持不变，下一 tick 按原目标重试
    pub current_freq: u32,
    pub policy_id: usize,
    pub(super) cpu_ids: Vec<usize>,
    pub cluster_profile: ClusterProfile,
    pub freq_hold_frames: u32,
    pub freq_min: f32,
    pub freq_max: f32,

    /// 上一次成功写入、尚待校验的目标；写失败时清空（半写后的读回不再代表上次成功值）
    verify_freq: Option<u32>,
    verify_timer: Instant,
    /// 目标值最后一次变化时刻，用于等待目标稳定后再校验；None = 尚未写过
    verify_at: Option<Instant>,
    /// 写频后校验间隔（来自 FasRulesConfig::verify_freq_interval_secs）
    verify_interval: Duration,

    /// QoS 钳制状态：读回 scaling_max_freq < 上次写入值时进入
    /// 写 scaling_max_freq 只是追加一条 FREQ_QOS_MAX 请求（thermal cooling/msm_performance/input-boost 各持一条，聚合取 min），热压期读回偏小是合法态不是篡改，重写改不了聚合 min 只会白写
    qos_clamped: bool,
    /// 进入钳制时刻，用于长窗口告警与退出时报告持续时长
    qos_clamped_since: Option<Instant>,
    /// 长窗口告警只打一次的标记，退出钳制时复位
    qos_clamp_warned: bool,

    /// 校验读回失败只报一次的标记：scaling_max_freq 读不到时写入是否生效不可判定
    /// 校验链路断供但对外表现与正常态同形，故进失效态报一次，读回恢复时复位允许再报
    verify_read_failed: bool,

    pub ignore_write: bool,

    /// 接管前该 policy 的原始 governor（load_policies 快照）
    /// FAS 接管期间改写为 performance 配合 min=max 锁频，退出必须恢复，否则性能模式泄漏到 CLG/akmode/系统调频
    orig_governor: Option<String>,
}

impl PolicyController {
    // [init]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_writer: FastWriter,
        min_writer: FastWriter,
        available_freqs: Vec<u32>,
        policy_id: usize,
        cluster_profile: ClusterProfile,
        current_freq: u32,
        orig_governor: Option<String>,
        verify_interval_secs: u32,
    ) -> Self {
        let freq_min = *available_freqs.first().unwrap_or(&0) as f32;
        let freq_max = *available_freqs.last().unwrap_or(&1) as f32;
        let range = (freq_max - freq_min).max(1.0);
        let cached_ratios: Vec<f32> = available_freqs
            .iter()
            .map(|&f| (f as f32 - freq_min) / range)
            .collect();
        Self {
            max_writer,
            min_writer,
            available_freqs,
            cached_ratios,
            current_freq,
            policy_id,
            cpu_ids: fs::read_to_string(format!(
                "/sys/devices/system/cpu/cpufreq/policy{policy_id}/related_cpus"
            ))
            .or_else(|_| {
                fs::read_to_string(format!(
                    "/sys/devices/system/cpu/cpufreq/policy{policy_id}/affected_cpus"
                ))
            })
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|cpu| cpu.parse().ok())
            .collect(),
            cluster_profile,
            freq_hold_frames: 0,
            freq_min,
            freq_max,
            verify_freq: None,
            verify_timer: Instant::now(),
            verify_at: None,
            verify_interval: Duration::from_secs(verify_interval_secs.max(1) as u64),
            qos_clamped: false,
            qos_clamped_since: None,
            qos_clamp_warned: false,
            verify_read_failed: false,
            ignore_write: false,
            orig_governor,
        }
    }

    pub fn find_nearest_freq(&self, target_ratio: f32) -> u32 {
        let idx = self.cached_ratios.partition_point(|&r| r < target_ratio);
        if idx == 0 {
            self.available_freqs[0]
        } else if idx >= self.available_freqs.len() {
            *self.available_freqs.last().unwrap()
        } else {
            let lo = idx - 1;
            let hi = idx;
            if (self.cached_ratios[hi] - target_ratio).abs()
                < (self.cached_ratios[lo] - target_ratio).abs()
            {
                self.available_freqs[hi]
            } else {
                self.available_freqs[lo]
            }
        }
    }

    pub fn current_ratio(&self) -> f32 {
        (self.current_freq as f32 - self.freq_min) / (self.freq_max - self.freq_min).max(1.0)
    }

    // [apply]
    /// 锁频写入 (min=max)，用于关键 cluster
    ///
    /// 只有 min/max 两侧都写入成功才推进 current_freq/hold/verify；任一写失败（含 raise/lower
    /// 半写）都保留上次成功值，下一 tick 因目标不等而重试，且不保留本次 verify 目标。
    pub fn apply_freq_locked(&mut self, target_freq: u32) {
        if self.ignore_write {
            return;
        }
        // 必须在本次写入前校验上一次写入：内核调频异步，写完立刻读 scaling_cur_freq 是旧档会假性判成被改写；且要求目标已稳定 ≥ VERIFY_SETTLE，频繁改频期间跳过
        self.verify_prev_freq();
        let (max_ok, min_ok) = if target_freq >= self.current_freq {
            let max_ok = self.max_writer.write_value_force(target_freq);
            let min_ok = self.min_writer.write_value_force(target_freq);
            (max_ok, min_ok)
        } else {
            let min_ok = self.min_writer.write_value_force(target_freq);
            let max_ok = self.max_writer.write_value_force(target_freq);
            (max_ok, min_ok)
        };
        if !max_ok || !min_ok {
            // [write-failed] min/max 任一写失败或半写都不算命令已应用：保留 current_freq 让下一 tick
            // 因目标不等而重试；清空本次 verify 目标，半写后的读回不能代表上次成功写入值，
            // 否则 verify 会把写失败误判成 QoS 钳制/篡改。
            self.freq_hold_frames = 0;
            self.verify_freq = None;
            self.verify_at = None;
            return;
        }
        let changed = target_freq != self.current_freq;
        self.current_freq = target_freq;
        self.freq_hold_frames = 2;
        self.verify_freq = Some(target_freq);
        // 只在目标变化时重启稳定计时
        if changed || self.verify_at.is_none() {
            self.verify_at = Some(Instant::now());
        }
    }

    /// 校验上一次写入的目标频率：要求目标稳定 ≥ [`VERIFY_SETTLE`] 且距上次校验 ≥ verify_freq_interval_secs（默认 1s）
    /// 按读回 scaling_max_freq（FREQ_QOS_MAX 聚合结果）三级判定：== 写入值退出钳制；< 写入值属 QoS 钳制不重写不告警，超 [`QOS_CLAMP_WARN_WINDOW`] 才 warn；> 写入值属锁频被改写，告警 + 重写收敛
    /// TODO: 只校验 scaling_max_freq 不校验 scaling_min，写失败与 QoS 钳制在读回偏低时不可区分，按合法钳制处理
    fn verify_prev_freq(&mut self) {
        let (Some(expected), Some(at)) = (self.verify_freq, self.verify_at) else {
            return;
        };
        if at.elapsed() < VERIFY_SETTLE || self.verify_timer.elapsed() < self.verify_interval {
            return;
        }
        self.verify_timer = Instant::now();
        let Some(actual) = self.read_scaling_max_freq() else {
            // [verify-read-failed] 读回失败 = 写入是否生效不可判定，校验链路断供但对外与正常态同形
            // 进失效态报一次，读回恢复时复位允许再报
            if !self.verify_read_failed {
                self.verify_read_failed = true;
                warn!(
                    "{}",
                    t_with_args(
                        "fas-verify-read-failed",
                        &fluent_args!("pid" => self.policy_id.to_string())
                    )
                );
            }
            return;
        };
        if self.verify_read_failed {
            self.verify_read_failed = false;
            warn!(
                "{}",
                t_with_args(
                    "fas-verify-read-restored",
                    &fluent_args!("pid" => self.policy_id.to_string())
                )
            );
        }
        if actual == expected {
            // QoS 钳制解除：读回恢复 == 写入值
            if self.qos_clamped {
                let held = self
                    .qos_clamped_since
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                debug!(
                    "{}",
                    t_with_args(
                        "fas-qos-clamp-exit",
                        &fluent_args!(
                            "pid" => self.policy_id.to_string(),
                            "held" => held.to_string(),
                            "freq" => actual.to_string()
                        )
                    )
                );
                self.qos_clamped = false;
                self.qos_clamped_since = None;
                self.qos_clamp_warned = false;
            }
            return;
        }
        if actual < expected {
            // 合法 QoS 聚合态：不重写不告警；dcvsh_freq_limit 仅 debug 记录，低于硬件 max 说明另有硬件级限频叠加
            if !self.qos_clamped {
                self.qos_clamped = true;
                self.qos_clamped_since = Some(Instant::now());
                self.qos_clamp_warned = false;
                debug!(
                    "{}",
                    t_with_args(
                        "fas-qos-clamp-enter",
                        &fluent_args!(
                            "pid" => self.policy_id.to_string(),
                            "wrote" => expected.to_string(),
                            "read" => actual.to_string(),
                            "dcvsh" => self
                                .read_dcvsh_freq_limit()
                                .map(|f| f.to_string())
                                .unwrap_or_else(|| "n/a".into())
                        )
                    )
                );
            } else if !self.qos_clamp_warned {
                if let Some(since) = self.qos_clamped_since {
                    if since.elapsed() >= QOS_CLAMP_WARN_WINDOW {
                        warn!(
                            "{}",
                            t_with_args(
                                "fas-qos-clamp-long",
                                &fluent_args!(
                                    "pid" => self.policy_id.to_string(),
                                    "held" => since.elapsed().as_secs().to_string(),
                                    "wrote" => expected.to_string(),
                                    "read" => actual.to_string()
                                )
                            )
                        );
                        // [qos_clamp] 钳制与外部把 max 改小的读回签名相同不可区分，故附带一次重写试探
                        // 若属篡改，下次 verify 读回复原即退出钳制；若属真 QoS 聚合，取 min 不受影响，仅多一次幂等写
                        self.qos_clamp_warned = true;
                        self.max_writer.write_value_force(expected);
                        self.min_writer.write_value_force(expected);
                    }
                }
            }
            return;
        }
        // 读回 > 写入值：锁频被异常改写，兜底重写收敛回目标
        warn!(
            "{}",
            t_with_args(
                "fas-freq-tamper",
                &fluent_args!(
                    "pid" => self.policy_id.to_string(),
                    "wrote" => expected.to_string(),
                    "read" => actual.to_string()
                )
            )
        );
        self.max_writer.re_unmount();
        self.min_writer.re_unmount();
        self.max_writer.write_value_force(expected);
        self.min_writer.write_value_force(expected);
    }

    fn read_scaling_max_freq(&self) -> Option<u32> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            self.policy_id
        );
        fs::read_to_string(&path).ok()?.trim().parse::<u32>().ok()
    }

    /// dcvsh_freq_limit：硬件 DCVS 限频佐证节点，读 policy 首核（policy 目录即以首核命名）；部分内核未导出，返回 None
    fn read_dcvsh_freq_limit(&self) -> Option<u32> {
        let path = format!(
            "/sys/devices/system/cpu/cpu{}/dcvsh_freq_limit",
            self.policy_id
        );
        fs::read_to_string(&path).ok()?.trim().parse::<u32>().ok()
    }

    // [reset]
    /// 防篡改重写：目标值不变，把当前锁频值再写一遍，收敛被异常改写的节点
    /// ChiRi 是全局唯一调度程序，被改写属异常态，本机制是兜底不是常态对抗；`qos_clamped` 期间跳过，读回偏小是 FREQ_QOS_MAX 聚合的合法结果，重写改不了聚合 min 只会白写 sysfs
    pub fn force_reapply(&mut self) {
        if self.ignore_write || self.qos_clamped {
            return;
        }
        self.min_writer.write_value_force(self.current_freq);
        self.max_writer.write_value_force(self.current_freq);
    }

    /// 退出锁频：恢复 min/max 全范围；只有两侧写成功才把 current_freq 记为新范围（写失败不伪造已恢复）
    pub fn reset(&mut self) {
        let min_f = self.available_freqs[0];
        let max_f = *self.available_freqs.last().unwrap();
        let max_ok = self.max_writer.write_value_force(max_f);
        let min_ok = self.min_writer.write_value_force(min_f);
        // [write-failed] 写失败/半写不推进 current_freq：硬件可能仍是旧锁频，不能对外表现为已恢复；
        // reset 仍清空校验状态（明确终止锁频校验），下一 tick 目标不同时会重新下发。
        if max_ok && min_ok {
            self.current_freq = max_f;
        }
        self.verify_freq = None;
        self.verify_at = None;
        self.qos_clamped = false;
        self.qos_clamped_since = None;
        self.qos_clamp_warned = false;
        self.verify_read_failed = false;
        // 恢复接管前的 governor：performance 泄漏到 CLG/akmode/系统调频会让功耗停在性能拉满档
        if let Some(gov) = self.orig_governor.clone() {
            // 快照本身是 performance 说明它取自 GovernorGuard 已切 performance 之后（快照必须早于
            // load_policies），照写回去等于把 performance 泄漏持久化：改为回落到该 policy 的默认选型
            let gov = if gov.trim() == "performance" {
                let fallback = crate::common::cpu_governor_for_policy(self.policy_id as i32);
                warn!(
                    "{}",
                    t_with_args(
                        "fas-governor-reset-performance",
                        &fluent_args!(
                            "pid" => self.policy_id.to_string(),
                            "governor" => fallback.clone()
                        )
                    )
                );
                fallback
            } else {
                gov
            };
            let _ = crate::utils::write_sysfs(
                &format!(
                    "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                    self.policy_id
                ),
                &gov,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 可写临时文件路径：FastWriter 只有拿到真实可写句柄才返回 true；
    /// /nonexistent 路径则稳定返回 false，用作显式写失败注入，不沿用“节点无效但写入算成功”
    fn temp_writer_path(tag: &str) -> String {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "chiri-fas-policy-{}-{}-{}",
            std::process::id(),
            tag,
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, b"").expect("create fas test sysfs file");
        path.display().to_string()
    }

    fn controller(max_path: &str, min_path: &str, current_freq: u32) -> PolicyController {
        PolicyController::new(
            FastWriter::new(max_path),
            FastWriter::new(min_path),
            vec![100, 200, 300, 400, 500],
            0,
            ClusterProfile::default(),
            current_freq,
            None,
            1,
        )
    }

    fn failing_writer(tag: &str) -> FastWriter {
        FastWriter::new(format!(
            "/nonexistent/chiri-fas-{}-{}",
            tag,
            std::process::id()
        ))
    }

    #[test]
    fn apply_both_writers_failed_keeps_last_successful_state() {
        let max_path = temp_writer_path("max");
        let min_path = temp_writer_path("min");
        let mut policy = controller(&max_path, &min_path, 500);
        policy.apply_freq_locked(300);
        assert_eq!(policy.current_freq, 300);
        assert_eq!(policy.verify_freq, Some(300));

        // 构造后的两个 writer 同时失效（runtime 写失败）
        policy.max_writer = failing_writer("max");
        policy.min_writer = failing_writer("min");
        policy.apply_freq_locked(200);

        // 双写失败不得被当成命令已应用：current/hold/verify 全部停在上一成功态
        assert_eq!(policy.current_freq, 300);
        assert_eq!(policy.freq_hold_frames, 0);
        assert_eq!(policy.verify_freq, None);
        assert!(policy.verify_at.is_none());
    }

    #[test]
    fn apply_raise_half_write_keeps_previous_state() {
        let max_path = temp_writer_path("max");
        let mut policy = controller(&max_path, "/nonexistent/chiri-fas-raise-min", 200);
        policy.apply_freq_locked(400);

        // raise 先写 max 成功、后写 min 失败：半写不是成功，current 停在旧目标
        assert_eq!(policy.current_freq, 200);
        assert_eq!(policy.freq_hold_frames, 0);
        assert_eq!(policy.verify_freq, None);
        assert!(policy.verify_at.is_none());
        assert_eq!(fs::read_to_string(&max_path).unwrap(), "400\n");
    }

    #[test]
    fn apply_lower_half_write_keeps_previous_state() {
        let min_path = temp_writer_path("min");
        let mut policy = controller("/nonexistent/chiri-fas-lower-max", &min_path, 400);
        policy.apply_freq_locked(200);

        // lower 先写 min 成功、后写 max 失败：同样不得推进 current
        assert_eq!(policy.current_freq, 400);
        assert_eq!(policy.freq_hold_frames, 0);
        assert_eq!(policy.verify_freq, None);
        assert!(policy.verify_at.is_none());
        assert_eq!(fs::read_to_string(&min_path).unwrap(), "200\n");
    }

    #[test]
    fn apply_success_matches_baseline_and_keeps_verify_anchor() {
        let max_path = temp_writer_path("max");
        let min_path = temp_writer_path("min");
        let mut policy = controller(&max_path, &min_path, 500);

        policy.apply_freq_locked(300);
        assert_eq!(policy.current_freq, 300);
        assert_eq!(policy.freq_hold_frames, 2);
        assert_eq!(policy.verify_freq, Some(300));
        let anchor = policy.verify_at.expect("目标变化应重启 verify 稳定计时");
        assert_eq!(fs::read_to_string(&max_path).unwrap(), "300\n");
        assert_eq!(fs::read_to_string(&min_path).unwrap(), "300\n");

        // 同目标重复写入：hold 重置，但 verify 锚点不因目标未变而重启（保持基线语义）
        policy.apply_freq_locked(300);
        assert_eq!(policy.verify_at, Some(anchor));
        assert_eq!(policy.freq_hold_frames, 2);
        assert_eq!(policy.verify_freq, Some(300));
    }

    #[test]
    fn apply_recovery_after_write_failure_returns_to_baseline() {
        let mut policy = controller(
            "/nonexistent/chiri-fas-recover-max",
            "/nonexistent/chiri-fas-recover-min",
            500,
        );
        policy.apply_freq_locked(300);
        assert_eq!(policy.current_freq, 500);
        assert!(policy.verify_at.is_none());

        // writer 恢复后，下一次同样的正常输入应回到成功路径基线
        let max_path = temp_writer_path("max");
        let min_path = temp_writer_path("min");
        policy.max_writer = FastWriter::new(&max_path);
        policy.min_writer = FastWriter::new(&min_path);
        policy.apply_freq_locked(300);

        assert_eq!(policy.current_freq, 300);
        assert_eq!(policy.freq_hold_frames, 2);
        assert_eq!(policy.verify_freq, Some(300));
        assert!(policy.verify_at.is_some());
        assert_eq!(fs::read_to_string(&max_path).unwrap(), "300\n");
        assert_eq!(fs::read_to_string(&min_path).unwrap(), "300\n");
    }

    #[test]
    fn reset_commits_frequency_only_when_both_writes_succeed() {
        let mut policy = controller(
            "/nonexistent/chiri-fas-reset-max",
            "/nonexistent/chiri-fas-reset-min",
            300,
        );
        policy.reset();

        // 写失败不推进 current：硬件可能仍是旧锁频，不能对外表现为已恢复
        assert_eq!(policy.current_freq, 300);
        assert_eq!(policy.verify_freq, None);
        assert!(!policy.qos_clamped);

        let max_path = temp_writer_path("max");
        let min_path = temp_writer_path("min");
        policy.max_writer = FastWriter::new(&max_path);
        policy.min_writer = FastWriter::new(&min_path);
        policy.reset();

        assert_eq!(policy.current_freq, 500);
        assert_eq!(policy.verify_freq, None);
        assert_eq!(fs::read_to_string(&max_path).unwrap(), "500\n");
        assert_eq!(fs::read_to_string(&min_path).unwrap(), "100\n");
    }
}
