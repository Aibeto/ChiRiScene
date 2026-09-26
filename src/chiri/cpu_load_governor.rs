//! cpu_load_governor.rs: [cluster] [touch_state] [worker] [worker_handle] [governor]

use crate::chiri::config::CpuLoadGovernorConfig;
use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [thermal_clamp] 热压制「重钳」开关（feature.yaml `Thermal.clamp_heavy`，serde default = true）

/// 是否在 cap 窗口内对**所有簇**恒钳写频目标：true（默认）= 恒钳写侧，current_perf 照常平滑、不回写（窗口解除即恢复全速）；false = 回退旧行为（current_perf >= free_above 的簇豁免、不钳制）。
/// 进程级原子量，由 `Config::load` 启动/热重载时一次性写入；Worker 每次 flush 读一次，无额外轮询。
static CLAMP_HEAVY: AtomicBool = AtomicBool::new(true);

/// 由配置层同步 `Thermal.clamp_heavy`（`Config::load` 调用）。热重载即时生效。
pub fn set_clamp_heavy(v: bool) {
    CLAMP_HEAVY.store(v, Ordering::Relaxed);
}

// [cluster] PolicyRestore — CLG 接管前的系统状态快照，release 时恢复

struct PolicyRestore {
    policy_id: i32,
    /// 接管前的 scaling_governor；读取失败时为 None，恢复时跳过该字段，不写退化值
    governor: Option<String>,
    /// 接管前的 scaling_min_freq（kHz）；读取失败为 None，恢复时跳过
    min_freq: Option<u32>,
    /// 接管前的 scaling_max_freq（kHz）；读取失败为 None，恢复时跳过
    max_freq: Option<u32>,
    /// 该 policy 的硬件最大可用频率，恢复时先放宽上限到它
    hw_max: u32,
}

// ClusterState — 单 cluster 运行时状态

struct ClusterState {
    policy_id: i32,
    /// 受该 policy 管理的 CPU id 列表（从 affected_cpus 解析）
    affected_cpus: Vec<usize>,
    /// 可用频率档位（kHz，含 boost 频率，升序去重）
    available_freqs: Vec<u32>,
    /// 频率 -> [0,1] 性能比例的缓存：(f - fmin) / (fmax - fmin)
    cached_ratios: Vec<f32>,
    /// 该 cluster 的 boost 最高频率（kHz），无 boost 文件则为 0
    boost_max: u32,
    /// scaling_max_freq 写通道缓存
    max_writer: FastWriter,
    /// 当前目标性能比 [0,1]，调频时换算成频率档位
    current_perf: f32,
/// 已写入的 scaling_max_freq（kHz）；0 = 未写入成功、下次 tick 重试。注意是上限而非实际频率，实际由 schedutil 在 [硬件最低, 上限] 内自主决定
    current_freq: u32,
    /// 降频确认计数：连续满 down_rate_limit_ticks 才执行降频
    down_wait: u32,
    /// 升频确认计数：连续满 up_rate_limit_ticks 才执行升频
    up_wait: u32,
    /// 上一 tick 的 max_util（平滑后），用于尖峰跳升检测
    last_util: f32,
    /// 决策负载 EMA 状态（util_smoothing < 1 时启用；-1 = 未初始化），语义同 tuned
    ema_util: f32,
    // [dwell] 写频滞回状态（Phase 2）：最小驻留 + 死区 + 方向翻摆抑制
/// 上次**实际写频**成功时刻（含豁免写），dwell 以它计时；None = 接管初写未启动时钟，首次受控写频立即生效
    last_write_at: Option<Instant>,
    /// 上次实际写频方向：升 = 1、降 = -1、未写过 = 0（翻摆判定基准）
    last_write_dir: i8,
    /// 上次 write_freq 写入失败：下次写入视为防篡改补写、豁免滞回（重试时序不变）
    last_failed: bool,
    /// on_load_update 命中「极低负载立即降频」快路径（豁免滞回），flush 消费清零
    fast_down: bool,
}

