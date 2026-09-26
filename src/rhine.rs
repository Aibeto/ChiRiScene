//! rhine.rs: [consts] [defs] [state] [backup] [lock] [flow] [startup] [watch]
//!
//! 实验室（rhine）：模块内置的实验性调度模式开关，ChiRi 专属。三个文件各管一件事：
//! - `rhine-init.yaml`（编译期嵌入、不落盘）：模式定义；**缺省条目 = 该项不变更**。
//! - `rhine.chr`（模块根，对外暴露）：实验室状态，内容即模式 key；空/只有注释 = 未启用。
//! - `rhine-back.chr`（daemon 生成）：套用前一刻的原值快照，还原完即删；也是「上次启用过实验室」
//!   的唯一信号——开机时 rhine.chr 已被 service.sh 清空，光看它判断不出要不要还原。
//!
//! 落地两路：`fas_enabled`/`scenemode_enabled` 直接改写 meta.yaml（WebUI 开关同步显示）；
//! `global_mode`/`special_tuned` 编译期嵌入、写文件无效，只能走运行时覆盖层（见 common.rs）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::common;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};
use crate::utils;

// [consts]
/// 对外暴露的实验室状态文件（模块根）
pub const RHINE_CHR: &str = "rhine.chr";
/// 原值快照（模块根），仅在实验室套用期间存在
pub const RHINE_BACK_CHR: &str = "rhine-back.chr";

/// 随模块下发的 rhine.chr 内容，也是「文件缺失 / 内容非法」时的兜底：只有注释，解析即未启用。
const RHINE_DEFAULT_CHR: &str = include_str!("../module/rhine.chr");

/// 监听异常后的重建间隔（与 chiri config_watcher 同口径）
const WATCH_RETRY_BACKOFF: Duration = Duration::from_secs(2);

// [defs]
/// 单个实验室模式的影响项。全是 Option：**缺省 = 该项不变更**。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RhineModeDef {
    /// 运行时覆盖 rules.yaml 的 global_mode 兜底值
    #[serde(default)]
    pub global_mode: Option<String>,
    /// 改写生效 meta.yaml 的 fas_enabled
    #[serde(default)]
    pub fas_enabled: Option<bool>,
    /// 改写生效 meta.yaml 的 scenemode_enabled
    #[serde(default)]
    pub scenemode_enabled: Option<bool>,
    /// 运行时开关：false = 关闭所有场景特调（akmode 白名单判定失效）
    #[serde(default)]
    pub special_tuned: Option<bool>,
/// 改写生效 meta.yaml 的 thread_bind（实验室 frozen 专用）：false = 交还线程亲和/绑核与 core_ctl
    #[serde(default)]
    pub thread_bind: Option<bool>,
}

static DEFS: OnceLock<HashMap<String, RhineModeDef>> = OnceLock::new();

/// 全部实验室模式定义（来自嵌入的 rhine-init.yaml）；嵌入内容损坏时退化为空表——任何 key 都判非法，rhine.chr 兜底成未启用。
fn defs() -> &'static HashMap<String, RhineModeDef> {
    DEFS.get_or_init(|| {
        serde_yaml::from_str::<HashMap<String, RhineModeDef>>(common::embedded_rhine_init_str())
            .unwrap_or_default()
    })
}

/// 模式 key 是否在内嵌定义里（WebUI 侧另有一份硬编码列表，新增模式时两边都要改）
fn knows_mode(key: &str) -> bool {
    defs().contains_key(key)
}

// [state]
/// rhine.chr 的解析结果
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// 空文件 / 只有注释：实验室未启用
    Off,
    /// 已启用，值为模式 key
    On(String),
/// 显式强制关闭（保留字 `off`）：清掉锁定标记后按正常路径还原；空文件在锁定期间会被挡回。
    ForceOff,
    /// 内容不是合法 YAML 标量，或写了未定义的模式名
    Bad,
}

impl State {
    fn key(&self) -> Option<String> {
        match self {
            State::On(k) => Some(k.clone()),
            _ => None,
        }
    }
}

