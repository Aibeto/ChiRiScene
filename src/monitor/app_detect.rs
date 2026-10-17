//! app_detect.rs: [state] [cgroup] [mode] [rules] [cfgwatch] [loop]

use inotify::{Inotify, WatchMask};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::watch;

use super::config::{self, RulesConfig};
use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};
use crate::utils;

// [state]
// 缓存有效的 Cgroup 路径索引，避免每次循环都去探测无效路径
static VALID_CGROUP_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);

static CURRENT_PID: AtomicI32 = AtomicI32::new(0);

/// Activity 级规则是否配置（`activity_aware` 且 `app_modes` 里存在 `pkg/.Activity` 键）。
/// 由检测循环每轮更新；调度 1s 块据此决定是否请求全局 Activity 采样，避免无 activity 配置时
/// 白白承担 dumpsys 自耗。
static ACTIVITY_RULES_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 当前是否配置了 Activity 级规则（供调度侧决定采样开关）
pub fn activity_rules_configured() -> bool {
    ACTIVITY_RULES_ACTIVE.load(Ordering::Relaxed)
}

// 获取系统已启用的输入法列表
fn get_system_ime_packages() -> HashSet<String> {
    let mut imes = HashSet::new();

    let output = Command::new("settings")
        .arg("get")
        .arg("secure")
        .arg("enabled_input_methods")
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for entry in stdout.split(':') {
            if let Some(pkg) = entry.split('/').next() {
                let clean_pkg = pkg.trim();
                if !clean_pkg.is_empty() {
                    imes.insert(clean_pkg.to_string());
                    // 启动期一次性发现（影响后续前台过滤行为），稳态不再出现 → info
                    info!(
                        "{}",
                        t_with_args("app-detect-ime-auto", &fluent_args!("pkg" => clean_pkg))
                    );
                }
            }
        }
    }

    if imes.is_empty() {
        warn!("{}", t("app-detect-ime-fallback"));
        imes.insert("com.sohu.inputmethod.sogou.xiaomi".to_string());
        imes.insert("com.sohu.inputmethod.sogouoem".to_string());
        imes.insert("com.google.android.inputmethod.latin".to_string());
        imes.insert("com.baidu.input_mi".to_string());
        imes.insert("com.iflytek.inputmethod.miui".to_string());
    }

    imes
}

/// 前台身份快照：包名用 Arc<str>（读方零分配）、进程启动时间用于防 PID 复用，检测代数随身份变化递增
#[derive(Clone, Debug)]
pub struct ForegroundSnapshot {
    pub package: Arc<str>,
    pub pid: i32,
    pub process_starttime: Option<u64>,
    pub generation: u64,
    /// 前台 Activity 查表键（`pkg/.MainActivity` 短形）；空 = 未知。
    /// **不参与 `identity_matches`**：activity 切换不得改变 generation，否则每次页面跳转都会
    /// 重下 ModeChange + 触发 FAS 重接管。消费方只把它当作 `determine_mode` 的附加查表维度。
    pub activity: Arc<str>,
}

impl ForegroundSnapshot {
    /// 身份一致性：包名、PID、进程启动时间三者全同才算同一前台实例；PID 复用或进程重启都会破坏它
    /// （activity 刻意不参与：它是路由信息，不是进程身份）
    pub fn identity_matches(&self, other: &ForegroundSnapshot) -> bool {
        self.package == other.package
            && self.pid == other.pid
            && self.process_starttime == other.process_starttime
    }
}

// 身份与 raw 提示同锁存放：读方一次锁内拿到一致快照，避免读到半更新的包名/PID 组合
struct ForegroundState {
    package: Arc<str>,
    pid: i32,
    process_starttime: Option<u64>,
    generation: u64,
    raw_package: Arc<str>,
    activity: Arc<str>,
}

impl Default for ForegroundState {
    fn default() -> Self {
        Self {
            package: Arc::from(""),
            pid: 0,
            process_starttime: None,
            generation: 0,
            raw_package: Arc::from(""),
            activity: Arc::from(""),
        }
    }
}

impl ForegroundState {
    fn snapshot(&self) -> ForegroundSnapshot {
        ForegroundSnapshot {
            package: self.package.clone(),
            pid: self.pid,
            process_starttime: self.process_starttime,
            generation: self.generation,
            activity: self.activity.clone(),
        }
    }
}

static FOREGROUND_STATE: LazyLock<Mutex<ForegroundState>> =
    LazyLock::new(|| Mutex::new(ForegroundState::default()));
static IME_BLOCKLIST: LazyLock<HashSet<String>> = LazyLock::new(get_system_ime_packages);

pub fn get_current_pid() -> i32 {
    CURRENT_PID.load(Ordering::Relaxed)
}

/// 当前前台包名（实时，含同模式切换——包名变化即更新）；供 scheduler_ipc 的 devimp snap 行等消费，避免各处维护过期副本
pub fn get_current_package() -> String {
    FOREGROUND_STATE.lock().unwrap().package.to_string()
}

/// 当前前台包名的**零分配**快照：Arc<str> 克隆只递增引用计数；供周期块（如 1s 的 FAS 巡检）代替 get_current_package()
pub fn current_package_arc() -> Arc<str> {
    FOREGROUND_STATE.lock().unwrap().package.clone()
}

