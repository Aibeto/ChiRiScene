//! tuned.rs: [restore] [governor] [init_release] [load_freq]特调执行器（TunedGovernor）：
//! akmode / playback / daily 等模式共用同一套连续控制，参数组按模式名从 Config.tuned_profiles 分派，差异只在参数

use crate::chiri::config::SpecialTunedConfig;
use crate::monitor::FasSignal;
use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;

/// 跨核迁移成本（ns，全局节点）：稳态场景（视频）接管期间调高、退出时恢复
const MIGRATION_COST_PATH: &str = "/proc/sys/kernel/sched_migration_cost_ns";
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::t_with_args;

// [thermal_ceil]
/// 热保护 cap 的进程级镜像（f32 bit pattern 存 `AtomicU32`，与 CLG 的 `thermal_cap` 同口径）：热保护 cap 原本只下发 CLG，
/// tuned（playback/akmode/daily）对温度零响应——播放态占2026-09-27 会话 68% 能量却没有任何温度响应（结构缺口）热块在 cap 变化时经`set_thermal_cap` 写入，
/// `on_load_update` 每 tick 读一次，无额外轮询
static THERMAL_CAP: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());
/// 热态 tuned 响应总开关与性能下限（`Thermal.tuned_resp_enabled` / `tuned_thermal_floor`）：由 `Config::load` 同步，热重载即时生效；
/// 关闭 = 精确回到加这段之前（tuned 零温度响应）
static THERMAL_RESP_ENABLED: AtomicBool = AtomicBool::new(true);
static THERMAL_FLOOR: AtomicU32 = AtomicU32::new(0.55_f32.to_bits());