impl ClusterState {
/// 目标性能比映射到频率表中 **≤ 目标** 的最大档（floor 对齐）：写的是 scaling_max 上限，落点不得高于计算目标；
/// 目标落两档之间时内核本就把 max 向下 clamp——先对齐再写才能账实一致、同值不落盘去重有效。
    #[inline]
    fn find_floor_freq(&self, target_ratio: f32) -> u32 {
        let idx = self.cached_ratios.partition_point(|&r| r <= target_ratio);
        if idx == 0 {
            self.available_freqs[0]
        } else {
            self.available_freqs[idx - 1]
        }
    }

// [deadzone_hold] 死区频差（kHz）= write_deadzone × 硬件最高频；hold 判定与 write_freq 吞写 gate 共用此换算
    #[inline]
    fn deadzone_band(&self, deadzone_ratio: f32) -> f32 {
        let hw_max = *self.available_freqs.last().unwrap_or(&0) as f32;
        hw_max * deadzone_ratio
    }

/// 写 scaling_max_freq（性能上限）：min 已在 init 压到硬件最低，之后只调 max，无写序问题。
/// schedutil 在 [min, max] 内自主调频，内核微秒级即可降频，空转发热少。
    fn write_freq(&mut self, freq: u32, dwell_ms: u64, deadzone_ratio: f32, exempt: bool) {
        if freq == self.current_freq {
// 目标与缓存一致：上次写失败的补写诉求已消失，清标记防下次真实写频白豁免滞回一次
            self.last_failed = false;
            return;
        }
// [dwell] 写频滞回：死区内不写；距上次实际写频不足 dwell 且方向翻摆时延迟写，到期后下次 flush 补写当前 target。
// 豁免路径（触摸 floor 提频 / 极低负载立即降频 / 防篡改补写）直通；接管初写/恢复不走本函数。
        if !exempt {
// [deadzone_hold] 死区 gate（双保险）：与 on_load_update hold 判定同源走 deadzone_band()；flush 层落点与决策层不一致（热 clamp / 触摸 floor 后）时同样不写近距档
            if (freq.abs_diff(self.current_freq) as f32) < self.deadzone_band(deadzone_ratio) {
                return;
            }
            let dir: i8 = if freq > self.current_freq { 1 } else { -1 };
            let flip = self.last_write_dir != 0 && dir != self.last_write_dir;
            if flip {
                if let Some(at) = self.last_write_at {
                    if at.elapsed() < Duration::from_millis(dwell_ms) {
                        return;
                    }
                }
            }
        }
        let old_freq = self.current_freq;
        let ok = self.max_writer.write_value_force(freq);
        // 写入成功才更新缓存，失败则下次 tick 自动重试
        if ok {
            self.current_freq = freq;
            // [dwell] 实际写频记录（含豁免写）：dwell 计时与翻摆判定基准
            self.last_write_at = Some(Instant::now());
            self.last_write_dir = if freq > old_freq { 1 } else { -1 };
            self.last_failed = false;
            debug!(
                "{}",
                t_with_args(
                    "clg-freq-set",
                    &fluent_args!(
                        "pid" => self.policy_id.to_string(),
                        "old_khz" => (old_freq / 1000).to_string(),
                        "new_khz" => (freq / 1000).to_string()
                    )
                )
            );
        } else {
            // [dwell] 写失败：下次写入视为防篡改补写、豁免滞回（重试时序不变）
            self.last_failed = true;
            debug!(
                "{}",
                t_with_args(
                    "clg-freq-write-failed-cached",
                    &fluent_args!(
                        "pid" => self.policy_id.to_string(),
                        "target_khz" => (freq / 1000).to_string(),
                        "cached_khz" => (self.current_freq / 1000).to_string()
                    )
                )
            );
        }
    }

    /// 取该 cluster 受影响 CPU 中的最大利用率（利用率来源为 eBPF 各核心负载）
    #[inline]
    fn max_util(&self, core_utils: &[f32]) -> f32 {
        self.affected_cpus
            .iter()
            .filter_map(|&cpu| core_utils.get(cpu))
            .copied()
            .fold(0.0_f32, f32::max)
    }

/// 频率（kHz）→ [0,1] 性能比例（与 cached_ratios 同口径）；降频时把 current_perf 同步到实际写入档位
    fn ratio_of_freq(&self, freq: u32) -> f32 {
        let fmin = *self.available_freqs.first().unwrap_or(&0) as f32;
        let fmax = *self.available_freqs.last().unwrap_or(&0) as f32;
        ((freq as f32) - fmin) / (fmax - fmin).max(1.0)
    }

/// 将单个 policy 恢复为接管前状态；返回是否全部写入成功，失败时调用方保留快照以便重试。
    fn restore_policy(r: &PolicyRestore) -> bool {
        let gov_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
            r.policy_id
        );
        let min_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
            r.policy_id
        );
        let max_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            r.policy_id
        );

        let mut all_ok = true;
// 写序保证任意中间态 min <= max：1) governor（None 跳过）2) 上限放宽到 hw_max 3) 恢复下限 4) 恢复上限；各步失败均记录，由调用方决定重试
        if let Some(gov) = &r.governor {
            if crate::utils::write_to_file(&gov_path, gov.as_bytes()).is_err() {
                all_ok = false;
            }
        }
        if crate::utils::write_to_file(&max_path, r.hw_max.to_string()).is_err() {
            all_ok = false;
        }
        if let Some(min) = r.min_freq {
            if crate::utils::write_to_file(&min_path, min.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(max) = r.max_freq {
            if crate::utils::write_to_file(&max_path, max.to_string()).is_err() {
                all_ok = false;
            }
        }

        debug!(
            "{}",
            t_with_args(
                "clg-restore",
                &fluent_args!(
                    "pid" => r.policy_id.to_string(),
                    "governor" => r.governor.clone().unwrap_or_else(|| "<unread>".to_string()),
                    "min" => r.min_freq.map(|v| v.to_string()).unwrap_or_else(|| "<unread>".to_string()),
                    "max" => r.max_freq.map(|v| v.to_string()).unwrap_or_else(|| "<unread>".to_string())
                )
            )
        );
        all_ok
    }
}

/// 触摸升频开关：false = `on_touch` 正常工作（升频窗口 + Worker 唤醒 + 打点），参数由 feature.yaml `touch_boost_enabled` 配置
const TOUCH_BOOST_SUSPENDED: bool = false;

// AtomicTouchState — 跨线程共享的触摸升频状态

/// 跨线程共享的触摸升频状态，Worker 通过 `Arc<AtomicTouchState>` 读取当前窗口；f32 以 bit pattern 存 AtomicU32。
/// set_epoch_ms 充当 generation 标志保证 set/get 一致性：不匹配视为写入中，返回 0.0 下次 tick 重试。
// [touch_state]
struct AtomicTouchState {
    /// 触摸升频地板性能比（f32 的 bit pattern），0 表示无窗口
    floor_bits: AtomicU32,
/// 窗口持续时长（相对写入时刻的毫秒偏移，u32 可表示 ~49 天）
    duration_ms: AtomicU32,
    /// 写入时刻的毫秒级 epoch 时间戳（u32 截断，~49 天一轮回，足够触摸窗口判断）
    set_epoch_ms: AtomicU32,
}