/// 是否强制关闭的写法（保留字 `off`，允许带引号、忽略大小写）。`off` 在 YAML 里是布尔字面量、
/// 各实现口径不一，**必须先做字面量比较、不能交给类型推断**；`off` 是保留字，模式 key 不得取用。
fn is_force_off(raw: &str) -> bool {
    // 行内注释先剥（规则与 YAML 一致，` #` 才算）：否则 `off # 注释` 会走 YAML 分支被判非法重置，与界面的「强制关闭」结论分叉。
    let body = match raw.find(" #") {
        Some(at) => &raw[..at],
        None => raw,
    };
    body.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .eq_ignore_ascii_case("off")
}

/// 解析 rhine.chr 内容。空行与 `#` 注释行先剥掉——文件允许只写注释（语义为空）。
fn parse_state(text: &str) -> State {
    let body: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if body.is_empty() {
        return State::Off;
    }
    if body.len() == 1 && is_force_off(body[0]) {
        return State::ForceOff;
    }
    match serde_yaml::from_str::<String>(&body.join("\n")) {
        Ok(key) if knows_mode(&key) => State::On(key),
        _ => State::Bad,
    }
}

/// rhine.chr 在本轮被就地修复的方式；启动期那次早于 logger::init、日志会被丢，打点只能带出来由调用方补打。
#[derive(Debug, Clone)]
pub enum StateFix {
    /// 文件缺失，补建了内置默认内容
    Created,
    /// 内容非法，覆盖为内置默认内容（附截断后的原文，便于定位用户写了什么）
    Reset(String),
}

/// 读取 rhine.chr 并就地兜底（缺失补建 / 非法覆盖为内置默认），两种情况都当未启用返回；第二项是本轮修复动作。
fn read_state(root: &Path) -> (State, Option<StateFix>) {
    let path = root.join(RHINE_CHR);
    let Ok(text) = fs::read_to_string(&path) else {
        // 缺失（首次安装 / 被误删）：补建内置默认内容，让用户看得到该文件与格式说明
        let wrote = common::write_file_no_panic(&path, RHINE_DEFAULT_CHR.as_bytes());
        return (State::Off, wrote.then_some(StateFix::Created));
    };
    match parse_state(&text) {
        State::Bad => {
            let raw: String = text.trim().chars().take(64).collect();
            let wrote = common::write_file_no_panic(&path, RHINE_DEFAULT_CHR.as_bytes());
            (State::Off, wrote.then_some(StateFix::Reset(raw)))
        }
        s => (s, None),
    }
}

/// rhine.chr 修复动作打点（启动期那一路由 main 在 logger::init 之后补打）
fn log_state_fix(fix: &Option<StateFix>) {
    match fix {
        Some(StateFix::Created) => info!("{}", t("rhine-state-created")),
        Some(StateFix::Reset(raw)) => warn!(
            "{}",
            t_with_args(
                "rhine-state-invalid",
                &fluent_args!("value" => raw.as_str())
            )
        ),
        None => {}
    }
}

// [backup]
/// rhine-back.chr 的内容：套用前一刻的原值；还原**只处理出现过的字段**（缺省字段 = 未被实验室改动）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    /// 触发本次快照的实验室模式 key（仅供排查时对照）
    origin: String,
    #[serde(default)]
    global_mode: Option<String>,
    #[serde(default)]
    fas_enabled: Option<bool>,
    #[serde(default)]
    scenemode_enabled: Option<bool>,
    #[serde(default)]
    special_tuned: Option<bool>,
    #[serde(default)]
    thread_bind: Option<bool>,
}

fn write_backup(root: &Path, backup: &Backup) -> bool {
    let body = match serde_yaml::to_string(backup) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut text = String::from(
        "# rhine-back.chr: 实验室启用前一刻的原值快照，还原后本文件即删除。\n\
         # 没出现的字段表示该项没被实验室改动过，还原时不动它。\n",
    );
    text.push_str(&body);
    common::write_file_no_panic(&root.join(RHINE_BACK_CHR), text.as_bytes())
}

/// 当前生效的全局模式（无实验室覆盖时即 rules.yaml 里的值）
fn current_global_mode() -> String {
    common::lab_global_mode().unwrap_or_else(|| common::embedded_rules().global_mode.clone())
}

