//! mod.rs: [consts] [thermal] [policies] [aff_snap] [affinity] [threads] [config_watcher] [ipc_main] [ipc_state]
//! [evt_loop] [evt_screen] [evt_mode] [evt_pkg_switch] [evt_load] [evt_frame] [evt_reload] [evt_bpf]
//! [panic_recovery] [fas_focus]

use anyhow::Result;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

// [consts]
// CLG 看门狗：SystemLoadUpdate 常规 160ms / 特调 40ms 投喂一次，超过 CLG_STALE_MAX 未收到
// 事件即判负载源失效（eBPF 失败/探针崩溃/通道断开），release() 回系统调频防锁频
const CLG_STALE_MAX: Duration = Duration::from_secs(5);
/// 事件轮询间隔：主事件通道无事件时的最大阻塞时长（动态超时上限）实际阻塞到最近周期任务的 deadline，空闲不再固定空转；性能敏感路径（负载事件/模式切换/触摸）均为推送事件、到达即唤醒零延迟，
/// 此值仅作 deadline 兜底上限
const EVENT_POLL_MS: Duration = Duration::from_millis(1000);
/// 热保护温度采样间隔：2s 一次，温度变化缓慢，更密的采样只浪费 IO
const THERMAL_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// 遥测 CSV 落盘间隔：1s 一次（功耗统计精度 1s；telemetry 线程 1s 刷新共享原子量）
const TELEMETRY_LOG_INTERVAL: Duration = Duration::from_secs(1);
/// scenemode 饱和退出：**常驻簇（小核 + 大核）** max_util 持续高于该值视为顶满性能上限（util 是忙时占比，与频率无关——后台负载压不住常驻核时即饱和）
const SCENEMODE_SAT_UTIL: f32 = 0.75;
/// 饱和持续判定时长：连续满足才退出，防止瞬时突发误触发
const SCENEMODE_SAT_SECS: Duration = Duration::from_secs(10);
/// scenemode 冷却：饱和退出后 300s 内不得重新进入（防止与后台负载反复拉锯）
const SCENEMODE_COOLDOWN: Duration = Duration::from_secs(300);
/// scheduler_ipc 事件循环 panic 自愈：连续崩溃超过该次数后放弃重启（防止 poisoned lock 等确定性 panic 变成打满 CPU 的重启风暴），仅保留最终清理
const SCHEDULER_IPC_RESTART_MAX: u32 = 5;
/// panic 重启退避：每次重启前等待，错开引发 panic 的外部状态（如负载风暴）
const SCHEDULER_IPC_RESTART_BACKOFF: Duration = Duration::from_secs(1);
/// 电池状态节点（标准 power_supply 接口）：区分充电/放电
const BATT_STATUS_PATH: &str = "/sys/class/power_supply/battery/status";

/// status 节点取值不认识时的告警去重（值恢复为认识的内容后重新武装）
static BATT_STATUS_UNKNOWN_WARNED: AtomicBool = AtomicBool::new(false);
/// 热保护解除斜坡步长：压制加深立即生效，解除方向每个采样周期（2s）最多恢复 0.15若解除即全量恢复，温度围绕阈值震荡时 cap 会在 40/70/100 间以 ~8s 周期 bang-bang 跳变、
/// current_perf 反复砸回低位致周期性掉帧；限速解除后反弹会中途重新压制
const THERMAL_UNPRESS_STEP: f32 = 0.15;

// [thermal]
/// 读电池充放电状态（1s snap 处消费）：归一为小写短词；节点缺失或未知值返回"-"（与 CSV 缺失占位一致）。电流符号因厂商节点方向不一，不可靠，故读 status 字符串
pub(crate) fn read_battery_charge_state() -> String {
    let state = match std::fs::read_to_string(BATT_STATUS_PATH) {
        Ok(s) => {
            let raw = s.trim();
            // 大小写与多余空白都容忍：不同内核会写 Charging / charging，规范化后再比对
            match raw.to_ascii_lowercase().as_str() {
                "charging" => "charging",
                "discharging" => "discharging",
                "full" => "full",
                "not charging" => "not_charging",
                _ => {
                    // 取值不认识 = 永不取样（门控只认 discharging），必须打出原文
                    if !BATT_STATUS_UNKNOWN_WARNED.swap(true, Ordering::Relaxed) {
                        log::warn!(
                            "{}",
                            crate::i18n::t_with_args(
                                "battery-status-unknown",
                                &crate::fluent_args!("raw" => raw.to_string())
                            )
                        );
                    }
                    "-"
                }
            }
        }
        Err(_) => "-",
    };
    if state != "-" {
        BATT_STATUS_UNKNOWN_WARNED.store(false, Ordering::Relaxed);
    }
    state.to_string()
}

/// 温度传感器滤波器：物理范围门 + 毛刺丢弃 + 3 样本中值 + 斜率限制。MTK soc_max 等合成温区读数跳变极大（2s 内数十度、物理上不可能），直接用于带回滞的阈值判定会让 cap 以 ~8s 周期反复跳变，
/// 逐级平滑后才能作为秒级热判定的输入
struct TempFilter {
    /// 物理合理区间 [min_c, max_c]：节点异常/未初始化的读数直接丢弃
    min_c: f32,
    max_c: f32,
    /// 相对上次有效输出的最大可信跳变（°C），超过视为传感器毛刺丢弃
    max_jump: f32,
    /// 输出斜率限制（°C/样本）：真实热质量决定温度不可能瞬间大幅变化
    max_step: f32,
    window: [f32; 3],
    win_len: usize,
    win_idx: usize,
    last_out: Option<f32>,
}

impl TempFilter {
    fn new(min_c: f32, max_c: f32, max_jump: f32, max_step: f32) -> Self {
        Self {
            min_c,
            max_c,
            max_jump,
            max_step,
            window: [0.0; 3],
            win_len: 0,
            win_idx: 0,
            last_out: None,
        }
    }

    /// 喂入一个原始样本，返回滤波后的温度。样本被丢弃时返回上次输出（首个有效样本到来前返回 None）
    fn push(&mut self, raw: f32) -> Option<f32> {
        // 1) 物理范围门：明显不合理的读数（如 0.3°C 的电池温度节点）丢弃
        if !(raw >= self.min_c && raw <= self.max_c) {
            return self.last_out;
        }
        // 2) 毛刺丢弃：相对上次输出的跳变超过真实热质量上限（如压制后频率骤降叠加
        // 合成温区切换热点），丢弃以防「骤降→解除→反弹」振荡
        if let Some(prev) = self.last_out {
            if (raw - prev).abs() > self.max_jump {
                return self.last_out;
            }
        }
        // 3) 3 样本中值：滤掉单点毛刺
        self.window[self.win_idx] = raw;
        self.win_idx = (self.win_idx + 1) % 3;
        self.win_len = (self.win_len + 1).min(3);
        let mut sorted = self.window[..self.win_len].to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];
        // 4) 斜率限制：压制/解除两个方向都平滑，消除控制环振荡
        let out = match self.last_out {
            None => median,
            Some(prev) => prev + (median - prev).clamp(-self.max_step, self.max_step),
        };
        self.last_out = Some(out);
        self.last_out
    }
}

/// 单传感器四级热阶梯参数：三级温度阈值 + 对应三档性能上限 + 三级独立回滞batt / cpu 各构一份（阈值不同、cap 与回滞共用），见 `[thermal]` 块契约：温度 soft < mid < hard；
/// cap hard <= mid <= soft <= 1.0（config normalize 保证）
#[derive(Clone, Copy)]
struct ThermalLadder {
    soft: f32,
    mid: f32,
    hard: f32,
    soft_cap: f32,
    mid_cap: f32,
    hard_cap: f32,
    /// **软档**回滞（°C）：降到 软限 - hyst 以下才解除软档
    hyst: f32,
    /// 中档回滞（°C）：独立于软档（`Thermal.hysteresis_mid_c`）。共用软档回滞时中档解除点
    /// （中限 - hyst）会落到软限以下 → 中档咬住后跳过软档、必须一路冷过软限才恢复（实测踩过）；
    /// normalize 已强制 `中限 - 本值 >= 软限`
    hyst_mid: f32,
    /// 硬档回滞（°C）：独立配置（`Thermal.hysteresis_hard_c`），硬档 0.40 的阶跃比软档敏感
    hyst_hard: f32,
}

/// 单传感器四级判定（每级上下两条边都带回滞）：>= 硬限压 hard_cap；>= 中限压 mid_cap；
/// >= 软限压 soft_cap；回落到 软限-hyst 以下解除`temp_c` 必须传 `TempFilter::push` 的
/// 滤波输出（1s snap 写入 last_batt_temp/last_cpu_temp，本 2s 热块复用），喂原始读数会让
/// cap bang-bang 振荡
/// 回滞规则（2026-09-27 修）：每级的**退出**都要降到自己阈值 − 该级回滞以下，判据里的
/// `current <= 本级 cap` 表示「上周期仍压在本级或更深」（深档值更小、天然满足），所以退档
/// 只会逐级回退、不会跨级跳改前硬限→软限这条边**无回滞**：batt 一降到 44.9°C 就从 0.40
/// 跳回 0.85，批次2 在 75 分钟内 5 次进硬档、两次 60s 内重入（最近相隔 2.0 s）回滞带内
/// 且已解除（current >= 1.0）时保持 1.0；压制中退档由调用方的 `THERMAL_UNPRESS_STEP` 斜坡兜住
fn eval_thermal_cap(temp_c: f32, current: f32, l: &ThermalLadder) -> f32 {
    if temp_c >= l.hard || (current <= l.hard_cap && temp_c >= l.hard - l.hyst_hard) {
        l.hard_cap
    } else if temp_c >= l.mid || (current <= l.mid_cap && temp_c >= l.mid - l.hyst_mid) {
        l.mid_cap
    } else if temp_c >= l.soft || (current <= l.soft_cap && temp_c >= l.soft - l.hyst) {
        l.soft_cap
    } else {
        1.0
    }
}

pub mod config;
pub mod scheduler;
// FAS（帧感知调度）：引擎位于 crate::scheduler::fas（算法层），ChiRi 侧由 fas_manager 提供多实例生命周期管理
pub mod affinity;
pub mod core_ctl;
pub mod cpu_load_governor;
pub mod diag_cost;
pub mod diag_worker;
pub mod energy_cost;
mod fas_focus;
pub mod fas_manager;
pub mod fast;
pub mod governor;
pub mod gpu;
pub mod power_base;
pub mod touch_detect;
pub mod tuned;

use crate::common;
use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{load_language, t, t_with_args};
use crate::logger;
use crate::utils;
use config::Config;
use scheduler::CpuScheduler;

// [fas_focus]
fn fas_package_ready(package: &str) -> bool {
    crate::common::is_chiri_soc()
        && crate::common::fas_enabled()
        && crate::common::fas_available()
        && crate::common::fas_whitelist_entry(package)
            .and_then(|cfg| crate::common::fas_app_config(cfg))
            .is_some()
}

/// 供 line 内部复用的一致前台快照。
#[derive(Clone, Debug)]
pub(crate) struct FasForeground {
    pub package: Arc<str>,
    pub pid: i32,
    pub process_starttime: Option<u64>,
    pub generation: u64,
}

impl FasForeground {
    pub(crate) fn is_valid(&self) -> bool {
        self.pid > 0 && !self.package.is_empty()
    }
}

fn fas_foreground_snapshot() -> Option<FasForeground> {
    let snapshot = crate::monitor::app_detect::foreground_snapshot()?;
    Some(FasForeground {
        package: snapshot.package,
        pid: snapshot.pid,
        process_starttime: snapshot.process_starttime,
        generation: snapshot.generation,
    })
}

fn fas_last_determined_mode() -> Option<String> {
    let determined = crate::monitor::app_detect::last_determined_mode();
    Some(determined)
}

fn fas_raw_foreground_package() -> Arc<str> {
    crate::monitor::app_detect::raw_foreground_package_arc()
}

/// 事件携带的 PID 只是普通整数，须借 /proc 读出 starttime 才能与快照做 ABA 判据。
fn manager_process_starttime(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    let starttime = after_comm.split_whitespace().nth(19)?;
    starttime.parse().ok()
}

fn reconcile_fas_focus(
    manager: &mut fas_manager::FasManager,
    probe: &crate::monitor::window_visibility::WindowVisibilityProbe,
    confirmed_focus: &mut Option<(crate::monitor::window_visibility::WindowRequest, Instant)>,
    pending_mode: &mut Option<String>,
    screen_on: bool,
) {
    use crate::monitor::window_visibility::{WindowRequest, WindowVisibility};
    use fas_focus::{FocusDecision, decide_focus};

    let Some(owner) = manager.active_pkg() else {
        if confirmed_focus.take().is_some() {
            probe.invalidate();
        }
        return;
    };
    let owner = owner.to_string();
    if !screen_on {
        let feedback_changed = manager.frame_source().is_some_and(|source| source.frame_feedback);
        manager.set_frame_feedback_enabled(false);
        let had_confirmation = confirmed_focus.take().is_some();
        if feedback_changed || had_confirmation {
            probe.invalidate();
        }
        return;
    }

    let snapshot = fas_foreground_snapshot();
    let owner_process_alive = manager.owner_is_alive();
    let owner_matches = snapshot.as_ref().is_some_and(|snapshot| {
        manager.owner_matches_process(snapshot.pid, snapshot.process_starttime)
    }) && owner_process_alive;
    let raw_foreground = fas_raw_foreground_package();
    let owner_allowed = fas_package_ready(&owner);
    let foreground_is_owner = snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.is_valid() && snapshot.package.as_ref() == owner);
    let fast_same = foreground_is_owner
        && raw_foreground.as_ref() == owner
        && owner_matches
        && owner_allowed;
    if fast_same {
        let feedback_changed = manager.frame_source().is_some_and(|source| !source.frame_feedback);
        manager.set_frame_feedback_enabled(true);
        manager.cancel_delayed_exit();
        *pending_mode = None;
        let had_confirmation = confirmed_focus.take().is_some();
        if feedback_changed || had_confirmation {
            probe.invalidate();
        }
        return;
    }

    // 进程已死：不读窗口证明续期，直接走延迟退出。
    let deadline_managed = !owner_process_alive || (foreground_is_owner && !owner_matches);
    let make_request = |source: crate::monitor::FrameSource| WindowRequest {
        package: owner.clone(),
        pid: source.pid,
        generation: source.generation,
        foreground: raw_foreground.to_string(),
    };
    let Some(source) = manager.frame_source() else { return; };
    let initial_request = make_request(source);
    let previous_focus = probe
        .latest(&initial_request)
        .filter(|observation| {
            observation.visibility == WindowVisibility::Focused
                && observation.observed_at.elapsed() < fas_focus::WINDOW_EVIDENCE_MAX_AGE
        })
        .map(|observation| observation.observed_at)
        .or_else(|| {
            confirmed_focus
                .as_ref()
                .filter(|(request, observed_at)| {
                    *request == initial_request
                        && observed_at.elapsed() < fas_focus::WINDOW_EVIDENCE_MAX_AGE
                })
                .map(|(_, observed_at)| *observed_at)
        });
    // 非 Focused 结果会先暂停反馈；缓存只在 fresh Focused 时保留。
    if previous_focus.is_none() {
        manager.set_frame_feedback_enabled(false);
    }
    let Some(source) = manager.frame_source() else { return; };
    let request = make_request(source);
    probe.request(request.clone());
    let observation = probe.latest(&request);
    let evidence = observation
        .as_ref()
        .filter(|observation| {
            observation.visibility != WindowVisibility::Focused
                || observation.observed_at.elapsed() < fas_focus::WINDOW_EVIDENCE_MAX_AGE
        })
        .map(|observation| {
            let decision = match observation.visibility {
                WindowVisibility::Focused => FocusDecision::Focused,
                WindowVisibility::VisibleUnfocused => FocusDecision::LoadOnly,
                WindowVisibility::Hidden | WindowVisibility::Unknown => FocusDecision::Grace,
            };
            (decision, observation.observed_at.elapsed())
        })
        .or_else(|| {
            previous_focus.map(|observed_at| (FocusDecision::Focused, observed_at.elapsed()))
        });
    let decision = if !owner_process_alive {
        FocusDecision::Grace
    } else if deadline_managed {
        decide_focus(false, false, None)
    } else if owner_allowed {
        decide_focus(false, false, evidence)
    } else {
        FocusDecision::Grace
    };
    match decision {
        FocusDecision::Focused => {
            let observed_at = observation
                .as_ref()
                .map(|value| value.observed_at)
                .or(previous_focus)
                .unwrap();
            let feedback_changed = !source.frame_feedback;
            manager.set_frame_feedback_enabled(true);
            manager.cancel_delayed_exit();
            *pending_mode = None;
            if feedback_changed {
                probe.invalidate();
            }
            if let Some(restored_source) = manager.frame_source() {
                let restored_request = make_request(restored_source);
                *confirmed_focus = Some((restored_request.clone(), observed_at));
                probe.request(restored_request);
            }
        }
        FocusDecision::LoadOnly => {
            *confirmed_focus = None;
            manager.set_frame_feedback_enabled(false);
            manager.cancel_delayed_exit();
            *pending_mode = None;
        }
        FocusDecision::Grace => {
            *confirmed_focus = None;
            manager.set_frame_feedback_enabled(false);
            manager.request_delayed_exit();
            *pending_mode = Some(fas_delayed_exit_target(manager, snapshot.as_ref()));
        }
    }
}

/// 延迟退出目标：只有实例仍在、且当前快照确实不是 FAS 前台时才采信determine 的实时模式，防止用旧 generation 的目标。
fn fas_delayed_exit_target(
    manager: &fas_manager::FasManager,
    snapshot: Option<&FasForeground>,
) -> String {
    let owner = manager.active_pkg();
    let determined = fas_last_determined_mode().unwrap_or_default();
    let snapshot_is_owner = snapshot.is_some_and(|snapshot| {
        owner.is_some_and(|owner| snapshot.package.as_ref() == owner)
    });
    if snapshot_is_owner || determined.is_empty() || determined == "fas" {
        return "default".to_string();
    }
    determined
}

// [policies]
/// CPU 频率策略簇信息
pub struct CpuPolicy {
    /// policy 编号，对应 /sys/devices/system/cpu/cpufreq/policy<id>
    pub id: i32,
    /// boost 频率列表（单位 kHz），有的簇没有此文件则为空
    pub boost_frequencies: Vec<u32>,
}

/// 枚举系统中实际可用的 cpufreq policy，并读取各 policy 的 boost 频率。结果按 policy id 升序返回（供 CLG 遍历初始化）
pub fn get_cpu_policies() -> Vec<CpuPolicy> {
    let mut policies = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("policy") {
                    if let Ok(pid) = name["policy".len()..].parse::<i32>() {
                        let boost_freqs = read_boost_frequencies(pid);
                        policies.push(CpuPolicy {
                            id: pid,
                            boost_frequencies: boost_freqs,
                        });
                    }
                }
            }
        }
    }
    policies.sort_unstable_by_key(|p| p.id);
    policies
}

/// policy 发现缓存 TTL（秒）：非空缓存距上次成功扫描 ≥ 该值即重扫比对 id 列表。
/// **语义边界**：运行期新增/删除 policy 最多延迟该时长被发现；空结果/失败不永久缓存（见 `policy_cache_merge`）
const POLICY_RESCAN_TTL: Duration = Duration::from_secs(60);

// [policies]
/// `cpu_freq_snapshot` 的 policy **发现结果与基础路径**缓存（仅缓存发现，不缓存实际频率值）。
/// 非空才缓存；空结果不永久缓存、失败保留旧列表且不推进时刻（下次照常重试）。与逐值读取解耦：
/// 取到列表后释放锁，实际 cur/min/max/gov 由调用方在锁外逐个读
static POLICIES_CACHE: OnceLock<Mutex<Option<PolicyBases>>> = OnceLock::new();

/// 缓存的 policy 发现结果：id 列表（升序）+ 各 policy 基础路径 + 上次成功扫描时刻
struct PolicyBases {
    ids: Vec<i32>,
    bases: Vec<String>,
    scanned_at: Instant,
}

impl PolicyBases {
    /// 由 id 列表构造缓存项（基础路径 = `/sys/devices/system/cpu/cpufreq/policy<id>`）
    fn from_ids(ids: Vec<i32>, scanned_at: Instant) -> Self {
        let bases = ids
            .iter()
            .map(|id| format!("/sys/devices/system/cpu/cpufreq/policy{id}"))
            .collect();
        Self {
            ids,
            bases,
            scanned_at,
        }
    }

    /// 克隆 (id, 基础路径) 供锁外读取
    fn entries(&self) -> Vec<(i32, String)> {
        self.ids
            .iter()
            .cloned()
            .zip(self.bases.iter().cloned())
            .collect()
    }
}

/// 纯逻辑（便于测试）：是否需要一次新的发现扫描——无缓存，或距上次成功扫描 ≥ [`POLICY_RESCAN_TTL`]
fn policy_cache_needs_scan(cache: &Option<PolicyBases>, now: Instant) -> bool {
    match cache {
        Some(c) => now.duration_since(c.scanned_at) >= POLICY_RESCAN_TTL,
        None => true,
    }
}

/// 纯逻辑（不读 sysfs，便于测试）：合并一次发现结果。
/// - 结果非空：有增删 → 重建（含基础路径）；无增删 → 仅刷新扫描时刻（避免每次调用重扫）；
/// - 结果为空：**不清空**旧缓存、**不更新**时刻（空期间每次调用照常重试，不永久缓存空结果）
/// - `scan_started_at` 为本次扫描**开始**时刻：并发保护——扫描在锁外进行，若两个线程同时在扫，
///   后完成的「旧扫描」不得覆盖先完成的「新扫描」。只接受开始时刻不早于缓存时刻的结果。
fn policy_cache_merge(
    cache: &mut Option<PolicyBases>,
    ids: Vec<i32>,
    scan_started_at: Instant,
    now: Instant,
) {
    if ids.is_empty() {
        return;
    }
    if let Some(c) = cache {
        if scan_started_at < c.scanned_at {
            return;
        }
    }
    match cache {
        Some(c) if c.ids == ids => c.scanned_at = now,
        _ => *cache = Some(PolicyBases::from_ids(ids, now)),
    }
}

/// 取当前 policy 的 (id, 基础路径) 列表（必要时重扫）。**锁外做 sysfs 访问**：
/// 判定/扫描/写缓存各自为短临界区，扫描（read_dir + boost 读）与逐值读取都不持锁
fn policy_bases() -> Vec<(i32, String)> {
    let cell = POLICIES_CACHE.get_or_init(|| Mutex::new(None));
    // 1) 是否需要重扫（短临界区只读缓存状态，随后释放锁）
    let need_scan = {
        let guard = cell.lock().unwrap_or_else(|e| e.into_inner());
        policy_cache_needs_scan(&guard, Instant::now())
    };
    if need_scan {
        // 2) 扫描在锁外（read_dir + boost 读，避免持锁做 sysfs 访问）。
        //    先取扫描开始时刻，供锁内合并做并发保护（旧扫描不得覆盖新结果）
        let scan_started = Instant::now();
        let ids: Vec<i32> = get_cpu_policies().into_iter().map(|p| p.id).collect();
        // 3) 写缓存在锁内（短临界区，无 sysfs 访问）
        let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
        policy_cache_merge(&mut guard, ids, scan_started, Instant::now());
    }
    // 4) 取缓存（克隆 id + 基础路径），释放锁；实际值读取在调用方锁外
    let guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    let cache: &Option<PolicyBases> = &guard;
    cache.as_ref().map(PolicyBases::entries).unwrap_or_default()
}

/// main_ snap 行用：各 policy 的**实际**当前频率/上限/下限/调速器，`;` 分隔、每项`policy<id>:<值>`，读不到写 `-`只在诊断开启时调用（每秒一次 sysfs 读），
/// 与 tick 行 cur_freq_khz/max_freq_khz（调度器决策值）互补：那两列是「我们写了多少」，这里是「内核实际是多少」
/// **C3**：policy **发现结果**（id 列表 + 基础路径）走缓存（非空才缓存、60s 重扫），实际值仍每次照读——
/// 故输出在静态拓扑下逐字节不变；运行期新增 policy 最多延迟 [`POLICY_RESCAN_TTL`]（60s）被发现
pub fn cpu_freq_snapshot() -> (String, String, String, String) {
    // 先取（必要时重扫）发现结果并释放锁，再逐个读实际值节点（不跨节点读取持锁）
    let entries: Vec<(i32, (String, String, String, String))> = policy_bases()
        .into_iter()
        .map(|(id, base)| {
            let read = |f: &str| -> String {
                std::fs::read_to_string(format!("{base}/{f}"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "-".to_string())
            };
            (
                id,
                (
                    read("scaling_cur_freq"),
                    read("scaling_max_freq"),
                    read("scaling_min_freq"),
                    read("scaling_governor"),
                ),
            )
        })
        .collect();
    render_freq_snapshot(&entries)
}

/// 纯函数（便于逐字节对拍，不读 sysfs）：把 (policy id, 四项值) 渲染成 4 条 `;` 分隔的
/// `policy<id>:<值>` 串，顺序 = 入参顺序，读不到的值由调用方传 `-`
fn render_freq_snapshot(
    entries: &[(i32, (String, String, String, String))],
) -> (String, String, String, String) {
    let (mut cur, mut max, mut min, mut gov) =
        (String::new(), String::new(), String::new(), String::new());
    let push = |dst: &mut String, id: i32, v: &str| {
        if !dst.is_empty() {
            dst.push(';');
        }
        dst.push_str(&format!("policy{id}:{v}"));
    };
    for (id, (c, ma, mi, g)) in entries {
        push(&mut cur, *id, c);
        push(&mut max, *id, ma);
        push(&mut min, *id, mi);
        push(&mut gov, *id, g);
    }
    (cur, max, min, gov)
}

// [clamp_evidence]
/// P1-2 锁频钳制旁证快照（`clamp_change` 行 reason），把「锁频被压」归因到：
/// 1. FREQ_QOS 钳制（thermal cooling / msm_performance / input-boost）——smax 读回
/// 低于 ChiRi 写入值，msmp/corectl 佐证第三方并发写；
/// 2. 硬件热压（LMh/DCVS）——smax 正常而实际频率被压，佐证 dcvsh_freq_limit（每簇
/// 首核）是否低于硬件 max；
/// 3. 厂商并发写——msm_performance / core_ctl min_cpus/max_cpus/enable 被改；
/// `/proc/horae_qmi` 只记存在性
/// 【临时采集】P2 真机取证用，验证期结束后评估去留全部只读；节点缺失/读失败记
/// `-` 并 warn 一次（去重后静默，防 2s 周期刷屏）；msmp 二节点带读取退避（连续 8
/// 次读空后停读、每 64 次重试一轮，见函数内状态机）只在 `diag_active()` 时由 2s
/// 热块调用；调用方比对快照字符串，未变化零落盘
fn clamp_evidence_snapshot(warned: &mut HashSet<String>) -> String {
    /// 缺失节点首见告警一次（key 形如 "smax@policy4"，附完整路径便于排查）
    fn warn_once(warned: &mut HashSet<String>, key: String, path: &str) {
        if warned.insert(key.clone()) {
            log::info!(
                "{}",
                t_with_args(
                    "clampev-node-missing",
                    &fluent_args!("key" => key.as_str(), "path" => path)
                )
            );
        }
    }
    /// 非空读取：缺失/空串统一返回 None（节点存在但内容空按缺失处理）
    fn read_trim(path: &str) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    /// 按 `policy<id>:<val>` 追加分段（与 cpu_freq_snapshot 的多 policy 拼法一致）
    fn seg_push(dst: &mut String, id: i32, val: &str) {
        if !dst.is_empty() {
            dst.push(';');
        }
        dst.push_str(&format!("policy{}:{}", id, val));
    }

    static POLICIES: OnceLock<Vec<i32>> = OnceLock::new();
    let policies = POLICIES.get_or_init(|| get_cpu_policies().into_iter().map(|p| p.id).collect());

    let mut smax = String::new();
    let mut dcvsh = String::new();
    let mut corectl = String::new();
    for id in policies {
        // smax：FREQ_QOS_MAX 聚合结果的读回值（低于写入值 = 被 QoS 钳制）
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            id
        );
        match read_trim(&path) {
            Some(v) => seg_push(&mut smax, *id, &v),
            None => {
                seg_push(&mut smax, *id, "-");
                warn_once(warned, format!("smax@policy{}", id), &path);
            }
        }
        // dcvsh：硬件 DCVS 限频佐证（policy id 即簇首核；部分内核未导出属常态）
        let path = format!("/sys/devices/system/cpu/cpu{}/dcvsh_freq_limit", id);
        match read_trim(&path) {
            Some(v) => seg_push(&mut dcvsh, *id, &v),
            None => {
                seg_push(&mut dcvsh, *id, "-");
                warn_once(warned, format!("dcvsh@cpu{}", id), &path);
            }
        }
        // core_ctl：厂商并发写观测（min_cpus/max_cpus/enable 每簇首核一份；目录缺失多为机型不支持，段记 "-"）
        let cdir = format!("/sys/devices/system/cpu/cpu{}/core_ctl", id);
        if !std::path::Path::new(&cdir).exists() {
            seg_push(&mut corectl, *id, "-");
            warn_once(warned, format!("corectl@cpu{}", id), &cdir);
        } else {
            let mut cc = String::new();
            for (i, name) in ["min_cpus", "max_cpus", "enable"].iter().enumerate() {
                if i > 0 {
                    cc.push('/');
                }
                let path = format!("{}/{}", cdir, name);
                match read_trim(&path) {
                    Some(v) => cc.push_str(&v),
                    None => {
                        cc.push('-');
                        warn_once(warned, format!("corectl.{}@cpu{}", name, id), &path);
                    }
                }
            }
            seg_push(&mut corectl, *id, &cc);
        }
    }
    // msm_performance 参数（单值，非逐 policy）；horae_qmi 只记存在性。【临时】读取退避：此二节点在部分机型恒不可读，连续 MSMP_FAIL_LIMIT 次读空后停读（快照项记 `-`），
    // 每 MSMP_RETRY_TICKS 次（≈128s）重试一轮，恢复即回归采集
    static MSMP_FAIL_STREAK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static MSMP_SKIP_TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    const MSMP_FAIL_LIMIT: u32 = 8;
    const MSMP_RETRY_TICKS: u32 = 64;
    let mut msmp = ["-".to_string(), "-".to_string()];
    let skip = MSMP_FAIL_STREAK.load(Ordering::Relaxed) >= MSMP_FAIL_LIMIT;
    // 退避期只在第 MSMP_RETRY_TICKS 个 tick 重试一轮，其余 tick 不读文件、直接记 `-`
    let retry = skip && MSMP_SKIP_TICKS.fetch_add(1, Ordering::Relaxed) + 1 >= MSMP_RETRY_TICKS;
    if !skip || retry {
        let mut all_ok = true;
        for (i, name) in ["cpu_min_freq", "cpu_max_freq"].iter().enumerate() {
            let path = format!("/sys/module/msm_performance/parameters/{}", name);
            match read_trim(&path) {
                Some(v) => msmp[i] = v,
                None => {
                    all_ok = false;
                    warn_once(warned, format!("msmp.{}", name), &path);
                }
            }
        }
        if all_ok {
            // 任一节点恢复可读即清零退避，回归正常采集
            MSMP_FAIL_STREAK.store(0, Ordering::Relaxed);
            MSMP_SKIP_TICKS.store(0, Ordering::Relaxed);
        } else if MSMP_FAIL_STREAK.load(Ordering::Relaxed) < MSMP_FAIL_LIMIT {
            MSMP_FAIL_STREAK.fetch_add(1, Ordering::Relaxed);
        } else {
            // 重试失败：计数归零，下一轮重试再等 MSMP_RETRY_TICKS
            MSMP_SKIP_TICKS.store(0, Ordering::Relaxed);
        }
    }
    let horae = if std::path::Path::new("/proc/horae_qmi").exists() {
        "1"
    } else {
        "-"
    };
    if horae == "-" {
        warn_once(warned, "horae_qmi".to_string(), "/proc/horae_qmi");
    }
    format!(
        "msmp_min={} msmp_max={} horae={} smax={} dcvsh={} corectl={}",
        msmp[0], msmp[1], horae, smax, dcvsh, corectl
    )
}