impl AtomicTouchState {
    fn new() -> Self {
        Self {
            floor_bits: AtomicU32::new(0.0_f32.to_bits()),
            duration_ms: AtomicU32::new(0),
            set_epoch_ms: AtomicU32::new(0),
        }
    }

/// 设置触摸升频窗口：floor 为大核性能下限、duration 为窗口时长；由 scheduler_ipc 线程调用
    fn set(&self, floor: f32, duration: Duration) {
// 写序：先 duration/floor，最后 set_epoch_ms（就绪标志）；Worker 先读 set_epoch_ms 再读其余，读取期间被更新最坏一次 tick 用错 floor、下 tick 修正，无害
        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        self.duration_ms
            .store(duration.as_millis() as u32, Ordering::Relaxed);
        self.floor_bits.store(floor.to_bits(), Ordering::Relaxed);
        // Release 保证前两行对读取侧可见
        self.set_epoch_ms.store(epoch_ms, Ordering::Release);
    }

/// Worker 调用：窗口未过期返回地板性能比，过期/无窗口返回 0.0；读取顺序与 set 写入顺序相反
    fn get_floor(&self) -> f32 {
        // Acquire 保证读到 set_epoch_ms 时，对应的 floor/duration 已可见
        let set_ms = self.set_epoch_ms.load(Ordering::Acquire);
        if set_ms == 0 {
            return 0.0;
        }
        let duration = self.duration_ms.load(Ordering::Relaxed);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        // u32 环形减法：即使 now_ms 溢出回绕，差值仍然正确（~49 天周期内）
        let elapsed = now_ms.wrapping_sub(set_ms);
        if elapsed < duration {
            f32::from_bits(self.floor_bits.load(Ordering::Relaxed))
        } else {
            0.0
        }
    }
}

// [worker] CoreGroupWorker — 每个核心组独立线程的调度 Worker

/// 负载通道发送端：逐核 util 以 Arc 共享（同一 tick 只分配一次，广播给全部 Worker）
type LoadSender = mpsc::SyncSender<Arc<Vec<f32>>>;
/// 负载通道接收端（与 [`LoadSender`] 配对）
type LoadReceiver = mpsc::Receiver<Arc<Vec<f32>>>;
/// Worker 派生成功后的返回（快照 + 线程句柄/负载发送端）；别名仅为压平类型嵌套深度（clippy::type_complexity）
type WorkerSpawn = (PolicyRestore, (JoinHandle<()>, LoadSender));

/// 单核心组独立调度 Worker：专属线程内持有 ClusterState，接收负载自主决策 + 写频，与其他核心组完全并行
struct CoreGroupWorker {
    cluster: ClusterState,
    cfg: CpuLoadGovernorConfig,
    restore: PolicyRestore,
    core_ranges: crate::common::CoreGroupRanges,
    load_rx: LoadReceiver,
    stop: Arc<AtomicBool>,
    touch: Arc<AtomicTouchState>,
/// 热保护性能上限（f32 bit pattern，1.0 = 无压制）；scheduler_ipc 采样电池/CPU 温度写入，Worker flush 读取并 clamp
    thermal_cap: Arc<AtomicU32>,
/// 压制豁免档（0..1）：仅 clamp_heavy=false 时生效，current_perf >= 该值不钳制，保证任何温度下持续高负载都能到达硬件最高频
    thermal_free_above: Arc<AtomicU32>,
    /// 上次决策摘要（main_ tick 行用，on_load_update 填写）
    dev_decision: &'static str,
    dev_over: u32,
    dev_under: u32,
    dev_tgt_perf: f32,
    dev_raw_util: f32,
    dev_prev_perf: f32,
/// 上次 flush 热钳制是否生效；仅用于 clamp_apply 证据事件的跃迁去重，不参与控制
    dev_clamp_binds: bool,
}

