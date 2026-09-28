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

/// QoS 钳制长窗口：qos_clamped 持续超过该时长才打 warnthermal cdev 降频恢复为秒级~分钟级，300s 已覆盖绝大多数单次热事件；超过则视为 thermal 近乎永久钳死，
/// 提醒用户锁频长期未生效
const QOS_CLAMP_WARN_WINDOW: Duration = Duration::from_secs(300);

// [struct]
// 单 policy 频率控制器状态
pub struct PolicyController {
    pub max_writer: FastWriter,
    pub min_writer: FastWriter,
    pub available_freqs: Vec<u32>,
    cached_ratios: Vec<f32>,
    pub current_freq: u32,
    pub policy_id: usize,
    pub cluster_profile: ClusterProfile,
    pub freq_hold_frames: u32,
    pub freq_min: f32,
    pub freq_max: f32,

    verify_freq: Option<u32>,
    verify_timer: Instant,
    /// 目标值最后一次变化的时刻，用于等待目标稳定后再校验；None = 尚未写过
    verify_at: Option<Instant>,
    /// 写频后校验间隔（来自 FasRulesConfig::verify_freq_interval_secs）
    verify_interval: Duration,

    /// QoS 钳制状态：读回 scaling_max_freq < 上次写入值时进入。写 scaling_max_freq 只是追加一条 FREQ_QOS_MAX 请求（thermal
    /// cooling/msm_performance/input-boost 各持一条，聚合取min），热压期内核请求更低、读回偏小是合法态不是篡改；重写改不了聚合 min，只会死循环白写
    qos_clamped: bool,
    /// 进入钳制的时刻，用于长窗口告警与退出时报告持续时长
    qos_clamped_since: Option<Instant>,
    /// 长窗口告警只打一次的标记，退出钳制时复位
    qos_clamp_warned: bool,

    pub ignore_write: bool,

    /// 接管前该 policy 的原始 governor（load_policies 快照）。FAS 接管期间把 governor 改写为 performance 配合 min=max 锁频，退出必须恢复，
    /// 否则性能模式泄漏到 CLG/akmode/系统调频
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
    pub fn apply_freq_locked(&mut self, target_freq: u32) {
        if self.ignore_write {
            return;
        }
        // 必须在本次写入前校验上一次写入：内核调频异步，写完立刻读 scaling_cur_freq 拿到的是旧档会假性判成「被改写」；且要求目标已稳定 ≥ VERIFY_SETTLE，频繁改频期间跳过
        self.verify_prev_freq();
        if target_freq >= self.current_freq {
            self.max_writer.write_value_force(target_freq);
            self.min_writer.write_value_force(target_freq);
        } else {
            self.min_writer.write_value_force(target_freq);
            self.max_writer.write_value_force(target_freq);
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

    /// 校验上一次写入的目标频率（apply 入口调用；要求目标稳定 ≥ [`VERIFY_SETTLE`]，
    /// 且距上次校验 ≥ verify_freq_interval_secs，默认 1s）TODO: 只校验 scaling_max_freq
    /// 不校验 scaling_min；写失败与 QoS 钳制在读回偏低时不可区分（按合法钳制处理）
    /// 按读回 scaling_max_freq（= FREQ_QOS_MAX 聚合结果）分三级判定：
    /// - `== 写入值`：正常；若此前处于 [`Self::qos_clamped`] 则退出钳制态；
    /// - `< 写入值`：QoS 钳制（thermal cooling/msm_performance/input-boost 各持
    /// FREQ_QOS_MAX 请求聚合取 min，读回偏小是合法态不是篡改）——不重写不告警
    /// （降 debug），持续超过 [`QOS_CLAMP_WARN_WINDOW`] 才打 warn硬件热压
    /// （LMh/DCVS）不碰 policy->max，佐证节点 dcvsh_freq_limit 仅 debug 记录；
    /// - `> 写入值`：锁频被异常改写（残留旧模块/手动调试/内核异常），告警 + 重写收敛
    fn verify_prev_freq(&mut self) {
        let (Some(expected), Some(at)) = (self.verify_freq, self.verify_at) else {
            return;
        };
        if at.elapsed() < VERIFY_SETTLE || self.verify_timer.elapsed() < self.verify_interval {
            return;
        }
        self.verify_timer = Instant::now();
        let Some(actual) = self.read_scaling_max_freq() else {
            return;
        };
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
            // 合法 QoS 聚合态：不重写、不告警dcvsh_freq_limit 仅 debug 记录，低于硬件 max 说明另有硬件级限频叠加
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
                        // [qos_clamp] 钳制与「外部把 max 改小」读回签名相同不可区分，故附带一次重写试探：若属篡改，下次 verify 读回复原即退出钳制；若属真 QoS 聚合，
                        // 取 min 不受影响，仅多一次幂等写
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
    /// （ChiRi 是全局唯一调度程序，被改写属异常态，本机制是兜底不是常态对抗）
    /// 原先的两次 `FastWriter::re_unmount()` 已删：它只 umount2(MNT_DETACH) 不重开 fd，
    /// 对收敛零贡献（真正换节点的是 unmount_and_reopen），且 scaling_max/min_freq 不是
    /// 挂载点必返回 EINVALverify_prev_freq 里那次 unmount 保留（兜底路径，受
    /// verify_freq_interval_secs 限频，代价可忽略）`qos_clamped` 期间跳过：
    /// 读回偏小是 FREQ_QOS_MAX 聚合的合法结果，重写改不了聚合 min，只会白写 sysfs
    /// `qos_clamped` 期间直接跳过：读回偏小是 FREQ_QOS_MAX 聚合的合法结果
    /// （thermal 等内核持有更低请求），此时重写改变不了聚合 min，只会白写 sysfs
    pub fn force_reapply(&mut self) {
        if self.ignore_write || self.qos_clamped {
            return;
        }
        self.min_writer.write_value_force(self.current_freq);
        self.max_writer.write_value_force(self.current_freq);
    }

    pub fn reset(&mut self) {
        let min_f = self.available_freqs[0];
        let max_f = *self.available_freqs.last().unwrap();
        self.max_writer.write_value_force(max_f);
        self.min_writer.write_value_force(min_f);
        self.current_freq = max_f;
        self.verify_freq = None;
        self.verify_at = None;
        self.qos_clamped = false;
        self.qos_clamped_since = None;
        self.qos_clamp_warned = false;
        // 恢复接管前的 governor：performance 泄漏到 CLG/akmode/系统调频会让功耗停在「性能拉满」档
        if let Some(gov) = self.orig_governor.clone() {
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