/// 前台身份快照（包名/PID/启动时间/检测代数）一次锁内取齐，供窗口核验与 FAS 门控绑定同一实例；
/// 无效身份（空包名或非正 PID）返回 None，调用方不得把缺失当成合法身份复用
pub fn foreground_snapshot() -> Option<ForegroundSnapshot> {
    let snapshot = FOREGROUND_STATE.lock().unwrap().snapshot();
    (snapshot.pid > 0 && !snapshot.package.is_empty()).then_some(snapshot)
}

/// 未经过 ignored/IME 筛选的 cgroup 候选，仅作为窗口核验提示，不证明可见性；检测失败即清空（Unknown），不沿用旧包名
pub fn raw_foreground_package_arc() -> Arc<str> {
    FOREGROUND_STATE.lock().unwrap().raw_package.clone()
}

/// 当前前台 Activity 查表键（`pkg/.MainActivity` 短形，零分配 Arc 克隆）；空 = 未知。
/// 由全局 [`ActivitySampler`](crate::monitor::window_visibility::ActivitySampler) 与 FAS 窗口探针
/// 共同刷新（屏幕亮且有有效前台时采样，**不限 FAS**）；空 = 未知，调用方必须回退包名级判定。
pub fn current_activity_arc() -> Arc<str> {
    FOREGROUND_STATE.lock().unwrap().activity.clone()
}

fn set_raw_foreground_package(package: &str) {
    let mut state = FOREGROUND_STATE.lock().unwrap();
    if state.raw_package.as_ref() != package {
        state.raw_package = Arc::from(package);
    }
}

/// 检测代数：身份（包名/PID/启动时间）变化或同身份强制刷新时递增；旧代数事件据此被后续快照拒绝
fn next_generation(
    current: &ForegroundSnapshot,
    candidate: &ForegroundSnapshot,
    force_bump: bool,
) -> u64 {
    if force_bump || !current.identity_matches(candidate) {
        current.generation.wrapping_add(1)
    } else {
        current.generation
    }
}

/// 检测到新包名时更新**预处理**：com.xx:push 子进程名一律先归一到主包名（com.xx）——子进程与所属包调度语义就是同一应用（厂商框架改写 cmdline 首段同理）归一发生在唯一入口，下游（规则匹配、
/// FAS/特调白名单、亲和迁移、devimp 分组、通知）拿到的都已是主包名，无需再剥后缀
/// 返回更新后的快照：事件生产取发送时快照的 generation 绑定检测代数，不在消费端重标
fn set_current_package(
    pkg: &str,
    pid: i32,
    process_starttime: Option<u64>,
    force_bump: bool,
    activity: &str,
) -> ForegroundSnapshot {
    let base = match pkg.split_once(':') {
        Some((b, _suffix)) => {
            debug!(
                "{}",
                t_with_args(
                    "app-detect-pkg-normalized",
                    &fluent_args!("pkg" => pkg, "base" => b)
                )
            );
            b
        }
        None => pkg,
    };
    let mut state = FOREGROUND_STATE.lock().unwrap();
    let candidate = ForegroundSnapshot {
        package: Arc::from(base),
        pid,
        process_starttime,
        generation: state.generation,
        activity: Arc::from(""),
    };
    let generation = next_generation(&state.snapshot(), &candidate, force_bump);
    // activity 槽更新规则见 `next_activity`：换包清空（防新包继承旧包的 activity）、
    // 空观测保留旧值（全局采样按 3s 节流，空观测/叠加层抢焦点是常态）
    let package_changed = state.package.as_ref() != base;
    state.activity = next_activity(package_changed, &state.activity, activity);
    state.package = candidate.package;
    state.pid = pid;
    state.process_starttime = process_starttime;
    state.generation = generation;
    CURRENT_PID.store(pid, Ordering::Relaxed);
    state.snapshot()
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn snapshot(
        package: &str,
        pid: i32,
        starttime: Option<u64>,
        generation: u64,
    ) -> ForegroundSnapshot {
        ForegroundSnapshot {
            package: Arc::from(package),
            pid,
            process_starttime: starttime,
            generation,
            activity: Arc::from(""),
        }
    }

    #[test]
    fn same_package_restart_updates_identity() {
        let previous = snapshot("com.example.game", 42, Some(1000), 7);
        let restarted = snapshot("com.example.game", 43, Some(1000), 7);
        assert!(!previous.identity_matches(&restarted));
        assert_eq!(next_generation(&previous, &restarted, false), 8);
    }

    #[test]
    fn package_change_clears_stale_activity() {
        // 换包必须丢弃旧包 activity，否则新包会继承旧包的规则（游戏→微信 例）
        assert_eq!(
            next_activity(true, "com.game/.MainActivity", "").as_ref(),
            ""
        );
        // 换包且当期观测到新 activity：直接用新的
        assert_eq!(
            next_activity(true, "com.game/.MainActivity", "com.tencent.mm/.Chat").as_ref(),
            "com.tencent.mm/.Chat"
        );
    }

    #[test]
    fn same_package_keeps_activity_when_observation_is_empty() {
        // 采样节流/叠加层抢焦点时会出现空观测，空观测不得抹掉已生效的 activity 规则
        assert_eq!(
            next_activity(false, "com.game/.MainActivity", "").as_ref(),
            "com.game/.MainActivity"
        );
        // 同包内换页（Activity 切换）：采纳新值
        assert_eq!(
            next_activity(false, "com.game/.Main", "com.game/.Settings").as_ref(),
            "com.game/.Settings"
        );
    }

    #[test]
    fn same_package_and_pid_with_new_starttime_updates_identity() {
        let previous = snapshot("com.example.game", 42, Some(1000), 7);
        let reused_pid = snapshot("com.example.game", 42, Some(2000), 7);
        assert!(!previous.identity_matches(&reused_pid));
        assert_eq!(next_generation(&previous, &reused_pid, false), 8);
    }

    #[test]
    fn unreadable_starttime_is_not_legal_reuse() {
        let previous = snapshot("com.example.game", 42, Some(1000), 7);
        let unreadable = snapshot("com.example.game", 42, None, 7);
        assert!(!previous.identity_matches(&unreadable));
        assert_eq!(next_generation(&previous, &unreadable, false), 8);
    }

    #[test]
    fn forced_refresh_advances_generation_so_old_events_are_rejected() {
        let previous = snapshot("com.example.game", 42, Some(1000), 7);
        let unchanged = snapshot("com.example.game", 42, Some(1000), 7);
        assert_eq!(
            next_generation(&previous, &unchanged, false),
            previous.generation
        );
        assert_eq!(
            next_generation(&previous, &unchanged, true),
            previous.generation.wrapping_add(1)
        );
    }

    #[test]
    fn rapid_return_replaces_pending_pid_without_package_debounce() {
        assert_eq!(
            confirmed_foreground_pid("com.example.game", "com.example.game", 43, 42),
            43
        );
        assert_eq!(
            confirmed_foreground_pid("com.example.game", "com.example.other", 43, 42),
            42
        );
    }
}