impl CoreGroupWorker {
/// 在新线程中运行 Worker 事件循环：接收负载、决策、写频；线程退出（stop 或 channel 断开）前恢复系统原始状态。
/// 性能响应全部走推送事件（负载包 / 触摸空包立即 flush）；超时分支只做两件非性能任务：清理过期触摸窗口、
/// 重写当前频率防篡改（异常改写是秒级动作，1s 粒度足够兜住），空闲时空转从 ~6 次/s 降到 1 次。
    fn run(mut self) {
        let tick_interval = Duration::from_secs(1);
        let mut log_counter: u32 = 0;

        loop {
            if self.stop.load(Ordering::Acquire) {
                break;
            }

            match self.load_rx.recv_timeout(tick_interval) {
                Ok(core_utils) => {
// 空负载包 = 触摸唤醒信号：只 flush 应用触摸升频，跳过决策——避免空数据把 util 算成 0 扰动 up/down_wait
                    if !core_utils.is_empty() {
                        self.on_load_update(&core_utils);
                    }
                    self.flush(&core_utils, &mut log_counter);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
// tick 到期：仅 flush（清理过期触摸窗口、重写频率防篡改）；无新数据传空切片，max_util=0 不触发决策变更
                    let empty: [f32; 0] = [];
                    self.flush(&empty, &mut log_counter);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // 线程退出时恢复系统原始状态
        ClusterState::restore_policy(&self.restore);
    }

/// 决策入口：只计算目标性能比不写 sysfs，同时记录 main_ tick 行摘要（over/under、目标性能、决策标签）。
/// 标签四种：up / down_wait / down / hold（hold = 降频落点与当前频点差在死区内，不计 debounce、不写频）。
    fn on_load_update(&mut self, core_utils: &[f32]) {
        let raw_util = self.cluster.max_util(core_utils);
        self.dev_raw_util = raw_util;
        self.dev_prev_perf = self.cluster.current_perf;

        // main_ tick 行：组内超升频阈值 / 低于降频阈值的核心数（0.0 核不计入 over）
        let mut over = 0u32;
        let mut under = 0u32;
        for &cpu in &self.cluster.affected_cpus {
            if let Some(&u) = core_utils.get(cpu) {
                if u > self.cfg.up_threshold {
                    over += 1;
                }
                if u < self.cfg.down_threshold {
                    under += 1;
                }
            }
        }
        self.dev_over = over;
        self.dev_under = under;

// 负载平滑（EMA，util_smoothing=1.0 关闭，语义同 tuned）：抑制抖动负载下 max_util 大幅摆动导致的决策翻摆与 scaling_max_freq 高频改写。
// main_ 的 max_util 列刻意写平滑前原始值，离线回放可自行试验系数。
        let smoothed = if self.cfg.util_smoothing >= 0.999 || self.cluster.ema_util < 0.0 {
            raw_util
        } else {
            self.cfg.util_smoothing * raw_util
                + (1.0 - self.cfg.util_smoothing) * self.cluster.ema_util
        };
        self.cluster.ema_util = smoothed;

        // 尖峰抑制：单 tick 跳升超过阈值时衰减其增量
        let util = if smoothed > self.cluster.last_util + self.cfg.spike_jump_threshold {
            self.cluster.last_util + (smoothed - self.cluster.last_util) * self.cfg.spike_decay
        } else {
            smoothed
        };
        self.cluster.last_util = smoothed;

// 主旋钮契约：target_perf = clamp(util × headroom, perf_floor, perf_ceil)，与 up_threshold 无关（up 只管 headroom ramp 与升频速度）；
// perf_ceil 默认 1.0（降功耗不封顶，除特调外）；headroom 在 up_threshold 附近线性过渡，避免阶跃振荡。
        let ramp_start = self.cfg.up_threshold - self.cfg.headroom_ramp;
        let headroom = if util >= self.cfg.up_threshold {
            self.cfg.headroom_factor
        } else if util > ramp_start {
            let t = ((util - ramp_start) / self.cfg.headroom_ramp.max(1e-6)).clamp(0.0, 1.0);
            1.0 + (self.cfg.headroom_factor - 1.0) * t
        } else {
            1.0
        };

        let target_perf = (util * headroom).clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
        self.dev_tgt_perf = target_perf;
        let old_perf = self.cluster.current_perf;

// [perf_eps] 浮点停滞防护：慢升分支 α≤0.5 在差 1 ulp 处增量舍入为 0，严格 > 比较使停滞态永久落 up 分支（假 up + up_wait 无限累加）；
// 加 ε=1e-6 让停滞态落入死区 hold 正常收尾，ε 远小于真实负载增量。
        if target_perf > old_perf + 1e-6 {
            self.cluster.down_wait = 0;
            self.cluster.up_wait += 1;

            // 升频速率限制：必须连续 up_rate_limit_ticks 才执行
            if self.cluster.up_wait < self.cfg.up_rate_limit_ticks {
                self.dev_decision = "up_wait";
                return;
            }
            self.dev_decision = "up";

// 动态上限：抬 ceiling 不强制频率跳变（schedutil 按实际需求在 [硬件最低, ceiling] 内取频），直接平滑抬升
            let is_high_load = util >= self.cfg.up_threshold;
            let is_significant_jump = target_perf > old_perf + self.cfg.up_jump_threshold;

            if is_high_load || is_significant_jump {
                self.cluster.current_perf += (target_perf - old_perf) * self.cfg.smoothing_up;
            } else {
                // 滞回带内升频：速率随 util 接近 up_threshold 线性提升
                let span = (self.cfg.up_threshold - self.cfg.down_threshold).max(1e-6);
                let gap = ((util - self.cfg.down_threshold) / span).clamp(0.0, 1.0);
                let speed = self.cfg.smoothing_up
                    * (self.cfg.slow_up_scale + (1.0 - self.cfg.slow_up_scale) * gap);
                self.cluster.current_perf += (target_perf - old_perf) * speed;
            }
        } else {
            self.cluster.up_wait = 0;
// 先算降频落点：与当前频点差在死区内（含同档 OPP）= 写频无效果，不计数不写频标 hold（否则稳态每 tick 假 down、deb_down 无限涨）；真实降频路径不变。util=0 不计升频、计入降频。
            let target_freq = self.cluster.find_floor_freq(target_perf);
// [deadzone_hold] hold 判定：≤ 死区频差（含相等；write_deadzone=0 退化为精确相等），与吞写 gate 同源，写不进 sysfs 的落点不累计 down_wait
            if (target_freq.abs_diff(self.cluster.current_freq) as f32)
                <= self.cluster.deadzone_band(self.cfg.write_deadzone)
            {
                self.cluster.down_wait = 0;
                self.dev_decision = "hold";
            } else {
                self.cluster.down_wait += 1;
                // 极低负载立即降频（跳过 down_wait 确认期），否则连续满 down_rate_limit_ticks
                if self.cluster.down_wait >= self.cfg.down_rate_limit_ticks
                    || util < self.cfg.down_fast_threshold
                {
                    // [dwell] 极低负载立即降频 = 豁免路径：本次写频绕过滞回（flush 消费）
                    self.cluster.fast_down = util < self.cfg.down_fast_threshold;
                    self.dev_decision = "down";
// 直接降上限（能效优先），一步到位写目标档：降 ceiling 只收窄 schedutil 区间，不会抬高实际频率
                    self.cluster.current_perf = self.cluster.ratio_of_freq(target_freq);
                } else {
                    self.dev_decision = "down_wait";
                }
            }
        }
    }

/// 把 current_perf 转成实际频率写 sysfs，每 tick 一次：触摸升频检查 → 热保护 clamp → 性能区间 clamp → 写频。
/// 热保护最后 clamp：低于豁免档才压，持续高负载平滑涨过豁免档后温度再高也不挡路（内核兜底）。
    fn flush(&mut self, core_utils: &[f32], log_counter: &mut u32) {
        // 触摸升频：Worker 自主检查共享的 AtomicTouchState
        let mut touch_active = false;
        // [dwell] floor 提频写入豁免滞回（floor 提频立即生效）
        let mut touch_raised = false;
        if self.cfg.touch_boost_enabled {
            let floor = self.touch.get_floor();
            if floor > 0.0 && Self::is_big_cluster(&self.cluster.affected_cpus, &self.core_ranges) {
                touch_raised = floor > self.cluster.current_perf;
                self.cluster.current_perf = self.cluster.current_perf.max(floor);
                touch_active = true;
            }
        }
        self.cluster.current_perf = self
            .cluster
            .current_perf
            .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
// 热压制（重钳，只作用于 CLG、不含 tuned）：只钳写频、不回写 current_perf（回写才会卡死在 cap），窗口解除立即恢复全速。
// clamp_heavy=true（默认）所有簇恒钳；false 时 current_perf >= 豁免档 free_above 不钳（豁免档需严格大于 soft_perf_cap，free_above 仅此分支生效）。
        let cap = f32::from_bits(self.thermal_cap.load(Ordering::Relaxed));
        let free_above = f32::from_bits(self.thermal_free_above.load(Ordering::Relaxed));
        let clamp_heavy = CLAMP_HEAVY.load(Ordering::Relaxed);
        let eff_perf = if clamp_heavy || self.cluster.current_perf < free_above {
            self.cluster.current_perf.min(cap)
        } else {
            self.cluster.current_perf
        };
        let target_freq = self.cluster.find_floor_freq(eff_perf);
// [thermal_clamp] 钳制「生效/解除」跃迁记一条 event（值未变不写，零分配）；生效判据 = 写频目标被 cap 压到 current_perf 之下
        let clamp_binds = eff_perf < self.cluster.current_perf;
        if clamp_binds != self.dev_clamp_binds {
            self.dev_clamp_binds = clamp_binds;
            if crate::logger::diag_active() {
                crate::logger::main_event(
                    "clamp_apply",
                    "-",
                    &format!(
                        "bind={} pid={} cap={:.0} perf={:.2} eff={:.2} tgt_khz={}",
                        clamp_binds,
                        self.cluster.policy_id,
                        cap * 100.0,
                        self.cluster.current_perf,
                        eff_perf,
                        target_freq
                    ),
                );
            }
        }
// [dwell] 豁免判定：触摸 floor 提频升向写 / 极低负载立即降频（fast_down 消费制，只作用本次 flush）/ 防篡改补写。
// 热 clamp 刻意不豁免（温度毛刺不得绕过滞回直写频率），经快路径/非翻摆写入自然生效。
        let fast_down = std::mem::take(&mut self.cluster.fast_down);
        let exempt = (touch_raised && target_freq > self.cluster.current_freq)
            || (fast_down && target_freq < self.cluster.current_freq)
            || self.cluster.last_failed;
        let dwell_ms = self.cfg.write_dwell_ms;
        let deadzone = self.cfg.write_deadzone;
        self.cluster.write_freq(target_freq, dwell_ms, deadzone, exempt);

        // main_ tick 行：仅决策 tick（core_utils 非空）且开发记录开启时写
        if !core_utils.is_empty() && crate::logger::diag_active() {
            let ranges = &self.core_ranges;
            let name = if self
                .cluster
                .affected_cpus
                .iter()
                .any(|c| ranges.prime.contains(c))
            {
                "prime"
            } else if self
                .cluster
                .affected_cpus
                .iter()
                .any(|c| ranges.big.contains(c))
            {
                "big"
            } else {
                "little"
            };
            crate::logger::main_tick(
                name,
                &format!("{:.2}", self.dev_raw_util),
                self.dev_over,
                self.dev_under,
                &format!("{:.2}", self.dev_prev_perf),
                &format!("{:.2}", self.dev_tgt_perf),
                // main_ 列口径：cur_freq_khz / max_freq_khz 均为原始 kHz
                &self.cluster.current_freq.to_string(),
                &self.restore.hw_max.to_string(),
                self.dev_decision,
                self.cluster.up_wait,
                self.cluster.down_wait,
                &format!("{:.0}", cap * 100.0),
                touch_active,
            );
        }

// 日志摘要：每 25 tick 输出一次；format! 在宏外求值，log_enabled! 门控省掉 INFO 级别下的分配
        *log_counter += 1;
        if *log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
            debug!(
                "{}",
                t_with_args(
                    "clg-tick-log",
                    &fluent_args!(
                        "pid" => self.cluster.policy_id.to_string(),
                        "util" => format!("{:.0}", self.cluster.max_util(core_utils) * 100.0),
                        "perf" => format!("{:.2}", self.cluster.current_perf),
                        "freq" => (self.cluster.current_freq / 1000).to_string(),
                        "boost" => format!("{:.0}", self.cluster.boost_max as f32 / 1000.0)
                    )
                )
            );
        }
    }

    /// 判定 cluster 是否覆盖当前 SoC 的大核区间：触摸升频只作用于大核。
    fn is_big_cluster(affected: &[usize], core_ranges: &crate::common::CoreGroupRanges) -> bool {
        let big = &core_ranges.big;
        affected.iter().any(|&c| big.contains(&c))
    }

    /// 创建 Worker 并在新线程中启动，返回（PolicyRestore, JoinHandle）。
    fn spawn(
        policy_id: i32,
        cfg: CpuLoadGovernorConfig,
        boost_frequencies: &[u32],
        core_ranges: crate::common::CoreGroupRanges,
        touch: Arc<AtomicTouchState>,
        thermal_cap: Arc<AtomicU32>,
        thermal_free_above: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
    ) -> Option<WorkerSpawn> {
        let pid = policy_id;
        let gov_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
            pid
        );
        let min_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
            pid
        );
        let max_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            pid
        );