// [lock]
/// 实验室锁定标记文件名：存在即「本次开机后启用过实验室」，运行时禁止关闭；落在 tmpfs 上，重启即消失——这就是判据。
pub const LOCK_NAME: &str = "chiri-labs.lock";
/// 候选目录按序探测（/dev 是 Android 必有的 tmpfs）；WebUI 按同一顺序探测，见 webui/src/contract/lab.ts。
const LOCK_DIRS: [&str; 2] = ["/tmp", "/dev"];

/// 痕迹代号（写进标记，WebUI 按 `lab.lock.note.*` 映射成文案）。刻意用代号而非现成文案：
/// 写痕迹可能发生在 load_language 之前，i18n 未就绪时 t() 只会原样返回 key。
const NOTE_STATE_RESET: &str = "state-reset";
const NOTE_CLOSE_REJECTED: &str = "close-rejected";
const NOTE_BACKUP_REBUILT: &str = "backup-rebuilt";

static LOCK_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 锁定标记的落点，首次调用探测一次并缓存：已有标记 → 可用；否则写探测文件试可写性（绝不碰真实标记）。
/// 两处都不可写返回 None：实验室照常能开关，只是丢了「防误关」保护，只 warn 一次、不阻断功能。
fn lock_path() -> Option<&'static Path> {
    LOCK_PATH
        .get_or_init(|| {
            for dir in LOCK_DIRS {
                let target = Path::new(dir).join(LOCK_NAME);
                if target.exists() {
                    return Some(target);
                }
                let probe = Path::new(dir).join(".chiri-labs.probe");
                if common::write_file_no_panic(&probe, b"x") {
                    let _ = fs::remove_file(&probe);
                    return Some(target);
                }
            }
            warn!("{}", t("rhine-lock-unavailable"));
            None
        })
        .as_deref()
}

fn lock_exists() -> bool {
    lock_path().is_some_and(|p| p.exists())
}

/// 写标记：第一行是模式名，其后每行 `# 记录`（异常痕迹，供界面提示）；重启后随 tmpfs 消失。
fn write_lock(mode: &str, notes: &[String]) -> bool {
    let Some(p) = lock_path() else {
        return false;
    };
    let mut text = format!("{mode}\n");
    for n in notes {
        text.push_str("# ");
        text.push_str(n);
        text.push('\n');
    }
    common::write_file_no_panic(p, text.as_bytes())
}

/// 标记里记录的模式（第一行）。文件不存在 / 内容不是已知模式 → None。
fn lock_mode() -> Option<String> {
    let text = fs::read_to_string(lock_path()?).ok()?;
    let first = text.lines().next()?.trim().to_string();
    knows_mode(&first).then_some(first)
}