// [cgroup]

#[cfg(test)]
mod activity_tests {
    use super::*;

    #[test]
    fn activity_for_package_rejects_foreign_and_malformed_keys() {
        assert_eq!(
            activity_for_package("com.game/.Main", "com.game").as_ref(),
            "com.game/.Main"
        );
        // 子进程包名（com.game:push）与主包视为同一应用
        assert_eq!(
            activity_for_package("com.game/.Main", "com.game:push").as_ref(),
            "com.game/.Main"
        );
        // 换包瞬间的旧包 activity / 系统叠加层 / 空值/畸形值一律丢弃
        assert_eq!(
            activity_for_package("com.other/.Main", "com.game").as_ref(),
            ""
        );
        assert_eq!(
            activity_for_package("com.android.systemui/.Panel", "com.game").as_ref(),
            ""
        );
        assert_eq!(activity_for_package("", "com.game").as_ref(), "");
        assert_eq!(activity_for_package("junk", "com.game").as_ref(), "");
    }

    #[test]
    fn activity_observation_changed_ignores_empty_and_identity_rounds() {
        // 同包内换页：触发重算
        assert!(activity_observation_changed(
            false,
            "com.game/.A",
            "com.game/.B"
        ));
        // 同值 / 空观测（节流、叠加层抢焦点）/ 换包轮次（交给 identity_changed）：不触发
        assert!(!activity_observation_changed(
            false,
            "com.game/.A",
            "com.game/.A"
        ));
        assert!(!activity_observation_changed(
            true,
            "com.game/.A",
            "com.other/.B"
        ));
        assert!(!activity_observation_changed(false, "", "com.game/.A"));
    }
}

/// 归一主包名（去掉 com.xx:push 子进程后缀），与 set_current_package 的归一保持同一口径
fn base_package(pkg: &str) -> &str {
    pkg.split_once(':').map_or(pkg, |(base, _suffix)| base)
}