        let freq_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_available_frequencies",
            pid
        );
        let mut freqs: Vec<u32> = fs::read_to_string(&freq_path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if freqs.is_empty() {
// scaling_available_frequencies 读不到/空表：按 policy 首核映射核心组，回退 soc.yaml [freq_khz] 兜底（只补表，不改取档逻辑）
            match crate::common::soc_freq_fallback_for_policy(pid) {
                Some(f) => freqs = f,
                None => return None,
            }
        }
        freqs.sort_unstable();
        freqs.dedup();

        // 合并 boost 频率（部分平台额外暴露的高频点），去重排序
        if !boost_frequencies.is_empty() {
            freqs.extend(boost_frequencies);
            freqs.sort_unstable();
            freqs.dedup();
        }

        let affected = Self::read_affected_cpus(pid);
        if affected.is_empty() {
            return None;
        }

        let fmin = *freqs.first().unwrap() as f32;
        let fmax = *freqs.last().unwrap() as f32;
        let range = (fmax - fmin).max(1.0);
        let cached_ratios: Vec<f32> = freqs.iter().map(|&f| (f as f32 - fmin) / range).collect();

        let max_writer = FastWriter::new(format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            pid
        ));
        let mut min_writer = FastWriter::new(format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
            pid
        ));

        if !max_writer.is_valid() || !min_writer.is_valid() {
            warn!(
                "{}",
                t_with_args(
                    "clg-writer-invalid",
                    &fluent_args!(
                        "pid" => pid.to_string(),
                        "max_valid" => max_writer.is_valid().to_string(),
                        "min_valid" => min_writer.is_valid().to_string()
                    )
                )
            );
            return None;
        }