/// GPU 频率快照的节流间隔：读 GPU 频率节点（尤其 Adreno 的 `gpuclk`）会把 GPU 从低功耗状态拉起，跟随 1s snap 等于每秒唤醒一次 GPU，采集代价盖过数据收益
/// 故 GPU 单独按 10s 采一次；CPU 的 cpufreq 节点读取廉价，仍每秒采
const GPU_SNAP_INTERVAL_MS: u64 = 10_000;

/// GPU 频率/调速器快照总开关：**当前默认关闭**——读 GPU 频率节点会唤醒 GPU、采集
/// 代价盖过收益（原因与数据见 GPU_SNAP_INTERVAL_MS），CPU 那 4 列不受影响
/// **恢复方式**：确需 GPU 频率数据时改回 true（节流间隔见 GPU_SNAP_INTERVAL_MS）；
/// 彻底避免唤醒需另找不触碰 GPU 硬件的读数路径，本开关只是先止血
const GPU_SNAPSHOT_ENABLED: bool = false;
static LAST_GPU_SNAP_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 距上次 GPU 快照是否已超过节流间隔（到点则顺带更新时间戳）
fn gpu_snapshot_due() -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev = LAST_GPU_SNAP_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now.saturating_sub(prev) < GPU_SNAP_INTERVAL_MS {
        return false;
    }
    LAST_GPU_SNAP_MS.store(now, std::sync::atomic::Ordering::Relaxed);
    true
}

/// 读取指定 policy 的 scaling_boost_frequencies（kHz）。文件不存在、为空或解析失败时返回空 Vec，不影响 policy 注册
fn read_boost_frequencies(pid: i32) -> Vec<u32> {
    let path = format!(
        "/sys/devices/system/cpu/cpufreq/policy{}/scaling_boost_frequencies",
        pid
    );
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// 按当前模式判定是否为 boost 类模式（亲和收窄/core_ctl 保大核的判定口径）：boost / vector / 特调（akmode）为 boost 类，reduce/default 及未知模式为 normal
/// contingency / babel 有独立分支，不走 boost 亲和收窄
fn is_boost_mode(mode: &str) -> bool {
    mode == "boost" || mode == "vector" || crate::common::is_special_mode(mode)
}

/// 特调的 boost 亲和开关（参数组 boost_affinity）：游戏特调 true（保响应），视频/轻载省电特调 false（不保大核在线、不收窄 cpuset）
/// 非特调恒 true （实际只对 is_boost_mode 命中的模式有意义）
fn tuned_boost_affinity(config: &crate::chiri::config::Config, mode: &str) -> bool {
    if crate::common::is_special_mode(mode) {
        config.get_tuned_profile(mode).boost_affinity
    } else {
        true
    }
}

/// governor/GPU/极速锁频与模式的同步（幂等，可在任意入口调用）：contingency → performance 调速器 + GPU 锁最高频 + 极速锁频（min=max=硬件最高，与 vector 同口径；
/// governor 节点只读的机型写不进 performance，靠锁频窗口等效）；babel → 仅 performance；其他模式 → governor/GPU 按快照恢复（fast_lock 不在这里释放：
/// vector 也走 `_` 分支，误释放会锁不住）
fn sync_lab_governor_gpu(
    mode: &str,
    governor: &mut governor::GovernorGuard,
    gpu: &mut gpu::GpuGuard,
    fast_lock: &mut crate::chiri::fast::FastLock,
) {
    match mode {
        "contingency" => {
            if !governor.is_active() {
                governor.activate();
            }
            if !gpu.is_active() {
                gpu.lock();
            }
            // 全核最高频硬锁：不能只靠 performance——部分机型 scaling_governor 节点只读（写不进），min=max=硬件最高在任意 governor 下都等效锁频
            if !fast_lock.is_active() {
                fast_lock.init(false);
            }
        }
        "babel" => {
            if !governor.is_active() {
                governor.activate();
            }
            if gpu.is_active() {
                gpu.release();
            }
        }
        _ => {
            if governor.is_active() {
                governor.release();
            }
            if gpu.is_active() {
                gpu.release();
            }
        }
    }
}

/// 硬锁档判定：vector 与 frozen 都走 `FastLock` 的 min=max 硬锁（不注册 CLG 参数），差别只在锁定目标——vector 锁硬件最高频（极速），frozen 锁硬件最低频（待春归：最低功耗、
/// 最冷，同时停掉亲和/迁移/诊断日志这类额外开销）。两档在事件分支、停摆重建、亮屏恢复里都必须同进同出，漏一个就是「切过去没锁上」
fn is_fast_lock(mode: &str) -> bool {
    mode == "vector" || mode == "frozen"
}

/// CLG 档位参数获取（未知/空模式名禁用 CLG，避免默认参数意外接管）。与调度线程内的 get_clg_cfg 闭包同逻辑；文件级供 FAS 延迟退出等辅助函数使用
fn clg_cfg_for(
    config: &crate::chiri::config::Config,
    mode: &str,
) -> crate::chiri::config::CpuLoadGovernorConfig {
    config
        .get_mode(mode)
        .map(|m| m.cpu_load_governor.clone())
        .unwrap_or_else(|| {
            let mut cfg = crate::chiri::config::CpuLoadGovernorConfig::default();
            cfg.enabled = false;
            cfg
        })
}

/// 按目标模式接管频率（特调 / vector / CLG）。FAS 正常退出与延迟退出共用。调用前提：FAS 已完全退出（频率 + governor 已恢复）、各 governor 处于安全态、亮屏
/// akmode init 失败不设冷却（冷却语义只属于前台切换路径），直接 CLG 回退
fn apply_mode_takeover(
    mode: &str,
    config: &crate::chiri::config::Config,
    cpu_governor: &mut crate::chiri::cpu_load_governor::CpuLoadGovernor,
    ak_governor: &mut crate::chiri::tuned::TunedGovernor,
    fast_lock: &mut crate::chiri::fast::FastLock,
) {
    if crate::common::is_special_mode(mode) {
        let ak_cfg = config.get_tuned_profile(mode);
        if !ak_governor.init_policies(mode, &ak_cfg) {
            let clg_cfg = clg_cfg_for(config, mode);
            if clg_cfg.enabled {
                cpu_governor.init_policies(&clg_cfg);
            }
        }
    } else if is_fast_lock(mode) {
        // vector / frozen 都由 fast_lock 硬锁（不注册 CLG 参数），漏 init 就是频率零接管。差别只在锁定目标：vector 锁硬件最高频（极速），frozen 锁硬件最低频（待春归）
        ak_governor.release();
        cpu_governor.release();
        fast_lock.init(mode == "frozen");
    } else {
        ak_governor.release();
        fast_lock.release();
        let clg_cfg = clg_cfg_for(config, mode);
        // CLG 角色的接管交给它自己：内部会在 Worker 与 PowerBase 之间二选一（见 cpu_load_governor [backend]），
        // 这里不再替它决定谁来接管
        if clg_cfg.enabled {
            if cpu_governor.is_active() {
                cpu_governor.reload_config(&clg_cfg);
            } else {
                cpu_governor.init_policies(&clg_cfg);
            }
        } else {
            cpu_governor.release();
        }
    }
}

// [aff_snap]
/// 强制全量刷新间隔（帧，1s/帧 = 30s），兼作长尾热窗长度：每 30 帧（含首帧）
/// 所有存活候选 tid 全量落行一次、帧头打 `full=1`——下游按「缺失行 = 与上次落盘相同」
/// 重建状态，长期省略会累积漂移，刷新帧重新锚定（**存活集合也以刷新帧为锚**）；
/// 最近 30 帧内槽有变化的长尾保持逐帧采样（差分判定需要「本帧 u」），静默满 30 帧后
/// 降为刷新帧采样
const AFF_SNAP_REFRESH_FRAMES: u64 = 30;

/// 息屏时 `@S` 帧采样间隔的倍率（叠在 meta `devimp_aff_secs` 之上）。息屏期线程摆放基本静止，
/// 逐秒下钻的边际信息量低，而守护进程自身的 stat 采样开销是实打实的常驻负载
const AFF_SNAP_OFFSCREEN_FACTOR: usize = 5;

/// `@S` 帧采样节流计数（进程级）：调度线程与独立诊断线程互斥运行（非 ChiRi 才起后者），共用一个计数器无歧义
static AFF_SNAP_TICK: AtomicU64 = AtomicU64::new(0);

/// `@S` 帧本轮是否该采：`every` 为 1 时每轮都采（历史行为）。
/// 跳过的轮次**不推进** `build_aff_snapshot` 内部帧号，故刷新帧间隔同步拉长到 `30 × every` 秒、
/// 存活集合的收敛粒度同比例变粗——离线读包按 `full=1` 收敛时要知道这一层
fn aff_snap_due(every: usize) -> bool {
    AFF_SNAP_TICK.fetch_add(1, Ordering::Relaxed) % every.max(1) as u64 == 0
}

// [selfcost]
// ── B1/B3：@S 同步成本测量（真实采样路径计时入账）；D1：可选诊断 worker 交接（默认关闭）──
// 累计器由**调用线程独占**（thread_local，无全局锁），仅在 `diag_active()` ∩ `aff_snap_due`
// 已门控的采样块内计时；关闭时不探测、不改变原采样口径。

/// @S 同步成本测量的线程局部状态：累计器 + 会话标识 + 窗口起点 + 上轮诊断开闭边沿
#[derive(Default)]
struct AffCostState {
    stats: diag_cost::CostStats,
    /// 会话标识（进入会话时取一次 epoch 秒，窗口内稳定不变）
    session: String,
    /// 当前窗口起点（None = 未开始计时）
    window_open_at: Option<Instant>,
    /// 当前窗口起点的 epoch 秒（写入摘要的 `window_start`）
    window_open_epoch: u64,
    /// 上轮 `diag_active()`：会话开闭边沿判定
    was_diag: bool,
    /// 慢调用告警节流（≥ [`diag_cost::SLOW_NS`]，最多每 [`AFF_COST_SLOW_WARN`] 一条）
    last_slow_warn: Option<Instant>,
    /// 上一次 flush 的摘要写入成本 + **其实际发生窗口**（延后到下一次 flush 报出，
    /// 但归属窗口用这里存的 `(window_start, window_end)`，而非被报告时的当前窗口）。
    /// 单独存放而**不**留在 `stats` 里：避免 `reset()` 把它当成「本窗口」入账导致归属错位。
    pending_summary: Option<(u64, u64, diag_cost::BlockStats)>,
}

// 累计器按调用线程独占
thread_local! {
    static AFF_COST: RefCell<AffCostState> = RefCell::new(AffCostState::default());
}

/// 窗口时长：每 60s 输出一次摘要并清零（不足 60s 的尾窗在会话结束时输出）
const AFF_COST_WINDOW: Duration = Duration::from_secs(60);
/// 慢调用告警节流间隔（只影响告警频度，不影响入账）
const AFF_COST_SLOW_WARN: Duration = Duration::from_secs(10);

/// 当前墙钟 epoch 秒（仅作会话/窗口标识，无需本地时区）
fn aff_cost_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// [B2] 输出一个窗口的 @S 自测量摘要（[`diag_cost::Block::ALL`] 三个 block 各一行）并清零累计器。
/// 空窗口不落行（避免产生无意义文件）；写失败记 `write_error` 并告警，**不阻断原 aff 采集**。
///
/// **Summary 归属约定**：`summary` 行代表「上一次结算写入本身」的成本，其 `window_*` 是该写入
/// **实际发生的区间**，**不是**被报告时的当前窗口——`Build`/`Write` 才是当前窗口。故上一轮
/// flush 的写入成本存在 [`AffCostState::pending_summary`]（连其自身窗口），本轮先 `absorb` 进
/// `Summary` 块、用 pending 自己的窗口成行报出；本轮的写入成本再存入 pending 留给下一轮。
fn aff_cost_flush(st: &mut AffCostState) {
    let window_end_epoch = aff_cost_epoch();
    // 1. 把上一次 flush 延后报出的 summary 写入成本并入 Summary 块，并记住它**自己的**窗口
    let summary_window: Option<(u64, u64)> = match st.pending_summary.take() {
        Some((ws, we, bs)) => {
            st.stats.absorb(diag_cost::Block::Summary, &bs);
            Some((ws, we))
        }
        None => None,
    };
    let total: u64 = diag_cost::Block::ALL
        .iter()
        .map(|b| st.stats.stats(*b).count)
        .sum();
    if total > 0 {
        let cur_ws = st.window_open_epoch.to_string();
        let cur_we = window_end_epoch.to_string();
        let mut ok = true;
        let s0 = Instant::now();
        let sc0 = diag_cost::thread_cpu_ns();
        for block in diag_cost::Block::ALL {
            // 本窗口该块无入账则不落行（Build/Write 看当前窗口；Summary 见下）
            if st.stats.stats(block).count == 0 {
                continue;
            }
            // 2. Build/Write 用当前窗口；Summary 用 pending 自己的窗口（本窗口无 pending 则跳过）
            let (ws, we) = match block {
                diag_cost::Block::Summary => match summary_window {
                    Some((pws, pwe)) => (pws.to_string(), pwe.to_string()),
                    None => continue,
                },
                _ => (cur_ws.clone(), cur_we.clone()),
            };
            let line = st.stats.summary_line(&st.session, &ws, &we, block);
            if !crate::logger::selfcost_write_line(&line) {
                ok = false;
            }
        }
        let sc1 = diag_cost::thread_cpu_ns();
        let summary_cpu = match (sc0, sc1) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a)),
            _ => {
                st.stats.note_clock_error();
                None
            }
        };
        if !ok {
            st.stats.note_write_error();
            log::warn!(
                "[selfcost] 摘要写入失败（会话={} 窗口=[{},{}]），该窗口测量不完整；原 aff 帧不受影响",
                st.session,
                cur_ws,
                cur_we
            );
        }
        // 3. 记录本次摘要写入成本，归属窗口取 [刚关闭窗口的 end, 当前 epoch]（写入确实发生在此区间）
        let mut tmp = diag_cost::CostStats::new();
        tmp.record(
            diag_cost::Block::Summary,
            s0.elapsed().as_nanos() as u64,
            summary_cpu,
        );
        let bs = tmp.stats(diag_cost::Block::Summary).clone();
        let write_window_end = aff_cost_epoch();
        log::debug!(
            "[selfcost] summary 写入成本归属窗口 [{},{}]（下一次 flush 报出）",
            window_end_epoch,
            write_window_end
        );
        st.pending_summary = Some((window_end_epoch, write_window_end, bs));
        // 4. 复位本窗口块统计（保留错误计数；已 absorb 的 pending 随 reset 一并清空）
        st.stats.reset();
    } else {
        st.stats.reset();
    }
    // 5. 开下一个窗口
    st.window_open_at = Some(Instant::now());
    st.window_open_epoch = window_end_epoch;
}

/// 会话结束的尾窗排空：[`aff_cost_flush`] 把本次结算的 summary 写入成本延后存入
/// [`AffCostState::pending_summary`]（留给「下一个窗口」报出）；但会话结束时已没有下一个窗口，
/// 这里把 `pending_summary` 按其**自身窗口边界**单独补写一行，否则该成本**永久丢失**。
/// 之后整会话清零（不再依赖「Summary 残留在 `CostStats` 里」——它已不留在 `stats`）。
fn aff_cost_drain_tail(st: &mut AffCostState) {
    if let Some((ws, we, bs)) = st.pending_summary.take() {
        // 并入 Summary 块以复用 `summary_line` 的字段序与会话级错误计数
        st.stats.absorb(diag_cost::Block::Summary, &bs);
        let window_start = ws.to_string();
        let window_end = we.to_string();
        let line = st
            .stats
            .summary_line(&st.session, &window_start, &window_end, diag_cost::Block::Summary);
        if !crate::logger::selfcost_write_line(&line) {
            log::warn!("[selfcost] 会话尾窗残留摘要写入失败（会话={}）", st.session);
        }
    }
    st.stats.reset_session();
}

/// [B1] 记一帧 @S 的 build/write 两段成本（**两段 CPU 区间不重叠**：build=c0→c1、write=c1→c2），
/// 并按窗口节流输出摘要。每次调用都入账（不因慢而跳过）；慢只做节流告警。
/// 段内任一端时钟读取失败 → 该段 `cpu_ns=None` 并记一次时钟错误（不兜底为 0 或进程时间）
fn aff_cost_record(
    t0: Instant,
    c0: Option<u64>,
    t1: Instant,
    c1: Option<u64>,
    t2: Instant,
    c2: Option<u64>,
) {
    AFF_COST.with(|cell| {
        let mut st = cell.borrow_mut();
        let wall_build = t1.duration_since(t0).as_nanos() as u64;
        let wall_write = t2.duration_since(t1).as_nanos() as u64;
        let build_cpu = match (c0, c1) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a)),
            _ => {
                st.stats.note_clock_error();
                None
            }
        };
        let write_cpu = match (c1, c2) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a)),
            _ => {
                st.stats.note_clock_error();
                None
            }
        };
        st.stats.record(diag_cost::Block::Build, wall_build, build_cpu);
        st.stats.record(diag_cost::Block::Write, wall_write, write_cpu);
        // 慢调用只做节流告警（不影响入账）；不额外读 /proc
        if (wall_build >= diag_cost::SLOW_NS || wall_write >= diag_cost::SLOW_NS)
            && st
                .last_slow_warn
                .map_or(true, |t| t.elapsed() >= AFF_COST_SLOW_WARN)
        {
            st.last_slow_warn = Some(Instant::now());
            log::warn!(
                "[selfcost] @S 同步块墙钟偏慢 build={:.1}ms write={:.1}ms（阈值 {}ms，仅告警）",
                wall_build as f64 / 1e6,
                wall_write as f64 / 1e6,
                diag_cost::SLOW_NS / 1_000_000
            );
        }
        // 窗口到期（每 60s）：输出累计摘要并清零。窗口未开（会话 tick 尚未跑）时先开窗，
        // **不立即结算**——否则首帧刚入账就被当成一个只有 1 帧的窗口写出
        match st.window_open_at {
            Some(t) if t.elapsed() >= AFF_COST_WINDOW => aff_cost_flush(&mut st),
            None => {
                let now_epoch = aff_cost_epoch();
                st.window_open_at = Some(Instant::now());
                st.window_open_epoch = now_epoch;
                if st.session.is_empty() {
                    st.session = format!("s{now_epoch}");
                }
            }
            _ => {}
        }
    });
}

/// [B1/B3] 会话开闭结算（由调度线程每秒调用，累计器线程独占）：
/// - diag 假→真：起新会话（会话 id / 窗口起点重置、`reset_session` 全清含错误计数），
///   并按需启动 D1 worker、递增代际；
/// - diag 真→假：输出不足 60s 尾窗、排空延后的 summary 写入成本、关闭 selfcost 文件句柄、停 worker（均幂等）。
///
/// 尾窗写入口 `logger::selfcost_write_line` 不以 `diag_active()` 为门（只要求 devimp/ 目录已存在），
/// 因此「真→假」这一轮尾窗仍能落盘。
fn aff_cost_session_tick(diag_now: bool) {
    AFF_COST.with(|cell| {
        let mut st = cell.borrow_mut();
        if diag_now {
            if !st.was_diag {
                st.was_diag = true;
                let now_epoch = aff_cost_epoch();
                st.session = format!("s{now_epoch}");
                st.window_open_at = Some(Instant::now());
                st.window_open_epoch = now_epoch;
                st.stats.reset_session();
                diag_worker_start();
                diag_worker_bump_generation();
            }
            return;
        }
        if st.was_diag {
            // 会话结束：输出尾窗（不足 60s 也输出），随后关闭 selfcost、停 worker。
            // 尾窗写入口 `logger::selfcost_write_line` 不以 `diag_active()` 为门（只要求 devimp/ 目录已存在），
            // 故 diag 已转假时尾窗仍能落盘。
            aff_cost_flush(&mut st);
            // 排空 flush 延后存入 `pending_summary` 的本次写入成本（会话结束没有下一个窗口可报）
            aff_cost_drain_tail(&mut st);
            crate::logger::selfcost_close();
            diag_worker_shutdown();
            st.was_diag = false;
            st.window_open_at = None;
        }
    });
}

// [diag_worker_handoff] D1（**默认关闭**，不改变现网行为）
/// D1 诊断渲染/写入 worker 交接开关：**默认 false** —— 保留现同步路径（`logger::aff_snapshot`
/// 直接调用）为默认，先固定原始基线。置 true 后调度线程仍在原采样时点完成采样/差分，
/// 仅把已渲染行交 worker 写（见 [`aff_snapshot_dispatch`]）。
/// 启用前置（**缺一不可**）：B 原始基线已固定、D4 整体迁移单独评审、前台切换代际隔离已接入
/// （[`diag_worker_bump_generation`] 由调度线程在前台包变化时调用）；风险见 `diag_worker` 模块头。
const DIAG_WORKER_ENABLED: bool = false;

/// D1 worker 句柄（仅在 [`DIAG_WORKER_ENABLED`] 时由 [`diag_worker_start`] 创建；默认始终为 None）。
/// 用 `Arc` 持有：提交路径要**先克隆句柄、释放本锁**，再在锁外做可能阻塞的交付——
/// 否则队满阻塞会把 worker 生命周期管理/停机/代际切换一起卡在这把锁上
/// （违反计划「不能把串行阻塞换成共享锁等待」）
static DIAG_WORKER: OnceLock<Mutex<Option<Arc<diag_worker::DiagWorker>>>> = OnceLock::new();

/// 启动诊断 worker（仅 [`DIAG_WORKER_ENABLED`] 生效）；幂等，已启动则 no-op
fn diag_worker_start() {
    if !DIAG_WORKER_ENABLED {
        return;
    }
    let mut g = DIAG_WORKER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(Arc::new(diag_worker::DiagWorker::start()));
        log::info!("[diag] render worker started");
    }
}

/// 停机/会话切换：排空并释放诊断 worker（幂等）；默认关闭时始终为 None，等价 no-op
fn diag_worker_shutdown() {
    let taken = {
        let mut g = DIAG_WORKER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        g.take()
    };
    if let Some(w) = taken {
        w.shutdown();
        log::info!("[diag] render worker stopped");
    }
}

/// 前台切换/诊断重开时递增 worker 代际（仅启用时生效）
fn diag_worker_bump_generation() {
    if !DIAG_WORKER_ENABLED {
        return;
    }
    let g = DIAG_WORKER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(w) = g.as_ref() {
        w.bump_generation();
    }
}

// [diag_identity]
/// 读 `/proc/<pid>/stat` 第 22 字段 `starttime`（进程启动以来 tick 数，PID 复用判别用）；
/// 读不到返回 0（=unknown，按「无法判别复用」处理）。
/// 用于身份隔离：同包同 PID 的进程重启 / PID 回收必须被识别为身份变化。
/// 从**最后一个 `)`** 之后切分：comm（第 2 字段）可能含空格与括号，跨过 comm 后字段序才稳定
/// （参考 `logger::self_cpu_ms` 的同一处理）；`rest` 起始于整体第 3 字段(state)，starttime(22) → 索引 19。
fn proc_starttime_ticks(pid: i32) -> u64 {
    if pid <= 0 {
        return 0;
    }
    let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return 0;
    };
    let Some((_, rest)) = text.rsplit_once(')') else {
        return 0;
    };
    rest.split_whitespace()
        .nth(19)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

/// 前台身份三元组：包名 + pid + 进程 starttime(tick)。starttime 参与判定使**同包同 PID 复用**
/// （进程重启 / PID 回收）也被视为身份变化——否则新旧进程的帧会被混拼进同一前台键下。
type DiagIdentity = (String, i32, u64);

