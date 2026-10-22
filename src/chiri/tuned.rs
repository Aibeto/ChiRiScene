//! tuned.rs: [restore] [governor] [init_release] [load_freq]
//! 特调执行器（TunedGovernor）：akmode / playback / daily 等模式共用同一套连续控制，参数组按模式名从 Config.tuned_profiles 分派，差异只在参数

use crate::chiri::config::SpecialTunedConfig;
use crate::monitor::FasSignal;
use crate::utils::FastWriter;
use log::{debug, error, info, warn};
use std::fs;

/// 跨核迁移成本（ns，全局节点）：稳态场景（视频）接管期间调高、退出时恢复
const MIGRATION_COST_PATH: &str = "/proc/sys/kernel/sched_migration_cost_ns";

/// 特调 tick 周期（ms）：与 `monitor::cpu_monitor` 的 `SAMPLE_MS_TUNED` 同一个值（那边是私有常量，且本文件不应反向依赖 monitor 层）
/// [sweet_cap] 的越顶放行窗口按它换算 tick → 毫秒；两处不一致会让窗口长度与设定 tick 数不符
const TUNED_TICK_MS: u64 = 40;

/// [sweet_cap] 「本 SoC 无 EEM 曲线」告警的进程级一次性门：甜点取自 `soc.yaml` 的 frontier_policy，
/// 无该段的机型每 tick 都会走到缺失分支，不去重就会把日志刷满
static SWEET_CAP_NO_CURVE_LOGGED: AtomicBool = AtomicBool::new(false);
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::t_with_args;

// [thermal_ceil]
/// 热保护 cap 的进程级镜像（f32 bit pattern 存 `AtomicU32`，与 CLG 的 `thermal_cap` 同口径）：热保护 cap 原本只下发 CLG，tuned（playback/akmode/daily）对温度零响应
/// 热块在 cap 变化时经 `set_thermal_cap` 写入，`on_load_update` 每 tick 读一次，无额外轮询
static THERMAL_CAP: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());
/// 热态 tuned 响应总开关与性能下限（`Thermal.tuned_resp_enabled` / `tuned_thermal_floor`）：由 `Config::load` 同步，热重载即时生效，关闭 = tuned 零温度响应
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

/// 热态有效上限 = `min(档位天花板, max(cap, floor))`；用 `max(cap, floor)` 而非直接套 cap：硬档 0.40 直接砍到播放态有掉帧风险，floor 把最坏情况钉在 0.55
/// `cap >= floor` 时退化为 `min(天花板, cap)`：软限 0.85 对 tuned 本就是 no-op（各簇天花板 little 0.55 / big 0.75 / prime 0.50）
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
    /// 该 policy 的内核编号（同一 cluster 名下可能有多个 policy，日志节流据此分桶）
    policy_id: i32,
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
    // [up_confirm] 严格升频审批：目标高于当前时需连续 N tick 才抬升（降频路径不变）
    /// 连续升频意图计数（确认满 up_confirm_ticks 才写），只在降频分支与「目标落入死区」分支清零，升频连续满足时不复位——与 CLG 的 up_rate_limit_ticks 同口径
    up_wait: u32,
    // [steady_decay] 稳态下探状态（只作写侧偏移，不回写已应用状态）
    /// 当前稳态下探量（比例，0 = 无下探）
    steady_decay: f32,
    /// 连续稳定 tick 计数（满 steady_decay_stable_ticks 才启动积分）
    steady_streak: u32,
    // [dwell] 写频滞回状态（Phase 2，与 CLG 同口径）
    /// 上次**实际写频**成功时刻：dwell 以它计时；None = 接管初写后尚未写过（初写不启动时钟，首次受控写频立即生效）
    last_write_at: Option<Instant>,
    /// 上次实际写频方向：升 = 1、降 = -1、未写过 = 0（翻摆判定基准）
    last_write_dir: i8,
    /// 上次写频失败：下次写入视为防篡改补写、豁免滞回（重试时序不变）
    last_failed: bool,
    // [sweet_cap] 甜点封顶的越顶放行状态
    /// 本簇占用曾达 `sweet_cap_release_units` 因而临时放开封顶时的截止时刻（None = 未放行）
    /// 放行窗口内经比例路径决策，窗口结束自动回到甜点封顶；`hold_ticks` 为 0 时不置位（低于阈值即收回）
    sweet_release_until: Option<Instant>,
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