// 记录系统原始状态（每 policy 一份），必须位于 governor 写入之前，保证 release 能还原
        let governor = fs::read_to_string(&gov_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let min_freq = fs::read_to_string(&min_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let max_freq = fs::read_to_string(&max_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let restore = PolicyRestore {
            policy_id: pid,
            governor,
            min_freq,
            max_freq,
            hw_max: *freqs.last().unwrap(),
        };

        // 写入 schedutil governor
        let _ = crate::utils::try_write_file(&gov_path, "schedutil");

// 动态上限接管：min 一次性压到硬件最低并保持，之后仅 write_freq 调 max；先降 min 再定 max，中间态恒 min <= max
        let hw_min = *freqs.first().unwrap();
        if !min_writer.write_value_force(hw_min) {
            warn!(
                "{}",
                t_with_args(
                    "clg-min-write-failed",
                    &fluent_args!("pid" => pid.to_string(), "khz" => (hw_min / 1000).to_string())
                )
            );
        }

        let init_perf = cfg.perf_init.clamp(cfg.perf_floor, cfg.perf_ceil);
        let boost_max = boost_frequencies.iter().copied().max().unwrap_or(0);
        let mut cluster = ClusterState {
            policy_id: pid,
            affected_cpus: affected.clone(),
            available_freqs: freqs,
            cached_ratios,
            boost_max,
            max_writer,
            current_perf: init_perf,
            current_freq: 0,
            down_wait: 0,
            up_wait: 0,
            ema_util: -1.0,
            last_util: 0.0,
            // [dwell] 写频滞回状态：接管初写不走 write_freq、不启动 dwell 时钟
            last_write_at: None,
            last_write_dir: 0,
            last_failed: false,
            fast_down: false,
        };

        let init_freq = cluster.find_floor_freq(init_perf);
        // 初始接管只写 max=perf_init 档（min 已在上面压到硬件最低）
        let init_ok = cluster.max_writer.write_value_force(init_freq);
        if init_ok {
            cluster.current_freq = init_freq;
        }

        let (load_tx, load_rx) = mpsc::sync_channel::<Arc<Vec<f32>>>(1);

        info!(
            "{}",
            t_with_args(
                "clg-init",
                &fluent_args!(
                    "pid" => pid.to_string(),
                    "cpus" => format!("{:?}", affected),
                    "fmin" => (fmin / 1000.0).to_string(),
                    "fmax" => (fmax / 1000.0).to_string(),
                    "perf" => format!("{:.2}", init_perf),
                    "freq" => (init_freq / 1000).to_string()
                )
            )
        );

        let worker = CoreGroupWorker {
            cluster,
            cfg,
            restore: PolicyRestore {
                policy_id: restore.policy_id,
                governor: restore.governor.clone(),
                min_freq: restore.min_freq,
                max_freq: restore.max_freq,
                hw_max: restore.hw_max,
            },
            core_ranges,
            load_rx,
            stop: stop.clone(),
            touch,
            thermal_cap,
            thermal_free_above,
            dev_decision: "hold",
            dev_over: 0,
            dev_under: 0,
            dev_tgt_perf: 0.0,
            dev_raw_util: 0.0,
            dev_prev_perf: 0.0,
            dev_clamp_binds: false,
        };

        let handle = thread::Builder::new()
            .name(format!("clg_worker_{}", pid))
            .spawn(move || worker.run())
            .ok()?;

        Some((restore, (handle, load_tx)))
    }

    /// 读取 policy 的 affected_cpus，解析成 CPU id 列表
    fn read_affected_cpus(policy_id: i32) -> Vec<usize> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/affected_cpus",
            policy_id
        );
        fs::read_to_string(&path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|s| s.parse::<usize>().ok())
            .collect()
    }
}