// [D1]
/// 配置 Debug 指纹规范化：按缩进保留层级，仅排序匿名 map 容器。
/// 列表与命名结构保持原序；只适用于 Config 的派生 Debug 格式。
fn canonical_debug(dbg: &str) -> String {
    // 每个非空行一个节点；children 存子节点下标。
    struct Node {
        line: String,
        indent: usize,
        children: Vec<usize>,
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();
    // 栈保存「当前打开的父链」，栈顶即下一个更大缩进行的父节点。
    let mut stack: Vec<usize> = Vec::new();
    for raw in dbg.lines() {
        let indent = raw.len() - raw.trim_start().len();
        let idx = nodes.len();
        nodes.push(Node {
            line: raw.to_string(),
            indent,
            children: Vec::new(),
        });
        // 弹掉缩进 >= 本行的节点，栈顶即缩进更小的父节点。
        while matches!(stack.last(), Some(&t) if nodes[t].indent >= indent) {
            stack.pop();
        }
        match stack.last() {
            Some(&parent) => nodes[parent].children.push(idx),
            None => roots.push(idx),
        }
        stack.push(idx);
    }
    // 前序 DFS；子节点按自身行规范化文本排序后入栈（逆序入栈以按序弹出）。
    let mut out = String::new();
    let mut visit: Vec<usize> = roots.iter().rev().copied().collect();
    while let Some(idx) = visit.pop() {
        out.push_str(&nodes[idx].line);
        out.push('\n');
        let mut kids = nodes[idx].children.clone();
        // Debug 的匿名花括号容器是 map；列表和命名结构保持原序。
        let parent_line = nodes[idx].line.trim();
        if parent_line == "{" || parent_line.ends_with(": {") {
            kids.sort_by(|&left, &right| {
                nodes[left].line.trim().cmp(nodes[right].line.trim())
            });
        }
        for &k in kids.iter().rev() {
            visit.push(k);
        }
    }
    // 收尾与旧的 `lines().join("\n")` 一致：不留末尾换行。
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// 全量配置身份：派生 Debug 保留字段层级与列表顺序，map 条目规范化排序。
fn diag_config_identity(cfg: &Config) -> String {
    canonical_debug(&format!("{cfg:#?}"))
}

/// 当前配置身份（跨线程：config_watcher 重载时写、调度线程提交帧时读填
/// `FrameTask.config_identity`）。默认关闭 D1 时不参与实际路径。
static DIAG_CONFIG_IDENTITY: OnceLock<RwLock<String>> = OnceLock::new();

/// 写入当前配置身份，返回是否**发生变化**（变化即由调用方 [`diag_worker_bump_generation`]）。
/// 首次写入（原为空串）不算变化——避免启动即无谓递增代际。
fn diag_config_identity_set(cfg: &Config) -> bool {
    let id = diag_config_identity(cfg);
    let cell = DIAG_CONFIG_IDENTITY.get_or_init(|| RwLock::new(String::new()));
    let mut g = cell.write().unwrap_or_else(|e| e.into_inner());
    let changed = !g.is_empty() && *g != id;
    *g = id;
    changed
}

/// 读取当前配置身份串（未初始化时为空串）；供 [`aff_snapshot_dispatch`] 填 `config_identity`。
fn diag_config_identity_get() -> String {
    DIAG_CONFIG_IDENTITY
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// [D1] 前台身份变化判定（纯函数，便于测试）：三元组（包名 / pid / starttime）**任一**变化即 `true`。
/// `prev` 为 `None`（首次）视为变化；调用方据此决定是否 bump 代际（首次不 bump，见调度循环）。
/// starttime 参与判定使「同包同 PID 复用」（进程重启 / PID 回收）也被识别为身份变化。
fn diag_identity_changed(prev: &Option<DiagIdentity>, now: &DiagIdentity) -> bool {
    match prev {
        Some(p) => p.0 != now.0 || p.1 != now.1 || p.2 != now.2,
        None => true,
    }
}

/// 采样时点冻结的**一致快照**（仅 D1 启用时构造；默认路径为 `None`）。
/// 在 `t0`**之前**、且在前台身份判定（「身份变化 → bump 代际」）**之后**一次性取齐，
/// 保证帧任务携带的身份/fg_pid/代际/帧号都取自采样时点（而非 build/写出/交付时点）。
///
/// **它消除两类错配**：
/// - 「新 PID 配旧 generation」：fg_pid/fg_starttime 复用身份判定同一次取值，generation 在此
///   冻结（dispatch 不再重读）；本 tick 检测到前台切换时 bump 已先发生，快照必携带新代际；
/// - 配置指纹、代际与采样参数在配置读锁内读取，热重载在对应写锁内发布。
struct SampledFrame {
    /// 单调采样时点，不包含 build 与队列等待。
    sampled_at: Instant,
    /// 采样时点墙钟（`MMDD-HHmmss`），作帧头 `@S ts=`
    ts: String,
    /// 采样时点前台 pid（与 `build_aff_snapshot` 同一次取值，避免两次读取不一致）
    fg_pid: i32,
    /// 采样时点前台进程 `starttime`（tick，0 = 读不到）：与 `fg_pid` 合起来识别同包同 PID 的进程重启 / PID 回收
    fg_starttime: u64,
    /// 采样时点的当前配置身份串（与 `generation`/`sequence` 同段读取）
    config_identity: String,
    /// 采样时点冻结的诊断代际（**dispatch 不得再重新读取**）：与前台身份判定在同一 tick 内、
    /// 且晚于「身份变化 → bump 代际」，故新前台必配新代际，直接用它组 `FrameId`。
    generation: u64,
    /// 采样时点分配并冻结的帧序号（与采样顺序一致）：dispatch 直接取用，不再在交付时取号；
    /// 被丢弃的帧会留下序号空洞（可接受，离线按 generation+sequence 判序）。
    sequence: u64,
}

/// 采样身份冻结：调用方须持配置读锁，且已完成前台代际更新。
fn diag_sample_frame(
    ts: String,
    fg_pid: i32,
    fg_starttime: u64,
    worker: &diag_worker::DiagWorker,
) -> SampledFrame {
    SampledFrame {
        sampled_at: Instant::now(),
        ts,
        fg_pid,
        fg_starttime,
        // 配置读锁覆盖指纹与代际读取。
        config_identity: diag_config_identity_get(),
        generation: worker.generation(),
        sequence: worker.next_sequence(),
    }
}

/// D1：@S 帧交付入口。
///
/// **时间语义分离（有意迁移，仅在 [`DIAG_WORKER_ENABLED`] 启用后生效）**：
/// - **同步路径（默认）**：调 `logger::aff_snapshot`，帧头 `@S ts=` 取**写出时刻**
///   （原始语义，逐字节不变；此时 `sampled` 恒为 `None`）；
/// - **异步路径（D1 启用）**：帧头 `@S ts=` 取**采样时点**（`sampled.ts`，由调用方在采样时点取），
///   使整帧被异步延迟写出时 `@S ts=` 仍与 `status.csv` 时序配对。
///
/// 两者 ts 来源不同是**有意的语义迁移**：默认关闭时完全不触发，行为与旧同步路径一致。
///
/// **锁序**：先克隆 `Arc<DiagWorker>` 并释放 [`DIAG_WORKER`] 锁，再在锁外调用 `submit`。
/// `submit` 在队满时会阻塞（保真优先回压），必须发生在锁外——否则停机、代际切换、
/// worker 生命周期管理会被同一次阻塞卡住，等于把串行阻塞换成了共享锁等待（计划明令禁止）。
///
/// **失败处理**：`submit` 返回 `Err(task)` 时整帧所有权被交还，这里**回退到 drain 出口**
/// （`aff_snapshot_drain`，采样 ts、不受诊断门控），不做静默丢帧（保 `full=1`、帧序号与完整
/// 快照契约）；与 worker 共用同一出口，保住整帧与**采样时点**。回退只发生在 D1 启用路径上
/// （`sampled == Some` 且 worker 曾可用），默认同步路径不经过此处。回退发生在锁外，不引入锁等待。
///
/// **代际/帧号冻结**：`generation`/`sequence` 由调用方在采样时点冻结进 [`SampledFrame`]，
/// 本函数**不**再调用 `w.generation()`/`w.next_sequence()`——避免交付时点重读造成「新 PID
/// 配旧 generation」；此处取 worker 句柄**仅**用于 `submit`。
fn aff_snapshot_dispatch(rows: Vec<String>, full: bool, sampled: Option<SampledFrame>) -> bool {
    if DIAG_WORKER_ENABLED {
        if let Some(s) = sampled {
            let worker = {
                let g = DIAG_WORKER
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                g.as_ref().map(Arc::clone)
            };
            if let Some(w) = worker {
                let id = diag_worker::FrameId {
                    generation: s.generation,
                    sequence: s.sequence,
                };
                let task = diag_worker::FrameTask {
                    id,
                    sampled_at: s.sampled_at,
                    ts: s.ts,
                    rows,
                    full,
                    fg_pid: s.fg_pid,
                    fg_starttime: s.fg_starttime,
                    config_identity: s.config_identity,
                };
                return match w.submit(task) {
                    Ok(()) => true,
                    Err(task) => {
                        // worker 已退出/通道断开：回退 drain 出口（采样 ts、不受诊断门控），
                        // 保住整帧（不丢帧、不重复写、不被 diag_active 门控丢弃）
                        log::warn!("[diag] render worker 不可用，本帧回退 drain 出口写入");
                        crate::logger::aff_snapshot_drain(&task.rows, task.full, &task.ts);
                        false
                    }
                };
            }
        }
    }
    // 默认路径 / 无采样帧：原始同步语义，帧头 ts 取写出时刻
    crate::logger::aff_snapshot(&rows, full);
    false
}

/// `uclamp` 槽的占位值：本版不下钻、恒 -1（预留字段），故除刷新帧外该槽永不落盘
const AFF_UCLAMP_RESERVED: i32 = -1;

/// 槽真值恰为 `-` 时的替身 token：`-` 已被占用为「与上次落盘相同」，故真值是 `-`
/// 的槽**改写 `NaN` 落盘**，读者按 `NaN → -` 还原。实际上只有 comm 槽取得到 `-`
/// （数值槽是整数/`-1`），但没有这条区分，「comm 变成 `-`」会被读成「comm 没变」，
/// 该线程的 comm 会永久停在旧值
const AFF_DASH_VALUE: &str = "NaN";

/// `t` 行字段级差分渲染：槽序固定 `comm / u / core / home / pin / uclamp`，
/// 与上次落盘值相同的槽写 `-`，**尾部连续未变的槽整段省略**（下游按「缺省 = 未变」
/// 逐槽合并）；真值为 `-` 的槽写 [`AFF_DASH_VALUE`]，与「未变」区分开；`pid`/`tid`
/// 恒写，是行标识而不是槽。全槽未变时渲染成只含标识的 `t <pid> <tid>`——只在 tid
/// 被复用、归属变了而各槽恰好未变时出现，用于让下游更新归属；调用方在无任何变化时
/// **不得**落行（见 `build_aff_snapshot`）
#[allow(clippy::too_many_arguments)]
fn render_t_row(
    pid: u32,
    tid: u32,
    comm: Option<&str>,
    util: Option<i32>,
    core: Option<i32>,
    home: Option<i16>,
    pin: Option<bool>,
    uclamp: Option<i32>,
) -> String {
    use std::fmt::Write as _;
    let mut out = format!("t {pid} {tid}");
    // `tail` = 最后一个「有值槽」的字节末尾；收尾按它截断即省掉尾部连续的 `-`
    let mut tail = out.len();
    match comm {
        Some(v) => {
            out.push(' ');
            // 真值为 `-` 时换 `NaN`：否则与「未变」占位同形，下游会把这次变化直接吞掉
            out.push_str(if v == "-" { AFF_DASH_VALUE } else { v });
            tail = out.len();
        }
        None => out.push_str(" -"),
    }
    match util {
        Some(v) => {
            let _ = write!(out, " u={v}");
            tail = out.len();
        }
        None => out.push_str(" -"),
    }
    match core {
        Some(v) => {
            let _ = write!(out, " core={v}");
            tail = out.len();
        }
        None => out.push_str(" -"),
    }
    match home {
        Some(v) => {
            let _ = write!(out, " home={v}");
            tail = out.len();
        }
        None => out.push_str(" -"),
    }
    match pin {
        Some(v) => {
            let _ = write!(out, " pin={}", u8::from(v));
            tail = out.len();
        }
        None => out.push_str(" -"),
    }
    match uclamp {
        Some(v) => {
            let _ = write!(out, " uclamp={v}");
            tail = out.len();
        }
        None => out.push_str(" -"),
    }
    out.truncate(tail);
    out
}

/// 单个 tid 的差分基线 + 上次落盘值（**槽级差分的对比基准**：每槽与本结构比对，
/// 未变槽写 `-`；任一槽变化即落行并回写本结构，故「本结构 == 上次落盘值」恒成立）
/// **原地更新**：entry 原地改写 + `retain` 增量清理（旧实现每帧整表新建替换），分配为 0
struct ThSnap {
    /// 上轮采样的 utime+stime（差分减数）
    ticks: u64,
    /// 上轮采样时刻：本轮 util 的窗口分母（冷长尾跨刷新帧时 = 实际间隔）
    at: Instant,
    /// 上轮**落盘**值（槽级差分基准）
    util: i32,
    core: i32,
    home: i16,
    pin: bool,
    /// 上轮落盘的归属 pid：tid 被别的进程复用时它会变，必须算「变化」
    pid: u32,
    /// 上轮落盘的线程 comm（`sample_one_tid` 的原始 stat 值，**不是**落盘用的 `aff_token` 归一化串——归一化折叠不同 comm，比对它会把「换线程」判成没变）
    /// 变更时借 `clear + push_str` 复用容量，稳态零堆分配
    comm: String,
    /// 热窗截止帧号：`frame <= hot_until` 时逐帧采样（见上方的常量说明）
    hot_until: u64,
}

/// @S 线程下钻的 stat 差分状态：上次采样时刻 + 帧序号 + 每 tid 基线
/// 与 cpu_monitor 的进程级基线（SnapState）分开，各自独立滚动
#[derive(Default)]
struct AffThState {
    /// 上次采样时刻（None = 首帧：只建基线、util 全 0）
    last_at: Option<Instant>,
    /// 帧序号（1 起，首帧 = 1）：刷新帧与热窗判定用
    frame: u64,
    /// tid → 差分基线（原地更新、增量清理，不整表重建）
    base: std::collections::HashMap<u32, ThSnap>,
}
static AFF_TH_STATE: std::sync::OnceLock<Mutex<AffThState>> = std::sync::OnceLock::new();

/// stat ticks → 秒的换算（USER_HZ，Android 恒 100）：只探测一次
fn clk_tck() -> f32 {
    static CLK_TCK: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *CLK_TCK.get_or_init(|| {
        let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if v > 0 { v as f32 } else { 100.0 }
    })
}

// （线程所在核 processor 由 affinity::sample_one_tid 同一次 stat 读取顺带解析，不再单独读 /proc/<tid>/stat——见 ThreadSample::processor）

/// 核占用掩码 hex 位图：`read_tid_mask` 给出允许核列表（如 0..7 全核），转成位图 hex（8 核全占 = `ff`）；查不到（线程消亡等）写 `-`
fn mask_hex(pid: i32) -> String {
    let Some(cpus) = affinity::read_tid_mask(pid) else {
        return "-".to_string();
    };
    let mut bits: u64 = 0;
    for c in cpus {
        if c < 64 {
            bits |= 1u64 << c;
        }
    }
    format!("{bits:x}")
}

/// @S 每秒进程/线程快照帧（`devimp/aff_<ts>.log`）组装：top-N 进程 + 前台树/
/// 被管进程线程下钻。只在 `diag_active()` 时由 1s 块调用（与 main_snap 同门控，
/// 关闭路径零采样零写入），行数组交 `logger::aff_snapshot`（帧头计数由行反推），
/// 同时返回本帧是否为刷新帧（帧头打 `full=1`，见 [`AFF_SNAP_REFRESH_FRAMES`]）
/// 候选集 = util top-N（N = meta.devimp_top_n）∪ 前台进程全部线程 ∪ 被管进程
/// 全部线程；后两者 rank=0、util=0 也落盘（前台子进程无现成枚举手段，只落前台
/// 进程+线程，不硬造 /proc 遍历）
/// ── 落行规则（字段级差分，读日志必须按此口径）──
/// **采样**：前台线程与被管条目每帧采样；长尾（只被扫到、从未动过）按热窗降采样，
/// 刷新帧全量采样
/// **落行**：本帧任一槽（comm/u/core/home/pin）与上次落盘值不同才落行；pid 只随
/// 归属变化（tid 复用）落行；**全槽未变不落行** —— 缺失 tid = 与上次落盘值相同
/// **槽级省略**（v2026-09-28）：未变槽写 `-`，尾部连续未变的槽整段省略，故整行
/// 只承载「本次变化的那部分」；下游必须按槽序 `comm/u/core/home/pin/uclamp` 逐槽
/// 合并，缺省槽 = 沿用上次值，**不能**把 `-` 当字段值。真值恰为 `-` 的槽（只可能是
/// comm）写 [`AFF_DASH_VALUE`]（`NaN`），下游还原为 `-`，以此与「未变」区分
/// **刷新帧**（帧头 `full=1`，首帧与每 [`AFF_SNAP_REFRESH_FRAMES`] 帧一次）所有存活
/// 候选 tid 全量落行：下游据此重建存活集合（连续两次刷新帧都不出现的 tid 视为已
/// 退出，粒度 = 刷新间隔），长期省略不累积漂移
/// 长尾的 `u` 是自上次采样（最多一个刷新间隔）窗口的平均值，前台/被管条目仍是 1s
/// 窗口值。代价须知：冷长尾不采样则 u 不可知，首次见到的变化后一个刷新间隔内逐帧
/// 可见（热窗），一直沉默偶发活跃的最迟下一个刷新帧才被记录，且窗口内短于刷新间隔
/// 的短促占用会被抹平——省的就是「从未动过」的长尾，别拿它做短时尖峰归因
/// 行格式（util 一律整数百分比）：
/// - 进程：`p <rank> <pid> <pkg|comm> u=<util%> mask=<核占用hex> home=<核|-1>`
/// - 线程：`t <pid> <tid> [comm] [u=] [core=] [home=] [pin=] [uclamp=]`（槽序固定，未变槽 `-`，真值 `-` 写 `NaN`）
/// 数据来源与成本：进程 util 走 `snapshot_procs`（eBPF map 差分）；线程 util 走
/// `sample_one_tid` stat 差分（只对下钻线程，长尾按热窗/刷新帧降采样）；核掩码
/// 只对落盘进程查 `read_tid_mask`；home/pin 直取入参 `th_diag`（ChiRi 由
/// AffinityManager 派生，非 ChiRi 传空表 = 无亲和接管）；uclamp
/// 不下钻恒 -1（本版永不变化，故差分行里只可能出现在刷新帧）
///
/// 入参解耦：`th_diag`（被管线程 (tid,pid,home,pinned)）与 `managed_pids`
/// 由调用方传入，使本函数脱离 `AffinityManager`，非 ChiRi 的独立诊断线程
/// 也能复用（传 `&[]` 即仅落 top-N 进程 + 前台线程流）
fn build_aff_snapshot(
    th_diag: &[(i32, i32, i16, bool)],
    managed_pids: &[i32],
    fg_pid: i32,
    fg_pkg: &str,
    top_n: usize,
) -> (Vec<String>, bool) {
    use std::collections::{HashMap, HashSet};

    // 被管线程状态（home/pin）一次取完；进程级 home 从这里派生
    let mut th_state: HashMap<u32, (i16, bool)> = HashMap::with_capacity(th_diag.len());
    let mut pid_home: HashMap<u32, i16> = HashMap::new();
    for (tid, pid, home, pinned) in th_diag {
        th_state.insert(*tid as u32, (*home, *pinned));
        // 进程级 home（确定性归属，与 HashMap 遍历序无关）：取最小有效 home，全无有效记录落 -1——多钉线程进程的 p 行 home 不再逐帧漂移
        let slot = pid_home.entry(*pid as u32).or_insert(-1);
        if *home >= 0 && (*slot < 0 || *home < *slot) {
            *slot = *home;
        }
    }

    // ── 线程下钻候选集 = 前台树全量 ∪ 被管线程表（按 tid 去重，稳定排序）──
    // full = 每帧全量采样 + 全量落行（①前台进程线程 / ②被动过的被管条目；
    // thread_diag 的 pinned 已含 home ≥ 0 ∪ group_bind ≠ None）；full=false 为长尾
    let mut tids: Vec<(u32, u32, bool)> = Vec::new(); // (pid, tid, full)
    let mut seen_tid: HashSet<u32> = HashSet::new();
    if fg_pid > 0 {
        for tid in crate::monitor::cpu_monitor::get_thread_tids(fg_pid as u32) {
            if seen_tid.insert(tid) {
                tids.push((fg_pid as u32, tid, true));
            }
        }
    }
    for (tid, pid, _, pinned) in th_diag {
        if seen_tid.insert(*tid as u32) {
            tids.push((*pid as u32, *tid as u32, *pinned));
        }
    }
    tids.sort_unstable();

    // 线程采样：stat 差分（首见/首帧只建基线 util=0）+ comm；core 取 stat 的 processor 字段（读不到 -1）。采样失败（线程已退出）即不落行
    // 末位 = 本帧要落的 `t` 行（None = 全槽未变、省行；差分的核心：采样了也可能省行）
    // [C1] 第 3 字段为**已净化的 comm token**（`aff_token` 结果）：只在需要时构造（惰性），
    // 未构造的省行路径存占位 `Cow::Borrowed("")`。因 `s.comm` 是迭代内局部值、token 要跨迭代
    // 存入本表，故存 `'static`（构造时 `into_owned`，借用不跨迭代保存）
    let mut th_rows: Vec<(u32, u32, Cow<'static, str>, Option<String>)> =
        Vec::with_capacity(tids.len());
    // 本帧是否刷新帧（帧头据此打 `full=1`，下游按它重建存活集合）
    let refresh;
    {
        let mut st = AFF_TH_STATE
            .get_or_init(|| Mutex::new(AffThState::default()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let first = st.last_at.is_none();
        st.last_at = Some(now);
        st.frame += 1;
        let frame = st.frame;
        // 刷新帧：首帧必全量（全建基线、util 全 0），此后每 30 帧一次（防漂移）
        refresh = first || frame % AFF_SNAP_REFRESH_FRAMES == 0;
        // [C1] 本帧「已采样到的 pid」集合：用于判定每 pid 本帧首条线程——其净化 comm 是补位
        // p 行的兜底来源，即使该线程本帧省行也必须保留 token（把旧实现「所有线程都带 token」
        // 的隐式依赖显式化，保证下方 `pid_comm` 兜底仍取到每 pid 首条的净化 comm）
        let mut seen_pid_frame: HashSet<u32> = HashSet::new();
        for &(pid, tid, full) in &tids {
            let (home, pinned) = th_state.get(&tid).copied().unwrap_or((-1, false));
            // 长尾且已退冷：本轮**不采样、不落行**（省 stat 读与行；语义由「缺失行 = 与上次落盘相同」承载）。刷新帧或热窗内的仍逐帧采样
            if !full && !refresh && !st.base.get(&tid).is_some_and(|e| e.hot_until >= frame) {
                continue;
            }
            let Some(s) = affinity::sample_one_tid(tid as i32) else {
                // 线程已退出：顺手回收基线（原地 remove，不整表重建）
                st.base.remove(&tid);
                continue;
            };
            // [C1] 本帧该 pid 首条被采样线程（见上方 seen_pid_frame 说明）：即使本线程省行，
            // 也要为它保留净化 comm，供 p 行兜底
            let first_pid = seen_pid_frame.insert(pid);
            // [C1] 净化 comm 并落成 `'static`：`aff_token` 可能借用 `s.comm`，而 token 要跨迭代
            // 存进 th_rows，故必须 `into_owned()`（借用不跨迭代保存）。此闭包只在需要时调用，
            // 常态省行路径完全不构造——**注意**：一旦构造就必然分配一次，
            // 所以 C1 的实际收益是「减少需要构造 token 的线程数」，不是快照内部零分配。
            // `ThSnap.comm` 的比对仍用**原始** `s.comm`（不参与本 token）
            let clean_comm = |c: &str| -> Cow<'static, str> {
                crate::logger::aff_token(c).into_owned().into()
            };
            // 窗口分母取该 tid 自己的上次采样时刻：冷长尾跨刷新帧时窗口 = 实际间隔
            let win_secs = match st.base.get(&tid) {
                Some(e) => now.duration_since(e.at).as_secs_f32(),
                None => 0.0,
            };
            // 单线程只能跑在一个核上：util ≤ 100%（采样抖动由 clamp 兜底）
            let util_pct = match st.base.get(&tid) {
                Some(e) if win_secs > 0.0 && s.ticks >= e.ticks => {
                    (((s.ticks - e.ticks) as f32 / clk_tck()) / win_secs * 100.0).clamp(0.0, 100.0)
                }
                _ => 0.0,
            };
            let util = util_pct.round() as i32;
            // [C1] 惰性 token 槽：None = 尚未构造（本帧不需要落 comm）
            let mut comm_tok: Option<Cow<'static, str>> = None;
            // 基线**原地更新**（复用 entry），顺带按槽级差分组出本帧要落的行
            let row = match st.base.get_mut(&tid) {
                Some(e) => {
                    // 槽级差分基准 = 本结构（恒等于上次落盘值）；身份（pid/comm）一并参与：冷长尾最长 30 帧才采样一次，期间 tid 复用若不比身份，新线程会沿用旧 pid/comm 的行
                    let comm_ch = e.comm != s.comm;
                    let util_ch = e.util != util;
                    let core_ch = e.core != s.processor;
                    let home_ch = e.home != home;
                    let pin_ch = e.pin != pinned;
                    // pid 不是槽（行标识恒写），但归属变了也必须落一行，否则下游的 tid→归属映射会一直停在旧值
                    let pid_ch = e.pid != pid;
                    let slot_ch = comm_ch || util_ch || core_ch || home_ch || pin_ch;
                    let any_ch = slot_ch || pid_ch;
                    // 有变化即续热窗 30 帧：之后仍逐帧采样（差分判定要看「本帧 u」）；静默满 30 帧自然退冷，回到 30 帧一次的刷新采样
                    if any_ch {
                        e.hot_until = frame + AFF_SNAP_REFRESH_FRAMES;
                        // comm 只在真变了才改写，复用 String 容量、稳态零分配
                        if comm_ch {
                            e.comm.clear();
                            e.comm.push_str(&s.comm);
                        }
                    }
                    e.ticks = s.ticks;
                    e.at = now;
                    e.util = util;
                    e.core = s.processor;
                    e.home = home;
                    e.pin = pinned;
                    e.pid = pid;
                    if refresh {
                        // 刷新帧：全量重锚（下游据此重建存活集合），不做槽级省略；comm 必构造
                        comm_tok = Some(clean_comm(&s.comm));
                        Some(render_t_row(
                            pid,
                            tid,
                            comm_tok.as_ref().map(|c| &**c),
                            Some(util),
                            Some(s.processor),
                            Some(home),
                            Some(pinned),
                            Some(AFF_UCLAMP_RESERVED),
                        ))
                    } else if any_ch {
                        // 常态：只写变化的那部分，未变槽 `-`、尾部未变槽整段省略；
                        // [C1] 仅 comm 真变化时才构造 token（其余槽变化不构造）
                        if comm_ch {
                            comm_tok = Some(clean_comm(&s.comm));
                        }
                        Some(render_t_row(
                            pid,
                            tid,
                            comm_tok.as_ref().map(|c| &**c),
                            util_ch.then_some(util),
                            core_ch.then_some(s.processor),
                            home_ch.then_some(home),
                            pin_ch.then_some(pinned),
                            None,
                        ))
                    } else {
                        None
                    }
                }
                None => {
                    // 首见（新线程 / 新进程线程）：无基准可比，六槽全写（全量行），长尾则进热窗观察 30 帧；comm 必构造
                    st.base.insert(
                        tid,
                        ThSnap {
                            ticks: s.ticks,
                            at: now,
                            util,
                            core: s.processor,
                            home,
                            pin: pinned,
                            pid,
                            comm: s.comm.clone(),
                            hot_until: frame + AFF_SNAP_REFRESH_FRAMES,
                        },
                    );
                    comm_tok = Some(clean_comm(&s.comm));
                    Some(render_t_row(
                        pid,
                        tid,
                        comm_tok.as_ref().map(|c| &**c),
                        Some(util),
                        Some(s.processor),
                        Some(home),
                        Some(pinned),
                        Some(AFF_UCLAMP_RESERVED),
                    ))
                }
            };
            // [C1] 每 pid 本帧首条线程：即便上面未构造 token（该线程全槽未变、本帧省行），
            // 也要补齐，否则 p 行 comm 兜底会取不到该 pid 的净化 comm（与旧行为不一致）
            if first_pid && comm_tok.is_none() {
                comm_tok = Some(clean_comm(&s.comm));
            }
            th_rows.push((pid, tid, comm_tok.unwrap_or(Cow::Borrowed("")), row));
        }
        // 增量清理：只保留本轮候选集内的 tid（线程消亡随候选集消失回收），`retain` 原地收缩零分配注意保留本轮未采样的冷长尾：其基线 `at` 要留到刷新帧才能算出覆盖整段间隔的 util，
        // 删掉会把窗口重置为 0
        st.base.retain(|tid, _| seen_tid.contains(tid));
    }
    // 被管进程的 comm 兜底（不在进程快照里时用其任一线程 comm）。取**采样到**（含本帧省行）的线程：省行的线程其进程仍可能在补位 p 行上
    // [C1] 每 pid 首条线程的 token 已在上方保证构造（`first_pid` 分支）；`or_insert` 先到先得，
    // 故兜底仍拿到「每 pid 本帧首条线程的净化 comm」，与旧实现一致。占位空串只出现在非首条线程上，永不入选
    let mut pid_comm: HashMap<u32, &str> = HashMap::new();
    for (pid, _, comm, _) in &th_rows {
        pid_comm.entry(*pid).or_insert(&**comm);
    }

    // ── 进程集合：util top-N ∪ 前台进程 ∪ 被管进程 ──
    let mut procs = crate::monitor::cpu_monitor::snapshot_procs();
    // util 降序 + pid 升序 tie-break：等 util（常见于大量 util=0 的冷进程）时行序与 top-N 成员不随 BPF map 遍历序逐帧抖动
    procs.sort_by(|a, b| {
        b.util
            .partial_cmp(&a.util)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.pid.cmp(&b.pid))
    });
    // config normalize 已钳 1..=64；此处兜底还要接住 Config::load 失败降级路径（derive Default 的 devimp_top_n=0，未经 normalize）
    // ——0 按缺省 10 恢复
    let top_n = if top_n == 0 { 10 } else { top_n.clamp(1, 64) };
    let top_len = top_n.min(procs.len());
    let proc_of: HashMap<u32, _> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut extra: Vec<u32> = Vec::new();
    let mut seen_pid: HashSet<u32> = HashSet::new();
    for p in procs.iter().take(top_len) {
        seen_pid.insert(p.pid);
    }
    if fg_pid > 0 && seen_pid.insert(fg_pid as u32) {
        extra.push(fg_pid as u32);
    }
    let mut managed: Vec<u32> = managed_pids.iter().map(|p| *p as u32).collect();
    managed.sort_unstable();
    for pid in managed {
        if seen_pid.insert(pid) {
            extra.push(pid);
        }
    }

    // ── 拼行：p 行在前（top-N rank=1..N 按 util 降序，补位行 rank=0），t 行随后 ──
    let mut rows: Vec<String> = Vec::with_capacity(top_len + extra.len() + th_rows.len());
    for (rank, p) in procs.iter().take(top_len).enumerate() {
        let name = if p.pid as i32 == fg_pid && !fg_pkg.is_empty() {
            fg_pkg
        } else {
            p.comm.as_str()
        };
        // comm/cmdline 可含空白与控制字符（含换行会拆帧），过 aff_token 净化
        let name = crate::logger::aff_token(name);
        rows.push(format!(
            "p {} {} {} u={} mask={} home={}",
            rank + 1,
            p.pid,
            name,
            p.util.round() as i32,
            mask_hex(p.pid as i32),
            pid_home.get(&p.pid).copied().unwrap_or(-1),
        ));
    }
    for pid in extra {
        let (name, util) = match proc_of.get(&pid) {
            Some(p) => (p.comm.as_str(), p.util),
            // 快照里没有（不在 TGID map 的冷进程）：comm 兜底用其下钻线程的 comm
            None => (pid_comm.get(&pid).copied().unwrap_or("-"), 0.0),
        };
        // 前台进程优先写包名（pkg|comm 口径）；comm 同样净化（防拆帧）
        let name = if pid as i32 == fg_pid && !fg_pkg.is_empty() {
            fg_pkg
        } else {
            name
        };
        let name = crate::logger::aff_token(name);
        // 与 t 行取整口径一致（round，非 {:.0} 的向偶取整）
        let u = util.round() as i32;
        rows.push(format!(
            "p 0 {pid} {name} u={u} mask={} home={}",
            mask_hex(pid as i32),
            pid_home.get(&pid).copied().unwrap_or(-1),
        ));
    }
    // t 行已在上方按槽级差分渲染好：`None` = 全槽未变（省行），`Some` = 本帧要落的差异行
    rows.extend(th_rows.into_iter().filter_map(|(_, _, _, row)| row));
    (rows, refresh)
}

// [affinity]
/// 应用 CPU 亲和布局与 core_ctl 在线策略（ChiRi 专属，跟随模式/屏幕/前台 PID）。内部带去重：布局与 PID 未变化时无 sysfs 写入，可安全周期性调用
/// `core_utils` 为最近一次 SystemLoadUpdate 的逐核 util（按核选核打分输入）。`scene_mode_active` 为 **scenemode 激活标志**
/// （与配置字段 `CoreCtl.scenemode_offline` 不是一回事，勿混）：激活时抑制 boost（boost 会把 min_cpus 抬回全组常在线、
/// 把被压制的核拉回来）；是否真的把 little 簇下线（除引导核）+ WALT core_ctl `max_cpus` 钳核数，另由 `config.core_ctl.scenemode_offline` 门控
/// （见 core_ctl.rs 的 `scenemode_targets` / `scenemode_keep_cpus`）
fn apply_affinity_and_corectl(
    affinity: &mut affinity::AffinityManager,
    corectl: &mut core_ctl::CoreCtlManager,
    config: &Config,
    mode: &str,
    screen_on: bool,
    fg_pid: i32,
    core_utils: &[f32],
    scene_mode_active: bool,
) {
    // 实验室静态分组模式（contingency/babel）：停线程迁移、按模式写组级 cpus；governor/GPU 由调用方 sync（sync_lab_governor_gpu）
    // 周期重入即纠偏（框架写回的 top-app/foreground 会被重写）。core_ctl 交回系统（NONE）
    if mode == "contingency" || mode == "babel" {
        // lab 静态分组本质是线程/核心摆放，与普通亲和一样受总闸约束（机型子开关与 meta.thread_bind 在 Config::load 取「与」——后者是实验室 frozen 专用闸）
        if config.affinity.enabled {
            affinity.lab_static_apply(mode);
        } else {
            affinity.lab_static_deactivate();
        }
        corectl.set_power_state(false, false);
        return;
    }
    // stardust 家族（scenemode）语义：停线程迁移与动态分组、全部 cpuset 恢复全核——压频只压 CLG 频率上限；
    // 核心层面是否下线 little 簇（除引导核）+ WALT core_ctl `max_cpus` 钳制保留核数（见 core_ctl.rs [max_cpus]）
    // 由 `CoreCtl.scenemode_offline` 决定（默认 true 保持既有行为）；置 false 时保留全核、只靠频率上限压制，
    // 线程/组摆放不做任何特化。affinity.release 会把此前收窄的组按快照恢复。本分支由 2s 周期块与场景事件反复进入：
    // 仅在持有接管时 release 一次，避免息屏全程每 2s 重复回写后台组 uclamp max 并刷「已释放接管」日志
    if scene_mode_active {
        if affinity.is_active() {
            affinity.release();
        }
        corectl.set_power_state(
            false,
            config.core_ctl.enabled && config.core_ctl.scenemode_offline,
        );
        return;
    }
    // fas 模式按 boost 处理：FAS 只负责调频，线程摆放沿用 boost 布局（top-app/foreground 收窄 prime∪big + 前台钉核 + core_ctl 保大核）；
    // FAS 与屏幕状态完全解耦（息屏不再释放实例），故全时段入 boost，不按 screen_on 回退 normal语义声明：boost 只看 mode、
    // 不查 FAS 引擎是否活跃——初始化失败冷却期（最长FAS_COOLDOWN=300s）与重激活间隙内， mode 仍为 fas，boost 布局照常生效（调频为CLG default）。这是有意的：
    // mode=="fas" 时前台必为白名单游戏，摆放非负收益，gating is_active 反而会引入激活边界的布局抖动省电型特调（boost_affinity=false，如 playback/daily）
    // 不走 boost：不收窄 cpuset、不保大核常在线——与省电目标相反（大核空转漏电、解码线程被低上限压住）
    let boost = ((is_boost_mode(mode) && tuned_boost_affinity(config, mode)) || mode == "fas")
        && !scene_mode_active;
    // top-app uclamp.max 放开（激活期写 100 让重线程可被 EAS 放到 prime）的管理归属：特调（akmode）由本函数按当前模式同步 Some(true)；
    // fas 交给 fas_affinity_hook（None = 本函数不干预，避免与其时序打架）；其余 boost 模式 Some(false)——保证离开特调后不残留 100
    // 省电型特调同样不放开（不抬 prime 上限）
    let uclamp_override = if mode == "fas" {
        None
    } else {
        Some(crate::common::is_special_mode(mode) && tuned_boost_affinity(config, mode))
    };
    affinity.apply(
        screen_on,
        fg_pid,
        &config.affinity,
        boost,
        core_utils,
        uclamp_override,
    );
    // core_ctl 在线接管随 boost 走；scenemode 的 prime 压制在上方分支处理（core_ctl max_cpus 首选、逐核 offline 兜底），不与本路径混用
    corectl.set_power_state(config.core_ctl.enabled && boost, false);
}

/// FAS 亲和接入点：FAS 激活/去激活时调整线程摆放相关状态。当前职责：激活期放开 top-app uclamp.max 写 100（boost 布局写的 85 会钳制 EAS 对重线程的 capacity 视图、
/// 抑制 prime 放置，与 FAS 让 prime 承接负载相悖；FAS 激活期锁频绕过 schedutil，85 对调频无效），去激活还原后续 FAS 线程钉核扩展也在此实现
fn fas_affinity_hook(
    affinity_mgr: &mut affinity::AffinityManager,
    corectl_mgr: &mut core_ctl::CoreCtlManager,
    active: bool,
    fg_pid: i32,
) {
    let _ = (corectl_mgr, fg_pid);
    affinity_mgr.set_boost_uclamp_override(active);
}

// [mode_file]
/// `current_mode.chr` 的写入记账：值未变且磁盘内容一致时跳过写入5s 自愈块的无条件重写是 5 个 syscall（exists + chmod 664 + write + chmod 444 及开关文件）
/// ，稳态下内容几乎从不变化，纯浪费；记账后稳态轮次只剩读一次内容比对（1 个 syscall）**不能只判 `exists()`**：文件被外部清空/改写时它依然存在，而「被清空」
/// 恰是这类状态文件最常见的损坏形态——读内容比对既保住「外部清空/改写也会自愈」的语义，又远便宜于无条件重写写路径、文件权限（664 → 444）与失败处理仍交给 `try_write_file`，与原实现一致
struct ModeFile {
    path: std::path::PathBuf,
    /// 上次经本写入器落盘的模式值（None = 尚未写过 / 文件已被删除）
    last: Option<String>,
}

impl ModeFile {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path, last: None }
    }

    /// 无条件写入（启动初始写入、模式切换 / FAS 退出 / DOWN 进入等事件路径用）：事件发生时模式确实变了，保持原有「每次都写」行为，只顺带更新记账
    fn write(&mut self, mode: &str) {
        let _ = utils::try_write_file(&self.path, mode.as_bytes());
        self.last = Some(mode.to_string());
    }

    /// 删除文件（DOWN 退出时调用）：必须一并清记账——否则文件缺失后，一个「与记账相同」的模式切换会被误判为无需写入，留下最长 5s 的文件空窗
    fn remove(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        self.last = None;
    }

    /// 5s 自愈：记账值相同 **且** 磁盘内容一致才跳过，否则无条件重写（外部清空 / 改写 / 删除都会因比对不等而触发自愈）
    fn heal(&mut self, mode: &str) {
        if self.last.as_deref() == Some(mode) && file_content_eq(&self.path, mode.as_bytes()) {
            return;
        }
        self.write(mode);
    }
}

/// 读一次文件内容与期望字节比对（供跳过无变化的周期重写）。读失败（文件缺失 / 不可读）即判不等 → 由调用方重写自愈
fn file_content_eq(path: &std::path::Path, expected: &[u8]) -> bool {
    matches!(std::fs::read(path), Ok(cur) if cur == expected)
}

// [diag_thread]
/// 独立诊断写入线程（**非 ChiRi 机型专用**，由 main.rs 在无调度线程时启动）：
/// 把 devimp 的 `main_*` / `aff_*` 产出从 ChiRi `scheduler_ipc` 的 1s 块中解耦出来
/// ——ChiRi 的 devimp 写入全在调度线程内，非 ChiRi 不启动调度线程即零产出；本线程
/// 让 `devimp_main` 在任意 SoC、任意模式（含 DOWN/停摆）下都能随 `meta.dev_record`
/// 独立开关，不受调度接管与停摆门控影响。
///
/// 只读快照 + 写 devimp，不触碰任何 sysfs 调度节点（无接管语义，纯采集）：
/// - 模式列：非 ChiRi 无调度接管，取 `app_detect` 判定结果，空则回退 rules 全局模式；
/// - 温度：电池为安全边界、CPU 仅记录，与调度线程同口径（不做调度侧滤波）；
/// - `aff_` 线程流：非 ChiRi 无亲和接管，被管线程表传空 → 仅落 top-N 进程 + 前台线程；
/// - thermal/clg 语义为 ChiRi 专属，非 ChiRi 恒写中性值（cap=100、clg=0）。
///
/// `config_path` 用于每 5s 重读 `meta.dev_record` / `meta.devimp_top_n`（热重载口径
/// 与调度线程一致）。线程名 `diag_writer`，与调度线程互不依赖。
pub fn start_diag_thread(config_path: std::path::PathBuf) -> Result<()> {
    // 温度源探测（与 scheduler_ipc 同口径）：电池作为热安全边界，CPU 温度仅记录
    let temp_sensor_path = crate::utils::find_cpu_temp_path().ok();
    let batt_sensor_exists = crate::utils::find_battery_temp_path().is_some();
    // 模式列回退值：rules 全局模式（非 ChiRi 无 per-app 接管时的缺省）
    let fallback_mode = {
        let g = crate::common::embedded_rules().global_mode;
        if g.is_empty() {
            crate::down::DOWN_MODE.to_string()
        } else {
            g
        }
    };

    // 记录线程不许静默死：monitor 侧的线程由 `monitor::spawn_guarded` 兜底（panic → 打点 → 交看门狗拉起），
    // 本线程由 chiri 侧自起、不在其覆盖范围，故自带一层 catch_unwind。DOWN 的职责就是记录，
    // 记录线程无声停摆是最难查的形态（表现为「devimp 忽然不长了」），至少留一条 error
    thread::Builder::new()
        .name("diag_writer".to_string())
        .spawn(move || {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut last_cfg_sync: Option<Instant> = None;
            let mut dev_record = false;
            let mut top_n: usize = 10;
            let mut aff_secs: usize = 1;
            // 变化跟踪（null = 无基准，重开后首帧按「已变化」处理）
            let mut prev_screen_on: Option<bool> = None;
            let mut prev_mode: Option<String> = None;

            loop {
                // 每 5s 重读 meta：dev_record / devimp_top_n / devimp_aff_secs 热重载生效（与调度线程同口径）
                if last_cfg_sync.map_or(true, |t| t.elapsed() >= Duration::from_secs(5)) {
                    last_cfg_sync = Some(Instant::now());
                    if let Ok(cfg) = Config::load(config_path.to_str().unwrap_or_default()) {
                        dev_record = cfg.meta.dev_record;
                        top_n = cfg.meta.devimp_top_n;
                        aff_secs = cfg.meta.devimp_aff_secs;
                    }
                }
                crate::logger::set_diag_active(dev_record);

                // 关闭态：零采样零写入（不读包名/屏幕、不读温度），仅保留 1s 节拍等开关重开
                if !dev_record {
                    prev_screen_on = None;
                    prev_mode = None;
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }

                let fg_pkg = crate::monitor::app_detect::get_current_package();
                let is_screen_on = crate::common::screen_on();
                // 模式列：非 ChiRi 无调度接管，取 app_detect 判定结果；空（未判定/息屏清空）回退 rules 全局模式
                let mode = {
                    let m = crate::monitor::app_detect::last_determined_mode();
                    if m.is_empty() {
                        fallback_mode.clone()
                    } else {
                        m
                    }
                };
                crate::logger::set_diag_mode(&mode);
                // 诊断日志按前台包名分组：变化即切换 main_ 文件（内部去重，空包名不切换）
                crate::logger::set_diag_package(&fg_pkg);

                // 状态变化事件（与 ChiRi 同口径，decision 列记事件名）。首帧只建立基准不报事件：
                // 初始 screen/mode 已由每秒 snap 行承载，事件行只表达「变化」
                if let Some(prev) = prev_screen_on {
                    if prev != is_screen_on {
                        crate::logger::main_event(
                            "screen",
                            "-",
                            if is_screen_on { "on" } else { "off" },
                        );
                    }
                }
                prev_screen_on = Some(is_screen_on);
                if let Some(prev) = prev_mode.as_deref() {
                    if prev != mode.as_str() {
                        crate::logger::main_event("mode_change", &fg_pkg, &mode);
                    }
                }
                prev_mode = Some(mode);

                // 温度本秒读一次（原始值；本线程无热判定，不做调度侧滤波）
                let batt_temp = if batt_sensor_exists {
                    crate::utils::read_battery_temp_celsius()
                } else {
                    None
                };
                let cpu_temp = temp_sensor_path
                    .as_ref()
                    .and_then(|p| crate::utils::read_f64_from_file(p).ok())
                    .map(|v| (v / 1000.0) as f32);

                let tm = crate::monitor::telemetry::telemetry();
                let fmt_opt = |v: Option<f32>, digits: usize| {
                    v.map(|x| format!("{:.*}", digits, x))
                        .unwrap_or_else(|| "-".to_string())
                };
                // 电池三元组本秒只读一次（同一快照），各列共用
                let (batt_voltage, batt_current, batt_power) = tm.batt_sample();
                let (cpu_cur, cpu_max, cpu_min, cpu_gov) = crate::chiri::cpu_freq_snapshot();
                let (gpu_cur, gpu_max, gpu_min, gpu_gov) =
                    if GPU_SNAPSHOT_ENABLED && gpu_snapshot_due() {
                        crate::chiri::gpu::devfreq_snapshot()
                    } else {
                        (
                            "-".to_string(),
                            "-".to_string(),
                            "-".to_string(),
                            "-".to_string(),
                        )
                    };

                // snap 行（1s）：thermal/clg 为 ChiRi 专属语义，非 ChiRi 写中性值
                crate::logger::main_snap(
                    is_screen_on,
                    &fmt_opt(batt_temp, 1),
                    &fmt_opt(cpu_temp, 1),
                    "100",
                    false,
                    &fmt_opt(Some(tm.psi_cpu_some()), 2),
                    &fmt_opt(Some(tm.psi_io_some()), 2),
                    &fmt_opt(Some(tm.psi_mem_some()), 2),
                    &fmt_opt(tm.gpu_busy(), 0),
                    &fmt_opt(batt_voltage, 3),
                    &fmt_opt(batt_current, 0),
                    &fmt_opt(batt_power, 2),
                    0,
                    0,
                    0,
                    &cpu_cur,
                    &cpu_max,
                    &cpu_min,
                    &cpu_gov,
                    &gpu_cur,
                    &gpu_max,
                    &gpu_min,
                    &gpu_gov,
                );
                // @S 进程/线程快照（aff_* 线程流文件）：非 ChiRi 无亲和接管，
                // 被管线程表与被管进程集传空 → 仅 top-N 进程 + 前台线程。
                // 采样间隔受 meta `devimp_aff_secs` 节流（息屏再乘倍率）：跳过的轮次不推进帧号
                let aff_every = aff_secs * if is_screen_on { 1 } else { AFF_SNAP_OFFSCREEN_FACTOR };
                if aff_snap_due(aff_every) {
                    let (rows, full) = build_aff_snapshot(
                        &[],
                        &[],
                        crate::monitor::app_detect::get_current_pid(),
                        &fg_pkg,
                        top_n,
                    );
                    crate::logger::aff_snapshot(&rows, full);
                }

                thread::sleep(Duration::from_secs(1));
            }
        }))
        .is_err()
        {
            log::error!("{}", t("main-diag-thread-panic"));
        }
        })?;
    Ok(())
}