/// 读取 /proc/<pid>/stat 第 22 字段（进程启动时间）：防同包同 PID 的 ABA；读取失败返回 None，调用方不得当作合法复用
fn read_process_starttime(pid: i32) -> Option<u64> {
    let stat = utils::read_file_content(&format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
}

fn confirmed_foreground_pid(
    previous_package: &str,
    detected_package: &str,
    detected_pid: i32,
    previous_pid: i32,
) -> i32 {
    if detected_package == previous_package && detected_pid > 0 {
        detected_pid
    } else {
        previous_pid
    }
}

/// 判断是否为有效的用户应用包名
fn is_valid_user_app(pkg: &str, ignored_apps: &[String]) -> bool {
    if pkg.is_empty()
        || !pkg.contains('.')
        || pkg.starts_with('/')
        || pkg.starts_with('.')
        || pkg.contains(':')
    {
        return false;
    }
    if IME_BLOCKLIST.contains(pkg) {
        return false;
    }
    if ignored_apps
        .iter()
        .any(|ignored| pkg == ignored || pkg.contains(ignored))
    {
        return false;
    }
    match pkg {
        "com.android.systemui" => false,
        "system_server" => false,
        "surfaceflinger" => false,
        "android.hardware.graphics.composer" => false,
        "com.android.phone" => false,
        "com.android.permissioncontroller" => false,
        "chiri" => false,
        "com.xiaomi.vtcamera" => false,
        "com.android.providers.media.module" => false,
        "com.google.android.gms.ui" => false,
        "com.xiaomi.mibrain.speech" => false,
        _ => {
            if pkg.contains("magisk") || pkg.contains("mtiodaemon") {
                return false;
            }
            if pkg.contains("ads_monitor") {
                return false;
            }
            if pkg.contains("inputmethod") {
                return false;
            }
            true
        }
    }
}

/// 倒序扫描（Android 把最新前台放在 procs 末尾，命中即返回）；反向迭代 + 复用 cmdline 路径缓冲，每轮省一次 Vec 与 String 分配**刻意不做「内容未变则复用」短路**：
/// 冷启动期间 pid 先入组、exec 后 cmdline 才可读，缓存会漏检这类前台切换
fn check_cgroup_path(
    path: &str,
    ignored_apps: &[String],
) -> Option<(Option<String>, Option<(String, i32)>)> {
    let Ok(content) = utils::read_file_content(path) else {
        return None;
    };
    let mut raw_candidate = None;
    let mut cmdline_path = String::with_capacity(32);
    for pid_str in content.split_whitespace().rev() {
        let Ok(pid) = pid_str.parse::<i32>() else {
            continue;
        };
        if pid <= 0 {
            continue;
        }
        cmdline_path.clear();
        let _ =
            std::fmt::Write::write_fmt(&mut cmdline_path, format_args!("/proc/{pid_str}/cmdline"));
        if let Ok(cmdline) = utils::read_file_content(&cmdline_path) {
            let pkg_name = cmdline.split('\0').next().unwrap_or("").trim();
            let candidate_base = base_package(pkg_name);
            if raw_candidate.is_none()
                && candidate_base.contains('.')
                && !candidate_base.starts_with('/')
                && !candidate_base.starts_with('.')
            {
                raw_candidate = Some(candidate_base.to_string());
            }
            if is_valid_user_app(pkg_name, ignored_apps) {
                return Some((raw_candidate, Some((pkg_name.to_string(), pid))));
            }
        }
    }
    Some((raw_candidate, None))
}

/// 从 Cgroup 读取前台应用
fn get_focused_app_from_cgroup(ignored_apps: &[String]) -> Result<(String, i32), Box<dyn Error>> {
    let paths = [
        "/dev/cpuset/top-app/cgroup.procs",
        "/sys/fs/cgroup/cpuset/top-app/cgroup.procs",
        "/dev/stune/top-app/cgroup.procs",
    ];

    let cached = VALID_CGROUP_IDX.load(Ordering::Relaxed);
    let mut raw_candidate = None;
    let mut read_succeeded = false;
    let path_indices = (cached < paths.len())
        .then_some(cached)
        .into_iter()
        .chain((0..paths.len()).filter(|index| *index != cached));
    for index in path_indices {
        if let Some((candidate, filtered)) = check_cgroup_path(paths[index], ignored_apps) {
            read_succeeded = true;
            if raw_candidate.is_none() {
                raw_candidate = candidate;
            }
            if let Some(foreground) = filtered {
                set_raw_foreground_package(raw_candidate.as_deref().unwrap_or(""));
                VALID_CGROUP_IDX.store(index, Ordering::Relaxed);
                return Ok(foreground);
            }
        }
    }
    if read_succeeded {
        set_raw_foreground_package(raw_candidate.as_deref().unwrap_or(""));
    }
    Err("No valid app found".into())
}

// [mode]

/// 前台 activity 槽的下一个值（纯函数，便于回归测试）。两条规则：
/// - **换包清空**：activity 属于「上一个前台包」，包变了旧值就失去意义，必须丢弃，
///   否则新包会继承旧包的 activity（游戏→微信 时微信可能被套上游戏的模式）。
/// - **空值不覆盖**：全局采样是节流的，窗口叠加层抢焦点/采样失败等也会给出空值；
///   空值保留旧值，避免规则在两次采样之间来回抖回包名级。
fn next_activity(package_changed: bool, current: &str, observed: &str) -> Arc<str> {
    if package_changed {
        if observed.is_empty() {
            return Arc::from("");
        }
        return Arc::from(observed);
    }
    if observed.is_empty() {
        return Arc::from(current);
    }
    Arc::from(observed)
}

/// 只接受属于当前前台包的 Activity 键：`LAST_ACTIVITY` 是全局焦点窗口，换包瞬间可能仍是旧包
/// （采样器尚未刷新），系统叠加层（systemui 等）也可能短暂抢焦点。不校验前缀会让新包命中旧包的
/// activity 规则（`activity_lookup` 直接按整键查表）。包的比较用 `base_package`
/// （`com.x:push` 与 `com.x` 视为同一应用）。
fn activity_for_package(activity: &str, package: &str) -> Arc<str> {
    let base = base_package(package);
    match activity.split_once('/') {
        Some((pkg, _)) if !pkg.is_empty() && pkg == base => Arc::from(activity),
        _ => Arc::from(""),
    }
}

/// 同包内 Activity 观测是否变化（纯函数，便于测试）：换包轮次由 identity_changed 处理，
/// 空观测（采样节流/叠加层抢焦点）不触发重算，避免模式在两包之间抖动。
fn activity_observation_changed(identity_changed: bool, observed: &str, last: &str) -> bool {
    !identity_changed && !observed.is_empty() && observed != last
}

/// Activity 级规则查表（纯函数，便于回归测试）：返回命中的模式名。
/// 三个前提缺一不可 —— 总开关开启、activity 非空、`app_modes` 里确有该 activity 键；
/// 任一不满足即返回 `None`，由调用方回退包名级查表（默认回退语义）。
fn activity_lookup<'a>(
    app_modes: &'a HashMap<String, String>,
    _current_package: &str,
    activity: &str,
    activity_aware: bool,
) -> Option<&'a String> {
    if !activity_aware || activity.is_empty() {
        return None;
    }
    app_modes.get(activity)
}

