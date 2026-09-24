//! policy_controller.rs: [struct] [init] [apply] [reset]

use crate::fas_types::ClusterProfile;
use crate::utils::FastWriter;
use log::warn;
use std::fs;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::t_with_args;

/// 频率校验的最小等待：内核调频异步，过早读 scaling_cur_freq 得到的是旧档
const VERIFY_SETTLE: Duration = Duration::from_millis(200);

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

    pub ignore_write: bool,

    /// 接管前该 policy 的原始 governor（load_policies 快照）。
    /// FAS 接管期间把 governor 改写为 performance 以配合 min=max 锁频；
    /// 若退出时不恢复，后续 CLG/akmode 与系统调频都会在 performance 下
    /// 恒跑 max cap——功耗不降且低负载降频节能全部失效（泄漏到所有模式）
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
        // 校验上一次写入，必须在本次写入之前：内核调频异步，写完立刻读 scaling_cur_freq
        // 拿到的是旧档，会被判成「被压制」而触发无效重写。校验还要求目标已稳定
        // ≥ VERIFY_SETTLE，频繁改频期间跳过。
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

    // TODO: 实机复核校验延后后 fas-freq-mismatch 是否消失（旧实现约 55 次/11min）；
    // 若仍复现，说明锁频值确被压制（内核 thermal / QoS 收窄等异常态），保留重写逻辑。
    /// 校验上一次写入的目标频率（由 apply 入口调用；要求目标已稳定 ≥ [`VERIFY_SETTLE`]，
    /// 且距上次校验 ≥ verify_freq_interval_secs，默认 1s）。
    /// 只校验下沿：高于目标属瞬态，性能无损；低于目标才是锁频未生效——ChiRi 默认是
    /// 全局唯一调度程序，值被压制/改写属异常态（内核 thermal / QoS 收窄、残留旧模块、
    /// 手动调试），重写是兜底收敛回目标。
    fn verify_prev_freq(&mut self) {
        let (Some(expected), Some(at)) = (self.verify_freq, self.verify_at) else {
            return;
        };
        if at.elapsed() < VERIFY_SETTLE || self.verify_timer.elapsed() < self.verify_interval {
            return;
        }
        self.verify_timer = Instant::now();
        let Some(actual) = self.read_current_freq() else {
            return;
        };
        let min_ok = self
            .available_freqs
            .iter()
            .take_while(|&&f| f <= expected)
            .last()
            .copied()
            .unwrap_or(expected);
        if actual < min_ok {
            warn!(
                "{}",
                t_with_args(
                    "fas-freq-mismatch",
                    &fluent_args!(
                        "pid" => self.policy_id.to_string(),
                        "min" => min_ok.to_string(),
                        "max" => expected.to_string(),
                        "actual" => actual.to_string()
                    )
                )
            );
            self.max_writer.re_unmount();
            self.min_writer.re_unmount();
            self.max_writer.write_value_force(expected);
            self.min_writer.write_value_force(expected);
        }
    }

    fn read_current_freq(&self) -> Option<u32> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_cur_freq",
            self.policy_id
        );
        fs::read_to_string(&path).ok()?.trim().parse::<u32>().ok()
    }

    // [reset]
    /// 防篡改重写：目标值不变，只把当前锁频值再写一遍，把被异常改写的节点收敛回
    /// 目标（ChiRi 默认是全局唯一调度程序，被改写属异常态——残留旧模块/手动调试/
    /// 内核异常；本机制是兜底收敛，不是与常驻竞争者的常态对抗）。
    ///
    /// 这里原先还会先做两次 `FastWriter::re_unmount()`，现已删除，理由：
    /// 1. `re_unmount` 只调 `umount2(MNT_DETACH)`，**不含重开 fd**；而紧接着的
    ///    `write_value_force` 走的是构造（或上一次 `unmount_and_reopen`）时打开并常驻的 fd。
    ///    卸载既不会改变该 fd 指向的 kernfs 节点，也不会让写入更有效，对收敛零贡献；
    ///    真正能换节点的是 `unmount_and_reopen`（卸载 + 重新 open）。
    /// 2. scaling_max_freq / scaling_min_freq 正常不是挂载点，`umount2` 直接返回 EINVAL，
    ///    两次调用是纯系统调用浪费。
    /// 3. 与 `utils::FastWriter` 的设计口径冲突：那里只做「惰性卸载」
    ///    （utils.rs：「避免对正常设备上每个节点无条件 detach（可能拆掉合法挂载）」），
    ///    每次强制重写都无条件 detach 两次是反例。
    ///
    /// `verify_prev_freq` 里的那次 unmount 保留：它是「读回发现频率被压制」后的兜底重写路径，
    /// 触发受 verify_freq_interval_secs（默认 1s）限制，代价可忽略。
    pub fn force_reapply(&mut self) {
        if self.ignore_write {
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
        // 恢复接管前的 governor：load_policies 期间写入的 performance
        // 只对 FAS 锁频模式有意义，泄漏到 CLG/akmode/系统调频会让功耗
        // 停留在「性能拉满」档（用户实测 FAS 用后功耗不降的直接原因）
        if let Some(gov) = self.orig_governor.clone() {
            let _ = crate::utils::try_write_file(
                &format!(
                    "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                    self.policy_id
                ),
                &gov,
            );
        }
    }
}