// [threads]
/// 启动 Chiri 调度线程组（由 main.rs 调用）：-`config_watcher` 监听 config 目录，热重载 Config 并重放一次性系统调整；
/// `scheduler_ipc` 消费 `DaemonEvent` 状态机，驱动 CLG 接管/释放/配置切换`rx` 为 Monitor↔调度层的有界事件通道，`shared_config` 为全局共享配置，
/// `ak_active` 为特调激活标志（Monitor 层据此切换采样间隔），`fas_signal` 为 FAS 前台激活信号（FasManager 置位并唤醒等待者，
/// fps_monitor 据此门控 eBPF 探针加载与 uprobe 挂载——反偷跑）
pub fn start_scheduler_thread(
    rx: mpsc::Receiver<DaemonEvent>,
    shared_config: Arc<RwLock<Config>>,
    ak_active: Arc<AtomicBool>,
    fas_signal: Arc<crate::monitor::FasSignal>,
    rhine_initial: Option<String>,
) -> Result<()> {
    let root = common::get_module_root();
    // 配置路径：8550 等 Chiri 目标 SoC 使用处理器子目录 config/{soc}/meta.yaml，热重载跟随该文件
    let config_path = common::get_config_path();
    let config_dir = root.join("config");

    // 初始模式透传 rules.yaml 的 global_mode：硬编码 "default" 会在开机到首个ModeChange（约 2 秒）前按错误模式接管 CPU；未配置/未注册时回退 default
    let initial_mode = {
        // 嵌入 rules.yaml 为唯一规则来源（编译期打包防篡改；磁盘文件仅展示副本）实验室启用时其全局模式覆盖优先，否则开机后 ~2s 会按 rules 旧值接管
        let rules = crate::common::embedded_rules();
        let m = crate::common::lab_global_mode().unwrap_or_else(|| rules.global_mode.clone());
        if m.is_empty() || shared_config.read().unwrap().get_mode(&m).is_none() {
            "default".to_string()
        } else {
            m
        }
    };

    // 当前生效模式名（跨线程共享，仅在 scheduler_ipc 线程内写）；另留一份给 DOWN 停摆退出恢复用，初值只是启动缺省，真正停摆时会被当时的实际模式覆盖
    let mut down_resume_mode = initial_mode.clone();
    let shared_mode_name = Arc::new(Mutex::new(initial_mode));
    // sysfs 路径存在性缓存，避免每次 IO 调整前重复探测
    let sys_path_exist = Arc::new(utils::SysPathExist::new());
    // 触摸事件通道（事件驱动）：触摸检测线程发送触摸事件，scheduler_ipc 即时处理并触发大核升频
    let (touch_tx, touch_rx) = mpsc::sync_channel::<()>(8);
    // feature/meta 热重载联动标志：config_watcher 成功重载后置位，scheduler_ipc 轮询消费——修复「调参要等下次 ModeChange 才生效」的热更新断链
    let config_dirty = Arc::new(AtomicBool::new(false));

    // [down_watcher]
    // DOWN 停摆状态监听：down.chr 一被写入/清空就切换停摆，与 meta.yaml、rhine.chr
    // 同语义；这里只维护「该不该停摆」，真正的释放/恢复在调度循环里做（governor
    // 归它独占）**必须排在所有会写 sysfs 的线程与首次下发之前**——一次性系统
    // 调整等都按它决定是否下发，否则「重启后仍停摆」的启动会先写一遍 tweak
    let down_root = root.clone();
    crate::down::on_startup(&down_root);
    thread::Builder::new()
        .name("down_watcher".to_string())
        .spawn(move || crate::down::watch_loop(down_root))?;

    // 启动时立即应用一次性系统调整（cpuidle / IO / 屏蔽系统自带触摸升频 / 内核 sched
    // 参数），避免首次配置变更前未生效（config_watcher 仅在配置变化后重放）
    // **停摆启动期跳过**：DOWN 语义是「一切交回系统」，这些节点里正有一批与调度
    // 直接相关，下发等于停摆期还在替用户调度，基线不再是系统原状；停摆判定在
    // `apply_system_tweaks` 内（唯一下发入口，带互斥闸），此处不重复判
    if let Err(e) =
        CpuScheduler::new(shared_config.clone(), sys_path_exist.clone()).apply_system_tweaks()
    {
        log::error!(
            "{}",
            t_with_args(
                "config-apply-tweaks-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
    }

    // 触摸检测线程（Chiri 专属）：读取 /dev/input 触摸事件，经事件通道驱动大核触摸升频
    thread::Builder::new()
        .name("touch_detect".to_string())
        .spawn(move || {
            crate::chiri::touch_detect::monitor_touch(touch_tx);
        })?;

    // [config_watcher]
    let config_clone = shared_config.clone();
    let sys_path_clone = sys_path_exist.clone();
    let dirty_clone = config_dirty.clone();
    // config_path 下面要被 config_watcher 的闭包吃掉，实验室监听线程留一份
    let config_path_for_rhine = config_path.clone();
    // 只关心生效 meta 文件自身的事件：同目录临时文件（WebUI `*.webui.tmp`、自愈`*.tmp`）必须忽略，否则原子替换完成前提前重载读到旧内容（详见 utils::DirWatcher）
    let config_file_name = config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "meta.yaml".to_string());

    thread::Builder::new()
        .name("config_watcher".to_string())
        .spawn(move || {
            // 监听生效配置的父目录而非固定的 config/ 根：生效配置在处理器子目录（如 config/8550/feature.yaml），inotify 目录监听不递归，
            // 监听根目录收不到子目录内的 CLOSE_WRITE/MOVED_TO，热重载会完全失效
            let watch_dir = config_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(config_dir);
            // inotify 实例跨重载复用（旧实现每轮重建，重载窗口内到达的事件被丢弃）
            let mut watcher: Option<utils::DirWatcher> = None;
            loop {
                if watcher.is_none() {
                    match utils::DirWatcher::new(&watch_dir) {
                        Ok(w) => watcher = Some(w),
                        Err(e) => {
                            log::error!(
                                "{}",
                                t_with_args(
                                    "config-watch-error",
                                    &fluent_args!("error" => e.to_string())
                                )
                            );
                            // 退避后再重试，避免持续错误时忙循环刷 CPU
                            thread::sleep(std::time::Duration::from_secs(2));
                            continue;
                        }
                    }
                }
                let waited = watcher.as_mut().unwrap().wait_change(&config_file_name);
                if let Err(e) = waited {
                    log::error!(
                        "{}",
                        t_with_args(
                            "config-watch-error",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                    // 丢弃异常实例（目录被重建/ fd 异常），下轮重新建立监听
                    watcher = None;
                    thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                log::info!("{}", t("config-reloading"));

                // meta.yaml 自愈先于重载：字段非法用嵌入默认整体覆盖并追加警告注释，文件缺失则重建，再加载（Config::load 读到的一定是合法 meta）；nofix 防篡改开启时跳过，
                // 非法字段由 Config::load 自行回退（不落盘）
                if !common::nofix_active() {
                    common::sync_meta_snapshot(&config_path);
                }

                let old_lang = config_clone.read().unwrap().meta.language.clone();

                match Config::load(config_path.to_str().unwrap()) {
                    Ok(new_config) => {
                        logger::update_level(&new_config.meta.loglevel);
                        // [D1] 配置身份变化 → 递增诊断代际（避免跨配置混写）；须在移入 config_clone 前取身份
                        {
                            // 配置、指纹与代际在同一个配置写锁内发布。
                            let mut configuration = config_clone.write().unwrap();
                            let diag_cfg_changed = diag_config_identity_set(&new_config);
                            *configuration = new_config;
                            if diag_cfg_changed {
                                diag_worker_bump_generation();
                            }
                        }

                        let new_lang = config_clone.read().unwrap().meta.language.clone();
                        if old_lang != new_lang {
                            load_language(&new_lang);
                        }

                        log::info!("{}", t("config-reloaded-success"));

                        // 一次性系统调整的下发要过停摆判定：本线程独立于调度循环、拿不到 `halted`，判定放在唯一入口 apply_system_tweaks（原子标志 + 互斥闸）
                        // 停摆期照常重载配置（日志/语言生效）但不写系统，退出停摆时由调度循环补发
                        let scheduler =
                            CpuScheduler::new(config_clone.clone(), sys_path_clone.clone());
                        if let Err(e) = scheduler.apply_system_tweaks() {
                            log::error!(
                                "{}",
                                t_with_args(
                                    "config-apply-tweaks-failed",
                                    &fluent_args!("error" => e.to_string())
                                )
                            );
                        }

                        // 通知 scheduler_ipc：运行中的 CLG/akmode/亲和/core_ctl 需按新配置重载
                        dirty_clone.store(true, Ordering::Release);
                    }
                    Err(load_err) => log::error!(
                        "{}",
                        t_with_args(
                            "config-reload-fail",
                            &fluent_args!("error" => load_err.to_string())
                        )
                    ),
                }
            }
        })?;

    log::info!("{}", t("main-config-watch-thread-create"));

    // [rhine_watcher]
    // 实验室状态监听：rhine.chr 一被写入就套用/还原，与 meta.yaml 热重载同语义，
    // 不用重启盯的是模块根下的 rhine.chr（与 config_watcher 盯的 meta.yaml 不是
    // 一回事）；套用/还原会改写 meta.yaml，写入方只有本线程，无并发改写
    let rhine_root = root.clone();
    thread::Builder::new()
        .name("rhine_watcher".to_string())
        .spawn(move || {
            crate::rhine::watch_loop(rhine_root, config_path_for_rhine, rhine_initial)
        })?;

    // [ipc_main]
    let config_clone = shared_config.clone();
    let mode_clone = shared_mode_name.clone();
    let dirty_ipc = config_dirty.clone();
    // sysfs 存在性缓存的另一份：停摆退出后由本线程补发一次性系统调整（ config_watcher 那条路径只在配置文件变化时才重放）
    let tweaks_sys_path = sys_path_exist.clone();

    thread::Builder::new()
        .name("scheduler_ipc".to_string())
        .spawn(move || {
            log::info!("{}", t("scheduler-ipc-started"));

            // [ipc_state]
            let root = common::get_module_root();
            // 当前模式持久化文件：每次模式切换时写入，供外部（如 WebUI）读取自愈：每 5s 巡检（见 MODE_FILE_REWRITE_INTERVAL 分支），内容未变跳过重写，
            // 被清空/改写/删除仍在 5s 内恢复，防 WebUI 读不到当前状态
            let mode_file_path = root.join("current_mode.chr");
            // 模式文件写入器：记账上次写入值，5s 自愈块在内容未变时跳过重写（稳态 5 syscall → 1 read，见 ModeFile 注释）
            let mut mode_file = ModeFile::new(mode_file_path);
            const MODE_FILE_REWRITE_INTERVAL: Duration = Duration::from_secs(5);
            let mut last_mode_file_write = Instant::now();
            // 停摆期心跳间隔 60s：停摆中日志完全静止，靠这条状态行证明「停摆仍在生效、进程未死」——间隔再长就区分不出来了
            const DOWN_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
            let mut last_down_heartbeat = Instant::now();
            // 停摆起始时刻（启动即停摆时就是现在）：心跳里带上已持续分钟数
            let mut halt_since: Option<Instant> = None;
            // [halt] 停摆状态机：进入/退出 DOWN 时做一次性的释放与恢复。放在调度循环里而不是监听线程里——这些 governor 对象归本线程独占
            let mut halted = crate::down::is_down();
            // 停摆状态下启动：内存里的当前模式直接是 down，下面写文件、status.csv 的 mode 列都用它（rules 里的真实模式不受影响，退出停摆时恢复）
            if halted {
                *mode_clone.lock().unwrap() = crate::down::DOWN_MODE.to_string();
                // **开机就停摆必须显式打点**：状态机不会走「进入停摆」分支，日志里
                // 没有任何停摆字样，与「调度线程根本没起来」完全同形，必须留痕
                log::warn!("{}", t("down-boot-halted"));
                halt_since = Some(Instant::now());
            }
            // 启动时先写一次初始模式，避免开机后文件缺失/被清空时 WebUI 显示未知状态
            {
                let mode = mode_clone.lock().unwrap().clone();
                mode_file.write(&mode);
            }

            let mut cpu_governor = crate::chiri::cpu_load_governor::CpuLoadGovernor::new();
            // 明日方舟特调（akmode）：独立于 CLG 的 4 档齿轮调度器，前台为白名单应用时接管。传入特调激活共享标志，接管/释放时联动 Monitor 层切换采样间隔
            let ak_governor_flag = ak_active.clone();
            let mut ak_governor =
                crate::chiri::tuned::TunedGovernor::new(ak_governor_flag, fas_signal.clone());

            // 极速模式（fast）专属锁频器：与 CLG 完全独立、不读 yaml 参数，锁所有cluster 的 min=max=硬件最高频，每 5s 重写兜底（节点被改写属异常态）
            let mut fast_lock = crate::chiri::fast::FastLock::new();

            // governor/GPU 接管层（contingency/babel 用；FAS 的 governor 在 FasManager 内部）。GpuGuard::
            // new 启动探测一次 devfreq 节点并缓存结果
            let mut governor_guard = governor::GovernorGuard::new();
            let mut gpu_guard = gpu::GpuGuard::new();

            // FAS 实例管理器：温度源独立探测（与下方 thermal 的 temp_sensor_path 语义不同）温度看电池不看处理器：电池是热安全边界，处理器长期 95℃ 属正常工作区；
            // 无电池节点传 None，FAS 限温关闭不影响其他功能电池温度刻度预识别一次（单位因内核/厂商而异）并全局缓存，CLG 热保护与 FAS 护栏共用同一结论防口径漂移；探测不出打 debug，
            // 读取侧退化为仅 CPU 温度
            match crate::utils::battery_temp_scale() {
                Some(scale) => log::info!(
                    "{}",
                    t_with_args(
                        "battery-temp-scale",
                        &fluent_args!(
                            "unit" => scale.label().to_string(),
                            "divisor" => scale.divisor().to_string()
                        )
                    )
                ),
                None => log::debug!("{}", t("battery-temp-scale-unknown")),
            }
            // FAS 温度源只记节点路径，除数每次刷新取全局预识别结论（勿在此固化）
            let fas_temp_path =
                crate::utils::find_battery_temp_path().map(std::path::PathBuf::from);
            let mut fas_mgr = fas_manager::FasManager::new(fas_temp_path, fas_signal.clone());

            // CPU 亲和与线程迁移控制器 + core_ctl 核心在线接管（ChiRi 专属）
            let mut affinity_mgr = affinity::AffinityManager::new(sys_path_exist.clone());
            let mut corectl_mgr = core_ctl::CoreCtlManager::new();

            // 最近一次 eBPF 扩展探针统计（BpfStats 事件 2s 一次增量），随遥测 CSV 落盘
            let mut last_bpf_stats: (u32, u32, u32) = (0, 0, 0);
            // 遥测 CSV 落盘计时（1s 精度）与 debug 摘要计数（每 20 行 = 20s 一条）
            let mut last_telemetry_log = Instant::now();
            let mut telemetry_log_counter: u32 = 0;
            // 上一次「PowerAVG 取样被跳过」的原因：只在原因变化时打点，避免每秒刷屏
            let mut sample_skip_reason = String::new();

            // 屏幕状态标记
            let mut is_screen_on = true;
            // 初值同步到进程级原子量（缺省也是 true）：CLG 选 PowerBase 后端时只读这个原子量
            crate::common::set_screen_on(is_screen_on);
            // 息屏计时：屏幕熄灭时记录，超过 scene_mode_delay_secs 后切到 scenemode 低功耗
            let mut screen_off_at: Option<Instant> = None;
            // 是否已进入 scenemode（一次性切换，亮屏/模式变更时复位）
            let mut scene_mode_active = false;
            // 特调模式冷却：init_policies 因配置缺失/硬件不支持失败后，5 分钟内不再触发
            const TUNED_COOLDOWN: Duration = Duration::from_secs(300);
            let mut tuned_cooldown_until: Option<Instant> = None;

            // FAS 初始化失败冷却：load_policies 无可用 policy 后 5 分钟内不再触发，CLG default 接管
            const FAS_COOLDOWN: Duration = Duration::from_secs(300);
            let mut fas_cooldown_until: Option<Instant> = None;

            // scenemode 饱和退出冷却：常驻簇（大核簇）util 持续顶满上限退回 reduce 后，300s 内不得重新进入 scenemode（防止与后台负载反复拉锯）
            let mut scenemode_cooldown_until: Option<Instant> = None;
            // scenemode 饱和计时起点（常驻大核簇 max_util 连续超阈值的窗口起点）
            let mut scenemode_sat_since: Option<Instant> = None;

            // scenemode 负载门槛打点去重：持续被拒期间只记一条 scene_hold
            let mut scene_hold_logged = false;

            // 热保护：启动时探测一次温度传感器（CPU + 电池），缺失的参考静默降级；事件循环内每 2s 采样，按 config.thermal 阈值计算压制上限下发给 CLG
            let temp_sensor_path = crate::utils::find_cpu_temp_path().ok();
            if temp_sensor_path.is_none() {
                log::debug!("{}", t("clg-thermal-no-sensor"));
            }
            let batt_sensor_exists = crate::utils::find_battery_temp_path().is_some();
            if !batt_sensor_exists {
                log::debug!("{}", t("clg-thermal-no-battery"));
            }
            let mut last_thermal_check = Instant::now();
            // 当前生效的热保护上限（1.0 = 无压制）；变化时才写 governor，避免高频原子写
            let mut thermal_cap_current: f32 = 1.0;
            // P1-2 旁证采集状态（【临时】，真机验证期用，见 clamp_evidence_snapshot）：上次 clamp_change 快照（变化才落行）+ 缺失节点 warn 去重（每节点首见告警一次，
            // 之后静默记 "-"）
            let mut clamp_prev: Option<String> = None;
            let mut clamp_warned: HashSet<String> = HashSet::new();
            // 当前生效的压制豁免档（与 governor 内原子量同步，配置热重载时下发新值）
            let mut thermal_free_current: f32 = config_clone.read().unwrap().thermal.free_above;
            // 启动即把配置豁免档同步给 governor（内部默认 0.80，配置可能不同）
            cpu_governor.set_thermal_limits(thermal_cap_current, thermal_free_current);
            // tuned 侧的 cap 镜像同样启动即同步一次（1.0 = 无压制，与上方初值一致）
            crate::chiri::tuned::set_thermal_cap(thermal_cap_current);

            let get_clg_cfg = |config: &Config, mode: &str| -> crate::chiri::config::CpuLoadGovernorConfig {
                config.get_mode(mode).map(|m| m.cpu_load_governor.clone()).unwrap_or_else(|| {
                    // 未知/空模式名：不意外启用 CLG，避免用默认参数接管 CPU
                    let mut cfg = crate::chiri::config::CpuLoadGovernorConfig::default();
                    cfg.enabled = false;
                    cfg
                })
            };

            // 启动时初始化
            {
                // 强制上线全部核心：上次运行可能在 scenemode 中途被杀，残留的离线核会让 cpufreq policy 目录消失，
                // 下方 CLG/fast_lock 初始化枚举不到该簇（永久失去 worker），必须先于任何接管**停摆期不做**：它写`cpuN/online` 是最直接的调度干预，停摆期本来就不做初始化
                if !halted {
                    corectl_mgr.force_online_all();
                }
                // 调速器残留清理：SIGKILL 等异常退出会把 performance 留在节点上（收尾 release 不会执行），启动时一律恢复该 policy 的默认调速器（选型见 common.rs [governor]）
                // **停摆期只报不写**——停摆要记录系统原状，进程无从区分残留与厂商默认
                if halted {
                    crate::chiri::governor::GovernorGuard::report_residue();
                } else {
                    crate::chiri::governor::GovernorGuard::cleanup_residue();
                }
                let current_mode = mode_clone.lock().unwrap().clone();
                // 启动接管（极速锁频 / CLG）：**停摆期一律不做**不靠「停摆时内存模式是 down」的脆弱不变量间接跳过——feature.yaml 一旦出现名为down 的模式段，
                // 停摆期开机就会直接锁频/接管 CLG
                if !halted {
                    if is_fast_lock(&current_mode) {
                        fast_lock.init(current_mode == "frozen");
                    } else if current_mode != "fas" {
                        let config_lock = config_clone.read().unwrap();
                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                        if clg_cfg.enabled {
                            cpu_governor.init_policies(&clg_cfg);
                            log::info!("{}", t_with_args("scheduler-clg-init", &fluent_args!("mode" => current_mode.clone())));
                        }
                    }
                }
                // 开发记录开关初始同步：**与停摆无关**（停摆只停调度，采集照常），必须先于下面的 halted 门控，否则停摆启动时诊断记录与配置不一致
                {
                    let cfg = config_clone.read().unwrap();
                    crate::logger::set_diag_active(cfg.meta.dev_record);
                }
                crate::logger::set_diag_mode(&current_mode);
                // 启动即按初始模式应用亲和布局与 core_ctl 在线策略。**停摆启动时跳过**：接管亲和等于停摆期还在写 cpuset；且 halted 初值就是is_down()，循环里「进入停摆」
                // 分支不会执行，没人会把它收回去
                if !halted {
                    let cfg = config_clone.read().unwrap();
                    sync_lab_governor_gpu(&current_mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                    apply_affinity_and_corectl(
                        &mut affinity_mgr,
                        &mut corectl_mgr,
                        &cfg,
                        &current_mode,
                        is_screen_on,
                        crate::monitor::app_detect::get_current_pid(),
                        &[],
                        scene_mode_active,
                    );
                }
            }

            // 事件循环包在 catch_unwind 中：panic 被捕获并记录，不会让调度线程静默挂掉（否则频率会停在最后状态）。最近一次 SystemLoadUpdate 到达时间：
            // 供 CLG 看门狗判定负载源是否失效
            let mut last_load_event = Instant::now();
            // FAS 延迟退出期间记住的目标模式（fas → X 的 ModeChange 被延迟时记录），超时退出完成后按它重新接管（1s tick 巡检消费）
            let mut pending_mode_after_fas: Option<String> = None;
            let window_probe = crate::monitor::window_visibility::WindowVisibilityProbe::new();
            let mut confirmed_window_focus = None;
            // 最近一次 SystemLoadUpdate 的逐核 util 快照（按核亲和选核输入）
            let mut last_core_utils: Vec<f32> = Vec::new();
            // 温度缓存（1s snap 块读取一次，status/devimp/thermal 三处共用）：thermal 2s 块复用 ≤1s 旧值，温度变化秒级，对带回滞判定无影响
            let mut last_batt_temp: Option<f32> = None;
            let mut last_cpu_temp: Option<f32> = None;
            // [D1] 上一帧前台身份三元组（包名 + 前台 pid + 进程 starttime）：**任一变化**即递增
            // 诊断代际，隔离新旧前台的帧任务——同包 PID 更换 / 进程重启（PID 复用，靠 starttime
            // 识别）同样要隔离（worker 启用时防止跨身份拼接；默认关闭时该判定零开销）
            let mut last_diag_identity: Option<DiagIdentity> = None;
            // [D1] 初始化当前配置身份串（worker 提交帧携带；后续由 config_watcher 重载时更新并递增代际）
            diag_config_identity_set(&config_clone.read().unwrap());
            // 温度滤波：MTK soc_max 跳变极大，不滤波会让热保护 cap 周期性 bang-bang 震荡
            let mut batt_filter = TempFilter::new(-10.0, 70.0, 10.0, 1.0);
            let mut cpu_filter = TempFilter::new(5.0, 110.0, 12.0, 3.0);
            // panic 自愈：panic 被捕获后清理到安全态并重新进入事件循环（退避 + 连续
            // 崩溃上限），仅 channel 关闭才正常退出——否则进程活着但调度已死
            // [evt_loop]
            let mut ipc_restart_count: u32 = 0;
            loop {
            let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                // ── DOWN 停摆 ──状态机：进入时释放全部接管并把 current_mode.chr 写成 down，退出时删掉那个文件，让下一次 ModeChange 或 5s 自愈把真实模式写回去
                let down = crate::down::is_down();
                if down != halted {
                    if down {
                        // 先记下进入停摆前的**实际**模式：初值是启动缺省，此后可能已被 app_modes/特调/FAS 改掉，退出要按真实值重建
                        down_resume_mode = mode_clone.lock().unwrap().clone();
                        // FAS 延迟退出的目标模式随之失效（DOWN 退出按 down_resume_mode 接管）
                        pending_mode_after_fas = None;
                        window_probe.invalidate();
                        confirmed_window_focus = None;
                        halt_since = Some(Instant::now());
                        // 顺序：先把频率与布局交回系统（各 release 会恢复自己的快照），再写状态文件——反了会有一瞬间「既停摆又还持有着」
                        cpu_governor.release();
                        ak_governor.release();
                        fast_lock.release();
                        fas_mgr.deactivate_all();
                        affinity_mgr.release();
                        affinity_mgr.lab_static_deactivate();
                        governor_guard.release();
                        gpu_guard.release();
                        corectl_mgr.set_power_state(false, false);
                        // 一次性系统调整也是「调度干预」，一并按快照还原：它们不被任何 governor 持有、release 清单碰不到，不还原则整段停摆期记录到的基线是被改过的系统，而不是系统自身
                        CpuScheduler::restore_system_tweaks();
                        *mode_clone.lock().unwrap() = crate::down::DOWN_MODE.to_string();
                        crate::logger::set_diag_mode(crate::down::DOWN_MODE);
                        mode_file.write(crate::down::DOWN_MODE);
                        log::warn!("{}", t("scheduler-down-enter"));
                    } else {
                        // 恢复目标模式：优先 monitor 的**实时判定**而非停摆前快照——停摆期间前台变化不会补发 ModeChange，
                        // 沿用旧快照会一直空窗（快照仅在 monitor 尚未判定/刚亮屏清空时兜底）
                        halt_since = None;
                        let live_mode = crate::monitor::app_detect::last_determined_mode();
                        let resume_mode = if live_mode.is_empty() {
                            down_resume_mode.clone()
                        } else {
                            live_mode
                        };
                        *mode_clone.lock().unwrap() = resume_mode.clone();
                        crate::logger::set_diag_mode(&resume_mode);
                        mode_file.remove();
                        // 停摆期 governors 全被 release 过，退出时**必须按恢复出的模式重新接管**：ModeChange 只在「模式真的变了」时才重接管，
                        // 前台没换就会一直空着且无异常提示fas/特调的事件不会再来，必须在此直接重建（口径与 panic 自愈后的重建一致）
                        {
                            let mode = resume_mode.clone();
                            let cfg = config_clone.read().unwrap();
                            let pkg = crate::monitor::app_detect::get_current_package();
                            let pid = crate::monitor::app_detect::get_current_pid();
                            if is_fast_lock(&mode) {
                                fast_lock.init(mode == "frozen");
                            } else if mode == "fas" {
                                // 与 ModeChange 的 fas 分支同款重建：三 governor 互斥释放→ activate（内部复查白名单，竞态下前台刚变即拒绝）→亲和 hook；
                                // 失败回退 CLG default 并进入冷却
                                ak_governor.release();
                                fast_lock.release();
                                cpu_governor.release();
                                if fas_package_ready(&pkg)
                                    && pid > 0
                                    && fas_cooldown_until.is_none_or(|until| Instant::now() >= until)
                                    && crate::monitor::app_detect::raw_foreground_package_arc().as_ref() == pkg
                                    && fas_mgr.activate(&pkg, pid)
                                {
                                    fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                    fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                } else {
                                    fas_mgr.deactivate_all();
                                    fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => pkg.clone()))
                                    );
                                    let clg_cfg = get_clg_cfg(&cfg, "default");
                                    if clg_cfg.enabled {
                                        cpu_governor.init_policies(&clg_cfg);
                                    }
                                }
                            } else if crate::common::is_special_mode(&mode) {
                                // 特调：按实时包名复查白名单（竞态下前台刚变则拒绝接管，与事件路径同语义），接管失败进冷却
                                if crate::common::is_special_mode_allowed(&pkg, &mode)
                                    && !ak_governor.init_policies(&mode, &cfg.get_tuned_profile(&mode))
                                {
                                    tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-tuned-cooldown", &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string()))
                                    );
                                }
                            } else {
                                let clg_cfg = get_clg_cfg(&cfg, &mode);
                                if clg_cfg.enabled {
                                    cpu_governor.init_policies(&clg_cfg);
                                }
                            }
                            // lab 模式的 governor/GPU 同步（退出停摆若回到 contingency/babel）
                            sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            apply_affinity_and_corectl(
                                &mut affinity_mgr,
                                &mut corectl_mgr,
                                &cfg,
                                &mode,
                                is_screen_on,
                                pid,
                                &last_core_utils,
                                scene_mode_active,
                            );
                        }
                        // 停摆期进 DOWN 时还原过的一次性系统调整在此补发（下发会重新记快照用于下次还原）
                        if let Err(e) =
                            CpuScheduler::new(config_clone.clone(), tweaks_sys_path.clone())
                                .apply_system_tweaks()
                        {
                            log::error!(
                                "{}",
                                t_with_args(
                                    "config-apply-tweaks-failed",
                                    &fluent_args!("error" => e.to_string())
                                )
                            );
                        }
                        // 停摆期负载事件被丢弃、last_load_event 停在进入前，不重置会被 stale 看门狗释放刚重建的接管
                        last_load_event = Instant::now();
                        log::info!("{}", t("scheduler-down-exit"));
                    }
                    halted = down;
                }

                // 当前模式文件自愈：每 5s 巡检一次，内容未变不重写（外部清空/改写/删除最多 5s 内自愈，比对不等即重写，见 ModeFile::heal）；
                // 停摆期只推进计时不落盘——覆盖 down 等于退出停摆
                if last_mode_file_write.elapsed() >= MODE_FILE_REWRITE_INTERVAL {
                    last_mode_file_write = Instant::now();
                    if !halted {
                        let mode = mode_clone.lock().unwrap().clone();
                        mode_file.heal(&mode);
                    }
                    // watchdog.pid 自愈：logs/ 被删后随目录消失，不重建则旧看门狗杀不掉、与重启实例双写日志
                    crate::logger::ensure_watchdog_pid_file();
                }

                // 停摆期心跳（DOWN_HEARTBEAT_INTERVAL=60s 一条）：停摆中调度动作全停、周期块大多被门控跳过，日志长时间静默——没有它分不清「停摆生效中」与「线程已死」
                // 采集（status.csv/devimp/心跳文件）由各自周期块与独立线程维持，与本心跳无关
                if halted && last_down_heartbeat.elapsed() >= DOWN_HEARTBEAT_INTERVAL {
                    last_down_heartbeat = Instant::now();
                    let mins = halt_since
                        .map(|t| t.elapsed().as_secs() / 60)
                        .unwrap_or(0);
                    log::info!(
                        "{}",
                        t_with_args("down-heartbeat", &fluent_args!("mins" => mins.to_string()))
                    );
                }

                // feature/meta 热重载联动：与 ConfigReload（rules.yaml）同口径——亮屏时按当前模式把新配置应用到运行中的 CLG/akmode 并刷新亲和/core_ctl；
                // 停摆期照重载（日志开关等仍生效）只是不下发
                let config_dirty = dirty_ipc.swap(false, Ordering::AcqRel);
                if config_dirty {
                    // 诊断采集开关与停摆无关（停摆只停调度）：无条件同步，改 meta.dev_record 即时生效
                    let cfg = config_clone.read().unwrap();
                    crate::logger::set_diag_active(cfg.meta.dev_record);
                }
                if config_dirty && !halted && is_screen_on {
                    let current_mode = mode_clone.lock().unwrap().clone();
                    let config_lock = config_clone.read().unwrap();
                    log::debug!(
                        "{}",
                        t_with_args(
                            "scheduler-config-dirty-reload",
                            &fluent_args!("mode" => current_mode.clone())
                        )
                    );
                    if crate::common::is_special_mode(&current_mode) {
                        // 特调运行中：按新配置重载 akmode
                        if ak_governor.is_active() {
                            let ak_cfg = config_lock.get_tuned_profile(&current_mode);
                            ak_governor.reload_config(&ak_cfg);
                        }
                    } else if is_fast_lock(&current_mode) {
                        // 硬锁档（vector/frozen）不读 yaml 调参，仅需确保 CLG 未意外持有
                        if cpu_governor.is_active() { cpu_governor.release(); }
                    } else if current_mode != "fas" {
                        // fas 模式下 CLG fallback 不参与热重载（FAS 配置编译期嵌入静态）
                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                        if clg_cfg.enabled {
                            // 热重载也走 CLG 自己的入口：后端要不要换（Worker↔PowerBase）由它在新配置下
                            // 自行判定，这里不再替它判断「谁在接管」
                            if cpu_governor.is_active() {
                                cpu_governor.reload_config(&clg_cfg);
                            } else {
                                cpu_governor.init_policies(&clg_cfg);
                            }
                        } else if cpu_governor.is_active() {
                            cpu_governor.release();
                        }
                    }
                    drop(config_lock);
                    // 亲和布局/core_ctl 可能随新配置开关变化，刷新一次
                    let cfg = config_clone.read().unwrap();
                    crate::logger::set_diag_active(cfg.meta.dev_record);
                    crate::logger::set_diag_mode(&current_mode);
                    apply_affinity_and_corectl(
                        &mut affinity_mgr,
                        &mut corectl_mgr,
                        &cfg,
                        &current_mode,
                        is_screen_on,
                        crate::monitor::app_detect::get_current_pid(),
                        &last_core_utils,
                        scene_mode_active,
                    );
                }

                // 亲和周期刷新（2s）：同模式前台切换不产生 ModeChange 事件，用最新前台 PID 兜底重迁移（内部去重）；同时同步开发记录开关（meta.dev_record 热重载生效）
                if !halted && last_thermal_check.elapsed() >= THERMAL_CHECK_INTERVAL {
                    let cfg = config_clone.read().unwrap();
                    let current_mode = mode_clone.lock().unwrap().clone();
                    crate::logger::set_diag_active(cfg.meta.dev_record);
                    apply_affinity_and_corectl(
                        &mut affinity_mgr,
                        &mut corectl_mgr,
                        &cfg,
                        &current_mode,
                        is_screen_on,
                        crate::monitor::app_detect::get_current_pid(),
                        &last_core_utils,
                        scene_mode_active,
                    );
                }

                // 状态日志 snapshot 行（1s）：整合遥测 + 热保护 + 模式状态到 logs/status.csvtelemetry 线程 1s 刷新共享原子量；
                // OPlus 功耗优先走 bcc_parms 私有节点（标准 power_supply 节点约 10s 才刷新，1s 采样必须绕开）
                if last_telemetry_log.elapsed() >= TELEMETRY_LOG_INTERVAL {
                    last_telemetry_log = Instant::now();
                    let tm = crate::monitor::telemetry::telemetry();
                    let fmt_opt = |v: Option<f32>, digits: usize| {
                        v.map(|x| format!("{:.*}", digits, x))
                            .unwrap_or_else(|| "-".to_string())
                    };
                    // status.csv 列全精度：文件保留全部小数位，取整交给显示层（WebUI 统一 toFixed(1)）；fmt_opt 的固定位数留给 devimp 与调试打点
                    let fmt_opt_full = |v: Option<f32>| {
                        v.map(|x| x.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    };
                    // 电池三元组本秒只读一次（同一快照），状态行 / PowerAVG / 通知 / main_snap / 调试摘要共用
                    let (batt_voltage, batt_current, batt_power) = tm.batt_sample();
                    let current_mode = mode_clone.lock().unwrap().clone();
                    // 温度本秒读一次，status/devimp/thermal 三处共用；滤波值参与热判定，原始值不再直供
                    last_batt_temp = if batt_sensor_exists {
                        crate::utils::read_battery_temp_celsius().and_then(|t| batt_filter.push(t))
                    } else {
                        None
                    };
                    last_cpu_temp = temp_sensor_path
                        .as_ref()
                        .and_then(|p| crate::utils::read_f64_from_file(p).ok())
                        .map(|v| (v / 1000.0) as f32)
                        .and_then(|t| cpu_filter.push(t));
                    // 前台包名实时取自 app_detect（含同模式切换；用事件维护会写过期包名）
                    let fg_package = crate::monitor::app_detect::get_current_package();
                    // [D1] 前台身份三元组（包名 + pid + 进程 starttime）**任一**变化 → 递增诊断
                    // 代际（仅在 worker 启用时生效，否则立即返回）：使旧前台的帧任务与新前台的帧任务
                    // 分属不同 generation，不跨身份拼接；pid 与 starttime 一并纳入——同包 PID 更换 /
                    // 进程重启（PID 复用）也需隔离。starttime 要读 /proc，仅在 D1 启用时读（默认零开销）
                    //
                    // 【顺序不变式（消除「新 PID 配旧 generation」的关键）】本段与下方采样快照处于
                    // **同一 tick**，且本段**先于**快照：先「身份变化 → bump 代际」，后「构造 SampledFrame
                    // （读 generation）」。快照的 fg_pid/fg_starttime **复用本段同一次取值**（不再重读），
                    // 故「身份判定所用 PID」与「快照携带 PID/代际」同源——本段一旦检测到切换，本 tick 的
                    // bump 必已被下方快照取到；不会出现「快照读到新 PID、代际仍属旧前台」。
                    let (fg_pid_now, fg_starttime_now): (i32, u64) = if DIAG_WORKER_ENABLED {
                        let pid = crate::monitor::app_detect::get_current_pid();
                        (pid, proc_starttime_ticks(pid))
                    } else {
                        (0, 0)
                    };
                    let now_identity: DiagIdentity =
                        (fg_package.clone(), fg_pid_now, fg_starttime_now);
                    if diag_identity_changed(&last_diag_identity, &now_identity) {
                        if last_diag_identity.is_some() {
                            diag_worker_bump_generation();
                        }
                        last_diag_identity = Some(now_identity);
                    }
                    // 诊断日志按前台包名分组：变化即切换 main_ 文件（内部去重，空包名不切换）
                    crate::logger::set_diag_package(&fg_package);
                    // 充放电状态：1s 一次读 status 节点（电流符号厂商方向不一，不可靠）
                    let charge_state = read_battery_charge_state();
                    // fps 列：仅 FAS 激活时取活跃实例窗口均值，其余为 None → 写 "-"
                    let fas_fps = fas_mgr.current_fps();
                    // screen_prop 列：debug.tracing.screen_state 原始值，1s 轮询读取（缺失为 "-"）
                    let screen_prop_raw =
                        crate::monitor::screen_detect::read_screen_prop_raw()
                            .unwrap_or_else(|| "-".to_string());
                    // 自测量基线：daemon 自身累计 CPU 时间（1s 采样读 /proc/self/stat），与状态行同行落盘
                    let (daemon_utime_ms, daemon_stime_ms) = crate::logger::self_cpu_ms();
                    // [selfcost] 会话开闭结算（每秒一次，与诊断是否开启无关）：诊断 假→真起新会话/启 worker、
                    // 真→假输出不足 60s 尾窗并关闭 selfcost/worker（幂等）
                    aff_cost_session_tick(crate::logger::diag_active());
                    let cpu_snapshot = crate::logger::diag_active()
                        .then(crate::chiri::cpu_freq_snapshot);
                    // 功耗分解：CPU 动态项估计（无功耗表的 SoC 恒 None）+ 残差（外围 / 静态 / GPU）
                    let cpu_dyn_w = if let Some((cpu_cur, _, _, _)) = cpu_snapshot.as_ref() {
                        let policy_frequencies: Vec<(u32, Option<u32>)> = cpu_cur
                            .split(';')
                            .filter_map(|entry| {
                                let (policy, frequency) = entry.split_once(':')?;
                                let policy_id = policy.strip_prefix("policy")?.parse().ok()?;
                                Some((policy_id, frequency.parse().ok()))
                            })
                            .collect();
                        crate::chiri::energy_cost::cpu_dynamic_power_w_with_frequencies(
                            &last_core_utils,
                            &policy_frequencies,
                        )
                    } else {
                        crate::chiri::energy_cost::cpu_dynamic_power_w(&last_core_utils)
                    };
                    let resid_w = match (batt_power, cpu_dyn_w) {
                        (Some(p), Some(c)) => Some(p - c),
                        _ => None,
                    };
                    crate::logger::status_log_snapshot(
                        &current_mode,
                        &fg_package,
                        &charge_state,
                        is_screen_on,
                        last_batt_temp,
                        last_cpu_temp,
                        &format!("{}", thermal_cap_current * 100.0),
                        &format!("{}", thermal_free_current * 100.0),
                        cpu_governor.is_active(),
                        &fmt_opt_full(Some(tm.psi_cpu_some())),
                        &fmt_opt_full(Some(tm.psi_io_some())),
                        &fmt_opt_full(Some(tm.psi_mem_some())),
                        &fmt_opt_full(tm.gpu_busy()),
                        &fmt_opt_full(batt_voltage),
                        // 全精度：电流可能只有 0.5 级小数量级，取整会抹平真值、归档无法反推
                        &fmt_opt_full(batt_current),
                        &fmt_opt_full(batt_power),
                        last_bpf_stats.0,
                        last_bpf_stats.1,
                        last_bpf_stats.2,
                        fas_fps,
                        &screen_prop_raw,
                        daemon_utime_ms,
                        daemon_stime_ms,
                        cpu_dyn_w,
                        resid_w,
                    );
                    // PowerAVG（耗电参考/平均）：紧跟 status 行**顺序**写入 PowerAVG.chr取样口径：
                    // ① 仅电池放电（充电功率不属于耗电均值）；② 平均模式再排除息屏（息屏占时长大，
                    // 等权全史会拉低均值）；③ 参考值保留息屏（滑动参考要反映整段放电）
                    // 判定式 `!use_average || is_screen_on`：平均 = 放电 && 亮屏，参考 = 放电（含息屏）
                    // 不满足时传 None → 递推整体跳过；开关 meta.power_avg 热重载即时生效
                    let (use_average, notify_on) = {
                        let cfg = config_clone.read().unwrap();
                        (cfg.meta.power_avg, cfg.meta.notify)
                    };
                    let sample_ok =
                        charge_state == "discharging" && (!use_average || is_screen_on);
                    let power_reading = if sample_ok { batt_power } else { None };
                    // 跳过时 PowerAVG.chr 不更新：跳过原因同一原因只报一次，写入样本后重新武装
                    let skip_reason = if !sample_ok {
                        if charge_state != "discharging" {
                            Some(format!("not-discharging({charge_state})"))
                        } else {
                            Some("screen-on".to_string())
                        }
                    } else if power_reading.is_none() {
                        Some("no-reading".to_string())
                    } else {
                        None
                    };
                    match skip_reason {
                        Some(reason) => {
                            if sample_skip_reason != reason {
                                log::info!(
                                    "{}",
                                    crate::i18n::t_with_args(
                                        "power-avg-skip",
                                        &crate::fluent_args!("reason" => reason.clone())
                                    )
                                );
                                sample_skip_reason = reason;
                            }
                        }
                        None => sample_skip_reason.clear(),
                    }
                    let power_now = crate::logger::power_avg_update(power_reading, use_average);
                    // 首轮标记必须在自增**之前**取——自增后判断恒为 false，首轮清残留通知不会执行
                    let first_tick = telemetry_log_counter == 0;
                    telemetry_log_counter += 1;
                    // 常驻状态通知：每 5s 更新（内容不变不重投，内部自带失败候选与告警去重）标题 = 前台包名，正文 = 模式/家族/子模式/温度/功耗（口径随 meta.power_avg）
                    // 这里只组装并**非阻塞投递**到 notify 线程，调度循环照常跑
                    if !notify_on {
                        // 开关关闭：撤销已投递通知；首轮无条件清掉上次运行残留的那条（此后幂等）
                        crate::notify::cancel(first_tick);
                    } else if telemetry_log_counter % crate::notify::INTERVAL_SECS == 0 {
                        crate::notify::update(&crate::notify::Snapshot {
                            pkg: &fg_package,
                            mode: &current_mode,
                            batt_temp: last_batt_temp,
                            cpu_temp: last_cpu_temp,
                            power_w: power_now,
                        });
                    }
                    // 开发记录 snap 行（1s）：开启 dev_record 才有 IO；前台包名行内自动填充
                    if let Some((cpu_cur, cpu_max, cpu_min, cpu_gov)) = cpu_snapshot {
                        // 频率/调速器快照只在诊断开启时采集（常态零开销，不缓存——内核会随时改值）CPU 每秒采；GPU 受 GPU_SNAPSHOT_ENABLED 总开关（当前关闭）控制，
                        // 开启时再叠加 gpu_snapshot_due() 的 10s 节流；关闭期间这 4 列恒为 "-"
                        let (gpu_cur, gpu_max, gpu_min, gpu_gov) = if GPU_SNAPSHOT_ENABLED
                            && gpu_snapshot_due()
                        {
                            crate::chiri::gpu::devfreq_snapshot()
                        } else {
                            ("-".to_string(), "-".to_string(), "-".to_string(), "-".to_string())
                        };
                        crate::logger::main_snap(
                            is_screen_on,
                            &fmt_opt(last_batt_temp, 1),
                            &fmt_opt(last_cpu_temp, 1),
                            &format!("{:.0}", thermal_cap_current * 100.0),
                            cpu_governor.is_active(),
                            &fmt_opt(Some(tm.psi_cpu_some()), 2),
                            &fmt_opt(Some(tm.psi_io_some()), 2),
                            &fmt_opt(Some(tm.psi_mem_some()), 2),
                            &fmt_opt(tm.gpu_busy(), 0),
                            &fmt_opt(batt_voltage, 3),
                            &fmt_opt(batt_current, 0),
                            &fmt_opt(batt_power, 2),
                            last_bpf_stats.0,
                            last_bpf_stats.1,
                            last_bpf_stats.2,
                            &cpu_cur,
                            &cpu_max,
                            &cpu_min,
                            &cpu_gov,
                            &gpu_cur,
                            &gpu_max,
                            &gpu_min,
                            &gpu_gov,
                        );
                        // @S 进程/线程快照（aff_* 线程流文件）：top-N 进程 + 前台树/被管进程
                        // 线程下钻同 main_snap 的 diag_active 门控（关闭零采样零写入）；开启时为
                        // getaffinity/stat 读取，冷长尾按 30 帧一次降采样（帧语义见 build_aff_snapshot）；
                        // 第二返回值为刷新帧标记，交由 logger 在帧头打 `full=1`。
                        // 采样间隔受 meta `devimp_aff_secs` 节流（息屏再乘倍率）：跳过的轮次不推进帧号，
                        // 且被管线程表只在真正要采时才取（省一轮快照拷贝）
                        // 读锁保留到身份冻结完成，热重载不能拆开配置与代际。
                        let snapshot_configuration = config_clone.read().unwrap();
                        let snap_top_n = snapshot_configuration.meta.devimp_top_n;
                        let snap_aff_secs = snapshot_configuration.meta.devimp_aff_secs;
                        let aff_every =
                            snap_aff_secs * if is_screen_on { 1 } else { AFF_SNAP_OFFSCREEN_FACTOR };
                        if aff_snap_due(aff_every) {
                            // 被管线程表/进程集一次取完传入（build_aff_snapshot 已与 AffinityManager 解耦，见其 doc）
                            let th_diag = affinity_mgr.thread_diag();
                            let managed = affinity_mgr.managed_pids();
                            // [B1/B3] @S 同步成本测量：端点严格按 before_build→after_build→after_write，
                            // Build=(t0,t1)/(c0,c1)、Write=(t1,t2)/(c1,c2)，两段 CPU 区间不重叠；
                            // 每次调用都入账，慢只节流告警。本块已由 diag_active ∩ aff_snap_due 门控，
                            // 故只在真正采样时计时。写入经 aff_snapshot_dispatch（默认同步，D1 启用时交 worker）
                            // [D1] 仅 worker 启用时才在**采样时点**（t0 之前）一次性冻结一致快照
                            // （const false 时整段被优化掉，默认路径零开销）；同步路径的帧头 ts 仍在
                            // **写出时刻**取（恢复原语义，字节不变）。快照的 fg_pid/fg_starttime 复用
                            // 上方身份判定的同一次取值；generation/sequence 在此从 worker 一次取出并
                            // 冻结——**晚于**身份判定（本 tick 检测到切换必已 bump），故不会「新 PID 配旧
                            // generation」；config_identity 与 generation 同段读取（见 diag_sample_frame），
                            // 故不会「旧配置指纹配新 generation」。dispatch 直接取快照里的 generation/
                            // sequence，不再在交付时重读。序号在采样时点分配：与采样顺序一致；被丢弃的帧
                            // 会留下序号空洞（可接受，离线按 generation+sequence 判序）。
                            let sampled = if DIAG_WORKER_ENABLED {
                                // 在同一段内克隆句柄并释放 DIAG_WORKER 锁（勿在别处再取一次锁）
                                let worker = {
                                    let g = DIAG_WORKER
                                        .get_or_init(|| Mutex::new(None))
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    g.as_ref().map(Arc::clone)
                                };
                                worker.map(|w| {
                                    diag_sample_frame(
                                        crate::logger::aff_frame_ts(),
                                        fg_pid_now,
                                        fg_starttime_now,
                                        &w,
                                    )
                                })
                            } else {
                                None
                            };
                            drop(snapshot_configuration);
                            let t0 = Instant::now();
                            let c0 = diag_cost::thread_cpu_ns();
                            // 采样时点前台 pid：D1 时复用上方身份判定同一次取值（保证「身份/代际/帧」
                            // 同源）；默认路径懒取一次，零额外开销。
                            let fg_pid_for_build = if DIAG_WORKER_ENABLED {
                                fg_pid_now
                            } else {
                                crate::monitor::app_detect::get_current_pid()
                            };
                            let (rows, full) = build_aff_snapshot(
                                &th_diag,
                                &managed,
                                fg_pid_for_build,
                                &fg_package,
                                snap_top_n,
                            );
                            let t1 = Instant::now();
                            let c1 = diag_cost::thread_cpu_ns();
                            aff_snapshot_dispatch(rows, full, sampled);
                            let t2 = Instant::now();
                            let c2 = diag_cost::thread_cpu_ns();
                            aff_cost_record(t0, c0, t1, c1, t2, c2);
                        }
                    }
                    if telemetry_log_counter % 20 == 0 && log::log_enabled!(log::Level::Debug) {
                        log::debug!(
                            "{}",
                            t_with_args(
                                "telemetry-summary",
                                &fluent_args!(
                                    "cpu" => format!("{:.1}", tm.psi_cpu_some()),
                                    "io" => format!("{:.1}", tm.psi_io_some()),
                                    "mem" => format!("{:.1}", tm.psi_mem_some()),
                                    "gpu" => fmt_opt(tm.gpu_busy(), 0),
                                    "wakeups" => last_bpf_stats.0.to_string(),
                                    "migrations" => last_bpf_stats.1.to_string(),
                                    "freq" => last_bpf_stats.2.to_string(),
                                    "power" => fmt_opt(batt_power, 2)
                                )
                            )
                        );
                    }

                    // FAS 延迟退出巡检（1s）：到期完成退出并按记住的目标模式重新接管；延迟期内切回白名单应用由 activate 无缝续期，这里 no-op
                    // 停摆期不可达（DOWN 的 deactivate_all 已清 deadline）——改这段别破坏该不变量，否则停摆会被 FAS 重新接管
                    if !halted {
                        if !crate::common::fas_enabled() {
                            window_probe.invalidate();
                            confirmed_window_focus = None;
                            // fas_enabled=false：实例已在别处注销；模式必须真正回到 default，
                            // 否则 mode=fas / active=false 会让热保护与自愈同时留洞。
                            fas_mgr.deactivate_all();
                            fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);
                            *mode_clone.lock().unwrap() = "default".to_string();
                            crate::logger::set_diag_mode("default");
                            mode_file.write("default");
                            if !cpu_governor.is_active()
                                && !ak_governor.is_active()
                                && !fast_lock.is_active()
                            {
                                let state = config_clone.read().unwrap();
                                let clg_configuration = get_clg_cfg(&state, "default");
                                if clg_configuration.enabled {
                                    cpu_governor.init_policies(&clg_configuration);
                                }
                            }
                            pending_mode_after_fas = None;
                        } else {
                            reconcile_fas_focus(
                                &mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                &mut pending_mode_after_fas, is_screen_on,
                            );
                        }
                    }
                    if !halted && crate::common::fas_enabled() && fas_mgr.tick() {
                        window_probe.invalidate();
                        confirmed_window_focus = None;
                        if let Some(mode) = pending_mode_after_fas.take() {
                            *mode_clone.lock().unwrap() = mode.clone();
                            crate::logger::set_diag_mode(&mode);
                            mode_file.write(&mode);
                            // governor/GPU：目标若是 contingency/babel 则接管，否则恢复
                            sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);
                            if mode == "contingency" || mode == "babel" {
                                // 进入 lab 静态模式：先释放全部其它调频接管——CLG/特调的 tick/防篡改会持续压频、release 会恢复接管前快照，
                                // 全部释放后再 sync 才是终值
                                cpu_governor.release();
                                ak_governor.release();
                                fast_lock.release();
                                governor_guard.release();
                                gpu_guard.release();
                                sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                                // lab 静态分组与屏幕状态无关：分组重建不门控亮屏
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &mode,
                                    is_screen_on,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            } else if is_screen_on {
                                apply_mode_takeover(
                                    &mode,
                                    &config_clone.read().unwrap(),
                                    &mut cpu_governor,
                                    &mut ak_governor,
                                    &mut fast_lock,
                                );
                            } else {
                                // 息屏：FAS 退出后交 CLG doze（同款低功耗配置），避免延迟期结束到亮屏间零接管
                                let cfg = config_clone.read().unwrap();
                                let mut doze_cfg = get_clg_cfg(&cfg, "reduce");
                                doze_cfg.enabled = true;
                                doze_cfg.perf_floor = 0.0;
                                doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30);
                                doze_cfg.smoothing_up = 0.10;
                                doze_cfg.touch_boost_enabled = false;
                                cpu_governor.init_policies(&doze_cfg);
                            }
                            log::info!("{}", t_with_args(
                                "scheduler-fas-delayed-exit",
                                &fluent_args!("mode" => mode.as_str())
                            ));
                        }
                    }

                    // PackageSwitch 兜底（事件丢失/启动即 fas 自愈）：1s 巡检一次，包名实际变化才触发热切换，天然幂等；借用检查限制与 arm 处各自内联
                    // 全时段生效（FAS 息屏保持接管）；app_detect 息屏不更新包名，稳态 no-op
                    if !halted
                        && mode_clone.lock().unwrap().as_str() == "fas"
                        && !crate::common::fas_enabled()
                    {
                        // fas_enabled=false 且内存模式仍是 fas：上面的 1s 前哨已把模式写回
                        // default 并交回 CLG；这里只兜住 mode=fas / active=false 的虚报态。
                        fas_mgr.deactivate_all();
                        fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);
                        *mode_clone.lock().unwrap() = "default".to_string();
                        crate::logger::set_diag_mode("default");
                        mode_file.write("default");
                        log::warn!("{}", t("scheduler-fas-switch-off"));
                        if !cpu_governor.is_active()
                            && !ak_governor.is_active()
                            && !fast_lock.is_active()
                        {
                            let config_lock = config_clone.read().unwrap();
                            let clg_cfg = get_clg_cfg(&config_lock, "default");
                            if clg_cfg.enabled {
                                cpu_governor.init_policies(&clg_cfg);
                            }
                        }
                    } else if !halted
                        && crate::monitor::app_detect::last_determined_mode() == "fas"
                        && crate::common::fas_enabled()
                    {
                        // 一致快照 + owner 进程判据；只有新会话才受 cooldown 限制。
                        let snapshot = fas_foreground_snapshot();
                        let raw_pkg = fas_raw_foreground_package();
                        let cur_pkg = snapshot
                            .as_ref()
                            .filter(|snapshot| snapshot.is_valid())
                            .map(|snapshot| snapshot.package.as_ref());
                        let new_session_ready = fas_cooldown_until
                            .is_none_or(|until| Instant::now() >= until);
                        if let Some(cur_pkg) = cur_pkg.filter(|pkg| {
                            raw_pkg.as_ref() == *pkg && fas_package_ready(pkg)
                        }) {
                            let Some(snapshot) = snapshot.as_ref() else { continue; };
                            let owner_matches = fas_mgr
                                .owner_matches_process(snapshot.pid, snapshot.process_starttime);
                            if fas_mgr.is_active() {
                                if let Some(active) = fas_mgr.active_pkg().map(str::to_string) {
                                    // 同包（延迟期内切回同一白名单应用）：续期取消退出
                                    if active == cur_pkg && owner_matches {
                                        fas_mgr.renew_if_same_pkg(&cur_pkg);
                                        fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                        pending_mode_after_fas = None;
                                    }
                                    if active != cur_pkg || !owner_matches {
                                        if crate::common::fas_whitelist_entry(&cur_pkg)
                                            .and_then(|cfg| crate::common::fas_app_config(cfg))
                                            .is_some()
                                        {
                                            fas_mgr.deactivate_active();
                                            window_probe.invalidate();
                                            confirmed_window_focus = None;
                                            if !fas_mgr.activate(&cur_pkg, snapshot.pid) {
                                                fas_mgr.deactivate_all();
                                                fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                                log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => cur_pkg)));
                                                // CLG default 回退
                                                let config_lock = config_clone.read().unwrap();
                                                let clg_cfg = get_clg_cfg(&config_lock, "default");
                                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                            } else {
                                                window_probe.invalidate();
                                                confirmed_window_focus = None;
                                                pending_mode_after_fas = None;
                                                fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                                *mode_clone.lock().unwrap() = "fas".to_string();
                                                crate::logger::set_diag_mode("fas");
                                                mode_file.write("fas");
                                                fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true,
                                                    snapshot.pid);
                                            }
                                        }
                                    // 非白名单包（determine 门控保证不应出现）：忽略，ModeChange 会退出 fas
                                    }
                                }
                                    } else if new_session_ready {
                                if crate::common::fas_whitelist_entry(&cur_pkg)
                                    .and_then(|cfg| crate::common::fas_app_config(cfg))
                                    .is_some()
                                {
                                    // FAS 激活优先于 scenemode：scenemode 期间 little 除引导核全部离线 + 专用大核独占，
                                    // 不允许 FAS 在残缺拓扑上接管——先恢复全部在线核再激活（与亮屏恢复同序）守卫维持「FAS 实例存在 ⇒ 非 scenemode」不变量
                                    if scene_mode_active {
                                        scene_mode_active = false;
                                        scene_hold_logged = false;
                                        log::info!("{}", t("scheduler-scene-mode-exit-fas"));
                                        {
                                            let cfg = config_clone.read().unwrap();
                                            let cur_mode = mode_clone.lock().unwrap().clone();
                                            apply_affinity_and_corectl(
                                                &mut affinity_mgr,
                                                &mut corectl_mgr,
                                                &cfg,
                                                &cur_mode,
                                                is_screen_on,
                                                crate::monitor::app_detect::get_current_pid(),
                                                &last_core_utils,
                                                false,
                                            );
                                        }
                                        crate::logger::main_event("scene_exit", "-", "fas_activate_preempt");
                                    }
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    affinity_mgr.lab_static_deactivate();
                                    governor_guard.release();
                                    gpu_guard.release();
                                    window_probe.invalidate();
                                    confirmed_window_focus = None;
                                    if !fas_mgr.activate(&cur_pkg, snapshot.pid) {
                                        fas_mgr.deactivate_all();
                                        fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                        log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => cur_pkg)));
                                        // CLG default 回退
                                        let config_lock = config_clone.read().unwrap();
                                        let clg_cfg = get_clg_cfg(&config_lock, "default");
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    } else {
                                        window_probe.invalidate();
                                        confirmed_window_focus = None;
                                        pending_mode_after_fas = None;
                                        fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                        *mode_clone.lock().unwrap() = "fas".to_string();
                                        crate::logger::set_diag_mode("fas");
                                        mode_file.write("fas");
                                        fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true,
                                            snapshot.pid);
                                    }
                                } else if !cpu_governor.is_active() && !ak_governor.is_active() && !fast_lock.is_active() {
                                    // 启动残留 fas 模式且前台非 FAS 应用：CLG default 自愈接管
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                }
                            }
                        }
                    }
                }

                // 热保护（2s 周期）：电池+CPU 双源取较小值，CPU 只在极端热时参与（阈值 75/85°C，内核 95°C 温控才是主力）；当前性能比已高于豁免档时不压制，高负载不挡路
                if last_thermal_check.elapsed() >= THERMAL_CHECK_INTERVAL {
                    last_thermal_check = Instant::now();
                    // 旁证节点采集（diag 开启时每 2s）：锁频被压归因（FREQ_QOS 钳制 / 硬件热压LMh·DCVS / 厂商并发写），快照未变则零产出；置于 halted/FAS 门控之外
                    if crate::logger::diag_active() {
                        let snap = clamp_evidence_snapshot(&mut clamp_warned);
                        if clamp_prev.as_deref() != Some(snap.as_str()) {
                            crate::logger::main_event("clamp_change", "-", &snap);
                            clamp_prev = Some(snap);
                        }
                    }
                    // FAS 活跃时整体跳过 ChiRi 热保护：温控交由 FAS 引擎（内部独立刷新温度），cap 冻结在当前值fas 模式但实例未活跃（息屏已释放/冷却/初始化失败）时照常生效，
                    // 否则 CLG doze 期间将失去热保护
                    if !halted
                        && !(mode_clone.lock().unwrap().as_str() == "fas" && fas_mgr.is_active())
                    {
                        let (enabled, batt_ladder, free_above) = {
                            let t = &config_clone.read().unwrap().thermal;
                            let (sc, mc, hc) =
                                (t.soft_perf_cap, t.mid_perf_cap, t.hard_perf_cap);
                            (
                                t.enabled,
                                ThermalLadder {
                                    soft: t.batt_soft_temp_c,
                                    mid: t.batt_mid_temp_c,
                                    hard: t.batt_hard_temp_c,
                                    soft_cap: sc,
                                    mid_cap: mc,
                                    hard_cap: hc,
                                    hyst: t.hysteresis_c,
                                    hyst_mid: t.hysteresis_mid_c,
                                    hyst_hard: t.hysteresis_hard_c,
                                },
                                t.free_above,
                            )
                        };
                        let cur = thermal_cap_current;
                        // 复用 snap 块（1s）缓存的温度，≤1s 旧值对带回滞的秒级热判定无影响
                        let batt_t = last_batt_temp;
                        // CPU 温度只进事件行的展示字段，不参与下面的压制判定
                        let cpu_t = last_cpu_temp;
                        // 限幅判定的唯一依据就是电池温度阶梯；CPU 温度只走 snap / status.csv / 事件文本，不参与压制
                        let new_cap = if !enabled {
                            1.0
                        } else {
                            match batt_t {
                                Some(b) => eval_thermal_cap(b, cur, &batt_ladder),
                                // 电池温度缺失/读失败：保持现状，下轮重试
                                None => cur,
                            }
                        };
                        // 解除方向限速：压制加深立即生效，解除每周期最多 +0.15（UNPRESS_STEP）逐级恢复——立即全量解除会温度反弹再触发深压，cap 振荡是高负载周期性卡顿的直接来源
                        let new_cap = if new_cap > thermal_cap_current {
                            (thermal_cap_current + THERMAL_UNPRESS_STEP).min(new_cap)
                        } else {
                            new_cap
                        };
                        let cap_changed = (new_cap - thermal_cap_current).abs() > f32::EPSILON;
                        let free_changed = (free_above - thermal_free_current).abs() > f32::EPSILON;
                        if cap_changed || free_changed {
                            thermal_cap_current = new_cap;
                            thermal_free_current = free_above;
                            cpu_governor.set_thermal_limits(new_cap, free_above);
                            // 同一份 cap 也发 tuned（playback/akmode 不走 CLG，见 tuned.rs [thermal_ceil]）：进程级原子量，与 CLG 侧同口径、
                            // 值相同，热重载经 Config::load 同步开关与下限
                            crate::chiri::tuned::set_thermal_cap(new_cap);
                            let fmt = |v: Option<f32>| {
                                v.map(|t| format!("{:.1}", t))
                                    .unwrap_or_else(|| "-".to_string())
                            };
                            log::debug!(
                                "{}",
                                t_with_args(
                                    "clg-thermal-cap",
                                    &fluent_args!(
                                        "batt" => fmt(batt_t),
                                        "cpu" => fmt(cpu_t),
                                        "cap" => format!("{:.0}", new_cap * 100.0),
                                        "free" => format!("{:.0}", free_above * 100.0)
                                    )
                                )
                            );
                            crate::logger::main_event(
                                "thermal_change",
                                "-",
                                &format!(
                                    "batt={} cpu={} cap={:.0} free={:.0}",
                                    fmt(batt_t),
                                    fmt(cpu_t),
                                    new_cap * 100.0,
                                    free_above * 100.0
                                ),
                            );
                        }
                        // 功耗监控已并入 status.csv snapshot 行（1s），此处不再重复写
                    }
                }

                // 触摸事件（事件驱动）：on_touch 更新共享触摸状态并唤醒全部 Worker 立即 flush 写频（大核直接应用触摸升频地板，不等 160ms tick）；
                // 持续触摸刷新截止时间（FastWriter 去重）。PowerBase 接管时走同一条链路（档位沿阶梯瞬时升档），
                // 分流在 CLG 内部完成
                while touch_rx.try_recv().is_ok() {
                    if !halted && cpu_governor.is_active() {
                        cpu_governor.on_touch();
                        log::debug!("{}", t("touch-event-received"));
                    }
                }

                // 极速模式：每 5s 重写一次硬件最高频兜底收敛（默认无竞争者，节点被改写属异常态）；返回距下次重写的剩余时间纳入动态超时计算
                // 停摆期显式断开不碰任何节点——tick 内部虽有 is_active 早退，防将来给 FastLock 加的开关绕过这道防线
                let fast_next = if halted { None } else { fast_lock.tick() };


                // 动态超时：阻塞到最近周期任务 deadline 或事件到达（先到者打断）周期任务（telemetry 1s /thermal+亲和 2s / mode file 5s / fast 重写
                // 5s）取最小值；负载/模式/触摸等推送事件随时打断、响应零延迟；空闲稳态从每秒 10 次空转降为 ~1 次
                let now_loop = Instant::now();
                let until = |last: Instant, period: Duration| -> Duration {
                    period.checked_sub(now_loop.duration_since(last)).unwrap_or(Duration::ZERO)
                };
                let mut wait = until(last_telemetry_log, TELEMETRY_LOG_INTERVAL)
                    .min(until(last_thermal_check, THERMAL_CHECK_INTERVAL))
                    .min(until(
                        last_mode_file_write,
                        MODE_FILE_REWRITE_INTERVAL,
                    ))
                    .min(EVENT_POLL_MS);
                if let Some(d) = fast_next {
                    wait = wait.min(d);
                }
                let msg = match rx.recv_timeout(wait) {
                    Ok(msg) => msg,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // eBPF 负载源超时自愈：超过 CLG_STALE_MAX 无负载事件 → 释放当前接管（CLG/akmode/fast_lock）
                        // 回系统原生调频防锁频（下次 ModeChange/配置事件会重新接管）
                        if last_load_event.elapsed() >= CLG_STALE_MAX {
                            if ak_governor.is_active() {
                                log::error!(
                                    "{}",
                                    t_with_args(
                                        "tuned-watchdog-release",
                                        &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                    )
                                );
                                ak_governor.release();
                            }
                            if cpu_governor.is_active() {
                                log::error!(
                                    "{}",
                                    t_with_args(
                                        "clg-watchdog-release",
                                        &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                    )
                                );
                                cpu_governor.release();
                            }
                            if fast_lock.is_active() {
                                log::error!(
                                    "{}",
                                    t_with_args(
                                        "fast-watchdog-release",
                                        &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                    )
                                );
                                fast_lock.release();
                            }
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                // 停摆期间事件照收但不下发：调度类动作全停，采集与遥测由上面的周期块负责
                // **屏幕事件例外**：只更新 is_screen_on 等本地状态（不做调度动作）——停摆期不吸收
                // 变化的话，退出停摆时 monitor 不会补发，亲和/core_ctl 会按过期屏幕状态应用
                if halted {
                    if let DaemonEvent::ScreenStateChange(on) = &msg {
                        is_screen_on = *on;
                        crate::common::set_screen_on(is_screen_on);
                        screen_off_at = if *on { None } else { Some(Instant::now()) };
                        scene_mode_active = false;
                    }
                    continue;
                }
                match msg {
                    // [evt_screen]
                    // 屏幕状态事件：息屏深度睡眠
                    DaemonEvent::ScreenStateChange(screen_on) => {
                        // 双源事件去重：uevent 直推与 app_detect 兜底可能重复上报，状态未变只打点
                        if screen_on == is_screen_on {
                            log::debug!("{}", t_with_args("scheduler-event-screen", &fluent_args!(
                                "on" => screen_on.to_string(),
                                "last" => is_screen_on.to_string()
                            )));
                            continue;
                        }
                        log::debug!("{}", t_with_args("scheduler-event-screen", &fluent_args!(
                            "on" => screen_on.to_string(),
                            "last" => is_screen_on.to_string()
                        )));
                        is_screen_on = screen_on;
                        if !screen_on {
                            fas_mgr.set_frame_feedback_enabled(false);
                            window_probe.invalidate();
                            confirmed_window_focus = None;
                        } else if fas_mgr.is_active() {
                            reconcile_fas_focus(&mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                &mut pending_mode_after_fas, true);
                        }
                        // 同步到进程级原子量：CLG 在接管那一刻要靠它判断能不能选 PowerBase 后端
                        // （它没有别的渠道拿到屏幕状态），必须与本地变量同点更新
                        crate::common::set_screen_on(is_screen_on);
                        // 息屏/亮屏 info 打点：uevent 直推绕过 app_detect 的变更日志，统一保证触发点可见
                        if screen_on {
                            log::info!("{}", t("scheduler-screen-on"));
                        } else {
                            log::info!("{}", t("scheduler-screen-off"));
                        }
                        let current_mode = mode_clone.lock().unwrap().clone();

                        if !is_screen_on {
                            log::info!("{}", t("scheduler-doze-enable"));
                            // 息屏计时起点：超过 scene_mode_delay_secs 后切到 scenemode 低功耗
                            screen_off_at = Some(Instant::now());
                            scene_mode_active = false;

                            // 特调息屏保持 akmode 接管不切 CLG doze：akmode 已统一默认调速器，息屏随负载自然降频，
                            // 避免 release + 亮屏 re-init 的 governor 反复切换
                            if crate::common::is_special_mode(&current_mode) {
                                // akmode 继续运行，CLG 保持释放状态
                                log::info!("{}", t("scheduler-doze-special-keep"));
                            } else if current_mode == "fas" && fas_mgr.is_active() {
                                // FAS 息屏保持接管：不释放实例、不切 CLG doze（与特调同语义）；FAS 失效（看门狗释放/初始化冷却）时走下方分支由 doze 接管
                            } else {
                                // 非特调模式：交回 CLG 处理深度睡眠本分支仅在非 fas 模式或 FAS 未活跃（看门狗释放/冷却）时执行；不可写低频锁——锁屏无帧事件时将永久锁死最低频
                                ak_governor.release();
                                fast_lock.release();

                                // 让 CLG 接管，动态生成一个低功耗配置
                                let config_lock = config_clone.read().unwrap();
                                let mut doze_cfg = get_clg_cfg(&config_lock, "reduce");
                                doze_cfg.enabled = true;
                                doze_cfg.perf_floor = 0.0;
                            // 息屏 doze 天花板 0.30：后台任务（sync/JobScheduler）突发时允许短暂借力，但压住"口袋发热"；5 分钟后 scenemode 进一步压到 0.
                            // 15 smoothing_up=0.10 升频极迟钝；touch_boost 关闭（息屏无触摸）
                                doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30);
                                doze_cfg.smoothing_up = 0.10;
                                doze_cfg.touch_boost_enabled = false;

                                cpu_governor.init_policies(&doze_cfg);
                            }
                            // 亲和/core_ctl 跟随息屏：top-app 恢复快照、后台压小核（特调保持 boost 布局）
                            {
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &current_mode,
                                    false,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            }
                        } else {
                            log::info!("{}", t("scheduler-doze-restore"));
                            // 亮屏：清空息屏计时与 scenemode 状态（恢复逻辑在下方重放原模式）
                            screen_off_at = None;
                            scene_mode_active = false;
                            scene_hold_logged = false;
                            let config_lock = config_clone.read().unwrap();

                            // 亲和/core_ctl 先恢复：此前处于 scenemode 时 prime 仍离线、其 policy 目录不存在——必须先恢复全部核上线，
                            // 下方 reload/init 才能枚举到完整 policy 列表（否则 prime 永久失去 worker，残留离线前的锁频、脱离调度控制）
                            apply_affinity_and_corectl(
                                &mut affinity_mgr,
                                &mut corectl_mgr,
                                &config_lock,
                                &current_mode,
                                true,
                                crate::monitor::app_detect::get_current_pid(),
                                &last_core_utils,
                                scene_mode_active,
                            );

                            if crate::common::is_special_mode(&current_mode) {
                                // 亮屏恢复特调：息屏时若看门狗释放过 akmode 必须重新接管，否则特调限频失效、采样间隔不切回 40ms；冷却期内跳过特调，直接走 CLG
                                let in_cooldown = tuned_cooldown_until
                                    .map_or(false, |until| Instant::now() < until);
                                if !ak_governor.is_active() && !in_cooldown {
                                    cpu_governor.release();
                                    let ak_cfg = config_lock.get_tuned_profile(&current_mode);
                                    if !ak_governor.init_policies(&current_mode, &ak_cfg) {
                                        // init 失败（配置缺失/硬件不支持）：冷却 5 分钟，CLG 接管
                                        tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                        log::warn!("{}", t_with_args(
                                            "scheduler-tuned-cooldown",
                                            &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string())
                                        ));
                                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                } else if !ak_governor.is_active() {
                                    // 冷却中：CLG 接管
                                    let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                }
                            } else if current_mode == "contingency" || current_mode == "babel" {
                                // 亮屏恢复 lab 静态模式：governor/GPU/极速锁频按模式重建；不能走下面通用else——它会释放 fast_lock 且不给 contingency
                                // 接管（CLG 未注册），锁频就空了
                                cpu_governor.release();
                                ak_governor.release();
                                fast_lock.release();
                                sync_lab_governor_gpu(&current_mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            } else if is_fast_lock(&current_mode) {
                                // 亮屏恢复硬锁档：释放 doze CLG 由 fast_lock 接管（vector 锁最高/frozen 锁最低）
                                ak_governor.release();
                                cpu_governor.release();
                                fast_lock.init(current_mode == "frozen");
                            } else if current_mode == "fas" {
                                // FAS 活跃：保持接管，亮屏不做任何恢复（CLG 绝不能接管 FAS 已接管的 CPU）FAS 失效（冷却/异常）：
                                // 激活 CLG default——冷却期内 1s 兜底被短路，此处是退出 doze/scenemode 低性能配置的唯一恢复路径，冷却结束由 1s 兜底重新激活
                                if !fas_mgr.is_active() {
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled {
                                        if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                        else { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                    else { cpu_governor.release(); }
                                }
                            } else {
                                ak_governor.release();
                                fast_lock.release();
                                let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                if clg_cfg.enabled {
                                    // 息屏 doze 期间 CLG 仍持有 writer，热切换配置即可
                                    if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                    else { cpu_governor.init_policies(&clg_cfg); }
                                }
                                else { cpu_governor.release(); }
                            }
                        }
                        crate::logger::main_event("screen", "-", if is_screen_on { "on" } else { "off" });
                    },

                    // [evt_mode]
                    // 前台模式切换事件
                    DaemonEvent::ModeChange { package_name, pid, mode, temperature, detection_generation } => {
                        // 事件必须与当前一致快照的 generation/package/PID 全同，旧代事件不接受。
                        let snapshot = fas_foreground_snapshot();
                        let event_matches = snapshot.as_ref().is_some_and(|snapshot| {
                            fas_focus::event_matches_snapshot(
                                &package_name,
                                pid,
                                detection_generation,
                                &snapshot.package,
                                snapshot.pid,
                                snapshot.generation,
                                manager_process_starttime(pid),
                                snapshot.process_starttime,
                            )
                        });
                        if !event_matches {
                            continue;
                        }
                        let Some(snapshot) = snapshot else { continue; };
                        let raw_package = fas_raw_foreground_package();
                        let fas_ready = fas_package_ready(&package_name)
                            && raw_package.as_ref() == package_name;
                        // FAS 同包返回与换游戏必须先于模式去重；已持有实例不受 cooldown 限制。
                        if mode == "fas" && fas_ready && fas_mgr.is_active() {
                            let same_session = fas_mgr
                                .owner_matches_process(pid, snapshot.process_starttime);
                            if same_session {
                                fas_mgr.renew_if_same_pkg(&package_name);
                            } else {
                                if fas_cooldown_until.is_some_and(|until| Instant::now() < until) {
                                    continue;
                                }
                                fas_mgr.deactivate_active();
                                window_probe.invalidate();
                                confirmed_window_focus = None;
                                if !fas_mgr.activate(&package_name, pid) {
                                    fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                    pending_mode_after_fas = None;
                                    fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);
                                    *mode_clone.lock().unwrap() = "default".to_string();
                                    crate::logger::set_diag_mode("default");
                                    mode_file.write("default");
                                    let configuration = config_clone.read().unwrap();
                                    let clg_configuration = get_clg_cfg(&configuration, "default");
                                    if clg_configuration.enabled {
                                        cpu_governor.init_policies(&clg_configuration);
                                    }
                                    continue;
                                } else {
                                    fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                }
                            }
                            if fas_mgr.is_active() {
                                fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                pending_mode_after_fas = None;
                                window_probe.invalidate();
                                confirmed_window_focus = None;
                            }
                        }
                        if mode == "fas" && !fas_ready {
                            reconcile_fas_focus(&mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                &mut pending_mode_after_fas, is_screen_on);
                            continue;
                        }
                        let mut current_mode_lock = mode_clone.lock().unwrap();
                        let old_mode = current_mode_lock.clone();
                        log::debug!("{}", t_with_args("scheduler-event-mode-change", &fluent_args!(
                            "pkg" => package_name.as_str(),
                            "old" => old_mode.clone(),
                            "new" => mode.as_str(),
                            "temp" => temperature
                        )));
                        // 前台切换由 app_detect 直写 status.csv fg 行；ModeChange 仅模式变化时产生

                        if old_mode != mode || (mode == "fas" && !fas_mgr.is_active()) {
                            log::info!("{}", t_with_args("scheduler-mode-change-request", &fluent_args!(
                                "old" => old_mode.clone(), "new" => mode.as_str(), "pkg" => package_name.as_str(), "temp" => temperature
                            )));

                            // FAS 延迟退出（15s）：fas → 非 fas 不立即切换——FAS 仍持有接管，延迟期内切回白名单应用由 activate 无缝续期，
                            // 到期由 1s tick 按 pending_mode_after_fas 重新接管mode_clone 不动，否则出现「文件写 default、FAS 还持着频率」
                            // 中间态
                            if old_mode == "fas" && mode != "fas" && fas_mgr.is_active() {
                                fas_mgr.set_frame_feedback_enabled(false);
                                fas_mgr.request_delayed_exit();
                                pending_mode_after_fas = Some(mode.clone());
                                reconcile_fas_focus(&mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                    &mut pending_mode_after_fas, is_screen_on);
                                drop(current_mode_lock);
                                continue;
                            }

                            *current_mode_lock = mode.clone();
                            drop(current_mode_lock);

                            crate::logger::set_diag_mode(&mode);
                            crate::logger::main_event(
                                "mode_change",
                                &package_name,
                                &format!("{old_mode}->{mode}"),
                            );

                            mode_file.write(&mode);

                            // 亲和布局/core_ctl 跟随模式切换（含前台 PID 变化的线程重迁移）
                            {
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &mode,
                                    is_screen_on,
                                    pid,
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            }

                            // 特调模式激活打点：仅 ChiRi 白名单应用可进入，info 级便于用户定位
                            if crate::common::is_special_mode(&mode) {
                                log::info!("{}", t_with_args("scheduler-special-mode-active", &fluent_args!(
                                    "pkg" => package_name.as_str(),
                                    "mode" => mode.as_str()
                                )));
                            }

                            // 屏幕状态不再影响 FAS 激活（息屏特殊分支已移除）
                            if mode == "fas" {
                                // 防御复查：determine_mode 已门控白名单，此处兜底（不应发生）
                                let fas_ready = fas_package_ready(&package_name);
                                let in_cooldown = fas_cooldown_until.map_or(false, |until| Instant::now() < until);
                                // 防御性去激活（不应有活跃实例，无活跃时为无操作）
                                fas_mgr.deactivate_active();
                                window_probe.invalidate();
                                confirmed_window_focus = None;
                                if !fas_ready {
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => package_name.as_str()))
                                    );
                                    // 可能从特调/极速模式切入：先释放独立 governor 再交给 CLG，避免双 governor 并存
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                } else if in_cooldown {
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-fas-cooldown", &fluent_args!("secs" => FAS_COOLDOWN.as_secs().to_string()))
                                    );
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                } else {
                                    // 三 governor 全停（互斥），再激活 FAS
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    if !fas_mgr.activate(&package_name, pid) {
                                        fas_mgr.deactivate_all();
                                        fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                        log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => package_name.as_str())));
                                        let config_lock = config_clone.read().unwrap();
                                        let clg_cfg = get_clg_cfg(&config_lock, "default");
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    } else {
                                        pending_mode_after_fas = None;
                                        window_probe.invalidate();
                                        confirmed_window_focus = None;
                                        fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                        fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                    }
                                }
                            } else {
                                // 退出 FAS：去激活（恢复频率）必须先于任何下一 governor 的 init，保证对方快照真实系统状态
                                fas_mgr.deactivate_active();
                                fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);

                                // 离开 lab 静态模式：先恢复组级 cpus 与 governor/GPU 快照，后续接管方才能快照到真实状态，否则 lab 分组值被当成「系统原值」
                                // 固化（top-app 永久占大核）
                                if !matches!(mode.as_str(), "contingency" | "babel") {
                                    affinity_mgr.lab_static_deactivate();
                                    governor_guard.release();
                                    gpu_guard.release();
                                    // contingency 的极速锁频也要解除：息屏不走亮屏接管块，漏了锁频残留到亮屏
                                    fast_lock.release();
                                }

                                // 仅在亮屏时处理调度接管。如果息屏，Doze 配置仍在生效，这里不能覆盖它
                                if is_screen_on {
                                    let config_lock = config_clone.read().unwrap();
                                    if crate::common::is_special_mode(&mode) {
                                    // 进入特调模式：停止 CLG 改由 akmode 接管；冷却期内跳过特调，直接走 CLG
                                        let in_cooldown = tuned_cooldown_until
                                            .map_or(false, |until| Instant::now() < until);
                                        if !in_cooldown {
                                            cpu_governor.release();
                                            let ak_cfg = config_lock.get_tuned_profile(&mode);
                                            if !ak_governor.init_policies(&mode, &ak_cfg) {
                                                // init 失败：冷却 5 分钟，CLG 接管
                                                tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                                log::warn!("{}", t_with_args(
                                                    "scheduler-tuned-cooldown",
                                                    &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string())
                                                ));
                                                let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                            }
                                        } else {
                                            // 冷却中：CLG 接管
                                            let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                            if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                        }
                                    } else if is_fast_lock(&mode) {
                                        // 硬锁档：停止特调/CLG 由 fast_lock 独立锁定（vector 锁满频/frozen 锁最低）
                                        ak_governor.release();
                                        cpu_governor.release();
                                        fast_lock.init(mode == "frozen");
                                    } else {
                                        // 退出特调/极速模式：停止 akmode/fast_lock 交回 CLG（后端由它自己选）
                                        ak_governor.release();
                                        fast_lock.release();
                                        let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                        if clg_cfg.enabled {
                                            // CLG 已激活时热切换配置，避免同模式反复切换全量重建
                                            if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                            else { cpu_governor.init_policies(&clg_cfg); }
                                        } else {
                                            cpu_governor.release();
                                        }
                                    }
                                }

                                // 进入 lab 静态模式：governor 写 performance / GPU 锁最高频 / contingency 极速锁频（sync 内做）
                                // 必须在旧 governor 释放之后——先写会被 CLG release 恢复的快照调速器覆盖，且 CLG tick/防篡改会持续压频；
                                // 息屏路径在此兜底（幂等）
                                if mode == "contingency" || mode == "babel" {
                                    cpu_governor.release();
                                    ak_governor.release();
                                    fast_lock.release();
                                }
                                sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            }
                        }
                    },

                    // [evt_pkg_switch]
                    // 同模式前台包切换（fas→fas 热切换通路，ChiRi 专属）
                    DaemonEvent::PackageSwitch { package_name, pid, detection_generation } => {
                        let snapshot = fas_foreground_snapshot();
                        let event_matches = snapshot.as_ref().is_some_and(|snapshot| {
                            fas_focus::event_matches_snapshot(
                                &package_name,
                                pid,
                                detection_generation,
                                &snapshot.package,
                                snapshot.pid,
                                snapshot.generation,
                                manager_process_starttime(pid),
                                snapshot.process_starttime,
                            )
                        });
                        let raw_package = fas_raw_foreground_package();
                        if !event_matches
                            || raw_package.as_ref() != package_name
                            || !fas_package_ready(&package_name)
                        {
                            reconcile_fas_focus(&mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                &mut pending_mode_after_fas, is_screen_on);
                            continue;
                        }
                        let Some(snapshot) = snapshot else { continue; };
                        let current_mode = mode_clone.lock().unwrap().clone();
                        // 同包去重前先续期：延迟期内同包回前台即时取消退出（1s 巡检兜底）
                        if fas_mgr.owner_matches_process(pid, snapshot.process_starttime) {
                            fas_mgr.renew_if_same_pkg(&package_name);
                            fas_mgr.set_frame_feedback_enabled(is_screen_on);
                            pending_mode_after_fas = None;
                            window_probe.invalidate();
                            confirmed_window_focus = None;
                        }
                        if current_mode == "fas" && fas_mgr.is_active() {
                            let switched = match fas_mgr.active_pkg() {
                                Some(active) if active != package_name.as_str()
                                    || !fas_mgr.owner_matches_process(pid, snapshot.process_starttime) => {
                                    if fas_package_ready(&package_name) {
                                        fas_mgr.deactivate_active();
                                        window_probe.invalidate();
                                        confirmed_window_focus = None;
                                        if fas_mgr.activate(&package_name, pid) {
                                            pending_mode_after_fas = None;
                                            window_probe.invalidate();
                                            confirmed_window_focus = None;
                                            fas_mgr.set_frame_feedback_enabled(is_screen_on);
                                            fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                            true
                                        } else {
                                            false
                                        }
                                    } else {
                                        // 非白名单包（不应出现，determine 门控保证）：视为已处理，忽略
                                        true
                                    }
                                }
                                // 同包去重：延迟期内同包回前台也续期（即时取消，1s 巡检兜底）
                                _ => {
                                    fas_mgr.renew_if_same_pkg(&package_name);
                                    true
                                }
                            };
                            if !switched {
                                fas_mgr.deactivate_all();
                                fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => package_name.as_str())));
                                let config_lock = config_clone.read().unwrap();
                                let clg_cfg = get_clg_cfg(&config_lock, "default");
                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                            }
                        }
                    },

                    // [evt_load]
                    // CPU 负载事件 (eBPF 驱动)
                    DaemonEvent::SystemLoadUpdate { core_utils, foreground_max_util } => {
                        if crate::common::fas_enabled() && fas_mgr.is_active() {
                            reconcile_fas_focus(&mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                &mut pending_mode_after_fas, is_screen_on);
                        }
                        // 刷新看门狗心跳：只要有负载事件到达即视为负载源存活
                        last_load_event = Instant::now();
                        // 逐核 util 快照赋值延后到下方投喂之后：投喂分支只读借用 core_utils，延后可用
                        // **移动**替代克隆，省掉每 tick 一次 Vec 分配事件常规 160ms/特调 40ms 一次，
                        // 摘要仅 DEBUG 输出（log_enabled! 门控，INFO 下零分配）
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("{}", t_with_args("scheduler-event-load", &fluent_args!(
                                "cores" => core_utils.iter().map(|u| format!("{:.0}", u * 100.0)).collect::<Vec<_>>().join(",")
                            )));
                        }
                        // 负载投喂优先级链：FAS 活跃优先投喂（前台最重线程 util + 逐核 util）；否则特调（akmode）白名单前台投喂做动态限频（无档位负载直拉：
                        // max 随组内负载在[最低档, 硬件最高] 间连续变化）；否则 CLG 活跃时投喂 CLG（含息屏 Doze）
                        if fas_mgr.is_active() {
                            // 事件未带帧源身份：快照前台不是 owner 时不得把 overlay 前台线程
                            // util 当游戏负载投喂，最小安全取 0（簇 util 仍照常投喂）。
                            let owner_is_foreground = fas_foreground_snapshot().is_some_and(|snapshot| {
                                fas_mgr.active_pkg() == Some(snapshot.package.as_ref())
                            });
                            let fas_foreground_util = if owner_is_foreground {
                                foreground_max_util
                            } else {
                                0.0
                            };
                            fas_mgr.on_load_update(fas_foreground_util, &core_utils);
                        } else if ak_governor.is_active() {
                            ak_governor.on_load_update(&core_utils);
                        } else if cpu_governor.is_active() {
                            // CLG 角色优先级链终点：内部再分流到 Worker 或 PowerBase 后端
                            cpu_governor.on_load_update(&core_utils);
                        }

                        // 逐核 util 快照：选核打分与 devimp tick 行的输入；移动而非克隆（投喂只读借用）
                        last_core_utils = core_utils;

                        // scenemode：息屏超过 scene_mode_delay_secs 后把 CLG 切到低功耗配置（一次性），亮屏后恢复原模式任意 FAS 实例存在（活跃或后台保留，
                        // 60s TTL reap 后放行）即禁止进入——CLG 绝不能接管 FAS 已接管的 CPU；特调由 akmode 接管不参与；scenemode 未启用时释放 CLG；
                        // 饱和退出冷却期内不得重进（防与后台负载拉锯）
                        let scenemode_cooldown_ok = scenemode_cooldown_until
                            .map_or(true, |u| Instant::now() >= u);
                        // scenemode_enabled=false 热重载生效：退出 scenemode，恢复快照并交回 CLG doze
                        if scene_mode_active && !crate::common::scenemode_enabled() {
                            scene_mode_active = false;
                            scene_hold_logged = false;
                            {
                                let cfg = config_clone.read().unwrap();
                                let cur_mode = mode_clone.lock().unwrap().clone();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &cur_mode,
                                    is_screen_on,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    false,
                                );
                            }
                            let config_lock = config_clone.read().unwrap();
                            let mut doze_cfg = get_clg_cfg(&config_lock, "reduce");
                            doze_cfg.enabled = true;
                            doze_cfg.perf_floor = 0.0;
                            doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30);
                            doze_cfg.smoothing_up = 0.10;
                            doze_cfg.touch_boost_enabled = false;
                            if cpu_governor.is_active() {
                                cpu_governor.reload_config(&doze_cfg);
                            } else {
                                cpu_governor.init_policies(&doze_cfg);
                            }
                            log::info!("{}", t("scheduler-scene-mode-exit-switch"));
                            crate::logger::main_event("scene_exit", "-", "switch_off");
                        }
                        if !is_screen_on
                            && crate::common::scenemode_enabled()
                            && !scene_mode_active
                            && scenemode_cooldown_ok
                        {
                            // 先用免锁的计时预判（最低 60s），避免息屏期间每个负载 tick 都抢锁
                            let delay_hit = screen_off_at
                                .map_or(false, |off| off.elapsed().as_secs() >= 60);
                            if delay_hit {
                                let current_mode = mode_clone.lock().unwrap().clone();
                                // 特调不参与；任意 FAS 实例存在（活跃或后台保留）不参与，reap 后放行
                                if !crate::common::is_special_mode(&current_mode)
                                    && !fas_mgr.has_any_instance()
                                {
                                    let config_read = config_clone.read().unwrap();
                                    let delay = config_read.scene_mode_delay_secs.max(60);
                                    let scene_cfg =
                                        config_read.scenemode.cpu_load_governor.clone();
                                    drop(config_read);
                                    if screen_off_at
                                        .map_or(false, |off| off.elapsed().as_secs() >= delay)
                                    {
                                        // 负载门槛：与饱和退出共用 SCENEMODE_SAT_UTIL——常驻簇仍顶满时进入必然~10s 后饱和退出，整夜「进→退→300s 冷却」拉锯；
                                        // 跳过本次，后续 tick 复评
                                        let ranges = crate::common::chiri_core_ranges();
                                        let standby_max = ranges
                                            .little
                                            .clone()
                                            .chain(ranges.big.clone())
                                            .filter_map(|c| last_core_utils.get(c).copied())
                                            .fold(0.0_f32, f32::max);
                                        // 长息屏兜底：息屏时长 ≥4× 进入延迟时不再受负载门槛——后台常驻负载会把util 长期顶在门槛上方，
                                        // 整夜待机会被永久挡在 scenemode 之外；短息屏仍防抖
                                        let long_off = screen_off_at.map_or(false, |off| {
                                            off.elapsed().as_secs() >= delay.saturating_mul(4)
                                        });
                                        if standby_max >= SCENEMODE_SAT_UTIL && !long_off {
                                            if !scene_hold_logged {
                                                scene_hold_logged = true;
                                                crate::logger::main_event(
                                                    "scene_hold",
                                                    "-",
                                                    &format!("util={:.0}", standby_max * 100.0),
                                                );
                                            }
                                        } else {
                                            scene_hold_logged = false;
                                            if scene_cfg.enabled {
                                                if cpu_governor.is_active() {
                                                    cpu_governor.reload_config(&scene_cfg);
                                                } else {
                                                    cpu_governor.init_policies(&scene_cfg);
                                                }
                                            } else {
                                                cpu_governor.release();
                                            }
                                            log::info!("{}", t("scheduler-scene-mode-enter"));
                                            scene_mode_active = true;
                                            // 立即应用 scenemode 核心压制（不等 2s 周期块）：下线 little 簇（除引导核）
                                            // + core_ctl `max_cpus` 钳制保留核数（`scenemode_keep_cpus()`）+ 专用大核 cpuset 独占
                                            {
                                                let cfg = config_clone.read().unwrap();
                                                apply_affinity_and_corectl(
                                                    &mut affinity_mgr,
                                                    &mut corectl_mgr,
                                                    &cfg,
                                                    &current_mode,
                                                    is_screen_on,
                                                    crate::monitor::app_detect::get_current_pid(),
                                                    &last_core_utils,
                                                    scene_mode_active,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // scenemode 饱和退出：**常驻核（little 引导核 + 全部 big）**里任一核 max_util 持续顶满
                        // （≥SCENEMODE_SAT_UTIL 且持续 SAT_SECS=10s）→ 退回 reduce（恢复全部在线核）+ 300s 冷却不得重进，防拉锯
                        // util 是忙时占比（与频率无关），饱和即真饱和，与 perf_ceil 数值无耦合。
                        // 2026-09-28 起 scenemode 下线 little 簇（只留引导核），故判据实际主要落在 big 上——
                        // 这条保护不会因「换了一批常驻核」而失效，对象变了而已
                        if scene_mode_active && !is_screen_on {
                            let ranges = crate::common::chiri_core_ranges();
                            let standby_max = ranges
                                .little
                                .clone()
                                .chain(ranges.big.clone())
                                .filter_map(|c| last_core_utils.get(c).copied())
                                .fold(0.0_f32, f32::max);
                            if standby_max >= SCENEMODE_SAT_UTIL {
                                let sustained = match scenemode_sat_since {
                                    Some(since) => since.elapsed() >= SCENEMODE_SAT_SECS,
                                    None => {
                                        scenemode_sat_since = Some(Instant::now());
                                        false
                                    }
                                };
                                if sustained {
                                    scenemode_sat_since = None;
                                    scene_mode_active = false;
                                    scene_hold_logged = false;
                                    scenemode_cooldown_until =
                                        Some(Instant::now() + SCENEMODE_COOLDOWN);
                                    let current_mode = mode_clone.lock().unwrap().clone();
                                    // 立即恢复全部在线核 + 解除专用核钉定，必须先于 CLG reload：worker 建起来时该簇在线核集应已完整
                                    //（下线 little 期间因引导核在场、policy0 不会消失；但反序会让 worker 少管核，故顺序不变）
                                    {
                                        let cfg = config_clone.read().unwrap();
                                        apply_affinity_and_corectl(
                                            &mut affinity_mgr,
                                            &mut corectl_mgr,
                                            &cfg,
                                            &current_mode,
                                            is_screen_on,
                                            crate::monitor::app_detect::get_current_pid(),
                                            &last_core_utils,
                                            scene_mode_active,
                                        );
                                    }
                                    // 退回 reduce：核已全部上线、枚举 policy 完整；reduce 未启用时释放（同 ModeChange）
                                    let ps_cfg = get_clg_cfg(&config_clone.read().unwrap(), "reduce");
                                    if ps_cfg.enabled {
                                        if cpu_governor.is_active() {
                                            cpu_governor.reload_config(&ps_cfg);
                                        } else {
                                            cpu_governor.init_policies(&ps_cfg);
                                        }
                                    } else {
                                        cpu_governor.release();
                                    }
                                    log::info!(
                                        "{}",
                                        t_with_args(
                                            "scheduler-scene-mode-saturation",
                                            &fluent_args!("util" => format!("{:.0}", standby_max * 100.0))
                                        )
                                    );
                                    crate::logger::main_event(
                                        "scene_exit",
                                        "-",
                                        "saturation->reduce+300s",
                                    );
                                }
                            } else {
                                scenemode_sat_since = None;
                            }
                        }
                    },

                    // [evt_frame]
                    // 帧率事件 (eBPF 驱动)
                    DaemonEvent::FrameUpdate { frame_delta_ns, source_pid, source_generation } => {
                        // 管理器复查 PID/generation；息屏与失焦会话只响应负载。
                        if crate::common::fas_enabled() && fas_mgr.is_active() {
                            let foreground = fas_foreground_snapshot();
                            let raw_foreground = fas_raw_foreground_package();
                            if !is_screen_on
                                || fas_mgr.active_pkg() != foreground.as_ref().map(|snapshot| &*snapshot.package)
                                || fas_mgr.active_pkg() != Some(raw_foreground.as_ref())
                            {
                                reconcile_fas_focus(&mut fas_mgr, &window_probe, &mut confirmed_window_focus,
                                    &mut pending_mode_after_fas, is_screen_on);
                            }
                            fas_mgr.on_frame(frame_delta_ns, source_pid, source_generation);
                        }
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("{}", t_with_args("scheduler-event-frame", &fluent_args!(
                                "delta_ms" => format!("{:.2}", frame_delta_ns as f64 / 1_000_000.0)
                            )));
                        }
                    }

                    // [evt_reload]
                    // 热重载配置事件
                    DaemonEvent::ConfigReload(_new_rules) => {
                        let current_mode = mode_clone.lock().unwrap().clone();
                        log::debug!("{}", t_with_args("scheduler-event-config-reload", &fluent_args!(
                            "mode" => current_mode.clone(),
                            "screen_on" => is_screen_on.to_string()
                        )));
                        // fas 模式：FAS 配置编译期嵌入静态，rules.yaml 重载不参与息屏时不要用新配置覆盖 Doze
                        if is_screen_on {
                            let config_lock = config_clone.read().unwrap();
                            if crate::common::is_special_mode(&current_mode) {
                                // 特调模式：重载 akmode 配置
                                let ak_cfg = config_lock.get_tuned_profile(&current_mode);
                                if ak_governor.is_active() {
                                    ak_governor.reload_config(&ak_cfg);
                                } else {
                                    let in_cooldown = tuned_cooldown_until
                                        .map_or(false, |until| Instant::now() < until);
                                    if !in_cooldown {
                                        if !ak_governor.init_policies(&current_mode, &ak_cfg) {
                                            tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                            log::warn!("{}", t_with_args(
                                                "scheduler-tuned-cooldown",
                                                &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string())
                                            ));
                                            let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                            if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                        }
                                    } else {
                                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                }
                            } else if is_fast_lock(&current_mode) {
                                // 硬锁档（vector 锁最高/frozen 锁最低）不读 yaml 参数，仅确保 CLG 未意外启动
                                // **frozen 必须进这一支**：落到下面 else 会 fast_lock.release() 放掉刚锁上的
                                // 最低频——配置热重载（改 meta/rules 即触发）时必现
                                if cpu_governor.is_active() { cpu_governor.release(); }
                            } else if current_mode != "fas" {
                                // 非特调/非极速模式：释放 fast_lock 由 CLG 接管（fas 不参与热重载，配置编译期嵌入）
                                ak_governor.release();
                                fast_lock.release();
                                let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                if clg_cfg.enabled {
                                    // 后端是否要换（Worker↔PowerBase）由 CLG 在新配置下自行判定
                                    if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                    else { cpu_governor.init_policies(&clg_cfg); }
                                } else if cpu_governor.is_active() {
                                    cpu_governor.release();
                                }
                            }
                        }
                        // 亲和/core_ctl 与配置联动（开关变化时切换布局；内部去重）
                        {
                            let cfg = config_clone.read().unwrap();
                            crate::logger::set_diag_active(cfg.meta.dev_record);
                            crate::logger::set_diag_mode(&current_mode);
                            apply_affinity_and_corectl(
                                &mut affinity_mgr,
                                &mut corectl_mgr,
                                &cfg,
                                &current_mode,
                                is_screen_on,
                                crate::monitor::app_detect::get_current_pid(),
                                &last_core_utils,
                                scene_mode_active,
                            );
                        }
                        crate::logger::main_event("config_reload", "-", "rules.yaml");
                    }

                    // [evt_bpf]
                    // eBPF 扩展探针统计（ChiRi 专属遥测，2s 一次增量）
                    DaemonEvent::BpfStats { wakeups, migrations, freq_transitions } => {
                        // 仅缓存供遥测落盘，不参与调频决策也不刷新看门狗心跳（探针失败增量为 0）
                        last_bpf_stats = (wakeups, migrations, freq_transitions);
                    }
                }
            }
            }));
                // [panic_recovery]
                if loop_result.is_ok() {
                    // channel 关闭：正常退出
                    break;
                }
                log::error!("{}", t("scheduler-ipc-panic"));
                // 清理到安全态（release 幂等；收尾包 catch_unwind，release 再 panic 不击穿重启循环）
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    cpu_governor.release();
                    ak_governor.release();
                    fast_lock.release();
                    fas_mgr.deactivate_all();
                    corectl_mgr.release();
                    affinity_mgr.release();
                }));
                ipc_restart_count += 1;
                if ipc_restart_count > SCHEDULER_IPC_RESTART_MAX {
                    log::error!(
                        "{}",
                        t_with_args(
                            "scheduler-ipc-restart-giveup",
                            &fluent_args!("count" => SCHEDULER_IPC_RESTART_MAX.to_string())
                        )
                    );
                    // 极端兜底：看门狗（service.sh）只监控**进程**存活（3s 拉起），线程级死亡感知不到——CPU 已清理到安全态，退出整个进程交给看门狗拉起全新 daemon（含全核上线）
                    std::process::exit(1);
                }
                // 重置状态机到亮屏安全态：真实屏幕状态由下一个 ScreenStateChange 事件纠正（必被处理）
                pending_mode_after_fas = None;
                confirmed_window_focus = None;
                window_probe.invalidate();
                is_screen_on = true;
                crate::common::set_screen_on(true);
                screen_off_at = None;
                scene_mode_active = false;
                scene_hold_logged = false;
                scenemode_sat_since = None;
                last_core_utils.clear();
                std::thread::sleep(SCHEDULER_IPC_RESTART_BACKOFF);
                // 按当前模式重新接管（等价亮屏恢复语义；特调/fas 由后续事件重建）。**停摆期间必须跳过**：重建无 mode 之外门控，模式停在 lab/vector 会把刚释放的接管重新接回
                if !halted {
                    let current_mode = mode_clone
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    if current_mode == "contingency" || current_mode == "babel" {
                        // lab 静态分组：governor/GPU 与分组都重建
                        sync_lab_governor_gpu(&current_mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                        let cfg = config_clone.read().unwrap();
                        apply_affinity_and_corectl(
                            &mut affinity_mgr,
                            &mut corectl_mgr,
                            &cfg,
                            &current_mode,
                            is_screen_on,
                            crate::monitor::app_detect::get_current_pid(),
                            &[],
                            scene_mode_active,
                        );
                    } else if is_fast_lock(&current_mode) {
                        // vector/frozen 档不能漏：不注册 CLG（get_mode 返回 None → enabled=false），漏掉即零接管
                        fast_lock.init(current_mode == "frozen");
                    } else if current_mode != "fas"
                        && !crate::common::is_special_mode(&current_mode)
                    {
                        let config_lock = config_clone.read().unwrap();
                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                        if clg_cfg.enabled {
                            cpu_governor.init_policies(&clg_cfg);
                        }
                    }
                }
                log::warn!(
                    "{}",
                    t_with_args(
                        "scheduler-ipc-restart",
                        &fluent_args!("count" => ipc_restart_count.to_string())
                    )
                );
            }
            log::warn!("{}", t("scheduler-channel-closed"));
            // [selfcost] 停机收尾：若诊断会话仍开着，输出本线程 @S 自耗的不足 60s 尾窗并关闭
            // selfcost/worker（幂等）；同一线程调用，thread_local 累计器有效
            aff_cost_session_tick(false);
            // 收尾：无论 channel 关闭还是 panic，都恢复 CPU 控制状态，避免频率/governor 残留
            cpu_governor.release();
            ak_governor.release();
            fast_lock.release();
            governor_guard.release();
            gpu_guard.release();
            affinity_mgr.lab_static_deactivate();
            // FAS 全部实例去激活（恢复频率）后清理
            fas_mgr.deactivate_all();
            // 亲和布局与 core_ctl 同步恢复系统原始状态
            corectl_mgr.release();
            affinity_mgr.release();
        })?;

    Ok(())
}