/// 模式判定：FAS 白名单 > **Activity 级规则** > 特调白名单 > app_modes(包名) > 特调映射 > 全局模式（见各分支注释）
/// **PowerBase 不在这里出现**：它只替换「谁来调频」，模式名与所有外部接口（current_mode.chr /
/// rules.yaml / WebUI / 通知）保持原样——开启后 current_mode 依然是 default/boost
///
/// `activity` = `current_activity_arc()` 的查表键（`pkg/.MainActivity`，空 = 未知）。
/// **默认回退**：activity 为空或查不到时完全落回包名级逻辑，行为与引入本参数前逐位一致。
/// **选择性覆盖**：只有用户在 `app_modes` 里显式写了 `pkg/.Activity` 键才会命中这一层。
fn determine_mode(config: &RulesConfig, current_package: &str, activity: &str) -> String {
    // 特调仅 Chiri SoC 且 tuned_profiles.yaml 加载成功时生效；缺配置的机型白名单应用回退 CLG 普通模式调度
    let chiri = crate::common::is_chiri_soc();
    let special_enabled = chiri && crate::common::is_special_tuned_available();

    // 全局模式兜底值：实验室（rhine）启用期间用它的模式覆盖 rules.yaml 的 global_mode，只换兜底值，FAS 白名单与 app_modes 优先级不动
    let global_mode: String =
        crate::common::lab_global_mode().unwrap_or_else(|| config.global_mode.clone());

    // FAS 白名单最高优先（ChiRi 专属）：命中且应用配置可用即进 fas 模式，不受 rules.yaml app_modes/global_mode 影响
    if chiri && crate::common::fas_available() {
        if let Some(cfg_name) = crate::common::fas_whitelist_entry(current_package) {
            if crate::common::fas_app_config(cfg_name).is_some() {
                debug!(
                    "{}",
                    t_with_args(
                        "app-detect-fas-fallback",
                        &fluent_args!("pkg" => current_package)
                    )
                );
                return "fas".to_string();
            }
        }
    }

    // 白名单应用始终进特调（ChiRi 专属），不管 rules.yaml 配了什么；配置的普通模式只作特调起始档（scheduler 侧识别）
    if special_enabled {
        if let Some(entry) = crate::common::special_tuned_entry(current_package) {
            debug!(
                "{}",
                t_with_args(
                    "app-detect-special-fallback",
                    &fluent_args!("pkg" => current_package, "mode" => entry.fallback.as_str())
                )
            );
            return entry.fallback.clone();
        }
    }
    if !config.dynamic_enabled {
        return global_mode;
    }
    // 特调为 ChiRi 专属且仅白名单应用生效：非 ChiRi SoC 或非白名单包名映射到特调模式时回退全局模式并告警（WebUI 扫描后会同步清理非法条目）规则表匹配：
    // [rules]
    // 前台名与规则键都已归一为主包名（见 set_current_package / config.rs），此处为精确查表，不做回退链
    // [activity] Activity 级规则（选择性优先级覆盖）：仅当总开关开启、activity 非空、且用户在
    // app_modes 里显式写了 `pkg/.Activity` 键时命中。未命中（或开关关闭/activity 未知）即落回
    // 下方包名级逻辑 —— 这就是「默认回退」，未配置 activity 的应用行为逐位不变。
    // 注意 activity 由全局采样器刷新（不限 FAS）；未开启 activity 规则或采样未命中时自动走包名级。
    let activity_mode = activity_lookup(
        &config.app_modes,
        current_package,
        activity,
        config.activity_aware,
    );
    if activity_mode.is_some() {
        debug!(
            "{}",
            t_with_args(
                "app-detect-activity-override",
                &fluent_args!("pkg" => current_package, "activity" => activity)
            )
        );
    }
    let app_mode = activity_mode.or_else(|| config.app_modes.get(current_package));
    if let Some(mode) = app_mode {
        if crate::common::is_special_mode(mode) || crate::common::is_fas_mode(mode) {
            // FAS 模式仅白名单驱动：app_modes 中手动映射的 "fas" 一律拒绝（防绕过白名单）
            if crate::common::is_fas_mode(mode) {
                warn!(
                    "{}",
                    t_with_args(
                        "app-detect-fas-rejected",
                        &fluent_args!("pkg" => current_package, "mode" => mode.as_str())
                    )
                );
                return global_mode.clone();
            }
            if special_enabled && crate::common::is_special_mode_allowed(current_package, mode) {
                debug!(
                    "{}",
                    t_with_args(
                        "app-detect-special-override",
                        &fluent_args!("pkg" => current_package, "mode" => mode.as_str())
                    )
                );
                return mode.clone();
            }
            // 特调不可用（Chiri SoC 缺 tuned_profiles.yaml）与非白名单非法映射区分告警，均回退全局模式
            if chiri && !crate::common::is_special_tuned_available() {
                warn!(
                    "{}",
                    t_with_args(
                        "app-detect-special-unavailable",
                        &fluent_args!("pkg" => current_package, "mode" => mode.as_str())
                    )
                );
            } else {
                warn!(
                    "{}",
                    t_with_args(
                        "app-detect-special-rejected",
                        &fluent_args!("pkg" => current_package, "mode" => mode.as_str())
                    )
                );
            }
            return global_mode.clone();
        }
        return mode.clone();
    }
    if special_enabled {
        if let Some(mode) = crate::common::special_tuned_mode(current_package) {
            debug!(
                "{}",
                t_with_args(
                    "app-detect-special-fallback",
                    &fluent_args!("pkg" => current_package, "mode" => mode.as_str())
                )
            );
            return mode;
        }
    }
    let global = global_mode;
    // 全局模式不允许指定为 FAS（FAS 仅白名单驱动）
    if crate::common::is_fas_mode(&global) {
        warn!(
            "{}",
            t_with_args(
                "app-detect-fas-global-rejected",
                &fluent_args!("pkg" => current_package, "mode" => global.as_str())
            )
        );
        return "default".to_string();
    }
    // 全局模式本身不允许直接指定特调：非白名单应用回退 default（默认模式）
    if crate::common::is_special_mode(&global)
        && !(special_enabled && crate::common::is_special_mode_allowed(current_package, &global))
    {
        warn!(
            "{}",
            t_with_args(
                "app-detect-special-global-rejected",
                &fluent_args!("pkg" => current_package, "mode" => global.as_str())
            )
        );
        return "default".to_string();
    }
    global
}