/// 明日方舟特调（akmode）控制器：独立于 CLG 的动态限频调度，无档位
/// 每个负载 tick 按核心组实时负载算 scaling_max_freq 上限：target_ratio = clamp(组内最大核心占用率 × headroom, perf_floor, 1.0)，升频立即执行，降频须偏离超 hysteresis 且持续 down_hold_ms
// [governor]
pub struct TunedGovernor {
    cfg: SpecialTunedConfig,
    /// 当前接管的特调模式名（akmode / playback / daily …）：日志用；release 后保留（deactivated 日志要报是哪个模式退出）
    mode: String,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 160ms）
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
    /// 接管前 `sched_migration_cost_ns` 原值（全局节点，只快照一次）：仅配置了 `migration_cost_ns` 且写成功才非 None，release 时写回
    migration_cost_restore: Option<String>,
    active: bool,
    /// 调试日志计数，每 25 tick 打一次摘要
    log_counter: u32,
    /// 帧源信号 `monitor::FasSignal`：接管 `playback` 时置位播放态旁路采样谓词，让 fps 探针在非 FAS 会话里也挂帧源做帧间隔直方图
    /// 只置位/清零这一位，不碰 FAS 激活位（它由 FasManager 独占）
    fas_signal: Arc<FasSignal>,
    // [activity_params]
    /// 当前 activity 的参数覆盖（进程级快照的本地副本），与 CLG 侧同一通道（`app_detect::current_activity_params`）
    /// `None` = 无覆盖；`Some` = 与 `self.cfg` 叠加后的有效配置已落在 `self.eff_cfg`
    /// 只在覆盖**实例变化**时重算（Arc 判等），稳态每 tick 只花一次加锁 + 一次指针比较
    act_override: Option<Arc<crate::monitor::config::ActivityParamOverride>>,
    /// 叠加 activity 覆盖后的有效特调参数（无覆盖时与 `self.cfg` 相同）
    eff_cfg: SpecialTunedConfig,
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
            // [activity_params] 初值：无覆盖，有效配置 = 特调配置本身（与加该机制前逐位一致）
            act_override: None,
            eff_cfg: SpecialTunedConfig::default(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：1) 先 release 清上次状态；2) 逐 policy 读可用频率与 affected_cpus 判定核心组；3) 快照 governor/min/max，写默认调速器（common.rs [governor] 选型）、min 压硬件最低（只调 max）；4) 初始 max = 硬件最高
    /// 返回 true = 成功接管；false = 无可用 cluster（配置错误或硬件不支持）
    // [init_release]
    pub fn init_policies(&mut self, mode: &str, cfg: &SpecialTunedConfig) -> bool {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        // [activity_params] 接管时先按模式原参数派生有效配置（覆盖由首 tick 的 refresh 叠加）
        self.act_override = None;
        self.eff_cfg = self.cfg.clone();
        self.mode = mode.to_string();

        let policies = crate::chiri::get_cpu_policies();

        // [init_release] 降级接管标记：频率表走 soc.yaml 兜底、或调速器/min 初写失败 —— 这些
        // 情形下调频决策照常生效、输出数据与正常接管完全同形，事后必须留痕
        let mut init_degraded = false;

        for policy in &policies {
            let pid = policy.id;
            // [init_release] 本簇是否走了降级路径（兜底频率表 / 初写失败），循环尾统一汇总
            let mut freq_fallback = false;
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
                    Some(f) => {
                        freq_fallback = true;
                        freqs = f;
                    }
                    None => {
                        // [init_release] 频率表与 soc.yaml 兜底双双取不到：本 policy 静默跳过（与 active 的
                        // 区别在输出数据上完全不可见），必须留痕
                        warn!(
                            "{}",
                            t_with_args(
                                "tuned-cluster-skipped",
                                &fluent_args!(
                                    "mode" => self.mode.clone(),
                                    "pid" => pid.to_string(),
                                    "reason" => "no-freq-table".to_string()
                                )
                            )
                        );
                        continue;
                    }
                }
            }
            freqs.sort_unstable();
            freqs.dedup();

            let affected = Self::read_affected_cpus(pid);
            if affected.is_empty() {
                // [init_release] affected_cpus 读不到/为空（节点缺失或格式异常）：静默跳过，留痕
                warn!(
                    "{}",
                    t_with_args(
                        "tuned-cluster-skipped",
                        &fluent_args!(
                            "mode" => self.mode.clone(),
                            "pid" => pid.to_string(),
                            "reason" => "no-affected-cpus".to_string()
                        )
                    )
                );
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

            // 统一默认调速器（common.rs [governor] 选型）+ min 压到硬件最低（避免设备出厂高 min 导致频率降不下去）
            // 这里用 write_to_file（返回 Result）而非 try_write_file（恒 Ok）：节点缺失(ENOENT)时后者只打 debug，INFO 级下完全看不见接管缺了一条腿
            let mut init_writes_ok = true;
            if let Err(e) =
                crate::utils::write_to_file(&gov_path, crate::common::cpu_governor_for_policy(pid))
            {
                init_writes_ok = false;
                warn!(
                    "[Tuned] {} P{} default governor write failed: {}",
                    self.mode, pid, e
                );
            }
            let min_hw = freqs[0];
            if let Err(e) = crate::utils::write_to_file(&min_path, min_hw.to_string()) {
                init_writes_ok = false;
                warn!(
                    "[Tuned] {} P{} scaling_min_freq write to hw min ({}kHz) failed: {}",
                    self.mode, pid, min_hw, e
                );
            }
            // [init_release] 调速器/min 写失败 = 该簇接管不完整（max 控制不被内核跟随 / 高 min 卡住降频）
            if !init_writes_ok {
                freq_fallback = true;
            }

            let current_max = *freqs.last().unwrap();
            let write_ok = max_writer.write_value_force(current_max);
            if !write_ok {
                // [init_release] 接管初写 max 失败：该簇上限停留在系统原值（可能非硬件最高），
                // 后续 tick 只在目标偏离超 hysteresis 时才动它，低负载下可长期不被纠正
                warn!(
                    "[Tuned] {} P{} initial scaling_max_freq ({}kHz) write failed, cap stays at system value",
                    self.mode, pid, current_max
                );
            }

            self.clusters.push(ClusterState {
                policy_id: pid,
                core_name: name,
                available_freqs: freqs,
                max_writer,
                // [init_release] 初写成功 = 账面即真值；失败则回读系统当下上限（前移会让后续
                // hysteresis 判据与账面对不上，低负载下这个偏差可能长期不被纠正）
                current_max: if write_ok {
                    current_max
                } else {
                    fs::read_to_string(&max_path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(current_max)
                },
                down_since: None,
                down_target: 0,
                ema_util: -1.0,
                // [up_confirm]/[steady_decay] 状态初值：无下探、无确认进度
                up_wait: 0,
                steady_decay: 0.0,
                steady_streak: 0,
                // [dwell] 写频滞回状态：接管初写不启动 dwell 时钟
                last_write_at: None,
                last_write_dir: 0,
                last_failed: false,
                // [sweet_cap] 接管初不处于越顶放行窗口
                sweet_release_until: None,
            });
            // [init_release] 本簇走了兜底频率表或初写失败：接管程度不完整但仍在 active，
            // 与完全正常接管在输出数据上同形，必须留痕
            if freq_fallback {
                init_degraded = true;
            }
        }

        self.apply_migration_cost();
        if init_degraded {
            warn!(
                "[Tuned] {} takeover incomplete: fallback freq table or initial writes failed",
                self.mode
            );
        }

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
            // 播放态帧源谓词：只有 `playback` 特调置位（akmode 是游戏、FAS 侧自有帧驱动，daily 组已移除）
            // 非 playback 模式恒清零——`init_policies` 开头已 `release()` 清过一次，这里按模式重设，切换不留残位
            self.fas_signal.set_playback(self.mode == "playback");
        } else {
            // [init_release] 无有效集群 = 特调整条链路失效（所有调用点都会进 5 分钟冷却 + CLG 接管），
            // 不靠本进程重试自愈、需人介入修配置/硬件 —— 按契约属 error，稳态运行时不得出现
            error!(
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

    /// 接管期间按需调高 sched_migration_cost_ns：稳态负载下原值偏小会频繁跨核搬迁（省搬迁开销与 cache 失效）；原值进 migration_cost_restore，release 写回，写失败当未配置
    fn apply_migration_cost(&mut self) {
        let Some(want) = self.cfg.migration_cost_ns.filter(|v| *v > 0) else {
            return;
        };
        let orig = fs::read_to_string(MIGRATION_COST_PATH)
            .ok()
            .map(|s| s.trim().to_string());
        // 用 write_to_file（真实 Result）而非 try_write_file（吞错恒返 Ok），只在真写成功时才记 migration_cost_restore 快照
        // 否则即便一个字节都没写下去也会当「已改、退出需还原」
        if crate::utils::write_to_file(MIGRATION_COST_PATH, &want.to_string()).is_ok() {
            self.migration_cost_restore = orig;
        }
    }

    /// 恢复接管前的 sched_migration_cost_ns，只对写成功的那次生效
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
        // [activity_params] 模式参数变了，有效配置要按当前覆盖重算（act_override 置 None 让下一 tick 必重算）
        self.act_override = None;
        self.eff_cfg = self.cfg.clone();
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

    // [sweet_cap] 甜点封顶：把「写侧比例」的天花板从 perf_ceil 改到甜点比例，
    // 语义 = 「这一簇再往上加频，单位功耗换来的算力已经不值」，而不是按 util 线性外推。
    /// 取某簇的甜点封顶频率（kHz）；返回 None = 本簇不封顶（`sweet_cap: off` 或 data 缺失）
    ///
    /// `auto` 走 EEM 曲线（`energy_model::curve` 的甜点档，跨机型自适配，数据来自 soc.yaml 的
    /// `frontier_policy.per_cluster`）；曲线缺失（该机型无 frontier_policy）时返回 None 而不是
    /// 猜一个比例——「不封顶」是安全侧，错了只是没省电，不会削性能。
    /// `ratio` 不看曲线，直接按 `sweet_cap_ratio × 硬件最高` floor 对齐，给无实测曲线的机型兜底。
    ///
    /// 返回值一定已落在本 policy 的频率表内（floor 对齐），调用方直接与比例路径的 target 取 min。
    fn sweet_cap_freq(
        override_mode: crate::chiri::config::SweetCapMode,
        ratio: f32,
        core_name: &str,
        mode_name: &str,
        freqs: &[u32],
    ) -> Option<u32> {
        use crate::chiri::config::SweetCapMode;
        match override_mode {
            SweetCapMode::Off => None,
            SweetCapMode::Ratio => {
                if freqs.is_empty() {
                    return None;
                }
                Some(Self::freq_for_ratio(freqs, ratio))
            }
            SweetCapMode::Auto => {
                let group = match core_name {
                    "little" => crate::common::CoreGroup::Little,
                    "big" => crate::common::CoreGroup::Big,
                    "prime" => crate::common::CoreGroup::Prime,
                    _ => return None,
                };
                let Some(curve) = crate::chiri::energy_model::curve(group) else {
                    // 本 SoC 无 frontier_policy（EEM 曲线缺失）：甜点无处可取。
                    // 安全侧 = 不封顶（只是不省电，不削性能），但必须留痕一次，
                    // 否则「配了 sweet_cap:auto 却不生效」在日志里完全不可见；进程级去重，不每 tick 刷
                    if !SWEET_CAP_NO_CURVE_LOGGED.swap(true, Ordering::Relaxed) {
                        warn!(
                            "{}",
                            t_with_args(
                                "tuned-sweet-cap-no-curve",
                                &fluent_args!("mode" => mode_name.to_string())
                            )
                        );
                    }
                    return None;
                };
                let khz = curve.sweet_khz();
                if khz == 0 {
                    return None;
                }
                // 曲线频档与本 policy 实测频率表理论上一致（同源于 soc.yaml 与 scaling_available_frequencies），
                // 但仍按本簇表 floor 对齐：两者不一致时以本簇实测表为准，避免写进表外档位被内核静默 clamp
                freqs
                    .iter()
                    .rev()
                    .copied()
                    .find(|&f| f <= khz)
                    .or_else(|| freqs.first().copied())
            }
        }
    }

    // [dwell] 受控写频（与 CLG write_freq 同口径）：死区由调用方 hysteresis 保证（|target−current| 超死区才调用），这里做最小驻留内方向翻摆延迟与写失败防篡改补写豁免
    // 接管初写/恢复不走本函数；返回是否写成功，成功才前移 current_max，失败/被延迟由下一 tick 补写 target
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

    // [activity_params] 每 tick 刷新「特调模式配置 + 当前 activity 覆盖」的有效配置
    /// 只在覆盖**实例变化**时重算（`Arc::ptr_eq` 命中即返回），稳态每 tick 只花一次加锁 + 一次指针比较。
    /// 未配置覆盖（绝大多数场景）时 `eff_cfg` 与 `cfg` 逐位相同，后续决策路径与加该机制前完全一致。
    /// 叠加四个跨链路同义的标量（headroom / util_smoothing / hysteresis / down_hold_ms），
    /// 与 CLG 侧同口径——同一份 `activity_params` 配置对两条链路都成立。
    fn refresh_activity_override(&mut self) {
        let Some(ov) = crate::monitor::app_detect::current_activity_params() else {
            if self.act_override.is_some() {
                self.act_override = None;
                self.eff_cfg = self.cfg.clone();
            }
            return;
        };
        if self
            .act_override
            .as_ref()
            .is_some_and(|cur| Arc::ptr_eq(cur, &ov))
        {
            return;
        }
        self.eff_cfg = self.cfg.clone();
        if let Some(v) = ov.headroom {
            self.eff_cfg.headroom = v;
        }
        if let Some(v) = ov.util_smoothing {
            self.eff_cfg.util_smoothing = v;
        }
        if let Some(v) = ov.hysteresis {
            self.eff_cfg.hysteresis = v;
        }
        if let Some(ms) = ov.down_hold_ms {
            // 特调 down_hold_ms 本就是毫秒，直接沿用（normalize 已钳到 5s 内）
            self.eff_cfg.down_hold_ms = ms;
        }
        self.act_override = Some(ov);
        debug!(
            "{}",
            t_with_args(
                "tuned-activity-params-applied",
                &fluent_args!(
                    "mode" => self.mode.clone(),
                    "headroom" => format!("{:.2}", self.eff_cfg.headroom),
                    "smooth" => format!("{:.2}", self.eff_cfg.util_smoothing),
                    "hyst" => format!("{:.3}", self.eff_cfg.hysteresis),
                    "down_ms" => self.eff_cfg.down_hold_ms.to_string()
                )
            )
        );
    }

    /// 无档位动态限频入口，每个 SystemLoadUpdate（特调 40ms）触发一次，每个核心组独立决策（语义与 CLG 对齐）
    /// up = 目标 > 当前 max + hysteresis 立即上调；down_wait = 目标 < 当前 max − hysteresis 保持计时（down_hold_ms）；down = 期满把上限收到目标档位；hold = 目标在死区内（或保持期内目标回升，取消等待）
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }
        // [activity_params] 决策前刷新一次：activity 页面切换后本 tick 即生效，不必等模式事件
        self.refresh_activity_override();

        let ranges = crate::common::chiri_core_ranges();
        let now = Instant::now();
        // 借用配置不克隆：SpecialTunedConfig 唯一堆字段是 per_cluster（HashMap，键为组名），每 tick 克隆要复制整张表；gated_write 是关联函数不借 &mut self，与 &self.cfg 不冲突
        // [activity_params] 决策一律读 `eff_cfg`（= cfg + activity 覆盖）
        let cfg = &self.eff_cfg;

        // main_ tick 行数据（每个 policy 一行：policy_id / cluster / util / decision / cur_max / hw_max），只在开发记录开启时收集（关闭时零开销、零分配、零写入）
        let diag = crate::logger::diag_active();
        let mut main_rows: Vec<(i32, &'static str, f32, &'static str, u32, u32)> = Vec::new();

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
            let prev_util = c.ema_util;
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

            // [steady_decay] 稳态下探 = 对写侧比例的限幅积分控制（与 CLG 同构，被控量是本簇平滑 util）：目标水位 T = up_ref × target_ratio，up_ref = 目标比例顶到 ceil 时的 util = ceil / headroom
            // util 高于 T 回收（步长 ×2 防过冲）、低于 T 加深、死区内保持，回路自带稳态；限幅 [0, max] 抗积分饱和；必须用更新前的 util 算差量
            let delta = util - prev_util;
            let stable = delta.abs() <= cfg.steady_decay_band;
            if !cfg.steady_decay_enabled
                || util * p.headroom >= ceil
                || util <= p.perf_floor
                || prev_util < 0.0
                || delta > cfg.steady_decay_band
            {
                c.steady_decay = 0.0;
                c.steady_streak = 0;
            } else if stable {
                c.steady_streak = c.steady_streak.saturating_add(1);
                if c.steady_streak >= cfg.steady_decay_stable_ticks {
                    let up_ref = (ceil / p.headroom.max(1e-6)).clamp(0.0, 1.0);
                    let target = up_ref * cfg.steady_decay_target_ratio;
                    if util > target + cfg.steady_decay_band {
                        c.steady_decay = (c.steady_decay - cfg.steady_decay_step * 2.0).max(0.0);
                    } else if util < target - cfg.steady_decay_band {
                        c.steady_decay =
                            (c.steady_decay + cfg.steady_decay_step).min(cfg.steady_decay_max);
                    }
                    // 死区内保持不变 → 消除抖动
                }
            } else {
                // 抖动只清进度，**不清**已建立的下探量（噪声不得反复打回）
                c.steady_streak = 0;
            }

            // 下探只改目标档的计算输入：tuned 没有 CLG 那种独立的决策比例，比较量就是 freq 本身，所以减过 decay 的 target_max 同时是目标档与 up/down 的比较量
            // 这是本架构的必然（与 CLG「下探不回写决策量」不同）；hysteresis / down_hold_ms / up_wait 的判据结构不受影响
            let write_ratio = (target_ratio - c.steady_decay).max(p.perf_floor);
            let mut target_max = Self::freq_for_ratio(&c.available_freqs, write_ratio);

            // [sweet_cap] 甜点封顶：只降不升（与 EEM 侧「target = min(ratio_freq, eem)」同口径）。
            // 越过甜点只在「本簇占用达 sweet_cap_release_units」时放行——那是「甜点确实不够用」的
            // 显式信号（多核并发共享同一档位时升频收益覆盖全簇，迁移也无处可去），
            // 缺省 0.0 = 不放行（硬封顶）。放行带 hold 窗口，避免占用在阈值附近抖动导致上限来回跳。
            if p.sweet_cap != crate::chiri::config::SweetCapMode::Off {
                // 簇内**平均**占用率（不是核当量）：单核簇也能表达「这颗核被吃满」，
                // 而核当量阈值对 prime 退化成恒真（只有 1 核，占用恒 ≤1.0）
                let cpus = range.clone().count().max(1) as f32;
                let occupied_avg: f32 = range
                    .clone()
                    .filter_map(|cpu| core_utils.get(cpu).copied())
                    .map(|u| u.clamp(0.0, 1.0))
                    .sum::<f32>()
                    / cpus;
                let release_units = cfg.sweet_cap_release_units;
                let in_release = release_units > 0.0 && occupied_avg >= release_units;
                if in_release && cfg.sweet_cap_release_hold_ticks > 0 {
                    c.sweet_release_until = Some(
                        now + Duration::from_millis(
                            (cfg.sweet_cap_release_hold_ticks as u64) * TUNED_TICK_MS,
                        ),
                    );
                }
                // 窗口过期即收回封顶（hold=0 时每一 tick 按当前占用重新判定，低于阈值立即收回）
                let holding = c.sweet_release_until.is_some_and(|until| until > now);
                if !holding && !in_release {
                    c.sweet_release_until = None;
                }
                // 放行 = 不封顶，直接走比例路径；否则压到甜点档（只降不升）
                if !(in_release || holding) {
                    if let Some(cap) = Self::sweet_cap_freq(
                        p.sweet_cap,
                        p.sweet_cap_ratio,
                        c.core_name,
                        &self.mode,
                        &c.available_freqs,
                    ) {
                        target_max = target_max.min(cap);
                    }
                }
            } else if c.sweet_release_until.is_some() {
                c.sweet_release_until = None;
            }
            let hyst_freq = (hw_max as f32 * p.hysteresis) as u32;

            let decision;
            if target_max > c.current_max + hyst_freq {
                // 升频：[up_confirm] 严格审批——目标比当前更高须连续 up_confirm_ticks tick 满足才抬升（0/1 = 关闭确认），写成功才前移状态，失败时下一 tick target 仍超死区 → 重试
                // [dwell] 经写频滞回（同 CLG）
                c.down_since = None;
                c.up_wait = c.up_wait.saturating_add(1);
                if c.up_wait >= cfg.up_confirm_ticks {
                    Self::gated_write(c, target_max, cfg.write_dwell_ms);
                    decision = "up";
                } else {
                    decision = "up_wait";
                }
            } else if target_max < c.current_max.saturating_sub(hyst_freq) {
                // 降频：目标须持续 down_hold_ms；计时重置只在目标明显回升（> down_target + hyst）时发生——相邻档位小幅跳动不重置，
                // 否则上限卡在高位降不下来（连续控制退化成只升不降）；目标继续下探只更新 down_target（执行用最新档），不重置计时
                c.up_wait = 0;
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
                // 死区内：目标与当前上限一致，取消进行中的降频等待与升频确认
                c.down_since = None;
                c.up_wait = 0;
                // [dwell] 目标收敛进死区：上次写失败的补写诉求消失，清标记防悬挂（否则任意久之后的下一次真实写频会白豁免滞回一次）
                c.last_failed = false;
                decision = "hold";
            }

            if diag {
                main_rows.push((
                    c.policy_id,
                    c.core_name,
                    util,
                    decision,
                    c.current_max,
                    hw_max,
                ));
            }
        }

        // main_ tick 行（开发记录开启时才有 IO）：cur_freq_khz 写当前动态 max、max_freq_khz 写硬件最高（kHz），over/under 无档位语义恒 0
        // util 列写决策用负载（util_smoothing < 1 时为 EMA 后的值），离线回放该列时不能当原始 util 二次平滑
        if diag {
            for (policy_id, name, util, decision, cur_max, hw_max) in &main_rows {
                crate::logger::main_tick(
                    *policy_id,
                    name,
                    *util,
                    0,
                    0,
                    None,
                    None,
                    Some(*cur_max),
                    Some(*hw_max),
                    decision,
                    0,
                    0,
                    None,
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