/// 标记里已有的异常记录
fn lock_notes() -> Vec<String> {
    let Some(p) = lock_path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(p) else {
        return Vec::new();
    };
    text.lines()
        .skip(1)
        .filter_map(|l| l.trim().strip_prefix('#').map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// 追加一条异常记录（模式行保留；重复的忽略，最多 8 条防刷爆）
fn lock_note(note: &str) {
    let Some(mode) = lock_mode() else {
        return;
    };
    let mut notes = lock_notes();
    if notes.iter().any(|n| n == note) || notes.len() >= 8 {
        return;
    }
    notes.push(note.to_string());
    write_lock(&mode, &notes);
}

fn clear_lock() {
    if let Some(p) = lock_path() {
        let _ = fs::remove_file(p);
    }
}

// [flow]
/// 套用某个实验室模式：备份原值 → 改 meta.yaml → 置运行时覆盖；返回 Err 保证「什么都没有生效」（meta 写失败发生在置运行时覆盖之前、且先撤掉快照，不存在改一半的中间态）。
fn apply(root: &Path, meta_path: &Path, key: &str) -> Result<(), String> {
    let Some(def) = defs().get(key) else {
        return Err(format!("unknown lab mode: {key}"));
    };
// fas/scenemode 的原值从磁盘 meta.yaml 读（与 Config::load 同源）；读不到就放弃套用——宁可没生效，也不要还原不了的改动
    let Some(meta) = common::read_external_meta(meta_path) else {
        return Err(format!("meta.yaml unreadable: {}", meta_path.display()));
    };

    // 快照必须记「当前生效值」：文件没写该键时等于内嵌默认（缺省 = 沿用）
    let d = common::embedded_meta_defaults();
    let eff_fas = meta.fas_enabled.or(d.fas_enabled).unwrap_or(true);
    let eff_scenemode = meta
        .scenemode_enabled
        .or(d.scenemode_enabled)
        .unwrap_or(true);
    let eff_thread = meta.thread_bind.or(d.thread_bind).unwrap_or(true);

    let backup = Backup {
        origin: key.to_string(),
        global_mode: def.global_mode.as_ref().map(|_| current_global_mode()),
        fas_enabled: def.fas_enabled.map(|_| eff_fas),
        scenemode_enabled: def.scenemode_enabled.map(|_| eff_scenemode),
        special_tuned: def
            .special_tuned
            .map(|_| !common::lab_special_tuned_disabled()),
        thread_bind: def.thread_bind.map(|_| eff_thread),
    };
    if !write_backup(root, &backup) {
        return Err("failed to write backup".to_string());
    }

    if !common::rewrite_meta_toggles(
        meta_path,
        def.fas_enabled,
        def.scenemode_enabled,
        def.thread_bind,
    ) {
        // 还没套用任何运行时覆盖，撤掉快照即可全身而退
        let _ = fs::remove_file(root.join(RHINE_BACK_CHR));
        return Err("failed to rewrite meta.yaml".to_string());
    }
    if let Some(mode) = &def.global_mode {
        common::set_lab_global_mode(Some(mode.clone()));
    }
    if let Some(enabled) = def.special_tuned {
        common::set_lab_special_tuned_disabled(!enabled);
    }
    Ok(())
}

/// 读回快照（不存在 / 非法 → None）。换模式回落时用它取「启用前的原值」。
fn read_backup(root: &Path) -> Option<Backup> {
    let text = fs::read_to_string(root.join(RHINE_BACK_CHR)).ok()?;
    serde_yaml::from_str::<Backup>(&text).ok()
}

/// 锁定期间的重新断言：把 meta.yaml 三个开关与运行时覆盖按定义装回去。**绝不碰 rhine-back.chr**——
/// 此刻 meta 已是实验室改过的值，重写快照会把改过的值记成原值，之后每还原一次偏一次。
/// `prev` = 切换前模式（None 或与 `key` 相同 = 单纯重申）；换模式时，上一模式改过、新模式不涉及的项按快照回落原值。
fn reassert(root: &Path, meta_path: &Path, key: &str, prev: Option<&str>) -> Result<(), String> {
    let Some(def) = defs().get(key) else {
        return Err(format!("unknown lab mode: {key}"));
    };
    let prev_def = prev.filter(|p| *p != key).and_then(|p| defs().get(p));
    let snap = prev_def.and_then(|_| read_backup(root));
    // 新值优先；新模式不管而旧模式管过 → 回落快照值；两边都不管 → None（保持不变）
    let pick = |new: Option<bool>, prev_had: bool, snap_val: Option<bool>| match new {
        Some(v) => Some(v),
        None if prev_had => snap_val,
        None => None,
    };
    let (fas, scenemode, thread_bind) = match prev_def {
        Some(pd) => (
            pick(
                def.fas_enabled,
                pd.fas_enabled.is_some(),
                snap.as_ref().and_then(|s| s.fas_enabled),
            ),
            pick(
                def.scenemode_enabled,
                pd.scenemode_enabled.is_some(),
                snap.as_ref().and_then(|s| s.scenemode_enabled),
            ),
            pick(
                def.thread_bind,
                pd.thread_bind.is_some(),
                snap.as_ref().and_then(|s| s.thread_bind),
            ),
        ),
        None => (def.fas_enabled, def.scenemode_enabled, def.thread_bind),
    };
    if !common::rewrite_meta_toggles(meta_path, fas, scenemode, thread_bind) {
        return Err("failed to rewrite meta.yaml".to_string());
    }
    match &def.global_mode {
        Some(mode) => common::set_lab_global_mode(Some(mode.clone())),
        // 新模式不管全局模式、旧模式管过 → 交回进程内覆盖（回落）
        None if prev_def.is_some_and(|p| p.global_mode.is_some()) => {
            common::set_lab_global_mode(None)
        }
        None => {}
    }
    match def.special_tuned {
        Some(enabled) => common::set_lab_special_tuned_disabled(!enabled),
        None if prev_def.is_some_and(|p| p.special_tuned.is_some()) => {
            common::set_lab_special_tuned_disabled(false)
        }
        None => {}
    }
    Ok(())
}

/// 快照丢了时的兜底：用内嵌默认补一份，保证重启后至少能回到出厂设定，并留痕迹让界面提示用户。
fn write_fallback_backup(root: &Path, key: &str) {
    let Some(def) = defs().get(key) else {
        return;
    };
    let d = common::embedded_meta_defaults();
    let backup = Backup {
        origin: key.to_string(),
        global_mode: def
            .global_mode
            .as_ref()
            .map(|_| common::embedded_rules().global_mode.clone()),
        fas_enabled: def.fas_enabled.map(|_| d.fas_enabled.unwrap_or(true)),
        scenemode_enabled: def
            .scenemode_enabled
            .map(|_| d.scenemode_enabled.unwrap_or(true)),
        special_tuned: def.special_tuned.map(|_| true),
        thread_bind: def.thread_bind.map(|_| d.thread_bind.unwrap_or(true)),
    };
    if write_backup(root, &backup) {
        lock_note(NOTE_BACKUP_REBUILT);
        warn!("{}", t("rhine-lock-note-no-backup"));
    }
}

/// 按 rhine-back.chr 还原原值并删除快照；没有快照 = 无事可做。快照非法（被改坏/截断）时用内嵌默认收尾：
/// 三个总闸回内嵌默认、运行时覆盖全清——最坏是回到出厂设定，不会卡在退不出的半覆盖状态。
fn restore(root: &Path, meta_path: &Path) -> Option<String> {
    let path = root.join(RHINE_BACK_CHR);
    if !path.exists() {
        // 没有快照也要清运行时覆盖：它是实验室自己的进程内状态、与快照不同步（快照被手删或
        // 强制关闭跳过补快照时就是这种情形），只有重启调度才消失。顺序安全：converge 永远先还原后套用。
        common::set_lab_global_mode(None);
        common::set_lab_special_tuned_disabled(false);
        return None;
    }
    let parsed = read_backup(root);

    let (origin, meta_ok) = match parsed {
        Some(backup) => {
            let ok = common::rewrite_meta_toggles(
                meta_path,
                backup.fas_enabled,
                backup.scenemode_enabled,
                backup.thread_bind,
            );
            // meta 写失败就不动运行时覆盖：两者是同一套改动，只清一半会留下半套用状态，下次重试更难对齐
            if ok {
                if backup.global_mode.is_some() {
                    common::set_lab_global_mode(None);
                }
                if backup.special_tuned.is_some() {
                    common::set_lab_special_tuned_disabled(false);
                }
            }
            (backup.origin, ok)
        }
        None => {
            warn!("{}", t("rhine-restore-invalid"));
            let defaults = common::embedded_meta_defaults();
            let ok = common::rewrite_meta_toggles(
                meta_path,
                defaults.fas_enabled.or(Some(true)),
                defaults.scenemode_enabled.or(Some(true)),
                defaults.thread_bind.or(Some(true)),
            );
            if ok {
                common::set_lab_global_mode(None);
                common::set_lab_special_tuned_disabled(false);
            }
            (String::new(), ok)
        }
    };
// meta.yaml 没还原成功就**保留快照**：原值只在快照里，删掉就再也回不去；留着幂等，下次启动会再走一遍还原。
    if meta_ok {
        let _ = fs::remove_file(&path);
        // 还原成功 = 实验室已关闭，锁定标记不该再留着（正常路径下它已随重启消失）
        clear_lock();
        Some(origin)
    } else {
        // 还原失败不报「已还原」：restored=None 才不会让日志与界面说谎
        warn!("{}", t("rhine-restore-meta-failed"));
        None
    }
}

/// 一次收敛的结果
#[derive(Debug, Default)]
struct Converged {
    /// 被还原的快照来源（origin）。空串表示快照非法、走的是内置默认值兜底
    restored: Option<String>,
    /// 成功套用的实验室模式
    enabled: Option<String>,
    /// rhine.chr 里写的模式（套用失败时用来回报「你想启用的是哪个」）
    wanted: Option<String>,
    /// 套用失败原因
    error: Option<String>,
    /// rhine.chr 本轮的修复动作（缺失补建 / 非法重置）
    state_fix: Option<StateFix>,
    /// 本次收敛时锁定标记有效（实验室启用过且未重启）
    locked: bool,
    /// 本次把「关闭请求」挡了回去（rhine.chr 被清空/改坏，锁定期间重新写回）
    refused: bool,
    /// 本次走的是强制关闭（rhine.chr 写了 off）：已清标记并按正常路径还原
    forced: bool,
}

/// 收敛到 rhine.chr 描述的目标状态：先还原旧快照，再决定要不要套用——顺序不能反。
/// 切模式 / 关闭都要先收回旧改动，否则备份里记的「原值」会变成旧模式改过的值，还原一次偏一次。
fn converge(root: &Path, meta_path: &Path) -> Converged {
    let (state, state_fix) = read_state(root);
    if matches!(state_fix, Some(StateFix::Reset(_))) {
        // 被改坏会被就地重置，留一条痕迹供界面提示「运行时可能异常」，别静默吞掉
        lock_note(NOTE_STATE_RESET);
    }
    converge_from(root, meta_path, state, state_fix)
}

/// 收敛主体：用调用方**已经读好的**状态，避免同一次文件事件被读两遍——watch 路径先 read_state
/// （内容非法会就地重置），这里再读一次只会看到重置出的「未启用」，把「内容非法」误判成「用户请求关闭」。
fn converge_from(
    root: &Path,
    meta_path: &Path,
    state: State,
    state_fix: Option<StateFix>,
) -> Converged {
    let mut out = Converged::default();
// 本轮这份「未启用」是刚被重置出来的：原因已记 state-reset，锁定分支不要再当关闭请求（写回仍要做）
    let just_reset = matches!(state_fix, Some(StateFix::Reset(_)));
    out.state_fix = state_fix;
// 保留字 `off` 是一次性的强制关闭请求：**无论有无锁定标记**都按正常路径还原，并在本轮写回「未启用」。
// 非锁定态也要写回——重启后标记已消失，但 off 还留在文件里，不写回界面会停在「已提交强制关闭」中间态。
    let force_off = matches!(state, State::ForceOff);
    if force_off {
        out.forced = true;
    }

    // ── 锁定期间：不接受关闭，也不做还原 ──
// 标记还在 = 本次开机后启用过实验室，关闭与还原只能靠重启；这里只做重新断言：拉回 rhine.chr 与运行时覆盖
    if lock_exists() {
        if force_off {
            // 强制关闭（保留字 off）：清掉标记后**不 return**，落到下面正常还原路径（恢复原地调度）。
            // 日志不在这里打（启动期早于 logger::init），统一由 log_converged 按 out.forced 输出。
            clear_lock();
        } else if let Some(key) = state.key().or_else(lock_mode) {
            // 锁定期间以**文件**为意图来源：文件写了模式就按它生效（换模式不拦），空/坏文件才退回
            // 标记记录（那是关闭请求，挡回）。标记只记「本次开机启用过」这个事实，不钉死模式。
            out.locked = true;
            // 切换前的模式要在 write_lock 覆盖标记第一行**之前**读出来（换模式回落要用）
            let prev = lock_mode();
            if !matches!(state, State::On(_)) {
                // 文件为空/被清空：把启用状态写回去（内容非法时文件已在 read_state 重置，写回同一模式 = 继续启用）
                common::write_file_no_panic(&root.join(RHINE_CHR), format!("{key}\n").as_bytes());
                if !just_reset {
                    // 只有「内容为空」才是关闭请求；刚被重置的已留过 state-reset，再补 close-rejected 会带偏排查
                    out.refused = true;
                    lock_note(NOTE_CLOSE_REJECTED);
                }
            } else if prev.as_deref() != Some(key.as_str()) {
                // 换模式：把标记第一行同步过来，保留已有痕迹
                write_lock(&key, &lock_notes());
            }
            if !root.join(RHINE_BACK_CHR).exists() {
                write_fallback_backup(root, &key);
            }
            out.wanted = Some(key.clone());
            match reassert(root, meta_path, &key, prev.as_deref()) {
                Ok(()) => out.enabled = Some(key),
                Err(e) => out.error = Some(e),
            }
            return out;
        } else {
            // 标记在，但标记与 rhine.chr 都说不出锁的是哪个模式：锁定对象已无从确定
            clear_lock();
            warn!("{}", t("rhine-lock-lost"));
        }
    }

    out.restored = restore(root, meta_path);
        // 强制关闭后写回「未启用」（内置模板只有注释）：off 是一次性请求，留在文件里会被反复解释、
        // 界面停在第三态。写回自身会触发一次监听事件，但那时 next == current == None，不会再收敛一轮。
    if force_off {
        common::write_file_no_panic(&root.join(RHINE_CHR), RHINE_DEFAULT_CHR.as_bytes());
    }
    out.wanted = state.key();
    if let Some(key) = out.wanted.clone() {
        match apply(root, meta_path, &key) {
            Ok(()) => {
                // 套用成功才建立锁定（清掉上一轮的痕迹）：它记录「本次开机后启用过」
                write_lock(&key, &[]);
                out.enabled = Some(key);
            }
            Err(e) => out.error = Some(e),
        }
    }
    out
}

/// 收敛结果打点。启动期还原发生在 logger::init 之前、日志会被丢弃，rhine.rs 内部不打 info：
/// 统一由 main 在 init 之后补打（StartupReport 带出字段），watcher 直接在这里打。
fn log_converged(c: &Converged) {
    log_state_fix(&c.state_fix);
    if c.forced {
        warn!("{}", t("rhine-force-off"));
    }
    if c.refused {
        warn!("{}", t("rhine-lock-refused"));
    }
    match c.restored.as_deref() {
        Some("") => warn!("{}", t("rhine-restore-invalid")),
        Some(origin) => info!(
            "{}",
            t_with_args("rhine-restored", &fluent_args!("origin" => origin))
        ),
        None => {}
    }
    match (&c.enabled, &c.wanted) {
        (Some(mode), _) => info!(
            "{}",
            t_with_args("rhine-mode-enabled", &fluent_args!("mode" => mode.as_str()))
        ),
        // 文件改成了「未启用」：还原过就说明确实收回了改动
        (None, None) if c.restored.as_deref().is_some_and(|o| !o.is_empty()) => {
            info!("{}", t("rhine-mode-disabled"))
        }
        _ => {}
    }
    if let (Some(mode), Some(error)) = (&c.wanted, &c.error) {
        warn!(
            "{}",
            t_with_args(
                "rhine-apply-failed",
                &fluent_args!("mode" => mode.as_str(), "error" => error.as_str())
            )
        );
    }
}

/// 通知 app_detect 重算模式：覆盖值改变后当前模式当场跟上，不用再切一次前台才看到效果。
fn refresh_mode() {
    crate::monitor::app_detect::request_mode_refresh();
}

// [startup]
/// 启动期处理结果（main 在 logger::init 之后补打点——归档/还原都发生在 init 之前）
#[derive(Debug, Default)]
pub struct StartupReport {
    /// 被还原的快照来源（origin）。None = 没有快照；空串 = 快照非法，走内置默认兜底
    pub restored: Option<String>,
    /// 本次启动后生效的实验室模式
    pub enabled: Option<String>,
    /// 套用失败：(rhine.chr 里写的模式, 原因)
    pub error: Option<(String, String)>,
    /// rhine.chr 本轮的修复动作（缺失补建 / 非法重置）
    pub state_fix: Option<StateFix>,
    /// 本次把「关闭请求」挡了回去（锁定期间 rhine.chr 被清空/改坏，已写回）
    pub refused: bool,
    /// 本次走的是强制关闭（rhine.chr 写了 off）：已清标记并按正常路径还原
    pub forced: bool,
}

/// 启动期处理，main 在首次 Config::load 之前调用（只有 Chiri 调用）。先还原后套用，三条路径自洽：
/// - 开机：service.sh 已清空 rhine.chr，只剩快照 → 只还原；
/// - 不停机重启调度：rhine.chr 与快照都在 → 先还原再重新套用、快照刷新，不会把改过的状态记成原值；
/// - 平时：两个文件都不在 → 什么都不做。
pub fn on_startup(root: &Path, meta_path: &Path) -> StartupReport {
    let c = converge(root, meta_path);
    let report = StartupReport {
        restored: c.restored.clone(),
        enabled: c.enabled.clone(),
        error: c
            .error
            .clone()
            .map(|e| (c.wanted.clone().unwrap_or_default(), e)),
        state_fix: c.state_fix.clone(),
        refused: c.refused,
        forced: c.forced,
    };
    if report.restored.is_some() || report.enabled.is_some() {
        refresh_mode();
    }
    report
}

/// 启动期结果打点，**必须在 logger::init 之后调用**——on_startup 发生在 init 之前，早打的日志会被丢弃。
pub fn report_startup(report: &StartupReport) {
    log_converged(&Converged {
        restored: report.restored.clone(),
        enabled: report.enabled.clone(),
        wanted: report.error.as_ref().map(|(m, _)| m.clone()),
        error: report.error.as_ref().map(|(_, e)| e.clone()),
        state_fix: report.state_fix.clone(),
        refused: report.refused,
        forced: report.forced,
        ..Default::default()
    });
}

// [watch]
/// 运行时监听模块根的 rhine.chr：写入即套用/还原，与 meta.yaml 热重载同语义、不用重启调度（只有 Chiri 起线程）。
/// `initial` 是启动期收敛后的状态，相同就不重复套用；监听异常时退避重建，与 chiri config_watcher 同口径。
pub fn watch_loop(root: PathBuf, meta_path: PathBuf, initial: Option<String>) {
    let mut current = initial;
    let mut watcher: Option<utils::DirWatcher> = None;
    loop {
        if watcher.is_none() {
            match utils::DirWatcher::new(&root) {
                Ok(w) => watcher = Some(w),
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args("rhine-watch-error", &fluent_args!("error" => e.to_string()))
                    );
                    std::thread::sleep(WATCH_RETRY_BACKOFF);
                    continue;
                }
            }
        }
        if let Err(e) = watcher.as_mut().unwrap().wait_change(RHINE_CHR) {
            warn!(
                "{}",
                t_with_args("rhine-watch-error", &fluent_args!("error" => e.to_string()))
            );
            watcher = None;
            std::thread::sleep(WATCH_RETRY_BACKOFF);
            continue;
        }

        let (state, state_fix) = read_state(&root);
        // 修复动作在这里打点（此时 logger 早就绪）；启动期那一路由 main 补打
        log_state_fix(&state_fix);
        // 运行时把文件写坏这一路：文件已在上面 read_state 里被重置，converge 再读只会看到「未启用」。
        // state-reset 痕迹要在这里补，否则标记会误记成「有人请求关闭」，与真实原因不符。
        if matches!(state_fix, Some(StateFix::Reset(_))) {
            lock_note(NOTE_STATE_RESET);
        }
        let next = state.key();
        if next == current {
            continue;
        }
        // 用上面读好的状态收敛：再读一次会读到刚被重置出的「未启用」，把「内容非法」误判成「关闭请求」（just_reset 为此）
        let converged = converge_from(&root, &meta_path, state, state_fix);
        log_converged(&converged);
        // 记下文件里写的值：套用失败也认这个值，等用户下次改动再试，不在这里空转重试
        current = converged.wanted;
        refresh_mode();
    }
}