/// 外部请求重算一次模式（实验室 rhine 套用/还原后调用）。规则热重载走 watch_config_file 的force_refresh_arc，实验室在调度线程侧拿不到，
/// 故补进程级标志由 app_detection_loop 下一轮swap 消费——用户点启用后模式当场重算（该变才发 ModeChange），不用等下次前台切换
static FORCE_MODE_REFRESH: AtomicBool = AtomicBool::new(false);

/// 请求 app_detection_loop 重新判定模式（模式实际变化时照常发 ModeChange 事件）
pub fn request_mode_refresh() {
    FORCE_MODE_REFRESH.store(true, Ordering::SeqCst);
}

/// 最近一次判定出的模式（与 app_detection_loop 的 last_mode 逐点同步；空串 = 尚未判定或亮屏后清空待重算）**DOWN 停摆退出专用**：
/// 停摆期间 ModeChange 被调度线程丢弃且不补发，退出停摆时若沿用旧快照会因「模式没变不发事件」而一直空窗，调度线程读这里对齐真实模式
static LAST_DETERMINED_MODE: Mutex<String> = Mutex::new(String::new());

/// 读最近一次判定出的模式（用途见 `LAST_DETERMINED_MODE` 的说明）
pub fn last_determined_mode() -> String {
    LAST_DETERMINED_MODE.lock().unwrap().clone()
}

// [cfgwatch]
pub fn watch_config_file(
    config_arc: Arc<Mutex<RulesConfig>>,
    force_refresh_arc: Arc<AtomicBool>,
    tx: SyncSender<DaemonEvent>,
) -> Result<(), Box<dyn Error>> {
    let mut inotify = Inotify::init()?;
    let rules_path = config::get_rules_path();
    if !rules_path.exists() {
        // 兜底：快照缺失时直接用嵌入内容落盘（正常路径由 main.rs::sync_rules_snapshot 完成）
        let _ = utils::try_write_file(&rules_path, crate::common::embedded_rules_str());
    }
    // 监听 rules.yaml 所在目录而非文件本身：WebUI 用「临时文件 + 原子 mv」替换时，挂在旧inode 上的 watch 会永久失效，目录级 watch 才能感知 MOVED_TO；
    // 其余文件靠文件名过滤避免误触发
    let rules_dir = rules_path.parent().ok_or("invalid rules.yaml path")?;
    inotify.watches().add(
        rules_dir,
        WatchMask::MODIFY | WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO,
    )?;
    info!(
        "{}",
        t_with_args(
            "app-detect-config-watch",
            &fluent_args!("path" => format!("{:?}", rules_path))
        )
    );
    let mut buffer = [0u8; 1024];
    loop {
        let events = inotify.read_events_blocking(&mut buffer)?;
        // 只处理 rules.yaml 自身的事件（原子替换为 MOVED_TO，直接改写为 MODIFY/CLOSE_WRITE）；events 只实现 IntoIterator，此处直接消费
        let rules_changed = events
            .into_iter()
            .any(|ev| ev.name.as_deref() == Some(std::ffi::OsStr::new("rules.yaml")));
        if rules_changed {
            info!("{}", t("app-detect-change-detected"));
            thread::sleep(Duration::from_millis(100));
            while let Ok(events) = inotify.read_events(&mut buffer) {
                if events.peekable().peek().is_none() {
                    break;
                }
            }
            info!("{}", t("app-detect-reloading"));

            // rules.yaml 运行时一律读嵌入内容（编译期打包防篡改）；磁盘文件仅是启动时复制的展示副本，改写不影响重载结果
            let new_config = crate::common::embedded_rules();

            *config_arc.lock().unwrap() = new_config.clone();

            if let Err(e) = tx.send(DaemonEvent::ConfigReload(Box::new(new_config))) {
                warn!("[Config] Failed to send ConfigReload event: {}", e);
            }

            info!("{}", t("app-detect-reload-success"));
            force_refresh_arc.store(true, Ordering::SeqCst);
        }
    }
}