// [worker_handle] Worker 句柄（线程 + 负载通道发送端）

/// 每个 Worker 的控制句柄：持有负载通道发送端和线程 JoinHandle。
struct WorkerHandle {
    policy_id: i32,
    load_tx: LoadSender,
    handle: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// 发送负载数据到 Worker（非阻塞，满则丢弃本 tick）
    fn send_load(&self, core_utils: Arc<Vec<f32>>) {
        let _ = self.load_tx.try_send(core_utils);
    }

    /// 等待 Worker 线程退出
    fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// [governor]
// CpuLoadGovernor — 主控制器（Worker 线程管理器）

pub struct CpuLoadGovernor {
    /// 当前生效的 CLG 配置（normalize 后的副本）
    cfg: CpuLoadGovernorConfig,
    /// Worker 句柄列表：每个 cpufreq policy 一个 Worker
    workers: Vec<WorkerHandle>,
    /// 接管前的系统状态快照：release 时恢复
    restores: Vec<PolicyRestore>,
    /// Worker 停止信号（共享，新 init 时替换为新实例）
    stop: Arc<AtomicBool>,
    /// 触摸升频状态（跨线程共享，Worker 自主读取）
    touch: Arc<AtomicTouchState>,
/// 热保护性能上限（f32 bit pattern，1.0 = 无压制）；scheduler_ipc 更新，Worker flush 读取，跨 init/reload 生命周期保持
    thermal_cap: Arc<AtomicU32>,
    /// 压制豁免档（f32 bit pattern）：仅 `clamp_heavy=false` 时生效，ceiling >= 豁免档不钳制
    thermal_free_above: Arc<AtomicU32>,
    /// 是否处于接管状态（至少一个 Worker 启动成功才为 true）
    active: bool,
/// 上一 tick 广播给 Worker 的负载缓冲；引用计数归 1 时下一 tick 原地复用，省每 tick 一次 Vec 分配
    load_buf: Option<Arc<Vec<f32>>>,
}

impl CpuLoadGovernor {
    /// 创建空的控制器（未激活、未接管任何 policy）。
    pub fn new() -> Self {
        Self {
            cfg: CpuLoadGovernorConfig::default(),
            workers: Vec::new(),
            restores: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            touch: Arc::new(AtomicTouchState::new()),
            thermal_cap: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            thermal_free_above: Arc::new(AtomicU32::new(0.80_f32.to_bits())),
            active: false,
            load_buf: None,
        }
    }

/// 下发热保护参数（scheduler_ipc 每 2s 调一次）。clamp_heavy=true（默认）时所有簇恒压到 cap；
/// false 时仅 current_perf < 豁免档 free_above 才压，否则不管。
    pub fn set_thermal_limits(&self, cap: f32, free_above: f32) {
        self.thermal_cap
            .store(cap.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.thermal_free_above
            .store(free_above.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// CLG 当前是否已接管 CPU 频率
    pub fn is_active(&self) -> bool {
        self.active
    }

/// 判定 policy 属于哪个核心组：policy 名即该簇首个 CPU id，配合 `chiri_core_ranges()` 定位，无需读 sysfs；
/// 落在未知区间按 prime 兜底（与 tuned.rs 判定顺序一致）。仅供 per_cluster 覆盖取参数用。
    fn cluster_name_for_policy(policy_id: i32) -> &'static str {
        let r = crate::common::chiri_core_ranges();
        let id = policy_id.max(0) as usize;
        if r.little.contains(&id) {
            "little"
        } else if r.big.contains(&id) {
            "big"
        } else {
            "prime"
        }
    }

    /// min 压到硬件最低 → max 按 perf_init 设初始值 → 为每个 policy 起 Worker 线程。
    pub fn init_policies(&mut self, gov_cfg: &CpuLoadGovernorConfig) {
        self.stop_workers();
        self.cfg = gov_cfg.clone();
        self.normalize_cfg();

        let policies = crate::chiri::get_cpu_policies();
        let core_ranges = crate::common::chiri_core_ranges();

        // 全新 stop/touch 实例；thermal_cap 跨 init 生命周期保持（字段本身复用）
        let stop = Arc::new(AtomicBool::new(false));
        let touch = Arc::new(AtomicTouchState::new());

        for policy in &policies {
            let result = CoreGroupWorker::spawn(
                policy.id,
                // 按核心组取有效参数：per_cluster 覆盖在此生效（未配置则原样克隆）
                self.cfg.effective(Self::cluster_name_for_policy(policy.id)),
                &policy.boost_frequencies,
                core_ranges.clone(),
                touch.clone(),
                self.thermal_cap.clone(),
                self.thermal_free_above.clone(),
                stop.clone(),
            );

            if let Some((restore, (handle, load_tx))) = result {
                self.restores.push(restore);
                self.workers.push(WorkerHandle {
                    policy_id: policy.id,
                    load_tx,
                    handle: Some(handle),
                });
            }
        }

        self.stop = stop;
        self.touch = touch;
        self.active = !self.workers.is_empty();
        if self.active {
            info!(
                "{}",
                t_with_args(
                    "clg-activated",
                    &fluent_args!("count" => self.workers.len().to_string())
                )
            );
        } else {
            warn!("{}", t("clg-no-clusters"));
        }
    }

    /// 释放接管：停止所有 Worker（Worker 线程退出前恢复系统原始状态），清空状态。
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("clg-deactivated"));
        }
        self.stop_workers();
        self.active = false;
    }

/// 热切换配置：停旧 Worker 并用新配置重建（与 init_policies 同路径，current_perf 重置到新 perf_init 并立即写频），
/// 避免息屏期间 current_perf 掉到 ~0 后亮屏恢复时频率从地板缓慢爬升数秒。
    pub fn reload_config(&mut self, gov_cfg: &CpuLoadGovernorConfig) {
        // 保存旧 Worker 的 policy 信息用于重建
        let policy_ids: Vec<i32> = self.workers.iter().map(|w| w.policy_id).collect();
        let policies = crate::chiri::get_cpu_policies();

        self.stop_workers();
        self.cfg = gov_cfg.clone();
        self.normalize_cfg();

        let core_ranges = crate::common::chiri_core_ranges();
        let stop = Arc::new(AtomicBool::new(false));
        let touch = Arc::new(AtomicTouchState::new());

        for policy in &policies {
            if !policy_ids.contains(&policy.id) {
                continue;
            }
            let result = CoreGroupWorker::spawn(
                policy.id,
                // 按核心组取有效参数：per_cluster 覆盖在此生效（未配置则原样克隆）
                self.cfg.effective(Self::cluster_name_for_policy(policy.id)),
                &policy.boost_frequencies,
                core_ranges.clone(),
                touch.clone(),
                self.thermal_cap.clone(),
                self.thermal_free_above.clone(),
                stop.clone(),
            );
            if let Some((restore, (handle, load_tx))) = result {
                self.restores.push(restore);
                self.workers.push(WorkerHandle {
                    policy_id: policy.id,
                    load_tx,
                    handle: Some(handle),
                });
            }
        }

        self.stop = stop;
        self.touch = touch;
        self.active = !self.workers.is_empty();

        debug!(
            "{}",
            t_with_args(
                "clg-config-reloaded",
                &fluent_args!(
                    "up" => format!("{:.2}", self.cfg.up_threshold),
                    "down" => format!("{:.2}", self.cfg.down_threshold),
                    "floor" => format!("{:.2}", self.cfg.perf_floor),
                    "ceil" => format!("{:.2}", self.cfg.perf_ceil)
                )
            )
        );
    }

/// 负载事件入口：core_utils 广播给所有 Worker（非阻塞，通道满丢弃本 tick），Worker 线程内自主决策 + 写频。
/// 本 tick 只做一次 Vec 分配、各 Worker 共享同一 Arc；Worker 只读该切片，不持有跨 tick 引用。
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }
// 复用上一 tick 的广播缓冲：引用计数归 1 且长度一致时原地覆盖，否则新建（两种路径内容与长度与原实现一致）
        let mut shared = match self.load_buf.take() {
            Some(arc) if arc.len() == core_utils.len() => arc,
            _ => Arc::new(core_utils.to_vec()),
        };
        if let Some(v) = Arc::get_mut(&mut shared) {
            v.copy_from_slice(core_utils);
        } else {
            shared = Arc::new(core_utils.to_vec());
        }
        for w in &self.workers {
            w.send_load(Arc::clone(&shared));
        }
        self.load_buf = Some(shared);
    }