/// 热块下发 cap（与 `CpuGovernor::set_thermal_limits` 同一处调用，值相同）
pub fn set_thermal_cap(cap: f32) {
    THERMAL_CAP.store(cap.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// `Config::load` 同步开关与下限（启动 / 热重载均调用）
pub fn set_thermal_response(enabled: bool, floor: f32) {
    THERMAL_RESP_ENABLED.store(enabled, Ordering::Relaxed);
    THERMAL_FLOOR.store(floor.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// 热态有效上限 = `min(档位天花板, max(cap, floor))`
/// - 用 `max(cap, floor)` 而非直接套 cap：硬档 0.40 直接砍到播放态有掉帧风险，且播放态的帧信号
/// （2026-09-27 起有，但只是**离线**直方图：`event` 行 `decision=playback_fps`，见 `.cursor/commands/devimp-log-analysis.md`）
/// **在线没有判据**——不可能拿它当本轮钳制的反馈，floor 仍把最坏情况钉在 0.55；
/// **[有意保留]** 视频解码需要稳定档位、不接受随温阶跃（硬档 0.40 的跳变是掉帧主因），故播放态在
/// ≥45°C 时大核可比 CLG（此时已压到 0.40）更热——这是取舍不是漏钳。代价是边充边放这类叠加发热的
/// 场景可能整机持续升温，待真机温度曲线验证（`agentsdocs/07-todos.md` 「播放态『边充边放』温度曲线复核」），
/// 首要嫌疑即该地板值 0.55；
/// - `cap >= floor` 时退化为 `min(天花板, cap)`：软限 0.85 对 tuned 本就是 no-op（tuned 各簇
/// 天花板 little 0.55 / big 0.75 / prime 0.50 全部 <= 0.85），与实测口径一致；
/// 无压制（cap=1.0）时结果就是档位天花板本身，行为零变化
#[inline]
fn thermal_ceiling(profile_ceil: f32) -> f32 {
    if !THERMAL_RESP_ENABLED.load(Ordering::Relaxed) {
        return profile_ceil;
    }
    let cap = f32::from_bits(THERMAL_CAP.load(Ordering::Relaxed));
    let floor = f32::from_bits(THERMAL_FLOOR.load(Ordering::Relaxed));
    profile_ceil.min(cap.max(floor))
}

// [restore]
/// 单个 policy 的 governor/min/max 快照：接管时保存，release 时恢复；字段读取失败为 None，恢复时跳过该字段
struct PolicyRestore {
    policy_id: i32,
    governor: Option<String>,
    min_freq: Option<u32>,
    max_freq: Option<u32>,
}

/// 单个 policy 的运行时状态：无档位连续 max 控制
struct ClusterState {
    core_name: &'static str,
    /// 内核可用频率（kHz，升序去重），目标上限在该表中就近取档
    available_freqs: Vec<u32>,
    max_writer: FastWriter,
    current_max: u32,
    /// 降频保持计时起点（目标首次低于当前上限的时刻）；None = 无待执行降频
    down_since: Option<Instant>,
    /// 降频保持期记录的目标档位：目标明显回升时重置计时（防止逼近移动目标）
    down_target: u32,
    /// 用于决策的负载 EMA（util_smoothing < 1 时启用；-1 = 未初始化）
    ema_util: f32,
    // [dwell] 写频滞回状态（Phase 2，与 CLG 同口径）
/// 上次**实际写频**成功时刻：dwell 以它计时；None = 接管初写后尚未写过（初写不启动时钟，首次受控写频立即生效）
    last_write_at: Option<Instant>,
    /// 上次实际写频方向：升 = 1、降 = -1、未写过 = 0（翻摆判定基准）
    last_write_dir: i8,
    /// 上次写频失败：下次写入视为防篡改补写、豁免滞回（重试时序不变）
    last_failed: bool,
}

/// 按 affected_cpus 首个 CPU ID 判定核心组，区间随命中 SoC 变化；由 common::chiri_core_ranges() 统一提供，新增 SoC 只需在那里追加
fn core_name_for(affected: &[usize]) -> Option<&'static str> {
    let first = affected.iter().copied().min()?;
    let r = crate::common::chiri_core_ranges();
    if r.little.contains(&first) {
        Some("little")
    } else if r.big.contains(&first) {
        Some("big")
    } else if r.prime.contains(&first) {
        Some("prime")
    } else {
        None
    }
}

    /// 明日方舟特调（akmode）控制器：独立于 CLG 的动态限频调度，**无档位**
    /// 每个负载 tick 按核心组实时负载直接计算 scaling_max_freq 目标上限：
    /// target_ratio = clamp(组内最大核心占用率 × headroom, perf_floor, 1.0)
    /// target_max   = 频率表中 ≤ ratio × 硬件最高的最大档位（floor 对齐）
    /// 升频立即执行（响应性优先）；降频须偏离超 hysteresis（绝对死区 = hw_max × hysteresis）且持续 down_hold_ms
    /// 替代原四档 core-count 阈值方案：后者在「少数线程高占用」负载下升频凑不齐、降频持续满足，max 单边下探到最低频
// [governor]
pub struct TunedGovernor {
    cfg: SpecialTunedConfig,
/// 当前接管的特调模式名（akmode / playback / daily …）：日志用；release 后保留（deactivated 日志要报是哪个模式退出）
    mode: String,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
/// 接管前 `sched_migration_cost_ns` 原值（全局节点，只快照一次）：仅配置了 `migration_cost_ns` 且写成功才非 None，release 时写回
    migration_cost_restore: Option<String>,
    active: bool,
    /// 调试日志计数，每 25 tick 打一次摘要
    log_counter: u32,
/// 帧源信号（见 `monitor::FasSignal`）：接管 `playback` 时置位**播放态旁路采样**谓词，
/// 让 fps 探针在非 FAS 会话里也挂帧源做帧间隔直方图（P2-3：播放态原本零帧信号，任何播放态调参
/// 都无法用流畅度判定）；只置位/清零这一位，不碰 FAS 激活位（它由 FasManager 独占）
    fas_signal: Arc<FasSignal>,
}

impl TunedGovernor {
    pub fn new(ak_active: Arc<AtomicBool>, fas_signal: Arc<FasSignal>) -> Self {
        Self {
            cfg: SpecialTunedConfig::default(),
            mode: String::new(),
            ak_active,
            clusters: Vec::new(),
            restore: Vec::new(),
            migration_cost_restore: None,
            active: false,
            log_counter: 0,
            fas_signal,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：1) 先 release 清上次状态；2) 逐 policy 读可用频率与 affected_cpus 判定核心组；
    /// 3) 快照 governor/min/max，写 schedutil、min 压硬件最低（与 CLG 同构，只调 max）；4) 初始 max = 硬件最高（接管瞬间多为场景切换，先给满上限由负载回落）
    /// 返回 true = 成功接管；false = 无可用 cluster（配置错误或硬件不支持）
    // [init_release]
    pub fn init_policies(&mut self, mode: &str, cfg: &SpecialTunedConfig) -> bool {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        self.mode = mode.to_string();

        let policies = crate::chiri::get_cpu_policies();

        for policy in &policies {
            let pid = policy.id;
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
                // scaling_available_frequencies 读不到/空表：按 policy 首核映射核心组，回退 soc.yaml [freq_khz] 兜底（只补表，
                // 不改取档/floor 对齐逻辑）
                match crate::common::soc_freq_fallback_for_policy(pid) {
                    Some(f) => freqs = f,
                    None => continue,
                }
            }
            freqs.sort_unstable();
            freqs.dedup();

            let affected = Self::read_affected_cpus(pid);
            if affected.is_empty() {
                continue;
            }

            let name = match core_name_for(&affected) {
                Some(n) => n,
                None => {
                    warn!(
                        "{}",
                        t_with_args(
                            "tuned-cluster-skipped",
                            &fluent_args!(
                                "mode" => self.mode.clone(),
                                "pid" => pid.to_string(),
                                "reason" => "unknown-cpu-range".to_string()
                            )
                        )
                    );
                    continue;
                }
            };

            let mut max_writer = FastWriter::new(max_path.clone());
            if !max_writer.is_valid() {
                warn!(
                    "{}",
                    t_with_args(
                        "tuned-cluster-skipped",
                        &fluent_args!(
                            "mode" => self.mode.clone(),
                            "pid" => pid.to_string(),
                            "reason" => "writer-invalid".to_string()
                        )
                    )
                );
                continue;
            }

            // 快照系统原始状态，release 时恢复（必须位于写入之前）
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
            self.restore.retain(|r| r.policy_id != pid);
            self.restore.push(PolicyRestore {
                policy_id: pid,
                governor,
                min_freq,
                max_freq,
            });

            // 统一 schedutil + min 压到硬件最低（避免设备出厂高 min 导致频率降不下去）
            let _ = crate::utils::try_write_file(&gov_path, "schedutil");
            let min_hw = freqs[0];
            let _ = crate::utils::try_write_file(&min_path, min_hw.to_string());

            let current_max = *freqs.last().unwrap();
            let _ = max_writer.write_value_force(current_max);

            self.clusters.push(ClusterState {
                core_name: name,
                available_freqs: freqs,
                max_writer,
                current_max,
                down_since: None,
                down_target: 0,
                ema_util: -1.0,
                // [dwell] 写频滞回状态：接管初写不启动 dwell 时钟
                last_write_at: None,
                last_write_dir: 0,
                last_failed: false,
            });
        }

        self.apply_migration_cost();

        self.active = !self.clusters.is_empty();
        if self.active {
            info!(
                "{}",
                t_with_args("tuned-init", &fluent_args!("mode" => self.mode.clone()))
            );
            info!(
                "{}",
                t_with_args(
                    "tuned-activated",
                    &fluent_args!("mode" => self.mode.clone())
                )
            );
            self.ak_active.store(true, Ordering::Relaxed);
// 播放态帧源谓词：只有 `playback` 特调置位（akmode 是游戏、FAS 侧自有帧驱动，daily 组已移除）；
// 非 playback 模式恒清零——`init_policies` 开头已 `release()` 清过一次，这里按模式重设，两种模式切换不留残位
            self.fas_signal.set_playback(self.mode == "playback");
        } else {
            warn!(
                "{}",
                t_with_args(
                    "tuned-no-clusters",
                    &fluent_args!("mode" => self.mode.clone())
                )
            );
        }
        self.active
    }

    /// 释放接管：恢复各 policy 的 governor/min/max，清空状态
    pub fn release(&mut self) {
        if self.active {
            info!(
                "{}",
                t_with_args(
                    "tuned-deactivated",
                    &fluent_args!("mode" => self.mode.clone())
                )
            );
        }
        self.active = false;
        self.restore.retain(|r| !Self::restore_policy(r));
        self.restore_migration_cost();
        self.clusters.clear();
        self.ak_active.store(false, Ordering::Relaxed);
// 播放态帧源谓词同点清零（幂等：非 playback 模式本就没置位）；与 set(true) 一样唤醒 fps 探针，
// 由它摘掉 uprobe 回零开销待机——不留「模式已退但探针还挂着」的窗口
        self.fas_signal.set_playback(false);
        self.log_counter = 0;
    }

/// 恢复单个 policy 的原 governor/min/max，返回是否全部写成功（失败保留快照下次重试）；写序先 max 后 min，避免中间态 min > max
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
        if let Some(max) = r.max_freq {
            if crate::utils::write_to_file(&max_path, max.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(min) = r.min_freq {
            if crate::utils::write_to_file(&min_path, min.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(gov) = &r.governor {
            if crate::utils::write_to_file(&gov_path, gov.as_bytes()).is_err() {
                all_ok = false;
            }
        }
        all_ok
    }

/// 接管期间按需调高 sched_migration_cost_ns：稳态负载下原值偏小会频繁跨核搬迁（省搬迁开销与 cache 失效）；原值进 migration_cost_restore，release 写回；
/// 写失败当未配置，不影响接管
    fn apply_migration_cost(&mut self) {
        let Some(want) = self.cfg.migration_cost_ns.filter(|v| *v > 0) else {
            return;
        };
        let orig = fs::read_to_string(MIGRATION_COST_PATH)
            .ok()
            .map(|s| s.trim().to_string());
        if crate::utils::try_write_file(MIGRATION_COST_PATH, &want.to_string()).is_ok() {
            self.migration_cost_restore = orig;
        }
    }

    /// 恢复接管前的 `sched_migration_cost_ns`（只对曾经写成功的那次生效）
    fn restore_migration_cost(&mut self) {
        if let Some(orig) = self.migration_cost_restore.take() {
            let _ = crate::utils::try_write_file(MIGRATION_COST_PATH, &orig);
        }
    }

    /// 热重载：tuned_profiles.yaml 参数变化后更新控制参数（max 动态状态保持不变）
    // [load_freq]
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        debug!(
            "{}",
            t_with_args(
                "tuned-config-reloaded",
                &fluent_args!("mode" => self.mode.clone())
            )
        );
    }

    /// 频率表中找「≤ ratio × 硬件最高」的最大档位（floor 对齐）：写的是 scaling_max 上限），落点不得高于计算目标（ceil 会比决策多给一档；目标落两档之间时内核本就把 max 向下 clamp，
    /// 先对齐再写账实一致、同值去重才有效）
    fn freq_for_ratio(freqs: &[u32], ratio: f32) -> u32 {
        let hw_max = *freqs.last().unwrap_or(&0);
        let want = (hw_max as f32 * ratio) as u32;
        freqs
            .iter()
            .rev()
            .copied()
            .find(|&f| f <= want)
            .unwrap_or_else(|| *freqs.first().unwrap_or(&0))
    }

    // [dwell] 受控写频（与 CLG write_freq 同口径）：死区由调用方 hysteresis 保证（|target−current| 超死区才调用），
    // 这里做「最小驻留内方向翻摆延迟」与写失败防篡改补写豁免；接管初写/恢复不走本函数。返回是否写成功：成功才前移 current_max，失败/被延迟由下一 tick 补写 target
    fn gated_write(c: &mut ClusterState, target: u32, dwell_ms: u64) -> bool {
        let dir: i8 = if target > c.current_max { 1 } else { -1 };
        if !c.last_failed && c.last_write_dir != 0 && dir != c.last_write_dir {
            if let Some(at) = c.last_write_at {
                if at.elapsed() < Duration::from_millis(dwell_ms) {
                    return false;
                }
            }
        }
        let old = c.current_max;
        let ok = c.max_writer.write_value_force(target);
        if ok {
            c.current_max = target;
            c.last_write_at = Some(Instant::now());
            c.last_write_dir = if target > old { 1 } else { -1 };
            c.last_failed = false;
        } else {
            c.last_failed = true;
        }
        ok
    }

    /// 无档位动态限频入口，每个 SystemLoadUpdate（特调 40ms）触发一次
    /// 每个核心组独立决策（语义与 CLG 对齐）：up = 目标 > 当前 max + hysteresis，立即上调；
    /// down_wait = 目标 < 当前 max − hysteresis，保持计时中（down_hold_ms）；down = 期满把上限收到目标档位；
    /// hold = 目标在死区内（或保持期内目标回升，取消等待）
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }

        let ranges = crate::common::chiri_core_ranges();
        let now = Instant::now();
        // 借用配置不克隆：SpecialTunedConfig 唯一堆字段是 per_cluster（HashMap，键为组名），每 tick 克隆要复制整张表；
        // gated_write 是关联函数不借 &mut self，与 &self.cfg 不冲突
        let cfg = &self.cfg;

        // main_ tick 行数据（每核心组一行：cluster / util / decision / cur_max / hw_max），只在开发记录开启时收集（关闭时零开销、零分配、零写入）
        let diag = crate::logger::diag_active();
        let mut main_rows: Vec<(&'static str, String, &'static str, u32, u32)> = Vec::new();

        for c in &mut self.clusters {
            let range: &std::ops::Range<usize> = if c.core_name == "little" {
                &ranges.little
            } else if c.core_name == "big" {
                &ranges.big
            } else {
                &ranges.prime
            };
            // 该核心组的有效参数（per_cluster 覆盖后的最终值）：三簇负载形态不同，必须按组取
            let p = cfg.for_cluster(c.core_name);
            let group_util = range
                .clone()
                .filter_map(|cpu| core_utils.get(cpu).copied())
                .fold(0.0_f32, f32::max);
            // 负载平滑（EMA，util_smoothing=1.0 关闭）：噪声型抖动负载（视频/轻载）滤瞬时尖峰，避免上限均值高于带平滑的 CLG；游戏默认 1.0 保持原始 util（瞬时升频是响应性的一部分）
            // 时间常数（α=0.35）≈ 采样间隔 × (1-α)/α，40ms tick ≈ 75ms
            let util = if p.util_smoothing >= 0.999 || c.ema_util < 0.0 {
                group_util
            } else {
                p.util_smoothing * group_util + (1.0 - p.util_smoothing) * c.ema_util
            };
            c.ema_util = util;

            let hw_max = *c.available_freqs.last().unwrap_or(&0);
            // perf_ceil 是天花板（默认 1.0 = 与加该字段前完全一致）；[thermal_ceil] 热态再压一层
            let ceil = thermal_ceiling(p.perf_ceil).max(p.perf_floor);
            let target_ratio = (util * p.headroom).clamp(p.perf_floor, ceil);
            let target_max = Self::freq_for_ratio(&c.available_freqs, target_ratio);
            let hyst_freq = (hw_max as f32 * p.hysteresis) as u32;

            let decision;
            if target_max > c.current_max + hyst_freq {
                // 升频：立即执行（响应性优先）并取消进行中的降频等待；写成功才前移状态，失败时下一 tick target 仍超死区 → up 重试（与 CLG 对齐）[dwell] 经写频滞回（同 CLG）：
                // 翻摆延迟、写失败补写豁免
                Self::gated_write(c, target_max, cfg.write_dwell_ms);
                c.down_since = None;
                decision = "up";
            } else if target_max < c.current_max.saturating_sub(hyst_freq) {
                // 降频：目标须持续 down_hold_ms；计时重置只在目标明显回升（> down_target + hyst）时发生——相邻档位小幅跳动不重置，
                // 否则上限卡在高位降不下来（连续控制退化成只升不降）；目标继续下探只更新 down_target（执行用最新档），不重置计时
                match c.down_since {
                    None => {
                        c.down_since = Some(now);
                        c.down_target = target_max;
                        decision = "down_wait";
                    }
                    Some(_) if target_max > c.down_target.saturating_add(hyst_freq) => {
                        c.down_since = Some(now);
                        c.down_target = target_max;
                        decision = "down_wait";
                    }
                    Some(since) => {
                        c.down_target = target_max;
                        if now.duration_since(since).as_millis() as u64 >= p.down_hold_ms {
                            // 写成功才前移状态并结束等待：失败/被 dwell 延迟保留计时起点，下一 tick（elapsed 仍满）立即重试[dwell] 经写频滞回（同 CLG）：翻摆延迟、
                            // 写失败补写豁免
                            if Self::gated_write(c, target_max, cfg.write_dwell_ms) {
                                c.down_since = None;
                            }
                            decision = "down";
                        } else {
                            decision = "down_wait";
                        }
                    }
                }
            } else {
                // 死区内：目标与当前上限一致，取消进行中的降频等待
                c.down_since = None;
                // [dwell] 目标收敛进死区：上次写失败的补写诉求消失，清标记防悬挂（否则任意久之后的下一次真实写频会白豁免滞回一次）
                c.last_failed = false;
                decision = "hold";
            }

            if diag {
                main_rows.push((
                    c.core_name,
                    format!("{:.2}", util),
                    decision,
                    c.current_max,
                    hw_max,
                ));
            }
        }

        // main_ tick 行（开发记录开启时才有 IO）：cur_freq_khz 写当前动态 max、max_freq_khz 写硬件最高（kHz），over/under 无档位语义恒 0；
        // util 列写决策用负载（util_smoothing < 1 时为 EMA 后的值），离线回放该列时不能当原始 util 二次平滑
        if diag {
            for (name, util, decision, cur_max, hw_max) in &main_rows {
                crate::logger::main_tick(
                    name,
                    util,
                    0,
                    0,
                    "-",
                    "-",
                    &cur_max.to_string(),
                    &hw_max.to_string(),
                    decision,
                    0,
                    0,
                    "-",
                    false,
                );
            }
        }

        self.log_counter += 1;
        // debug 心跳（独立于开发诊断，日志通道随时可用）：每 25 tick 汇总各簇 util/max，供诊断关闭时观测汇总从 clusters 现算（ema_util / current_max 就是本
        // tick 决策所用值）：dev_record 关闭时 main_rows 是空表，不能当数据源
        if self.log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
            let summary = self
                .clusters
                .iter()
                .map(|c| {
                    format!(
                        "{}={}MHz({:.2})",
                        c.core_name,
                        c.current_max / 1000,
                        c.ema_util
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            debug!(
                "{}",
                t_with_args(
                    "tuned-tick-log",
                    &fluent_args!("mode" => self.mode.clone(), "state" => summary)
                )
            );
        }
    }

    /// 读 policy 的 affected_cpus
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