// [loop]
/// `pid_tx`：前台 PID 变化广播源（cpu_monitor / fps_monitor 消费同一 watch 通道）替代原mod
/// rs 500ms 轮询的 pid_watcher 线程——`CURRENT_PID` 只有 set_current_package 一个写入点，变化即推送
pub fn app_detection_loop(
    config_arc: Arc<Mutex<RulesConfig>>,
    screen_state_arc: Arc<Mutex<bool>>,
    force_refresh_arc: Arc<AtomicBool>,
    tx: SyncSender<DaemonEvent>,
    pid_tx: watch::Sender<u32>,
) -> Result<(), Box<dyn Error>> {
    info!("{}", t("app-detect-loop-started"));

    let temp_sensor_path = utils::find_cpu_temp_path().unwrap_or_default();
    let mut last_package = String::new();
    let mut last_process_starttime: Option<u64> = None;
    let mut last_mode = String::new();
    let mut last_activity: Arc<str> = Arc::from("");
    let mut last_screen_state = true;

    // 状态机变量：用于无阻塞防抖
    let mut pending_package = String::new();
    let mut pending_pid = 0;
    let mut debounce_start = Instant::now();

    loop {
        // 屏幕状态自愈：属性轮询线程可能漏判（启动早期属性未就绪、线程停滞），先按debug.tracing.screen_state 校正一次，保证事件与真实屏幕一致（避免亮屏期间 scenemode 误计时、
        // 亮屏后无法退出）
        super::screen_detect::verify_screen_state(&screen_state_arc);
        // 两条刷新来源合并：规则热重载（watch_config_file）与实验室套用/还原（request_mode_refresh），任一为真都重算模式
        let force_refresh = force_refresh_arc.swap(false, Ordering::SeqCst)
            | FORCE_MODE_REFRESH.swap(false, Ordering::SeqCst);
        let current_screen_state = { *screen_state_arc.lock().unwrap() };

        if current_screen_state != last_screen_state {
            info!(
                "{}",
                t_with_args(
                    "app-detect-screen-changed",
                    &fluent_args!("old" => last_screen_state.to_string(), "new" => current_screen_state.to_string())
                )
            );
            last_screen_state = current_screen_state;
            let _ = tx.send(DaemonEvent::ScreenStateChange(current_screen_state));

            if current_screen_state {
                last_package.clear();
                last_process_starttime = None;
                pending_package.clear();
                last_mode.clear();
                last_activity = Arc::from("");
                *LAST_DETERMINED_MODE.lock().unwrap() = String::new();
                force_refresh_arc.store(true, Ordering::SeqCst);
            }
        }

        if !current_screen_state {
            set_raw_foreground_package("");
            last_activity = Arc::from("");
            thread::sleep(Duration::from_secs(1));
            continue;
        }

        // 只克隆本轮需要的 ignored_apps，**不整份克隆 RulesConfig**：app_modes 是 HashMap，整份克隆每轮复制整张表，而它只在包名变化/强制刷新轮次才用得上
        // guard 不跨文件读持有——cgroup 扫描是文件 IO，拉长持锁会让 config_watcher 写入空等
        let (ignored_apps, activity_rules) = {
            let cfg = config_arc.lock().unwrap();
            let activity_rules = cfg.dynamic_enabled
                && cfg.activity_aware
                && cfg.app_modes.keys().any(|key| key.contains('/'));
            (cfg.ignored_apps.clone(), activity_rules)
        };
        ACTIVITY_RULES_ACTIVE.store(activity_rules, Ordering::Relaxed);

        let (detected_pkg, detected_pid) = match get_focused_app_from_cgroup(&ignored_apps) {
            Ok(focused) => focused,
            Err(_) => {
                // 检测失败：清空 raw 提示（Unknown），不沿用旧包名让窗口核验拿旧候选续命
                set_raw_foreground_package("");
                (last_package.clone(), get_current_pid())
            }
        };

        let mut final_pkg = last_package.clone();
        let mut final_pid = confirmed_foreground_pid(
            &last_package,
            &detected_pkg,
            detected_pid,
            get_current_pid(),
        );

        // 无阻塞防抖逻辑
        if detected_pkg != last_package && !detected_pkg.is_empty() {
            if detected_pkg != pending_package || detected_pid != pending_pid {
                pending_package = detected_pkg.clone();
                pending_pid = detected_pid;
                debounce_start = Instant::now();
                debug!(
                    "{}",
                    t_with_args(
                        "app-detect-debounce-start",
                        &fluent_args!(
                            "pkg" => pending_package.as_str(),
                            "pid" => pending_pid.to_string()
                        )
                    )
                );
            } else if debounce_start.elapsed() >= Duration::from_millis(500) {
                final_pkg = pending_package.clone();
                final_pid = pending_pid;
                pending_package.clear();
                debug!(
                    "{}",
                    t_with_args(
                        "app-detect-debounce-confirmed",
                        &fluent_args!(
                            "pkg" => final_pkg.as_str(),
                            "pid" => final_pid.to_string()
                        )
                    )
                );
            }
        } else {
            pending_package.clear();
        }

        let current_temp = if !temp_sensor_path.is_empty() {
            utils::read_f64_from_file(&temp_sensor_path).unwrap_or(0.0) / 1000.0
        } else {
            0.0
        };

        let prev_pid = get_current_pid();
        // 低频路径：本轮回合只读一次 starttime（仅有效 PID），用于同包同 PID 的 ABA 判定，不新增高频扫描
        let final_starttime = if final_pid > 0 {
            read_process_starttime(final_pid)
        } else {
            None
        };
        let identity_changed = base_package(&final_pkg) != last_package
            || final_pid != prev_pid
            || final_starttime != last_process_starttime;

        // Activity 采样结果只接受属于当前前台包的键：LAST_ACTIVITY 是全局焦点窗口，换包瞬间
        // 可能仍是旧包（采样器尚未刷新），系统叠加层也可能短暂抢焦点。不校验前缀会让新包
        // 命中旧包的 activity 规则（activity_lookup 直接按整键查表）。
        let observed_activity = activity_for_package(
            &crate::monitor::window_visibility::last_activity_key(),
            &final_pkg,
        );
        // 同包内 Activity 变化也要重算模式：Activity 级规则用在页面切换（聊天→视频通话、
        // 浏览→播放）。activity 不参与 identity/generation，故不能只靠 identity_changed。
        let activity_changed =
            activity_observation_changed(identity_changed, &observed_activity, &last_activity);

        if identity_changed || force_refresh || activity_changed {
            if !final_pkg.is_empty() {
                if activity_changed {
                    debug!(
                        "{}",
                        t_with_args(
                            "app-detect-activity-changed",
                            &fluent_args!(
                                "pkg" => final_pkg.as_str(),
                                "activity" => observed_activity.to_string()
                            )
                        )
                    );
                }
                if identity_changed || force_refresh {
                    debug!(
                        "{}",
                        t_with_args(
                            "app-detect-pkg-change",
                            &fluent_args!(
                                "pkg" => final_pkg.as_str(),
                                "pid" => final_pid.to_string(),
                                "temp" => format!("{:.1}", current_temp),
                                "force" => force_refresh.to_string()
                            )
                        )
                    );
                }
                // 同身份强制刷新也递增 generation：旧 mode 事件不会被后续快照接受
                let snapshot = set_current_package(
                    &final_pkg,
                    final_pid,
                    final_starttime,
                    force_refresh,
                    &observed_activity,
                );
                // 前台 PID 变化即时广播（cpu_monitor/fps 共用）：仅 pid 真正变化且有效时发
                if final_pid != prev_pid && final_pid > 0 {
                    debug!(
                        "{}",
                        t_with_args(
                            "cpu-monitor-fg-pid-updated",
                            &fluent_args!(
                                "old" => prev_pid.to_string(),
                                "new" => final_pid.to_string()
                            )
                        )
                    );
                    let _ = pid_tx.send(final_pid as u32);
                }
                // 模式判定要读整份规则（global_mode/app_modes/dynamic_enabled/activity_aware），取一次短锁即可；guard 不可跨上方 cgroup 扫描持有，
                // 也不缓存整份配置——稳态每轮不碰锁
                let new_mode = {
                    let cfg = config_arc.lock().unwrap();
                    determine_mode(&cfg, &final_pkg, &observed_activity)
                };

                // force_refresh 只驱动重算模式，模式未变不重发 ModeChange（避免 default->default 冗余）；同模式应用切换对 CLG 无影响，
                // 但 ChiRi FAS 需热切换 uprobe 目标，故下方补发 PackageSwitch
                if last_mode != new_mode {
                    info!(
                        "{}",
                        t_with_args(
                            "app-detect-mode-change-pkg",
                            &fluent_args!("old" => last_mode.clone(), "new" => new_mode.as_str(), "pkg" => final_pkg.as_str())
                        )
                    );
                    let _ = tx.send(DaemonEvent::ModeChange {
                        package_name: final_pkg.clone(),
                        pid: final_pid,
                        mode: new_mode.clone(),
                        temperature: current_temp,
                        detection_generation: snapshot.generation,
                    });
                    last_mode = new_mode;
                    // 同步镜像：DOWN 停摆退出时调度线程按它对齐接管
                    *LAST_DETERMINED_MODE.lock().unwrap() = last_mode.clone();
                } else if crate::common::is_chiri_soc()
                    && !last_package.is_empty()
                    && identity_changed
                {
                    // 同模式前台切换（ChiRi 专属）：补发 PackageSwitch 供 FAS 切换 uprobe 目标；首轮与亮屏后 last_package 为空不触发
                    let _ = tx.send(DaemonEvent::PackageSwitch {
                        package_name: final_pkg.clone(),
                        pid: final_pid,
                        detection_generation: snapshot.generation,
                    });
                }
                last_package = base_package(&final_pkg).to_string();
                last_process_starttime = final_starttime;
            } else {
                debug!("{}", t("app-detect-no-app"));
            }
            last_activity = observed_activity;
        }

        thread::sleep(Duration::from_millis(1500));
    }
}