// [tests]
// 本仓库无 CI 执行 `cargo test`（Android target 未装），以下测试只保证编译通过并可作聚焦用例。
// 仅覆盖**纯函数**：频率快照渲染、policy 发现缓存（TTL/重扫/增删/空结果）、槽级差分渲染。
// 说明：`build_aff_snapshot` 内的差分**判定**与采样耦合、非纯函数，未单独断言（其可观测输出
// 即 `render_t_row`，已在下方覆盖）；`pid_comm` 兜底的正确性由 `first_pid` 分支保证，属集成行为。
#[cfg(test)]
mod tests {
    use super::*;

    // ── [C3] 频率快照渲染：格式/顺序/读不到写 `-` ──
    #[test]
    fn render_freq_snapshot_matches_legacy_format() {
        let entries = vec![
            (
                0,
                (
                    "1000".to_string(),
                    "2000".to_string(),
                    "300".to_string(),
                    "schedutil".to_string(),
                ),
            ),
            (
                4,
                (
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                ),
            ),
        ];
        let (cur, max, min, gov) = render_freq_snapshot(&entries);
        assert_eq!(cur, "policy0:1000;policy4:-");
        assert_eq!(max, "policy0:2000;policy4:-");
        assert_eq!(min, "policy0:300;policy4:-");
        assert_eq!(gov, "policy0:schedutil;policy4:-");
    }