/// 触摸事件入口：更新共享触摸升频状态，并广播空负载包立即唤醒全部 Worker 触发 flush，
/// 大核 Worker 本次 flush 即提升性能下限，无需等下一个 160ms tick。
    pub fn on_touch(&mut self) {
        if TOUCH_BOOST_SUSPENDED || !self.active || !self.cfg.touch_boost_enabled {
            return;
        }
        let floor = self.compute_touch_boost_floor();
        self.touch
            .set(floor, Duration::from_millis(self.cfg.touch_boost_ms));
// 空负载包仅触发 flush（大核应用触摸地板，小核写频去重无副作用）；try_send 满时丢弃——排队的真实负载同样会应用触摸升频，不丢结果
        for w in &self.workers {
            w.send_load(Arc::new(Vec::new()));
        }
        debug!(
            "{}",
            t_with_args(
                "clg-touch-boost",
                &fluent_args!(
                    "floor" => format!("{:.2}", floor),
                    "ms" => self.cfg.touch_boost_ms.to_string()
                )
            )
        );
    }

    /// 校验并规范化配置：防止 perf_floor > perf_ceil / NaN 导致 f32::clamp panic
    fn normalize_cfg(&mut self) {
        let floor = self.cfg.perf_floor;
        let ceil = self.cfg.perf_ceil;
        if floor.is_finite() && ceil.is_finite() && floor > ceil {
            warn!(
                "{}",
                t_with_args(
                    "clg-perf-clamped",
                    &fluent_args!(
                        "floor" => format!("{:.2}", floor),
                        "ceil" => format!("{:.2}", ceil)
                    )
                )
            );
        }
        self.cfg.normalize();
    }

    /// 停止所有 Worker：通知停止 → 等待线程退出 → 恢复系统状态。
    /// Worker 线程退出前会自行恢复其 policy，此处仅做 join 确保退出完成。
    fn stop_workers(&mut self) {
        // 通知所有 Worker 停止
        self.stop.store(true, Ordering::Release);

        // join 所有 Worker 线程（Worker 退出前自行 restore_policy）
        for w in &mut self.workers {
            w.join();
        }
        self.workers.clear();
        self.restores.clear();
    }

/// 计算触摸升频的大核性能下限：以 perf_init 为基线（Worker 架构下无法直读 current_freq，Worker 持有 ClusterState）
/// 加 touch_boost_tiers 档估算，一个窗口期内保持不变；Worker flush 时会读取共享 floor 并 clamp 到正确范围。
    fn compute_touch_boost_floor(&self) -> f32 {
        let base = self
            .cfg
            .perf_init
            .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
        // 向上抬 touch_boost_tiers 档：频率表通常 15-25 档，单档 ≈ 0.05 性能比（保守估算）
        let tier_step = 0.05;
        (base + self.cfg.touch_boost_tiers as f32 * tier_step).min(self.cfg.perf_ceil)
    }
}