    #[test]
    fn render_freq_snapshot_empty_is_empty_strings() {
        let (cur, max, min, gov) = render_freq_snapshot(&[]);
        assert!(cur.is_empty() && max.is_empty() && min.is_empty() && gov.is_empty());
    }

    // ── [C3] policy 发现缓存：TTL 边界 ──
    #[test]
    fn policy_cache_needs_scan_without_cache() {
        assert!(policy_cache_needs_scan(&None, Instant::now()));
    }

    #[test]
    fn policy_cache_needs_scan_ttl_boundary_is_inclusive() {
        let now = Instant::now();
        // 刚好 TTL：需要重扫（>=）
        let at_boundary = Some(PolicyBases::from_ids(
            vec![0, 4],
            now - POLICY_RESCAN_TTL,
        ));
        assert!(policy_cache_needs_scan(&at_boundary, now));
        // 差 1ns 到 TTL：不重扫
        let just_before = Some(PolicyBases::from_ids(
            vec![0, 4],
            now - POLICY_RESCAN_TTL + Duration::from_nanos(1),
        ));
        assert!(!policy_cache_needs_scan(&just_before, now));
    }

    // ── [C3] policy 发现缓存：合并（空不缓存/保留旧；增删重建；同列表刷新时刻）──
    #[test]
    fn policy_cache_merge_empty_does_not_cache_on_first() {
        let mut cache: Option<PolicyBases> = None;
        let t0 = Instant::now();
        policy_cache_merge(&mut cache, Vec::new(), t0, t0);
        assert!(cache.is_none(), "首次空结果不得永久缓存");
        // 空期间每次调用都仍判定需要扫描
        assert!(policy_cache_needs_scan(&cache, Instant::now()));
    }

    #[test]
    fn policy_cache_merge_empty_retains_old_and_keeps_clock() {
        let t0 = Instant::now();
        let mut cache = Some(PolicyBases::from_ids(vec![0, 4], t0));
        // 重扫失败/为空：保留旧 id、**不刷新**时刻（下次照常重试）
        let t1 = t0 + Duration::from_secs(90);
        policy_cache_merge(&mut cache, Vec::new(), t1, t1);
        let c = cache.as_ref().unwrap();
        assert_eq!(c.ids, vec![0, 4]);
        assert_eq!(c.scanned_at, t0, "空结果不得推进扫描时刻");
        assert_eq!(c.bases, vec!["/sys/devices/system/cpu/cpufreq/policy0".to_string(),
                                 "/sys/devices/system/cpu/cpufreq/policy4".to_string()]);
    }

    #[test]
    fn policy_cache_merge_same_ids_only_refreshes_clock() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(61);
        let mut cache = Some(PolicyBases::from_ids(vec![0, 4], t0));
        policy_cache_merge(&mut cache, vec![0, 4], t1, t1);
        let c = cache.as_ref().unwrap();
        assert_eq!(c.ids, vec![0, 4]);
        assert_eq!(c.scanned_at, t1);
    }

    #[test]
    fn policy_cache_merge_addition_rebuilds_bases() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(61);
        let mut cache = Some(PolicyBases::from_ids(vec![0, 4], t0));
        policy_cache_merge(&mut cache, vec![0, 4, 7], t1, t1);
        let c = cache.as_ref().unwrap();
        assert_eq!(c.ids, vec![0, 4, 7]);
        assert_eq!(c.bases.len(), 3);
        assert_eq!(c.bases[2], "/sys/devices/system/cpu/cpufreq/policy7");
    }

    /// 并发保护：扫描在锁外进行，先开始的「新扫描」结果不得被后完成的「旧扫描」覆盖
    #[test]
    fn policy_cache_merge_rejects_stale_concurrent_scan() {
        let t_new = Instant::now();
        let t_old = t_new - Duration::from_secs(5);
        // 缓存由「开始于 t_new 的扫描」写入，时刻推进到 t_new
        let mut cache = Some(PolicyBases::from_ids(vec![0, 4, 7], t_new));
        // 另一线程「开始于 t_old 的扫描」后完成（t_new 之后才写）：必须被拒绝
        policy_cache_merge(&mut cache, vec![0, 4], t_old, t_new + Duration::from_secs(1));
        let c = cache.as_ref().unwrap();
        assert_eq!(c.ids, vec![0, 4, 7], "旧扫描不得覆盖较新的结果");
        assert_eq!(c.scanned_at, t_new);
    }

    // ── [C1] 槽级差分渲染：未变槽 `-`、尾部省略、真值 `-` 写 NaN、全未变只留标识 ──
    #[test]
    fn render_t_row_all_slots_unchanged_is_identifiers_only() {
        // 全槽 None = 本帧无变化 → 只留标识（下游按缺省=未变合并）
        let row = render_t_row(7, 9, None, None, None, None, None, None);
        assert_eq!(row, "t 7 9");
    }

    #[test]
    fn render_t_row_omits_trailing_unchanged_slots() {
        // comm 未变(-)、util 变、core/home/pin/uclamp 未变 → 尾部整段省略
        let row = render_t_row(1, 2, None, Some(5), None, None, None, None);
        assert_eq!(row, "t 1 2 - u=5");
    }

    #[test]
    fn render_t_row_dash_comm_renders_nan() {
        // comm 真值为 `-`：写 NaN，与「未变」的 `-` 区分
        let row = render_t_row(1, 2, Some("-"), None, None, None, None, None);
        assert_eq!(row, "t 1 2 NaN");
    }

    #[test]
    fn render_t_row_full_row_when_all_slots_present() {
        let row = render_t_row(3, 4, Some("binder"), Some(50), Some(6), Some(4), Some(true), Some(-1));
        assert_eq!(row, "t 3 4 binder u=50 core=6 home=4 pin=1 uclamp=-1");
    }

    #[test]
    fn render_t_row_pin_false_is_zero() {
        let row = render_t_row(1, 2, Some("x"), None, None, None, Some(false), None);
        assert_eq!(row, "t 1 2 x - - - pin=0");
    }

    // ── [B1] 自耗累计：record 只入账、不因窗口未开而丢弃首帧 ──
    #[test]
    fn aff_cost_record_accumulates_without_flushing_first_frame() {
        let t0 = Instant::now();
        // 同一线程内连续两帧（thread_local 独占）
        aff_cost_record(t0, None, t0, None, t0, None);
        // 首次 record 只开窗、不结算：累计器保留本帧计数
        AFF_COST.with(|cell| {
            let st = cell.borrow();
            assert_eq!(st.stats.stats(diag_cost::Block::Build).count, 1);
            assert_eq!(st.stats.stats(diag_cost::Block::Write).count, 1);
        });
    }

    // ── [D1] 配置身份串：同配置稳定、关键字段变化即不同 ──
    #[test]
    fn diag_config_identity_stable_and_sensitive_to_key_fields() {
        let a = Config::default();
        let b = Config::default();
        assert_eq!(
            diag_config_identity(&a),
            diag_config_identity(&b),
            "同一配置下身份串应稳定"
        );
        // 稳定性：同一实例多次调用结果相同（HashMap 迭代序不得影响指纹）
        assert_eq!(
            diag_config_identity(&a),
            diag_config_identity(&a),
            "同一配置多次调用结果应相同"
        );
        // HashMap 迭代序不应影响指纹：同一内容、不同插入顺序 → 指纹相同
        let mut m1 = Config::default();
        m1.sched
            .params
            .insert("k1".to_string(), "v1".to_string());
        m1.sched
            .params
            .insert("k2".to_string(), "v2".to_string());
        m1.tuned_profiles.insert(
            "alpha".to_string(),
            crate::chiri::config::SpecialTunedConfig::default(),
        );
        m1.tuned_profiles.insert(
            "beta".to_string(),
            crate::chiri::config::SpecialTunedConfig::default(),
        );
        let mut m2 = Config::default();
        m2.tuned_profiles.insert(
            "beta".to_string(),
            crate::chiri::config::SpecialTunedConfig::default(),
        );
        m2.tuned_profiles.insert(
            "alpha".to_string(),
            crate::chiri::config::SpecialTunedConfig::default(),
        );
        m2.sched
            .params
            .insert("k2".to_string(), "v2".to_string());
        m2.sched
            .params
            .insert("k1".to_string(), "v1".to_string());
        assert_eq!(
            diag_config_identity(&m1),
            diag_config_identity(&m2),
            "HashMap 插入序不同、内容相同应产生相同指纹"
        );
        // 敏感性：meta 关键字段
        let mut c = Config::default();
        c.meta.devimp_top_n = a.meta.devimp_top_n + 1;
        assert_ne!(
            diag_config_identity(&a),
            diag_config_identity(&c),
            "关键字段变化应改变身份串"
        );
        let mut d = Config::default();
        d.meta.loglevel = "DEBUG".to_string();
        assert_ne!(
            diag_config_identity(&a),
            diag_config_identity(&d),
            "loglevel 变化应改变身份串"
        );
        // 敏感性：调度相关（非 meta）字段
        let mut e = Config::default();
        e.sched.enabled = !a.sched.enabled;
        assert_ne!(
            diag_config_identity(&a),
            diag_config_identity(&e),
            "调度相关字段变化应改变身份串"
        );
        let mut f = Config::default();
        f.scene_mode_delay_secs = a.scene_mode_delay_secs.wrapping_add(1);
        assert_ne!(
            diag_config_identity(&a),
            diag_config_identity(&f),
            "调度相关字段变化应改变身份串"
        );
    }

    // ── [D1] 前台身份变化判定：首次 / 包名变 / pid 变 / starttime 变 / 都不变 ──
    #[test]
    fn diag_identity_changed_covers_pkg_pid_and_first() {
        let prev_none: Option<DiagIdentity> = None;
        assert!(diag_identity_changed(
            &prev_none,
            &("com.foo".to_string(), 100, 1000)
        ));
        let prev: Option<DiagIdentity> = Some(("com.foo".to_string(), 100, 1000));
        // 包名变
        assert!(diag_identity_changed(
            &prev,
            &("com.bar".to_string(), 100, 1000)
        ));
        // pid 变（同包 PID 更换 / 进程重启）
        assert!(diag_identity_changed(
            &prev,
            &("com.foo".to_string(), 200, 1000)
        ));
        // starttime 变（包名与 pid 均不变 → 同包同 PID 复用 / PID 回收）
        assert!(diag_identity_changed(
            &prev,
            &("com.foo".to_string(), 100, 2000)
        ));
        // 都不变
        assert!(!diag_identity_changed(
            &prev,
            &("com.foo".to_string(), 100, 1000)
        ));
    }

    // ── [D1] 采样快照：generation 必须在「身份变化 → bump」之后取，才能取到新代际 ──
    // 覆盖顺序不变式「同一 tick 内先 bump 后取 generation ⇒ 取到的是新代际」
    // （消除「新 PID 配旧 generation」）。注：调度循环里「身份判定段先于采样段」这一先后
    // 关系由代码位置保证，只能靠静态审查；此处对可运行的原语（generation/sequence）做
    // 运行时验证。
    #[test]
    fn diag_sample_frame_takes_generation_after_bump() {
        let w = diag_worker::DiagWorker::start();
        let before = w.generation();
        // 先：身份变化 → bump 代际
        w.bump_generation();
        // 后：采样时点构造快照（读 generation/sequence，并在同段读 config_identity）
        let f = diag_sample_frame("0101-000000".to_string(), 1234, 987654, &w);
        assert_eq!(
            f.generation,
            before + 1,
            "快照必须取到 bump 后的新代际（先 bump 后取）"
        );
        assert_eq!(f.sequence, 0, "首个采样帧序号从 0 起");
        assert_eq!(f.fg_pid, 1234);
        assert_eq!(f.fg_starttime, 987654);
        // 再采一帧：序号递增（与采样顺序一致）；未 bump 时代际保持不变
        let f2 = diag_sample_frame("0101-000001".to_string(), 1234, 987654, &w);
        assert_eq!(f2.sequence, 1);
        assert_eq!(f2.generation, before + 1);
        w.shutdown();
    }

    // ── [D1] canonical_debug：跨父节点交换值必须改变指纹（旧实现整体排序会碰撞）──
    #[test]
    fn canonical_debug_distinguishes_cross_parent_value_swap() {
        // 两个 profile 的 params 值互换（**不同父节点**下）；两段文本行多重集相同。
        let a = r#"Config {
    tuned_profiles: {
        "a": P {
            params: {
                "x": "1",
            },
        },
        "b": P {
            params: {
                "x": "2",
            },
        },
    },
}"#;
        let b = r#"Config {
    tuned_profiles: {
        "a": P {
            params: {
                "x": "2",
            },
        },
        "b": P {
            params: {
                "x": "1",
            },
        },
    },
}"#;
        // 先证明这是本缺陷的回归：旧实现（整体 sort_unstable）下两段文本无法区分。
        let mut la: Vec<&str> = a.lines().collect();
        la.sort_unstable();
        let mut lb: Vec<&str> = b.lines().collect();
        lb.sort_unstable();
        assert_eq!(la, lb, "两段文本行多重集相同（旧实现确定性碰撞）");
        // 新实现：层级保留，跨父节点交换值 → 指纹必须不同。
        assert_ne!(canonical_debug(a), canonical_debug(b));
    }

    // ── [D1] canonical_debug：同层级 map 条目顺序颠倒（HashMap 迭代序）指纹不变 ──
    #[test]
    fn canonical_debug_is_map_iteration_order_independent() {
        let first = r#"Config {
    tuned_profiles: {
        "a": P {
            params: {
                "x": "1",
            },
        },
        "b": P {
            params: {
                "x": "2",
            },
        },
    },
}"#;
        // 同一层级两条 map 条目顺序颠倒：语义相同 → 指纹必须相同。
        let swapped = r#"Config {
    tuned_profiles: {
        "b": P {
            params: {
                "x": "2",
            },
        },
        "a": P {
            params: {
                "x": "1",
            },
        },
    },
}"#;
        assert_eq!(canonical_debug(first), canonical_debug(swapped));
    }

    #[test]
    fn canonical_debug_preserves_ordered_list_priority() {
        let first = "Config {\n    cpu_temp_zone_types: [\n        \"soc_max\",\n        \"cpuss\",\n    ],\n}";
        let swapped = "Config {\n    cpu_temp_zone_types: [\n        \"cpuss\",\n        \"soc_max\",\n    ],\n}";
        assert_ne!(canonical_debug(first), canonical_debug(swapped));
    }

    // ── [D1] canonical_debug：同一个值从一层移到另一层指纹必须不同（层级保留）──
    #[test]
    fn canonical_debug_preserves_hierarchy() {
        let shallow = r#"Config {
    "x": "1",
    p: {
    },
}"#;
        let deep = r#"Config {
    p: {
        "x": "1",
    },
}"#;
        assert_ne!(canonical_debug(shallow), canonical_debug(deep));
    }

    // ── [D1] canonical_debug：同一输入两次调用结果相同；空输入不 panic ──
    #[test]
    fn canonical_debug_is_stable() {
        let text = r#"Config {
    sched: {
        params: {
            "k1": "v1",
        },
    },
}"#;
        assert_eq!(canonical_debug(text), canonical_debug(text));
        assert_eq!(canonical_debug(""), "");
    }

    // ── [D1] diag_config_identity：交换不同 profile 的参数值必须改变指纹（缺陷直接体现）──
    #[test]
    fn diag_config_identity_distinguishes_cross_profile_value_swap() {
        let mut p = crate::chiri::config::SpecialTunedConfig::default();
        p.headroom = 1.11;
        let mut q = crate::chiri::config::SpecialTunedConfig::default();
        q.headroom = 2.22;

        let mut a = Config::default();
        a.tuned_profiles.insert("a".to_string(), p.clone());
        a.tuned_profiles.insert("b".to_string(), q.clone());

        // 交换 "a"/"b" 两个 profile 的 headroom 值（不同父节点下的同名叶子）。
        let mut b = Config::default();
        b.tuned_profiles.insert("a".to_string(), q);
        b.tuned_profiles.insert("b".to_string(), p);

        assert_ne!(
            diag_config_identity(&a),
            diag_config_identity(&b),
            "交换不同 profile 的参数值必须改变指纹（旧实现整体排序会碰撞）"
        );
    }
}
