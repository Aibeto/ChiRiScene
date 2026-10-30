//! affinity.rs: [consts] [cpuset_io] [reserve_core] [sys_probe] [manager] [rebalance] [fg_promote]

/// CPU 亲和与线程迁移控制器（按核心粒度放置，低开销版）：cgroup 层 boost 类模式收窄 top-app/foreground cpuset 到 big∪prime、后台组压小核，可选 uclamp.min/max，normal/doze 恢复快照
/// 线程层前台关键线程组绑定、普通线程单核钉定（优先 big、满溢 prime）、后台忙线程 promote 到 big、demote 回落，过载重钉带分数滞回 + 反跳回冷却双闸防 8s 乒乓；选核 score = 逐核 util + 钉线程数×pinned_weight（缺省 0.4），黑名单 affinity_blacklist.yaml 编译嵌入
use crate::chiri::config::{AffinityConfig, AffinityTuningConfig};
use crate::chiri::energy_cost;
use crate::chiri::energy_model;
use crate::chiri::placement;
use crate::common::CoreGroup;
use crate::utils::{FastReader, SysPathExist};
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

#[path = "affinity_retry.rs"]
mod affinity_retry;
use affinity_retry::{InvalidMaskRetry, MissingNodeCache};
#[path = "affinity_cpuset.rs"]
mod affinity_cpuset;
use affinity_cpuset::{CpusetCheck, check_group_cpuset};

// [consts]
const KIND_NONE: u8 = 0;
const KIND_BOOST: u8 = 1;
const KIND_NORMAL: u8 = 2;

/// 线程被绑到性能核组（big∪prime）的原因；None = 未绑定，单一枚举取代 group_pinned+busy_bound 两布尔量，免去各绑定/释放路径手工同步
/// 的矛盾态而静默失效
#[derive(Clone, Copy, PartialEq)]
enum GroupBind {
    /// 未绑定（全核掩码）
    None,
    /// 关键线程：boost 组掩码兜底 / default 小核高水位 normal_press
    Key,
    /// default 小核高水位下因「忙」绑定（normal_busy），空闲后单独释放
    Busy,
}

const GROUP_TOP_APP: &str = "top-app";
const GROUP_FOREGROUND: &str = "foreground";
const BACKGROUND_GROUPS: [&str; 3] = ["background", "system-background", "restricted"];
/// 后台降权组（cpu.uclamp.max 钳制）：刻意不含 system-background——系统后台承载媒体/后台播放等可感知场景，只压纯应用后台与受限组
const BG_DEMOTE_GROUPS: [&str; 2] = ["background", "restricted"];

/// 后台组 cpu.uclamp.max 的「值未变不写」守卫状态：纯原子量，不进 apply 周期锁路径编码：0 = 未写入（首启必写）；1..=101 = pct+1；BG_UCLAMP_MAX_CODE = max
static BG_UCLAMP_CODE: AtomicU64 = AtomicU64::new(0);
/// 上次**实际**写入时刻（相对 `BG_UCLAMP_EPOCH` 的毫秒；`Instant` 不能进原子量）
static BG_UCLAMP_AT_MS: AtomicU64 = AtomicU64::new(0);
/// restricted 缺失节点冷却；目标值变化和恢复请求不清除
static RESTRICTED_UCLAMP_MISSING: MissingNodeCache = MissingNodeCache::new();
/// 计时基准（首次调用时初始化一次）
static BG_UCLAMP_EPOCH: OnceLock<Instant> = OnceLock::new();

/// `max` 的守卫编码（数值 pct 最大 100 → 编码最大 101，无冲突）
const BG_UCLAMP_MAX_CODE: u64 = u64::MAX;

/// 值未变时的强制重写间隔：ChiRi 是全局唯一调度程序，cgroup 被改写属异常态，60s 再断言兜底纠偏（非常态对抗），稳态写入从 ~1 次/s 降到 1 次/60s
const BG_UCLAMP_REASSERT: Duration = Duration::from_secs(60);

/// 写失败计数（汇总观测）：跨调用累加，rebalance 每 25 tick 读取并清零，纯原子量，不进锁路径
static MASK_WRITE_FAILS: AtomicU64 = AtomicU64::new(0);
static CPUSET_WRITE_FAILS: AtomicU64 = AtomicU64::new(0);
static UCLAMP_WRITE_FAILS: AtomicU64 = AtomicU64::new(0);
/// 汇总间隔（tick 数，tick ≈ 2s），与 CLG/tuned 的 25 tick 摘要同节奏
const WRITE_FAIL_SUMMARY_EVERY_TICKS: u64 = 25;

/// 值字符串 → 守卫编码；解析失败返回 0（= 必须写，退化为改造前行为）仅服务本文件两个调用点
fn bg_uclamp_code(val: &str) -> u64 {
    if val == "max" {
        return BG_UCLAMP_MAX_CODE;
    }
    val.split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|p| p as u64 + 1)
        .unwrap_or(0)
}

/// 写后台组 cpu.uclamp.max：候选组逐个写、不预判存在性（restricted 由 init 按需创建，写入失败即「该组不可用」的权威证据）
/// 每节点仅实写补 @A 帧；同值 60s 再断言，restricted ENOENT 冷却 600s，到期不受同值守卫阻挡
fn write_bg_uclamp_max(val: &str, reason: &str) {
    let code = bg_uclamp_code(val);
    let now = Instant::now();
    let now_ms = now
        .duration_since(*BG_UCLAMP_EPOCH.get_or_init(|| now))
        .as_millis() as u64;
    let last_ms = BG_UCLAMP_AT_MS.load(Ordering::Relaxed);
    let normal_write_due = !(code != 0
        && code == BG_UCLAMP_CODE.load(Ordering::Relaxed)
        && now_ms.saturating_sub(last_ms) < BG_UCLAMP_REASSERT.as_millis() as u64);
    let restricted_probe_due = RESTRICTED_UCLAMP_MISSING.probe_due(now_ms);
    if !normal_write_due && !restricted_probe_due {
        return;
    }
    let items: Vec<(String, String)> = BG_DEMOTE_GROUPS
        .iter()
        .filter(|group| {
            if **group == "restricted" {
                RESTRICTED_UCLAMP_MISSING.allows_write(normal_write_due, now_ms)
            } else {
                normal_write_due
            }
        })
        .map(|g| (format!("/dev/cpuctl/{g}/cpu.uclamp.max"), val.to_string()))
        .collect();
    let outcomes = crate::utils::write_nodes_verbose(&items, "bg-uclamp-max");
    for (path, outcome) in &outcomes {
        if path == "/dev/cpuctl/restricted/cpu.uclamp.max" {
            RESTRICTED_UCLAMP_MISSING.record(outcome.as_ref().err().copied().flatten(), now_ms);
        }
    }
    // 写失败计数（汇总观测），失败组仍在 @A 帧 e0/errno 可见
    UCLAMP_WRITE_FAILS.fetch_add(
        outcomes.iter().filter(|(_, o)| o.is_err()).count() as u64,
        Ordering::Relaxed,
    );
    // 状态在写之后更新；写失败也更新：同值下周期即跳过，60s 兜底再断言，失败组在 @A 帧 e0 可见
    if normal_write_due {
        BG_UCLAMP_CODE.store(code, Ordering::Relaxed);
        BG_UCLAMP_AT_MS.store(now_ms, Ordering::Relaxed);
    }
    // 打点与写入同门控：跳过写入时也不产生帧，否则同值重写仍会把 aff 文件刷满
    // result 写真实 errno（如 `e2`=ENOENT 节点缺失，机型无该组属常态），不一律记 `e0`，否则离线成功率表会把「半个候选组不存在」误读成 50% 写失败
    if crate::logger::diag_active() {
        for (path, outcome) in &outcomes {
            let code = match outcome {
                Ok(()) => String::from("ok"),
                Err(Some(n)) => format!("e{n}"),
                Err(None) => String::from("e0"),
            };
            crate::logger::aff_action("uclamp", 0, 0, "-", "-", path, val, &code, reason);
        }
    }
}

const REBALANCE_INTERVAL: Duration = Duration::from_secs(2);
/// 单轮最多深扫的后台候选线程数
const BG_SCAN_WINDOW: usize = 64;
const BG_LIST_EVERY_ROUNDS: u64 = 2;
const PROMOTED_REVIEW_EVERY_ROUNDS: u64 = 2;
const ONLINE_EVERY_ROUNDS: u64 = 4;

/// 线程迁移最小间隔：负载窗口已做两窗/3 连防抖，这里只挡边界抖动
/// 下界恒为现值，不得改小/绕过（config 缺省值引用本 const）
pub(crate) const MIN_MIGRATE_INTERVAL: Duration = Duration::from_secs(4);
/// 重钉分数滞回：目标核 score 须比 home 核低至少该值；无滞回时 util 噪声驱动两核乒乓
pub(crate) const OVERLOAD_MARGIN: f32 = 0.15;
/// 反跳回冷却：迁离某核后该时长内禁止迁回，打破 A→B→A 振荡（乒乓周期 8s，取两倍错开节奏）
/// 下界恒为现值，不得改小/绕过
pub(crate) const RETURN_COOLDOWN: Duration = Duration::from_secs(16);
/// 过载重钉防抖：重钉打断 cache/TLB 本地性比 promote 更明显，拉长间隔减少迁移
/// 下界恒为现值，不得改小/绕过
pub(crate) const REPIN_DEBOUNCE: Duration = Duration::from_secs(8);
/// 单核钉定连续失败的退避上限：核不可用是**状态**不是**事件**，超过这个间隔重试无收益
pub(crate) const PIN_FAIL_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// 进程级窄核域备忘的有效期：同 pid 同簇被内核拒过后，这段时间内不再为该进程的线程尝试
///
/// 取值是两难折中：太短则一个 boost 批次里几十上百个线程逐个各撞一次（实测单秒 431 条失败、
/// 全为同一个 pid 的线程钉同一颗核）；太长则进程真回到前台（cgroup cpuset 放宽）后
/// 提升迟迟不生效。15s 覆盖「同一场景的连续 boost 重试」，又能在场景稳定后自愈。
pub(crate) const NARROW_PID_TTL: Duration = Duration::from_secs(15);
/// 核心过载阈值：钉定核 util 超此值重钉到低占用核，分散负载压制峰值频率（前台/promoted 共用）
pub(crate) const CORE_OVERLOAD_UTIL: f32 = 0.70;

/// boost（含 fas）期重钉保守系数：debounce ×此值。锁频（min=max）下换核不省能，
/// 重钉只剩「压排队」一个收益，付迁移代价前必须比 default 更确定——防抖拉长 3 倍。
pub(crate) const BOOST_REPIN_SCALE: u32 = 3;
/// boost 期过载阈值增量：0.70→0.85，核没接近饱和就换核纯属白付迁移 + 缓存失效
pub(crate) const BOOST_OVERLOAD_EXTRA: f32 = 0.15;
/// 已钉线程对 score 的保守负载基准：util 快照滞后时仍能把后续线程推向其他核，避免挤同核
pub(crate) const PINNED_WEIGHT: f32 = 0.4;
/// 单核钉定上限：钉核目的是「分散」非「圈养」，钉满继续钉只会在同核排队；达上限的线程留 boost cpuset（big∪prime）内 EAS 自调度
pub(crate) const MAX_PINS_PER_CORE: u32 = 3;
/// overload_hold 打点冷却：滞回不满足时同一过载线程每轮都重复评估，每 tid 半分钟一条足以观测长期滞留
const HOLD_LOG_COOLDOWN: Duration = Duration::from_secs(30);
/// 后台 promote util 门槛（%）：可配（Affinity.tuning.promote_util_pct），A4：能效口径门槛（把百分比
/// 换成能效公式）无量值依据、且本项目永久禁止新增 A/B 对照实验，待真机观察后再定，本轮不做公式改写
pub(crate) const PROMOTE_UTIL_PCT: f32 = 25.0;
pub(crate) const LITTLE_HIGH_WATER: f32 = 0.70;
/// default 模式关键线程组绑定的解除水位（迟滞下沿，防乒乓）
pub(crate) const KEY_BIND_RELEASE_WATER: f32 = 0.50;
/// default 小核高水位时非关键前台线程「忙」阈值（%）：连续两窗达此值即抬 big∪prime；仅绑关键线程不足以解除小核饱和
pub(crate) const FG_BUSY_UTIL_PCT: f32 = 30.0;
/// 压力窗口内每轮最多采样 util 的非关键前台线程数（游标轮转），限制新增文件 IO
const FG_SCAN_WINDOW: usize = 32;
/// normal_busy 释放水位（%）：与绑定水位构成滞回防反复绑/放；沿用 DEMOTE_UTIL_PCT(5%)会让 5~30% 中等负载线程长期滞留性能核、能耗反升
pub(crate) const FG_BUSY_RELEASE_UTIL_PCT: f32 = 15.0;
pub(crate) const LITTLE_PROMOTE_UTIL_PCT: f32 = 10.0;
pub(crate) const BIG_HIGH_WATER: f32 = 0.90;
pub(crate) const DEMOTE_UTIL_PCT: f32 = 5.0;
pub(crate) const DEMOTE_STREAK: u32 = 3;
pub(crate) const THREAD_STALE: Duration = Duration::from_secs(30);

/// [eem] 每轮最多发起的线程迁移数：同轮内负载快照不会因迁移而更新，
/// 无上限会让大批线程同时判「同一簇最便宜」而齐涌（上一代放置内核 `max_inserts` 配额留下的教训）。
/// 4 条 / 2s 周期 = 每秒最多 2 条；迁移是慢动作，不需要更快。
const EEM_MAX_MOVES_PER_ROUND: u32 = 4;

const KEY_THREAD_COMMS: [&str; 5] = [
    "RenderThread",
    "GLThread",
    "GameThread",
    "UnityMain",
    "UnityGfxDeviceW",
];

// [cpuset_io]
fn is_key_thread(tid: i32, main_tid: i32, comm: &str) -> bool {
    tid == main_tid || KEY_THREAD_COMMS.iter().any(|k| *k == comm)
}

fn format_cpu_list(mut ids: Vec<usize>) -> String {
    ids.sort_unstable();
    ids.dedup();
    ids.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn fmt_home(home: i16) -> String {
    if home >= 0 {
        home.to_string()
    } else {
        "-".to_string()
    }
}

fn read_cpuset_cpus(group: &str) -> Option<String> {
    std::fs::read_to_string(format!("/dev/cpuset/{}/cpus", group))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 批量写 cpuset 组 cpus（键 = 组名，组是「同类多节点」）：单组失败 debug、全组失败才 warn；每次批量写补一条 @A cpuset_cpus 汇总帧
fn write_cpuset_cpus_items(items: &[(String, String)]) {
    let nodes: Vec<(String, String)> = items
        .iter()
        .map(|(group, value)| (format!("/dev/cpuset/{group}/cpus"), value.clone()))
        .collect();
    // 独立告警键：cpuset-cpus 与 cpuset-restore 是不同操作，共用一键会互相清掉失败记录、热路径反复报 warn
    let written = crate::utils::write_nodes(&nodes, "cpuset-cpus");
    // cpuset 写失败计数
    CPUSET_WRITE_FAILS.fetch_add(
        nodes
            .iter()
            .filter(|(p, _)| !written.iter().any(|w| w == p))
            .count() as u64,
        Ordering::Relaxed,
    );
    if !items.is_empty() && crate::logger::diag_active() {
        let keys: Vec<&str> = items.iter().map(|(g, _)| g.as_str()).collect();
        let all_ok = nodes.iter().all(|(p, _)| written.iter().any(|w| w == p));
        crate::logger::aff_action(
            "cpuset_cpus",
            0,
            0,
            "-",
            "-",
            &keys.join(","),
            &items.len().to_string(),
            if all_ok { "ok" } else { "e0" },
            "cpuset-cpus",
        );
    }
}

/// 批量写 cpuset（键 = 完整节点路径，快照回写用）：单点失败 debug、全部失败 warn （恢复期最怕全都没写回去）；补一条 @A cpuset_cpus 汇总帧（同上口径）
fn write_cpuset_paths_items(items: &[(String, String)]) {
    let written = crate::utils::write_nodes(items, "cpuset-restore");
    // cpuset 写失败计数
    CPUSET_WRITE_FAILS.fetch_add(
        items
            .iter()
            .filter(|(p, _)| !written.iter().any(|w| w == p))
            .count() as u64,
        Ordering::Relaxed,
    );
    if !items.is_empty() && crate::logger::diag_active() {
        let keys: Vec<&str> = items.iter().map(|(p, _)| p.as_str()).collect();
        let all_ok = items.iter().all(|(p, _)| written.iter().any(|w| w == p));
        crate::logger::aff_action(
            "cpuset_cpus",
            0,
            0,
            "-",
            "-",
            &keys.join(","),
            &items.len().to_string(),
            if all_ok { "ok" } else { "e0" },
            "cpuset-restore",
        );
    }
}

/// 读 cpuset 组 tasks（返回组内全部 TID），替代 /proc 全量枚举
fn read_cpuset_tasks(group: &str) -> Vec<i32> {
    let Ok(text) = std::fs::read_to_string(format!("/dev/cpuset/{}/tasks", group)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect()
}

// 专用核独占（scenemode）：sched_setaffinity 自钉不排他，真独占 = 把该核从全部业务cpuset 组的 cpus 中移除（新进程继承父组掩码同样被排除）；调度服务自身线程移入根组（含全部在线核，
// 钉定不被组掩码二次过滤）应用/框架任务全在业务组内，根组仅 init 等空闲守护进程，此为实践意义上的完全独占

// [reserve_core]
/// scenemode 期间被独占核排除的业务 cpuset 组（与 apply 的组口径一致）
const RESERVED_EXCLUDE_GROUPS: [&str; 5] = [
    "top-app",
    "foreground",
    "background",
    "system-background",
    "restricted",
];

/// 解析 cpuset cpus 值（"0-3,5" → [0,1,2,3,5]）；防御性限制 b < 1024
fn parse_cpu_list(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                if a <= b && b < 1024 {
                    out.extend(a..=b);
                }
            }
        } else if let Ok(a) = part.parse::<usize>() {
            out.push(a);
        }
    }
    out
}

/// 把 core 从全部业务 cpuset 组 cpus 移除；被改组 (组名, 原始 cpus) 记入 snapshot（不重复记录，防框架重写后的中间值覆盖真实原值）；周期重入纠偏（框架会加回，每 2s 重写）
pub(crate) fn exclude_core_from_cpusets(core: usize, snapshot: &mut Vec<(String, String)>) {
    let mut writes: Vec<(String, String)> = Vec::new();
    for group in RESERVED_EXCLUDE_GROUPS {
        let Some(cur) = read_cpuset_cpus(group) else {
            continue;
        };
        let mut set = parse_cpu_list(&cur);
        if !set.contains(&core) {
            continue;
        }
        set.retain(|&c| c != core);
        let new = format_cpu_list(set);
        if new == cur {
            continue;
        }
        writes.push((group.to_string(), new));
        if !snapshot.iter().any(|(g, _)| g == group) {
            snapshot.push((group.to_string(), cur));
        }
    }
    // 组间无先后依赖：一次批量写（单组失败 debug、全组失败 warn）
    write_cpuset_cpus_items(&writes);
}

/// 恢复 exclude_core_from_cpusets 快照（退出 scenemode 时把保留核还给业务组）
pub(crate) fn restore_excluded_cpusets(snapshot: Vec<(String, String)>) {
    write_cpuset_cpus_items(&snapshot);
}

// [lab_groups]
/// 实验室静态分组：组级写 cpus 并把原值记入快照（每组只记一次防中间值覆盖），组缺失/写失败跳过
fn set_groups_cpus_tracked(groups: &[&str], value: &str, snapshot: &mut Vec<(String, String)>) {
    let mut writes: Vec<(String, String)> = Vec::new();
    for group in groups {
        let Some(cur) = read_cpuset_cpus(group) else {
            continue;
        };
        if !snapshot.iter().any(|(g, _)| g == group) {
            snapshot.push((group.to_string(), cur));
        }
        writes.push((group.to_string(), value.to_string()));
    }
    write_cpuset_cpus_items(&writes);
}

/// 小核列表（babel 的后台组）
/// **空区间兜底**：两簇 SoC（SM8750/sun）没有小核簇，little 是 start == end 的空区间，
/// 此时 `end - 1` 会 usize 下溢、`format!` 出 "6-5" 这种非法 cpuset 值（写下去必 EINVAL）。
/// 没有小核可用时退化为 big 组——后台仍要落到某些核上，big 是唯一可选项。
fn little_list() -> String {
    let r = crate::common::chiri_core_ranges().little;
    if r.start < r.end {
        format!("{}-{}", r.start, r.end - 1)
    } else {
        big_list()
    }
}

/// 小核前两颗（contingency 的后台收敛目标：0-1 两颗小核）
/// 空/单核 little 同样退化到 big 组前两颗，理由同 [`little_list`]
fn little_pair_list() -> String {
    let r = crate::common::chiri_core_ranges().little;
    if r.start + 1 < r.end {
        format!("{}-{}", r.start, r.start + 1)
    } else {
        let b = crate::common::chiri_core_ranges().big;
        // big 也空属「配置错误」而非真实机型，saturating_sub 只为杜绝下溢 panic
        format!("{}-{}", b.start, (b.start + 1).min(b.end.saturating_sub(1)))
    }
}

/// 大核列表（babel 的系统进程组）
fn big_list() -> String {
    let r = crate::common::chiri_core_ranges().big;
    format!("{}-{}", r.start, r.end - 1)
}

/// 大核 + 超大核列表（babel 的前台/顶部组）；无 prime 的机型（8998）退化为大核
fn big_plus_prime_list() -> String {
    let r = crate::common::chiri_core_ranges();
    if r.prime.start < r.prime.end {
        format!("{}-{}", r.big.start, r.prime.end - 1)
    } else {
        format!("{}-{}", r.big.start, r.big.end - 1)
    }
}

/// 全核列表：读根组 `/dev/cpuset/cpus`（全部允许核），失败按机型区间拼 0-(最大核)
fn all_cores_list() -> String {
    read_cpuset_cpus("").unwrap_or_else(|| {
        let r = crate::common::chiri_core_ranges();
        let last = r.prime.end.max(r.big.end);
        format!("0-{}", last - 1)
    })
}

/// 自身所在 cpuset 组的相对路径（"/"=根组；None = 无 cpuset 层级/读取失败）
fn self_cpuset_group() -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in text.lines() {
        // v1 格式：hierarchy-ID:controller-list:cgroup-path
        let mut parts = line.splitn(3, ':');
        let _hier = parts.next()?;
        let ctrls = parts.next()?;
        let path = parts.next()?;
        if ctrls.split(',').any(|c| c.trim() == "cpuset") {
            return Some(path.trim().to_string());
        }
    }
    None
}

/// cpuset 组 tasks 节点路径：group_path 来自 /proc/self/cgroup（可能带前导斜杠），两侧都裁掉斜杠再拼接，兼容两种内核约定
fn cpuset_tasks_path(group_path: &str) -> String {
    let p = group_path.trim_matches('/');
    if p.is_empty() {
        "/dev/cpuset/tasks".to_string()
    } else {
        format!("/dev/cpuset/{p}/tasks")
    }
}

fn self_tids() -> Vec<i32> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            if let Some(t) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
                out.push(t);
            }
        }
    }
    out
}

/// 把守护进程自身全部线程移入 cpuset 根组（专用核钉定不被原组掩码二次过滤）；返回原组相对路径供退出恢复，None = 读不到原组（不移动，独占退化为尽力而为）
pub(crate) fn move_self_to_cpuset_root() -> Option<String> {
    let orig = self_cpuset_group()?;
    let mut last: std::io::Result<()> = Ok(());
    let mut n = 0u32;
    for tid in self_tids() {
        let r = std::fs::write("/dev/cpuset/tasks", tid.to_string());
        // 首个失败 errno 为准（后续成功不覆盖失败），与 cleanup_thread 汇总口径一致
        if last.is_ok() {
            last = r;
        }
        n += 1;
    }
    if crate::logger::diag_active() {
        let pid = std::process::id() as i32;
        crate::logger::aff_action(
            "move_group",
            pid,
            0,
            "-",
            "-",
            "root",
            &n.to_string(),
            &io_result_tag(&last),
            "self_root",
        );
    }
    // [silent_fail] 静默失败补日志：移组失败时返回值仍是 Some(orig)，与成功同形——调用方（core_ctl
    // scenemode）无法据此区分，且专用核自钉会被原组掩码二次过滤、独占退化为「尽力而为」且无自愈路径
    if let Err(e) = &last {
        error!(
            "{}",
            t_with_args(
                "affinity-self-root-failed",
                &fluent_args!("errno" => e.raw_os_error().unwrap_or(0).to_string())
            )
        );
    }
    Some(orig)
}

/// 把守护进程自身全部线程移回原 cpuset 组（退出 scenemode）
pub(crate) fn move_self_to_cpuset_group(group_path: &str) {
    let tasks = cpuset_tasks_path(group_path);
    let mut last: std::io::Result<()> = Ok(());
    let mut n = 0u32;
    for tid in self_tids() {
        let r = std::fs::write(&tasks, tid.to_string());
        if last.is_ok() {
            last = r;
        }
        n += 1;
    }
    if crate::logger::diag_active() {
        let pid = std::process::id() as i32;
        crate::logger::aff_action(
            "move_group",
            pid,
            0,
            "-",
            "-",
            group_path,
            &n.to_string(),
            &io_result_tag(&last),
            "self_group",
        );
    }
}

/// 设置单线程 CPU 亲和掩码：zeroed cpu_set_t 保证对齐/布局合法（vec![0u8] 起始对齐仅 1 不可靠），libc::CPU_SET 置位（bionic 64 位 = [u64;
/// 16] 支持 1024 CPU，越界跳过）；errno 在失败当刻取得供 @A 帧格式化供 core_ctl scenemode 专用核自钉复用
pub(crate) fn set_tid_affinity(tid: i32, cpu_ids: &[usize]) -> std::io::Result<()> {
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let max_cpu = std::mem::size_of::<libc::cpu_set_t>() * 8;
    for &c in cpu_ids {
        if c < max_cpu {
            unsafe { libc::CPU_SET(c, &mut mask) };
        }
    }
    // AppOptR 式短路：掩码一致时跳过 sched_setaffinity（周期路径高频命中）
    if let Some(curr) = read_tid_mask(tid) {
        let mut want: Vec<usize> = cpu_ids.iter().copied().filter(|&c| c < max_cpu).collect();
        want.sort_unstable();
        want.dedup();
        if curr == want {
            return Ok(());
        }
    }
    let ret =
        unsafe { libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &mask) };
    if ret == 0 {
        Ok(())
    } else {
        // 掩码写失败计数（rebalance 每 25 tick 汇总）
        MASK_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::last_os_error())
    }
}

/// 读单线程当前亲和掩码（sched_getaffinity）返回置位核号升序，失败（ESRCH 等）返回 None；快照与短路共用
pub fn read_tid_mask(tid: i32) -> Option<Vec<usize>> {
    // SAFETY: curr 为 zeroed 合法 cpu_set_t，sched_getaffinity 仅写入该缓冲；按字节位展开为核号，不依赖内部布局
    unsafe {
        let mut curr: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &mut curr) != 0 {
            return None;
        }
        let size = std::mem::size_of::<libc::cpu_set_t>();
        let bytes = std::slice::from_raw_parts(&curr as *const libc::cpu_set_t as *const u8, size);
        let mut out = Vec::new();
        for (i, b) in bytes.iter().enumerate() {
            for bit in 0..8 {
                if b & (1 << bit) != 0 {
                    out.push(i * 8 + bit);
                }
            }
        }
        Some(out)
    }
}

/// 写入/亲和结果 → `@A` 帧的 result 值（`ok` / `e{errno}`，errno 不可得时 `e0`）
pub(crate) fn io_result_tag(res: &std::io::Result<()>) -> String {
    match res {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("e{}", e.raw_os_error().unwrap_or(0)),
    }
}

// [sys_probe]
fn kernel_version() -> Option<(u32, u32, u32)> {
    let s = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let head = s.trim().split(['-', '+', ' ']).next()?;
    let mut it = head.split('.');
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
    ))
}

#[derive(PartialEq)]
enum UclampSupport {
    Unknown,
    Ok,
    Unsupported,
}

const UCLAMP_MAX_PATH: &str = "/dev/cpuctl/top-app/cpu.uclamp.max";

fn read_cmdline(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 读线程归属进程 PID（/proc/<tid>/status 的 Tgid 行），仅后台候选首见建档时读一次（不进周期路径）@S t 行 pid=0 既有语义即「归属未知」，读不到保持 0、不臆造哨兵值
fn read_tgid(tid: i32) -> Option<i32> {
    let text = std::fs::read_to_string(format!("/proc/{tid}/status")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("Tgid:"))
        .and_then(|v| v.trim().parse::<i32>().ok())
}

/// 解析 cpuset 风格 CPU 列表为位图
fn parse_cpu_bitmap(s: &str, max_cpu: usize) -> Vec<bool> {
    let mut bits = vec![false; max_cpu];
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if let Some((a, b)) = tok.split_once('-') {
            let (Ok(lo), Ok(hi)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) else {
                continue;
            };
            for c in lo..=hi.min(max_cpu.saturating_sub(1)) {
                bits[c] = true;
            }
        } else if let Ok(c) = tok.parse::<usize>() {
            if c < max_cpu {
                bits[c] = true;
            }
        }
    }
    bits
}

/// 在线核位图：解析异常/缺失回退全在线（宁可保守不钉也不让线程冻结）
fn online_bitmap(max_cpu: usize) -> Vec<bool> {
    let s = std::fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default();
    if s.trim().is_empty() {
        return vec![true; max_cpu];
    }
    let bits = parse_cpu_bitmap(&s, max_cpu);
    if bits.iter().all(|&b| !b) {
        vec![true; max_cpu]
    } else {
        bits
    }
}

/// cpu → 核心组（纯函数，注入 ranges 便于测试）；不在任何簇内（越界）返回 None
fn cpu_group_of(cpu: usize, ranges: &crate::common::CoreGroupRanges) -> Option<CoreGroup> {
    if ranges.little.contains(&cpu) {
        Some(CoreGroup::Little)
    } else if ranges.big.contains(&cpu) {
        Some(CoreGroup::Big)
    } else if ranges.prime.contains(&cpu) {
        Some(CoreGroup::Prime)
    } else {
        None
    }
}

/// 组名 → CoreGroup（解析 aarch32_clusters 配置）；未知名返回 None
fn cluster_group(name: &str) -> Option<CoreGroup> {
    match name {
        "little" => Some(CoreGroup::Little),
        "big" => Some(CoreGroup::Big),
        "prime" => Some(CoreGroup::Prime),
        _ => None,
    }
}

/// 核心组是否被 aarch32_clusters 声明为支持 AArch32（判定与测试复用，纯函数）
fn cluster_is_aarch32(clusters: &[String], g: CoreGroup) -> bool {
    clusters.iter().any(|s| cluster_group(s) == Some(g))
}

/// 读 /proc/<tid>/exe 的 ELF 头 EI_CLASS 判 32 位（1=ELF32、2=ELF64）：解析失败/非 ELF/无 exe（内核线程）
/// 一律按 64 位（保守，不误封），仅在首见建档时调用一次，不进周期路径
fn detect_elf32(tid: i32) -> bool {
    let Ok(path) = std::fs::read_link(format!("/proc/{tid}/exe")) else {
        return false;
    };
    let Ok(mut f) = std::fs::File::open(&path) else {
        return false;
    };
    let mut ident = [0u8; 5];
    use std::io::Read;
    if f.read_exact(&mut ident).is_err() {
        return false;
    }
    ident[0] == 0x7f && ident[1] == b'E' && ident[2] == b'L' && ident[3] == b'F' && ident[4] == 1
}

/// 写内核节点并保留与 utils::try_write_file 等价的失败日志（节点缺失 debug、其余warn）；差别仅在返回真实 io 结果供 @A 帧格式化 errno（@A 有 diag_active 闸门，
/// 日志没有）
fn logged_write(path: &str, val: &str) -> std::io::Result<()> {
    let res = std::fs::write(path, val);
    if let Err(e) = &res {
        if e.kind() == std::io::ErrorKind::NotFound {
            debug!("write skipped (node missing): {path}");
        } else {
            warn!("Failed to write to {path}: {e}");
        }
    }
    res
}

pub struct ThreadSample {
    pub comm: String,
    pub ticks: u64,
    /// /proc stat 第 22 字段，仅供失败退避的线程身份核验
    pub starttime: Option<u64>,
    /// 当前所在核（stat 的 processor 字段，`)` 后第 36 段 0 基）；读不到 -1，与 comm/ticks 同次读取顺带解析
    pub processor: i32,
}

/// 单线程 stat 采样（快照取数复用）
pub fn sample_one_tid(tid: i32) -> Option<ThreadSample> {
    let text = std::fs::read_to_string(format!("/proc/{tid}/stat")).ok()?;
    // comm 以首 '(' 与末 ')' 定界：comm 自身可含 '(' / 空格，闭侧必须 rfind，从下标 1 切会把 tid 尾部混进 comm，白名单/黑名单全等匹配恒不命中
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let rest = &text[close + 1..];
    // 单次遍历同时取 utime/stime（`)` 后第 11/12 字段 0 基）、processor（第 36 段）与字段总数，避免 collect 成 Vec 的整段分配
    let mut utime: Option<u64> = None;
    let mut stime: Option<u64> = None;
    let mut processor: Option<i32> = None;
    let mut starttime: Option<u64> = None;
    let mut count = 0usize;
    for (i, tok) in rest.split_whitespace().enumerate() {
        match i {
            11 => utime = tok.parse().ok(),
            12 => stime = tok.parse().ok(),
            19 => starttime = tok.parse().ok(),
            36 => processor = tok.parse().ok(),
            _ => {}
        }
        count = i + 1;
    }
    if count < 37 {
        return None;
    }
    let ticks: u64 = utime.unwrap_or(0) + stime.unwrap_or(0);
    Some(ThreadSample {
        comm: text[open + 1..close].to_string(),
        ticks,
        starttime,
        processor: processor.unwrap_or(-1),
    })
}

fn clk_tck() -> f32 {
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 { v as f32 } else { 100.0 }
}

/// 线程放置状态
struct ThreadState {
    /// 归属 PID：前台 = 建档时 fg_pid；后台候选 read_tgid 尽力解析（@S t 行归属），0 = 未知（勿改哨兵）
    pid: i32,
    /// 是否前台归属条目（唯一权威判据，勿用 pid>0 代判）：pid 也承载后台真实 tgid，看 pid>0 会把后台候选误当前台条目处理；true 仅由前台建档与「收养」转换
    is_fg: bool,
    /// 线程名（首见 stat 采样缓存，devimp place 行复用，避免重复读 stat）
    comm: String,
    /// 前台关键线程（prime 池）；后台为 false
    is_key: bool,
    /// 当前钉核号；-1 = 未钉
    home: i16,
    /// 已 promote（后台，含 cpuset 组迁移）
    promoted: bool,
    /// 来源 cpuset 组（demote/清理迁回）；取值只来自常量故存 'static 借用，免建档 String 分配
    orig_group: &'static str,
    /// 是否迁移过 cpuset 组
    moved_group: bool,
    /// 最近一次忙采样时刻
    last_busy: Option<Instant>,
    /// 连续低 util 复查计数（demote）
    low_streak: u32,
    /// 上次采样 ticks 与时刻（窗口 util）
    last_ticks: u64,
    last_sample: Instant,
    /// 上次迁移时刻（防抖）
    last_move: Instant,
    /// 最近一次被看到时刻（失联清理）
    last_seen: Instant,
    /// 性能核组绑定状态（None/Key/Busy），替代两个易失同步的布尔量
    group_bind: GroupBind,
    /// 最近一次迁离的核心（反跳回冷却用）；-1 = 无迁离记录
    prev_home: i16,
    /// 迁离时刻（配合 RETURN_COOLDOWN）
    prev_home_at: Instant,
    /// 上次 overload_hold 打点时刻（HOLD_LOG_COOLDOWN 节流）
    last_hold_log: Instant,
    /// 升核掩码与线程允许核交集为空（32 位任务钉无 AArch32 核等）：内核恒拒 EINVAL），置位后跳过该线程升核尝试；线程退出随表清理
    pin_incapable: bool,
    /// 单核钉定的**连续失败次数**（0 = 正常）：驱动指数退避，成功后清零。
    ///
    /// 2026-10-26 真机取证：单核钉定失败后原实现既不记账也不限速，同一线程对同一颗
    /// 当时不可用的核会按调度周期无限重打（实测单 pid 单核 1 分钟 390~417 次，
    /// sm8550 侧 `pin fg_pin` 失败 63,856 次 = 90.5%）。失败是**核此刻不可用**的信号，
    /// 不是重试信号，退避到下一次核域变化再试才有意义。
    pin_fail_streak: u8,
    /// 线程是否 32 位（ELF32）：建档时读一次 /proc/<tid>/exe 判定，解析失败按 64 位（false）
    elf32: bool,
    /// 写去重：最近一次掩码写入的 (目标 tag, 时刻)；同 tag 在 repin_debounce 内不再重写
    last_mask_write: Option<(u64, Instant)>,
    /// [eem] 最近一次采样到的窗口 util%（0..100）：放置内核的需求来源
    util_pct: f32,
    /// [eem] 最近一次采样到的**当前所在核**（`/proc/<tid>/stat` 的 processor，-1 = 未采到）：
    /// 首次接管时用它确定起始簇（此时既没有 `home` 也没有记账簇）
    last_cpu: i32,
    /// [eem] 本线程当前占用的连续核区间（None = 尚未接管）
    eem_window: Option<placement::Window>,
    /// [eem] 当前所在簇（None = 未接管；与 `eem_window` 成对维护）
    eem_cluster: Option<CoreGroup>,
    starttime: Option<u64>,
    normal_bind_retry: InvalidMaskRetry,
}

/// 掩码写入目标 tag：区分单核钉定（tag = core 号）与两种组掩码，供同目标去重判断
const MASK_TAG_PERF: u64 = u64::MAX - 1;
const MASK_TAG_FULL: u64 = u64::MAX;

/// 「持续忙」两窗判定（前台 normal_busy 与后台 promote 共用）：util≥busy_pct 记忙窗并返回是否连续两窗忙；util<release_pct 清忙标记，滞回带保持上次值
/// 不依赖采样间隔（分片下同线程两次被采到可能相隔很久）；只维护 last_busy，其余状态由调用方处理
fn busy_window_update(
    st: &mut ThreadState,
    util: f32,
    now: Instant,
    busy_pct: f32,
    release_pct: f32,
) -> bool {
    let was_busy = st.last_busy.is_some();
    if util >= busy_pct {
        st.last_busy = Some(now);
    } else if util < release_pct {
        st.last_busy = None;
    }
    was_busy && util >= busy_pct
}

// [manager]
pub struct AffinityManager {
    sys: Arc<SysPathExist>,
    snapshot: Option<Vec<(String, String)>>,
    uclamp_snapshot: Option<String>,
    uclamp_max_snapshot: Option<String>,
    uclamp_max_support: UclampSupport,
    applied_kind: u8,
    threads: HashMap<i32, ThreadState>,
    core_pinned: Vec<u32>,
    core_utils: Vec<f32>,
    /// 在线核位图缓存（每 ONLINE_EVERY_ROUNDS 刷新）
    online: Vec<bool>,
    bg_cursor: usize,
    tick: u64,
    bg_checked: HashSet<i32>,
    last_rebalance: Instant,
    last_fg_pid: i32,
    last_boost: bool,
    self_pid: u32,
    /// 前台进程 cmdline 缓存（每轮 rebalance 刷新，aff/place 行共用）
    fg_cmdline: String,
    /// default little 高水位迟滞锁存：>HIGH_WATER 激活、<RELEASE_WATER 解除，防乒乓
    key_bind_active: bool,
    /// normal_busy 采样轮转游标：压力窗口内每轮只扫 FG_SCAN_WINDOW 个
    fg_cursor: usize,
    /// boost 期 top-app uclamp.max 放开（写 100）前的值快照（None = 未接管）：机型值85 真机走 waltgov 限频钳制不进能量模型，对 FAS/akmode 只有负效应，
    /// 故激活写 100
    boost_uclamp_prev: Option<String>,
    /// 实验室静态分组（contingency/babel）：当前模式（None = 未启用）
    lab_static_mode: Option<String>,
    /// 静态分组改动过的 (组名, 原 cpus) 快照，退出时恢复
    lab_group_snapshot: Vec<(String, String)>,
    /// [fast_reader] top-app uclamp.min/max keep-open 读取器（构造期缓存路径），快照读取免每次 open/close
    uclamp_reader: FastReader,
    uclamp_max_reader: FastReader,
    /// 可配调优参数（apply 时从 cfg 刷新；缺省 = affinity.rs 的同名 const）
    tuning: AffinityTuningConfig,
    /// 后台 promote 是否只允许 big 且选核失败即放弃（apply 刷新）
    bg_promote_exclude_prime: bool,
    /// 防拥挤：big 饱和保护是否对所有 promote 模式生效（apply 刷新）
    big_pressure_any_mode: bool,
    /// 位宽感知总开关与 AArch32 簇白名单（apply 刷新）
    /// 白名单**缺省为空 = 不限制**（主流系统带转译层，32 位程序可跑全核）；只在无转译机型上显式填写
    bitwidth_aware: bool,
    aarch32_clusters: Vec<String>,
    /// 写去重开关（apply 刷新）
    write_dedup: bool,
    /// [eem] 放置引擎：逐 tid 记账（区间状态机 + 冷却），不分前后台
    eem: placement::Engine,
    /// [eem] 总开关（apply 刷新，缺省 false = 逐位回到既有放置路径）
    eem_placement: bool,
    /// [eem] 相对成本改善阈值（0..1）
    eem_hysteresis: f32,
    /// [eem] 参与门槛（util%）
    eem_min_util_pct: f32,
    /// [eem] 各 CPU 容量（真机 cpu_capacity 优先，缺则 soc.yaml [capacity]）：进程内一次解析
    core_caps: Vec<Option<u32>>,
    /// [narrow] 进程级窄核域备忘：pid → (被拒的簇, 到期时刻)
    ///
    /// 取证（2026-10-26 sm8550/PHB110）：一次 boost 里 417 个前台线程被钉向同一颗 big 核、
    /// 整批 EINVAL（单秒 431 条），随后整批每轮重试 —— 这类拒绝的判据是**进程级**的
    /// （cgroup cpuset 把该 pid 收窄、或目标核此刻不可用），逐线程各撞一次纯属浪费。
    /// 记住一次，同进程其余线程直接跳过，到期自动重试。
    narrow_pid: HashMap<i32, (CoreGroup, Instant)>,
}

impl AffinityManager {
    pub fn new(sys: Arc<SysPathExist>) -> Self {
        Self {
            sys,
            snapshot: None,
            uclamp_snapshot: None,
            uclamp_max_snapshot: None,
            uclamp_max_support: UclampSupport::Unknown,
            applied_kind: KIND_NONE,
            threads: HashMap::new(),
            core_pinned: Vec::new(),
            core_utils: Vec::new(),
            online: Vec::new(),
            bg_cursor: 0,
            tick: 0,
            bg_checked: HashSet::new(),
            last_rebalance: Instant::now(),
            last_fg_pid: 0,
            last_boost: false,
            self_pid: std::process::id(),
            fg_cmdline: String::new(),
            key_bind_active: false,
            fg_cursor: 0,
            boost_uclamp_prev: None,
            lab_static_mode: None,
            lab_group_snapshot: Vec::new(),
            uclamp_reader: FastReader::new("/dev/cpuctl/top-app/cpu.uclamp.min"),
            uclamp_max_reader: FastReader::new(UCLAMP_MAX_PATH),
            tuning: AffinityTuningConfig::default(),
            bg_promote_exclude_prime: true,
            big_pressure_any_mode: true,
            bitwidth_aware: true,
            aarch32_clusters: Vec::new(),
            write_dedup: true,
            eem: placement::Engine::new(),
            eem_placement: false,
            eem_hysteresis: energy_model::SWITCH_HYSTERESIS,
            eem_min_util_pct: 8.0,
            core_caps: Vec::new(),
            narrow_pid: HashMap::new(),
        }
    }

    fn ensure_snapshot(&mut self) {
        if self.snapshot.is_some() {
            return;
        }
        let mut snap = Vec::new();
        for group in BACKGROUND_GROUPS
            .iter()
            .copied()
            .chain([GROUP_TOP_APP, GROUP_FOREGROUND])
        {
            if let Some(v) = read_cpuset_cpus(group) {
                snap.push((format!("/dev/cpuset/{}/cpus", group), v));
            }
        }
        self.snapshot = Some(snap);
        // [fast_reader] keep-open 读取：三态口径不变（读失败/空值均落 ""，非空存 trim 原文）
        self.uclamp_snapshot = Some(
            self.uclamp_reader
                .read_raw()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default(),
        );
        self.uclamp_max_snapshot = Some(
            self.uclamp_max_reader
                .read_raw()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default(),
        );
    }

    /// 应用布局（cgroup 收窄/uclamp）并按需再平衡线程，`boost_uclamp_override` 三态：Some(true)=本调用放开为 100（特调 akmode）、Some(false)=负责还原不残留 100、None=不干预
    /// 所有权协议：boost 进入由 apply_uclamp_max 写机型值 85，需 prime 放置再抬 100；释放统一走 boost 退出链 restore_uclamp_max
    pub fn apply(
        &mut self,
        screen_on: bool,
        fg_pid: i32,
        cfg: &AffinityConfig,
        boost: bool,
        core_utils: &[f32],
        boost_uclamp_override: Option<bool>,
    ) {
        if !cfg.enabled {
            if self.is_active() {
                self.release_impl("disabled");
            }
            return;
        }
        if !core_utils.is_empty() {
            self.core_utils = core_utils.to_vec();
        }
        // 热重载刷新可配参数与开关（缺省 = affinity.rs 的同名 const，不写 yaml 时行为不变）
        self.tuning = cfg.tuning.clone();
        self.bg_promote_exclude_prime = cfg.bg_promote_exclude_prime;
        self.big_pressure_any_mode = cfg.big_pressure_any_mode;
        self.bitwidth_aware = cfg.bitwidth_aware;
        self.aarch32_clusters = cfg.aarch32_clusters.clone();
        self.write_dedup = cfg.write_dedup;
        // [eem] 相对能效放置（不分前后台）：开关与阈值热重载即生效。
        // **关闭时必须归还掩码**：EEM 会把线程写成 1 核窗口，关掉后若没人还原，
        // 该线程会永久卡在那个窄掩码上（既有 pin/promote 路径不会主动扫它）。
        let want_eem = cfg.eem_placement && energy_model::available();
        if self.eem_placement && !want_eem {
            self.eem_release_all();
        }
        self.eem_placement = want_eem;
        self.eem_hysteresis = cfg.eem_hysteresis;
        self.eem_min_util_pct = cfg.eem_min_util_pct;

        let ranges = crate::common::chiri_core_ranges();
        let boost_list = format_cpu_list(ranges.big.clone().chain(ranges.prime.clone()).collect());
        // 空 little（两簇 SoC）→ format_cpu_list 返回空串，写 cpuset.cpus 会 EINVAL；
        // 退化到 little_list() 的同一口径（big 组），与 babel 静态分组保持一致
        let little_list = if ranges.little.start < ranges.little.end {
            format_cpu_list(ranges.little.clone().collect())
        } else {
            little_list()
        };

        let old_kind = self.applied_kind;
        if boost {
            self.ensure_snapshot();
            if old_kind != KIND_BOOST {
                let mut writes: Vec<(String, String)> = Vec::new();
                if self.sys.cpuset_top_app_exist {
                    writes.push((GROUP_TOP_APP.to_string(), boost_list.clone()));
                }
                if self.sys.cpuset_foreground_exist {
                    writes.push((GROUP_FOREGROUND.to_string(), boost_list.clone()));
                }
                write_cpuset_cpus_items(&writes);
                self.apply_uclamp(cfg);
                self.apply_uclamp_max(cfg);
                self.pin_background(&little_list);
                info!(
                    "{}",
                    t_with_args(
                        "affinity-boost-applied",
                        &fluent_args!("big" => boost_list, "little" => little_list)
                    )
                );
            }
            self.applied_kind = KIND_BOOST;
        } else {
            if old_kind == KIND_BOOST {
                self.restore_foreground_groups();
                self.restore_uclamp();
                self.restore_uclamp_max();
                info!("{}", t("affinity-normal-restore"));
            }
            if old_kind != KIND_NORMAL {
                self.pin_background(&little_list);
            }
            // 线程面小核降权（预置骨架，默认关闭）：前台组掩码剔除 little（A510 能效差、EAS 会把 64 位前台线程吸进 little），每 2s 纠偏一次
            // TODO: 待内核信息（32 位核位图 / A710 核位）针对化后决定开启与掩码
            if cfg.normal_fg_exclude_little && self.sys.cpuset_top_app_exist {
                self.ensure_snapshot();
                let target = big_plus_prime_list();
                let mut writes: Vec<(String, String)> = Vec::new();
                if let Some(cur) = read_cpuset_cpus(GROUP_TOP_APP) {
                    if parse_cpu_list(&cur) != parse_cpu_list(&target) {
                        writes.push((GROUP_TOP_APP.to_string(), target.clone()));
                    }
                }
                if self.sys.cpuset_foreground_exist {
                    if let Some(cur) = read_cpuset_cpus(GROUP_FOREGROUND) {
                        if parse_cpu_list(&cur) != parse_cpu_list(&target) {
                            writes.push((GROUP_FOREGROUND.to_string(), target));
                        }
                    }
                }
                write_cpuset_cpus_items(&writes);
            }
            self.applied_kind = KIND_NORMAL;
        }

        // 后台降权：两态都持续写（始终压低后台、给 UI/视频让路），幂等；释放/总闸由 release() 还原
        self.apply_bg_uclamp_max(cfg);

        // top-app uclamp.max 放开归属：特调 Some(true)、fas None（hook 管理）、其余 Some(false) 防残留 100
        if let Some(want) = boost_uclamp_override {
            self.set_boost_uclamp_override(boost && want);
        }

        let force = fg_pid != self.last_fg_pid || boost != self.last_boost;
        if force {
            // [narrow] 前台/boost 状态切换 → 核域大概率跟着放宽（ROM 的 cpuset 随前台变化），
            // 立刻作废备忘，避免把「刚切到前台的进程」按 NARROW_PID_TTL 压住。
            self.narrow_pid.clear();
        }
        if cfg.pin_foreground_threads
            && (force || self.last_rebalance.elapsed() >= REBALANCE_INTERVAL)
        {
            self.last_rebalance = Instant::now();
            self.last_fg_pid = fg_pid;
            self.last_boost = boost;
            self.rebalance(screen_on, fg_pid, boost);
        }
    }

    // 工具

    /// 线程名（状态表缓存；无缓存/为空返回 "-"，@A 帧 comm 字段用）
    fn thread_comm(&self, tid: i32) -> &str {
        self.threads
            .get(&tid)
            .map(|st| st.comm.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("-")
    }

    /// 选核：核池 ∩ 在线核，取 score=逐核 util+钉核数×pinned_weight（缺省 0.4）最低者（含当前占用）
    fn pick_core(&self, pool: &[usize]) -> Option<usize> {
        pool.iter()
            .copied()
            .filter(|&c| self.online.get(c).copied().unwrap_or(false))
            .min_by(|&a, &b| {
                let score = |c: usize| {
                    self.core_utils.get(c).copied().unwrap_or(0.0)
                        + self.pinned_of(c) as f32 * self.tuning.pinned_weight
                };
                score(a)
                    .partial_cmp(&score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// 核心打分（与 pick_core 同口径）：util + 钉核数×权重；过载重钉的滞回校验用
    fn core_score(&self, core: usize) -> f32 {
        self.core_utils.get(core).copied().unwrap_or(0.0)
            + self.pinned_of(core) as f32 * self.tuning.pinned_weight
    }

    /// 主池优先选核（普通线程专用）：main 有未钉核只在 main 选，main 全钉才并入overflow 未钉核按 score 竞争（大核满溢 prime）；
    /// 达 MAX_PINS_PER_CORE 返回 None 不钉定（线程留 boost cpuset 内 EAS 自调度）关键线程不走本函数
    fn pick_core_pref(&self, main: &[usize], overflow: &[usize]) -> Option<usize> {
        let core = if main.iter().any(|&c| self.pinned_of(c) == 0) {
            self.pick_core(main)
        } else {
            let mut cands: Vec<usize> = main.to_vec();
            cands.extend(overflow.iter().copied().filter(|&c| self.pinned_of(c) == 0));
            self.pick_core(&cands)
        }?;
        if self.pinned_of(core) >= self.tuning.max_pins_per_core {
            return None;
        }
        Some(core)
    }

    fn pinned_of(&self, core: usize) -> u32 {
        self.core_pinned.get(core).copied().unwrap_or(0)
    }

    fn add_pinned(&mut self, core: usize, delta: i32) {
        if self.core_pinned.len() <= core {
            self.core_pinned.resize(core + 1, 0);
        }
        let cur = &mut self.core_pinned[core];
        if delta >= 0 {
            *cur += delta as u32;
        } else {
            *cur = cur.saturating_sub((-delta) as u32);
        }
    }

    /// 目标掩码是否至少触达一个 AArch32 簇（内核 sched_setaffinity 只按此交集判合法性）
    /// 非 32 位线程 / 未启用位宽感知 / 白名单为空（缺省，= 不限制落点）/ 线程未知 → 恒 true；真被内核拒（无转译）时各路径仍有 EINVAL → `pin_incapable` 兜底
    fn mask_write_allowed(&self, tid: i32, cpus: &[usize]) -> bool {
        if !self.bitwidth_aware || self.aarch32_clusters.is_empty() {
            return true;
        }
        match self.threads.get(&tid) {
            Some(st) if st.elf32 => {}
            _ => return true,
        }
        let ranges = crate::common::chiri_core_ranges();
        cpus.iter().any(|&c| {
            cpu_group_of(c, &ranges).is_some_and(|g| cluster_is_aarch32(&self.aarch32_clusters, g))
        })
    }

    /// 写去重：write_dedup 开启时，目标掩码已生效（home/group_bind 状态）或同 tid 同 tag 在 repin_debounce 内已写过 → 返回 true 跳过写入（连 @A 帧都不产生）
    fn mask_write_skip(&self, tid: i32, tag: u64, now: Instant) -> bool {
        if !self.write_dedup {
            return false;
        }
        let Some(st) = self.threads.get(&tid) else {
            return false;
        };
        // 注：MASK_TAG_FULL 的 state_match 在唯一调用方 restore_group_mask 下恒为 false（该函数在
        // group_bind == None 时已提前返回），全核目标实际只靠 recent 生效；保留分支以固定「全核」语义判定
        let state_match = if tag == MASK_TAG_FULL {
            st.home < 0 && st.group_bind == GroupBind::None
        } else if tag == MASK_TAG_PERF {
            st.group_bind != GroupBind::None
        } else {
            st.home == tag as i16
        };
        let recent = st.last_mask_write.is_some_and(|(wt, at)| {
            wt == tag && now.duration_since(at) < self.tuning.repin_debounce()
        });
        state_match || recent
    }

    /// 掩码写入成功后的记账：记 (目标 tag, 时刻)，供同目标最短重写间隔判断
    fn record_mask_write(&mut self, tid: i32, tag: u64, now: Instant) {
        if let Some(st) = self.threads.get_mut(&tid) {
            st.last_mask_write = Some((tag, now));
            st.normal_bind_retry.clear();
            st.pin_fail_streak = 0;
        }
    }

    /// 单核钉定的**失败退避闸**：连续失败 n 次后，距上次尝试不足
    /// `REPIN_DEBOUNCE × 2^(n-1)`（封顶 `PIN_FAIL_BACKOFF_MAX`）就整条跳过（连 @A 帧都不产生）。
    ///
    /// 仅在失败后生效：成功后 `record_mask_write` 已把计数清零，本闸恒 false，正常路径零改变。
    fn pin_fail_wait(&self, tid: i32, now: Instant) -> bool {
        let Some(st) = self.threads.get(&tid) else {
            return false;
        };
        if st.pin_fail_streak == 0 {
            return false;
        }
        let Some((_, at)) = st.last_mask_write else {
            return false;
        };
        let mul = 1u32 << (st.pin_fail_streak.min(4) - 1);
        let wait = (REPIN_DEBOUNCE * mul).min(PIN_FAIL_BACKOFF_MAX);
        now.saturating_duration_since(at) < wait
    }

    /// [narrow] 该 pid 在该簇是否刚被内核拒过：未到期 → 直接跳过（不发起 syscall、不落 @A 帧）
    fn narrow_skip(&self, pid: i32, cluster: CoreGroup, now: Instant) -> bool {
        pid > 0
            && self
                .narrow_pid
                .get(&pid)
                .is_some_and(|&(c, until)| c == cluster && now < until)
    }

    /// [narrow] 记一次**进程级**拒绝（EINVAL）：同 pid 同簇在 `NARROW_PID_TTL` 内不再尝试。
    ///
    /// 只对 EINVAL 记账：ESRCH（线程已退出）与 ENOENT（节点缺失）不代表核域受限，
    /// 记进去只会白白压制正常的提升动作。
    fn note_narrow(&mut self, pid: i32, cluster: CoreGroup, now: Instant, err: Option<i32>) {
        if pid <= 0 || err != Some(libc::EINVAL) {
            return;
        }
        self.narrow_pid.insert(pid, (cluster, now + NARROW_PID_TTL));
        // 表按 pid 增长：超过阈值先清过期项（正常情况下一条 pid 只占一条）
        if self.narrow_pid.len() > 512 {
            self.narrow_pid.retain(|_, &mut (_, until)| until > now);
        }
    }

    /// [strip] 写**返回成功**但回读把某簇整簇摘掉：与 EINVAL 同样记进窄域备忘。
    ///
    /// 为什么写成功也要记账：写返回 0 只说明内核收下了这次写，cgroup cpuset 与暂停核会在
    /// 之后把越界的核摘掉。sm8550 实测（A02-15，29 批 52.8M 行）：`place` 写 ok 的 34,937
    /// 行里有 22,159 行「回读的 eff 少了目标核」，被摘的核 94% 落在核 0-2；`t` 行统计又显示
    /// 小核各承担 11-12% 的驻留、并非整体离线。**同一个 pid 反复往同一簇写、每次被摘一次**，
    /// 就是这种半可用核域下的典型浪费：迁移发出去了、日志记了 ok、线程却原地不动。
    ///
    /// 只对「整簇一颗都没拿到」记账，不做部分记账：簇内还有核可用就是有效落点，
    /// 压住整簇会误伤（实测 285 个目标含小核的 pid 里 93 个确实拿到过小核）。
    fn note_stripped(&mut self, pid: i32, cluster: CoreGroup, now: Instant) {
        if pid <= 0 {
            return;
        }
        self.narrow_pid.insert(pid, (cluster, now + NARROW_PID_TTL));
        if self.narrow_pid.len() > 512 {
            self.narrow_pid.retain(|_, &mut (_, until)| until > now);
        }
    }

    /// normal 组绑定的真实写入结果；身份未知时不建立退避
    fn record_normal_bind_result(
        &mut self,
        tid: i32,
        cpus: &[usize],
        result: &std::io::Result<()>,
        now: Instant,
    ) {
        if let Some(state) = self.threads.get_mut(&tid) {
            if let Some(starttime) = state.starttime {
                state.normal_bind_retry.record(
                    result.as_ref().err().and_then(|error| error.raw_os_error()),
                    starttime,
                    cpus,
                    now,
                );
            } else {
                state.normal_bind_retry.clear();
            }
        }
    }

    /// 组绑定写前守卫：32 位线程目标簇无 AArch32 → 置 pin_incapable 并按 e0 打点（首次才打，防每轮重复帧）
    /// 返回 false；写去重命中 → 静默跳过返回 false；否则返回 true（调用方执行写入）
    fn group_bind_precheck(
        &mut self,
        tid: i32,
        pid: i32,
        pkg: &str,
        cpus: &[usize],
        reason: &str,
    ) -> bool {
        if !self.mask_write_allowed(tid, cpus) {
            let first = !self.threads.get(&tid).is_some_and(|st| st.pin_incapable);
            if first && crate::logger::diag_active() {
                let comm = self.thread_comm(tid);
                crate::logger::aff_action("pin", pid, tid, pkg, comm, "group", "-", "e0", reason);
            }
            if let Some(st) = self.threads.get_mut(&tid) {
                st.pin_incapable = true;
            }
            return false;
        }
        let now = Instant::now();
        if self.mask_write_skip(tid, MASK_TAG_PERF, now) {
            // dedup hit 有意跳过 debounce 窗口内的 cpuset 重读：窗口内最后一次写入已过 cpuset precheck，重读只增 IO 不改语义
            // 窗口外的组/身份变化仍会在下一候选轮触发新检查
            return false;
        }
        // [group_cpuset] Preserve the candidate identity before retry refresh can replace it.
        // Keep the original target/retry key; the kernel applies partial intersections.
        let expected = self.threads.get(&tid).and_then(|state| state.starttime);
        match check_group_cpuset(
            tid,
            cpus,
            expected,
            |path| std::fs::read_to_string(path).ok(),
            || sample_one_tid(tid).and_then(|sample| sample.starttime),
        ) {
            CpusetCheck::IdentityChanged => {
                // Rebuild on the next scan; never carry old comm/busy/retry state to a reused TID.
                if let Some(state) = self.threads.remove(&tid) {
                    if state.home >= 0 {
                        self.add_pinned(state.home as usize, -1);
                    }
                }
                return false;
            }
            CpusetCheck::EmptyIntersection | CpusetCheck::GroupChanged => return false,
            // Unknown is not an empty set or a syscall failure: retain existing fallback.
            CpusetCheck::Allowed | CpusetCheck::Unknown => {}
        }
        if matches!(reason, "normal_busy" | "normal_press")
            && self
                .threads
                .get(&tid)
                .is_some_and(|state| state.normal_bind_retry.is_active())
        {
            // 仅失败线程额外读 stat；身份不可核验时放行，不压制可能复用的 TID
            let starttime = sample_one_tid(tid).and_then(|sample| sample.starttime);
            if let Some(state) = self.threads.get_mut(&tid) {
                state.starttime = starttime;
                match starttime {
                    Some(starttime)
                        if state.normal_bind_retry.should_skip(starttime, cpus, now) =>
                    {
                        return false;
                    }
                    None => state.normal_bind_retry.clear(),
                    _ => {}
                }
            }
        }
        true
    }

    /// 钉线程到单核，`pkg` 由调用方传入（前台为缓存 fg_cmdline，后台 "-"），避免重读 cmdline，成败均落 @A 帧；失败保持原语义直接 return（不置状态，下次重试）
    fn pin_core(
        &mut self,
        tid: i32,
        core: usize,
        prev_home: i16,
        pid: i32,
        pkg: &str,
        reason: &str,
    ) {
        // [eem] EEM 接管时禁止**自动负载均衡类**的单核钉定：钉定会把线程压到 1 核，
        // 覆盖掉 EEM 的连续区间掩码，且 `home` 记账与 `eem_window` 会分叉（两套状态互相踩）。
        // 迁移决策统一由 `eem_place_round` 负责；仍保留**显式策略**路径：
        //   * `fg_pin`（boost 期把前台线程钉到性能核）
        //   * `home_overload`（前台线程所在核过载时的紧急换核）
        if self.eem_placement && !matches!(reason, "fg_pin" | "home_overload") {
            return;
        }
        let now = Instant::now();
        // 32 位线程钉到无 AArch32 的簇必然 EINVAL——不发起 syscall，按 e0 口径打点（首次才打）并标记不可钉
        if !self.mask_write_allowed(tid, &[core]) {
            let first = !self.threads.get(&tid).is_some_and(|st| st.pin_incapable);
            if first && crate::logger::diag_active() {
                let comm = self.thread_comm(tid);
                crate::logger::aff_action(
                    "pin",
                    pid,
                    tid,
                    pkg,
                    comm,
                    &core.to_string(),
                    &fmt_home(prev_home),
                    "e0",
                    reason,
                );
            }
            if let Some(st) = self.threads.get_mut(&tid) {
                st.pin_incapable = true;
            }
            return;
        }
        // 目标核已是 home（或同核刚写过）→ 跳过写入与 @A 帧
        if self.mask_write_skip(tid, core as u64, now) {
            return;
        }
        // 上两次同核尝试失败过（核此刻多半不可用）→ 按指数退避跳过，不产生 @A 帧：
        // 失败不是重试信号。恢复靠下一次核域变化后的重新尝试（退避封顶 60s）。
        if self.pin_fail_wait(tid, now) {
            return;
        }
        // [narrow] 该进程在这个簇上刚被内核拒过（cpuset 收窄 / 核此刻不可用）：
        // 同 pid 其余线程整批跳过。一个 boost 批次里同 pid 常有几十到几百个线程，
        // 逐个各撞一次没有信息增量，只有 syscall 与日志体积（实测单秒 431 条全灭）。
        let core_group = cpu_group_of(core, &crate::common::chiri_core_ranges());
        if let Some(cl) = core_group
            && self.narrow_skip(pid, cl, now)
        {
            return;
        }
        let res = set_tid_affinity(tid, &[core]);
        if crate::logger::diag_active() {
            let comm = self.thread_comm(tid);
            crate::logger::aff_action(
                "pin",
                pid,
                tid,
                pkg,
                comm,
                &core.to_string(),
                &fmt_home(prev_home),
                &io_result_tag(&res),
                reason,
            );
        }
        if res.is_err() {
            // 失败记账（**必须**做，否则退避闸永远读不到失败）：计数 +1 并把本次尝试
            // 记进 last_mask_write，使 mask_write_skip 的 recent 分支与 pin_fail_wait 的
            // 指数退避都基于「最近一次真实尝试」而不是「最近一次成功」。
            let errno = res.as_ref().err().and_then(|e| e.raw_os_error());
            if let Some(st) = self.threads.get_mut(&tid) {
                st.pin_fail_streak = st.pin_fail_streak.saturating_add(1);
                st.last_mask_write = Some((core as u64, now));
            }
            if let Some(cl) = core_group {
                self.note_narrow(pid, cl, now, errno);
            }
            return;
        }
        // 写成功记账（同核最短重写间隔）
        self.record_mask_write(tid, core as u64, now);
        if prev_home >= 0 {
            self.add_pinned(prev_home as usize, -1);
        }
        self.add_pinned(core, 1);
        if let Some(st) = self.threads.get_mut(&tid) {
            if prev_home >= 0 {
                // 记录迁离核与时刻，供重钉校验的反跳回冷却禁回
                st.prev_home = prev_home;
                st.prev_home_at = Instant::now();
            }
            st.home = core as i16;
            st.last_move = Instant::now();
        }
    }

    /// 解除单核钉定（恢复全核掩码）pkg 同 pin_core（cleanup/demote 场景传 "-" 防日志错误归属）仅在写成功或 ESRCH（无从重试）时清状态；失败保留 home/钉核计数待重试
    fn unpin_core(&mut self, tid: i32, prev_home: i16, pid: i32, pkg: &str) -> std::io::Result<()> {
        let ranges = crate::common::chiri_core_ranges();
        let all: Vec<usize> = (0..ranges.prime.end.max(ranges.big.end)).collect();
        let res = set_tid_affinity(tid, &all);
        if crate::logger::diag_active() {
            let comm = self.thread_comm(tid);
            crate::logger::aff_action(
                "restore",
                pid,
                tid,
                pkg,
                comm,
                "full",
                "-",
                &io_result_tag(&res),
                "reset",
            );
        }
        let gone = res.as_ref().err().and_then(|e| e.raw_os_error()) == Some(libc::ESRCH);
        if res.is_ok() || gone {
            // 必须按「全核目标」记账：不记的话 last_mask_write 仍停在最后一次单核钉定上，使 repin_debounce 内的重钉被 mask_write_skip 的 recent 分支判成「刚写过同一核」而整条丢弃
            // （既不写内核也不更新 home，与解钉后的真实掩码分叉）
            self.record_mask_write(tid, MASK_TAG_FULL, Instant::now());
            if prev_home >= 0 {
                self.add_pinned(prev_home as usize, -1);
            }
            if let Some(st) = self.threads.get_mut(&tid) {
                st.home = -1;
                st.promoted = false;
                st.low_streak = 0;
                st.last_busy = None;
            }
        }
        res
    }

    /// 迁移线程到指定 cpuset 组：直接 std::fs::write 捕获 io 结果（底层 try_write_file 恒返 Ok 会吞 errno，无法供 @A 帧 result）
    fn move_tid_group(&self, tid: i32, group: &str) -> std::io::Result<()> {
        logged_write(&format!("/dev/cpuset/{group}/tasks"), &tid.to_string())
    }

    /// 关键线程组掩码兜底还原：恢复全核掩码并清 group_bind，不触碰单核钉定计数；失败不置 group_bind（下次重试），仅补 @A 帧未做组绑定时为无操作
    fn restore_group_mask(&mut self, tid: i32, pid: i32, pkg: &str) -> std::io::Result<()> {
        let pinned = self
            .threads
            .get(&tid)
            .map(|st| st.group_bind != GroupBind::None)
            .unwrap_or(false);
        if !pinned {
            return Ok(());
        }
        let max_cpu = {
            let ranges = crate::common::chiri_core_ranges();
            ranges.prime.end.max(ranges.big.end)
        };
        let all: Vec<usize> = (0..max_cpu).collect();
        let now = Instant::now();
        // 同目标（全核）在 repin_debounce 内已写过 → 跳过写入与 @A 帧；但掩码既已确为全核，组绑定在掩码层面同样已解除，必须补清 group_bind，否则 bound 判定会悬挂到下一个冷却期
        // （unpin_core 现在也记 MASK_TAG_FULL，本分支命中概率比只用本函数记账时高）
        if self.mask_write_skip(tid, MASK_TAG_FULL, now) {
            if let Some(st) = self.threads.get_mut(&tid) {
                st.group_bind = GroupBind::None;
            }
            return Ok(());
        }
        let res = set_tid_affinity(tid, &all);
        if crate::logger::diag_active() {
            let comm = self.thread_comm(tid);
            crate::logger::aff_action(
                "restore",
                pid,
                tid,
                pkg,
                comm,
                "full",
                "-",
                &io_result_tag(&res),
                "fg_group_reset",
            );
        }
        if res.is_ok() {
            if let Some(st) = self.threads.get_mut(&tid) {
                st.group_bind = GroupBind::None;
            }
            self.record_mask_write(tid, MASK_TAG_FULL, now);
        }
        res
    }

    /// 清理线程（单条）：迁回原组 + 恢复全核 + 移除状态，返回 (首个失败 errno, 是否「无副作用条目」) 交 cleanup_threads 汇总恢复写失败且线程仍在（非 ESRCH）
    /// 时保留状态条目待 departed/gone/stale 路径重试，防状态表与内核永久分叉
    fn cleanup_thread(&mut self, tid: i32, reason: &str) -> (Option<i32>, bool) {
        if let Some(state) = self.threads.get_mut(&tid) {
            state.normal_bind_retry.clear();
        }
        // [eem] 线程退出即清放置记账：否则 HashMap 随 tid churn 无界增长，
        // 且 tid 复用会继承上一条线程的窗口/冷却状态（决策失真）。
        self.eem.forget(tid);
        let (moved, orig, home, pid, group_bind) = match self.threads.get(&tid) {
            Some(st) => (
                st.moved_group,
                st.orig_group,
                st.home,
                st.pid,
                st.group_bind,
            ),
            None => return (None, false),
        };
        // 「无副作用条目」= 从未迁组/未钉核/未组绑定，自建档起未对内核写过任何东西，逐条 bind_release 帧纯属噪声
        let noop = !moved && home < 0 && group_bind == GroupBind::None;
        let mut err: Option<i32> = None;
        // 任一恢复写失败且非 ESRCH → 保留条目待重试
        let mut retain = false;
        if moved && !orig.is_empty() {
            let res = self.move_tid_group(tid, orig);
            if crate::logger::diag_active() {
                let comm = self.thread_comm(tid);
                crate::logger::aff_action(
                    "move_group",
                    pid,
                    tid,
                    "-",
                    comm,
                    orig,
                    "-",
                    &io_result_tag(&res),
                    reason,
                );
            }
            if let Err(e) = &res {
                err = err.or(Some(e.raw_os_error().unwrap_or(0)));
                retain |= e.raw_os_error() != Some(libc::ESRCH);
            }
        }
        let res = if home >= 0 {
            self.unpin_core(tid, home, pid, "-")
        } else {
            // 组掩码兜底的关键线程在此恢复全核（内部无绑定时为无操作）
            self.restore_group_mask(tid, pid, "-")
        };
        if let Err(e) = &res {
            err = err.or(Some(e.raw_os_error().unwrap_or(0)));
            retain |= e.raw_os_error() != Some(libc::ESRCH);
        }
        // 逐条 bind_release 帧只留给真正动过的条目；无副作用条目由 cleanup_threads 汇总成一条 bulk 帧
        if crate::logger::diag_active() && !noop {
            let comm = self.thread_comm(tid);
            let result = match err {
                None => "ok".to_string(),
                Some(n) => format!("e{n}"),
            };
            crate::logger::aff_action(
                "bind_release",
                pid,
                tid,
                "-",
                comm,
                "-",
                "-",
                &result,
                reason,
            );
        }
        if !retain {
            self.threads.remove(&tid);
        }
        (err, noop)
    }

    /// 批量清理：逐条走 cleanup_thread，把无副作用条目汇成一条 bind_release bulk 帧（dst=bulk、value=条数、pid/tid=0）
    /// 返回首个失败 errno（retain/ESRCH 语义全在 cleanup_thread 内部）
    fn cleanup_threads(&mut self, tids: &[i32], reason: &str) -> Option<i32> {
        let mut err: Option<i32> = None;
        let mut noop = 0u32;
        for &tid in tids {
            let (e, was_noop) = self.cleanup_thread(tid, reason);
            err = err.or(e);
            if was_noop {
                noop += 1;
            }
        }
        if noop > 0 && crate::logger::diag_active() {
            crate::logger::aff_action(
                "bind_release",
                0,
                0,
                "-",
                "-",
                "bulk",
                &noop.to_string(),
                "ok",
                &format!("{reason}_bulk"),
            );
        }
        err
    }

    // [rebalance]
    fn rebalance(&mut self, screen_on: bool, fg_pid: i32, boost: bool) {
        let now = Instant::now();
        self.tick = self.tick.wrapping_add(1);
        let t = self.tick;
        // 建档用的迁移最小间隔本地化——ThreadState 构造闭包不得捕获 self（与 entry 的 &mut 借用冲突）
        let min_mig = self.tuning.min_migrate_interval();
        let repin_debounce = self.tuning.repin_debounce();
        let return_cooldown = self.tuning.return_cooldown();
        let mut ranges = crate::common::chiri_core_ranges();
        let max_cpu = ranges.prime.end.max(ranges.big.end);
        let prime_pool: Vec<usize> = if ranges.prime.is_empty() {
            ranges.big.clone().collect()
        } else {
            ranges.prime.clone().collect()
        };
        let big_pool: Vec<usize> = ranges.big.clone().collect();
        // 前台线程合法落点 = 全部性能核（prime ∪ big；无 prime SoC 退化为 big）
        let mut perf_pool: Vec<usize> = ranges.big.clone().collect();
        perf_pool.extend(ranges.prime.clone());

        // 在线核位图刷新：hotplug uevent 置脏标记时立即刷新 + 周期兜底；每轮都消费脏
        // 标记（放条件外，避免 || 短路把标记留到下一轮多刷一次）
        let hotplug_dirty =
            crate::monitor::CPU_HOTPLUG_DIRTY.swap(false, std::sync::atomic::Ordering::Relaxed);
        if t % ONLINE_EVERY_ROUNDS == 0 || self.online.len() != max_cpu || hotplug_dirty {
            self.online = online_bitmap(max_cpu);
        }

        // —— 快速切换：立即解除上一个前台应用的钉定 ——同模式切换只靠 2s 周期发现，等 30s 失联清理期间旧前台线程仍持大核掩码（新前台关键线程与之同核互踩、core_pinned 计数漂移累积）
        // 判据 is_fg 且≠ 当前前台（后台条目 pid 现在也可能非 0，不能看 pid>0）；稳态为空集零文件 IO
        let departed: Vec<i32> = self
            .threads
            .iter()
            .filter(|(_, st)| st.is_fg && st.pid != fg_pid)
            .map(|(tid, _)| *tid)
            .collect();
        self.cleanup_threads(&departed, "departed");

        // —— 前台：每轮 1 次 read_dir，新增线程才读 stat ——
        if fg_pid > 0 {
            let fg_cmdline = read_cmdline(fg_pid);
            // cmdline 缓存：aff/place 行共用，钉核不再重读 /proc
            self.fg_cmdline = fg_cmdline.clone();
            let fg_ok =
                !fg_cmdline.is_empty() && !crate::common::is_affinity_blacklisted(&fg_cmdline);
            let pin_fg = fg_ok && boost && screen_on;

            // default（非 boost）轻量前台保护：little 高水位时把关键线程（主线程/RenderThread 等白名单）组绑定到 big∪prime——后台 promote 救不了前台关键线程
            // 迟滞：>HIGH_WATER 激活、<RELEASE_WATER 解除；boost/息屏强制解除（boost 有完整接管、息屏无绑定意义）
            let little_max = ranges
                .little
                .by_ref()
                .filter_map(|c| self.core_utils.get(c).copied())
                .fold(0.0_f32, f32::max);
            if !screen_on || boost {
                self.key_bind_active = false;
            } else if little_max > self.tuning.little_high_water {
                self.key_bind_active = true;
            } else if little_max < self.tuning.key_bind_release_water {
                self.key_bind_active = false;
            }
            let key_pressure = fg_ok && !boost && screen_on && self.key_bind_active;

            if fg_ok {
                let task_dir = format!("/proc/{fg_pid}/task");
                // 每轮克隆一次供两处 pin_core 复用（避免分支内重复克隆）
                let pkg = self.fg_cmdline.clone();
                match std::fs::read_dir(&task_dir) {
                    Ok(rd) => {
                        let mut seen = HashSet::new();
                        // default 小核高水位下待升核的非关键前台线程（本轮限量采样）
                        let mut scan_pool: Vec<i32> = Vec::new();
                        for entry in rd.flatten() {
                            let tid: i32 =
                                match entry.file_name().to_str().and_then(|s| s.parse().ok()) {
                                    Some(x) => x,
                                    None => continue,
                                };
                            seen.insert(tid);
                            let fresh = !self.threads.contains_key(&tid);
                            // 首见建档时判一次位宽（不每轮读），判定为 32 位即时落 elf32 帧
                            // bitwidth_aware=false 时连判定都不做——开关要能逐位回到不判位宽的路径（不读 /proc/<tid>/exe、不落帧）
                            let elf32 = self.bitwidth_aware && fresh && detect_elf32(tid);
                            if elf32 && crate::logger::diag_active() {
                                crate::logger::aff_action(
                                    "elf32",
                                    fg_pid,
                                    tid,
                                    &pkg,
                                    "-",
                                    GROUP_FOREGROUND,
                                    "32",
                                    "ok",
                                    "bitwidth",
                                );
                            }
                            let st = self.threads.entry(tid).or_insert_with(|| ThreadState {
                                pid: fg_pid,
                                is_fg: true,
                                comm: String::new(),
                                is_key: false,
                                home: -1,
                                promoted: false,
                                orig_group: GROUP_FOREGROUND,
                                moved_group: false,
                                last_busy: None,
                                low_streak: 0,
                                last_ticks: 0,
                                last_sample: now,
                                last_move: now - min_mig,
                                last_seen: now,
                                group_bind: GroupBind::None,
                                prev_home: -1,
                                prev_home_at: now,
                                last_hold_log: now - HOLD_LOG_COOLDOWN,
                                pin_incapable: false,
                                pin_fail_streak: 0,
                                elf32,
                                last_mask_write: None,
                                starttime: None,
                                normal_bind_retry: InvalidMaskRetry::default(),
                                util_pct: 0.0,
                                last_cpu: -1,
                                eem_window: None,
                                eem_cluster: None,
                            });
                            // 归属变化（前台换 PID 后复用 tid，或后台候选被前台收养）视作新线程：`!is_fg` 覆盖后者——后台候选身份仍是后台，收养后必须按前台身份重新采样
                            if !st.is_fg || st.pid != fg_pid {
                                st.is_fg = true;
                                st.pid = fg_pid;
                                st.comm.clear();
                                st.home = -1;
                                st.promoted = false;
                                st.moved_group = false;
                                st.group_bind = GroupBind::None;
                                st.prev_home = -1;
                                st.last_move = now - min_mig;
                                // ticks 基准一并作废：否则「fresh 或 last_ticks==0 才重采样」不成立，is_key 沿用旧后台身份（恒 false）会误走普通单核路径
                                st.last_ticks = 0;
                                st.last_busy = None;
                                st.low_streak = 0;
                                st.starttime = None;
                                st.normal_bind_retry.clear();
                            }
                            st.last_seen = now;
                            // 新增线程才读 stat（判关键线程 / 建档 ticks + comm 缓存）
                            if fresh || st.last_ticks == 0 {
                                if let Some(s) = sample_one_tid(tid) {
                                    st.is_key = is_key_thread(tid, fg_pid, &s.comm);
                                    st.comm = s.comm;
                                    st.last_ticks = s.ticks;
                                    st.last_sample = now;
                                    st.starttime = s.starttime;
                                }
                            }
                            let (home, is_key, group_bind) = (st.home, st.is_key, st.group_bind);
                            // 升核候选：未绑定，或已因「忙」绑定（后者需复查以便空闲释放）
                            if key_pressure
                                && !is_key
                                && home < 0
                                && matches!(group_bind, GroupBind::None | GroupBind::Busy)
                            {
                                scan_pool.push(tid);
                            }
                            if pin_fg {
                                if is_key {
                                    // 关键线程核组绑定（AppOptR 式）：不钉单核，boost 下cpuset 已收窄 prime∪big 组内自调度；
                                    // cpuset 不可用时写一次组掩码兜底（短路去重，不占钉核计数）
                                    if group_bind == GroupBind::None
                                        && !self.sys.cpuset_top_app_exist
                                        && self.group_bind_precheck(
                                            tid, fg_pid, &pkg, &perf_pool, "fg_group",
                                        )
                                    {
                                        let now_bind = Instant::now();
                                        let res = set_tid_affinity(tid, &perf_pool);
                                        if crate::logger::diag_active() {
                                            let comm = self.thread_comm(tid);
                                            crate::logger::aff_action(
                                                "pin",
                                                fg_pid,
                                                tid,
                                                &pkg,
                                                comm,
                                                "group",
                                                "-",
                                                &io_result_tag(&res),
                                                "fg_group",
                                            );
                                        }
                                        if res.is_ok() {
                                            if let Some(st) = self.threads.get_mut(&tid) {
                                                st.group_bind = GroupBind::Key;
                                            }
                                            self.record_mask_write(tid, MASK_TAG_PERF, now_bind);
                                        }
                                    }
                                } else {
                                    // 合法范围 = 全部性能核（含溢出落点 prime），避免溢出核被误判非法
                                    let core_ok = home >= 0
                                        && (home as usize) < max_cpu
                                        && self.online.get(home as usize).copied().unwrap_or(false)
                                        && perf_pool.contains(&(home as usize));
                                    if !core_ok {
                                        // 普通线程优先大核，大核钉满后溢出超大核
                                        if let Some(core) =
                                            self.pick_core_pref(&big_pool, &prime_pool)
                                        {
                                            self.pin_core(tid, core, home, fg_pid, &pkg, "fg_pin");
                                        }
                                    // [partition] boost 期过载重钉（保守版，2026-10-24 A02-14 修订）：
                                    // FAS 锁频下换核不省能，但**过载分散**换核能压排队（吞吐/时延），是 boost 期
                                    // 唯一不破坏 cpuset 分区的动态分散手段（线程仍在 top-app=big∪prime 组内）。
                                    // 「更少更稳」= debounce ×3 + 阈值 0.70+0.15=0.85，双滞回（margin + 反跳回冷却）照旧。
                                    // 本块位于 `if pin_fg`（= fg_ok && boost && screen_on）内，default 期从不进入；
                                    // 完全禁掉的版本（同日早前）会造成钉定线程在饱和核上排队无人解。
                                    } else if self.threads.get(&tid).is_some_and(|st| {
                                        now.duration_since(st.last_move)
                                            >= repin_debounce * BOOST_REPIN_SCALE
                                    }) && self
                                        .core_utils
                                        .get(home as usize)
                                        .copied()
                                        .unwrap_or(0.0)
                                        > (self.tuning.core_overload_util + BOOST_OVERLOAD_EXTRA)
                                            .min(0.98)
                                    {
                                        // home 核过载：重钉到全性能池（big∪prime）最低分核分散负载——旧口径只看 big，大核普遍过载时空闲 prime 永远进不了候选；
                                        // 双滞回：分数差 ≥ OVERLOAD_MARGIN 且目标严格更低 + 反跳回冷却禁回
                                        if let Some(core) = self.pick_core(&perf_pool) {
                                            let (left_home, left_at) = self
                                                .threads
                                                .get(&tid)
                                                .map(|st| (st.prev_home, st.prev_home_at))
                                                .unwrap_or((-1, now));
                                            let home_score = self.core_score(home as usize);
                                            let cand_score = self.core_score(core);
                                            let banned = left_home == core as i16
                                                && now.duration_since(left_at) < return_cooldown;
                                            if core != home as usize
                                                && cand_score
                                                    <= home_score - self.tuning.overload_margin
                                                && self.core_utils.get(core).copied().unwrap_or(0.0)
                                                    < self
                                                        .core_utils
                                                        .get(home as usize)
                                                        .copied()
                                                        .unwrap_or(0.0)
                                                && !banned
                                            {
                                                self.pin_core(
                                                    tid,
                                                    core,
                                                    home,
                                                    fg_pid,
                                                    &pkg,
                                                    "home_overload",
                                                );
                                            } else if core != home as usize {
                                                // cand==home（全员饱和无更优核）不打点，防每轮刷 hold 行；其余 hold 按 HOLD_LOG_COOLDOWN 节流
                                                if let Some(st) = self.threads.get_mut(&tid) {
                                                    if now.duration_since(st.last_hold_log)
                                                        >= HOLD_LOG_COOLDOWN
                                                    {
                                                        st.last_hold_log = now;
                                                        crate::logger::main_event(
                                                            "overload_hold",
                                                            &pkg,
                                                            &format!(
                                                                "tid={} home={} cand={} score_home={:.2} score_cand={:.2}",
                                                                tid,
                                                                home,
                                                                core,
                                                                home_score,
                                                                cand_score
                                                            ),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if home >= 0 {
                                let _ = self.unpin_core(tid, home, fg_pid, &pkg);
                            } else if group_bind != GroupBind::None {
                                // 组掩码兜底恢复（Key/Busy 同款）：压力解除或 boost 退出时promote_busy_foreground 不再被调用，必须在此兜住，
                                // 否则线程带 big∪prime 收窄掩码滞留性能核；压力活跃期的 Busy 空闲回落仍走下方采样块滞回
                                if !key_pressure {
                                    let _ = self.restore_group_mask(tid, fg_pid, &pkg);
                                }
                            } else if key_pressure
                                && is_key
                                && self.group_bind_precheck(
                                    tid,
                                    fg_pid,
                                    &pkg,
                                    &perf_pool,
                                    "normal_press",
                                )
                            {
                                // little 高水位的 default：关键线程组绑定 big∪prime（同 boost fg_group 机制、不同触发条件）；不钉单核不占钉核计数，
                                // EAS 组内自调度
                                let now_bind = Instant::now();
                                let res = set_tid_affinity(tid, &perf_pool);
                                self.record_normal_bind_result(tid, &perf_pool, &res, now_bind);
                                if crate::logger::diag_active() {
                                    let comm = self.thread_comm(tid);
                                    crate::logger::aff_action(
                                        "pin",
                                        fg_pid,
                                        tid,
                                        &pkg,
                                        comm,
                                        "group",
                                        "-",
                                        &io_result_tag(&res),
                                        "normal_press",
                                    );
                                }
                                if res.is_ok() {
                                    if let Some(st) = self.threads.get_mut(&tid) {
                                        st.group_bind = GroupBind::Key;
                                    }
                                    self.record_mask_write(tid, MASK_TAG_PERF, now_bind);
                                }
                            }
                        }
                        // 已消失线程立即清理释放钉核计数（is_fg 判据：pid 可能与前台相同但归属后台组）
                        let gone: Vec<i32> = self
                            .threads
                            .iter()
                            .filter(|(tid, st)| st.is_fg && st.pid == fg_pid && !seen.contains(tid))
                            .map(|(tid, _)| *tid)
                            .collect();
                        self.cleanup_threads(&gone, "gone");

                        // —— [eem] 前台线程 util 采样：不分前后台的前提 ——
                        // 旧路径只在「little 高水位 + 非关键线程」时才采样前台 util，
                        // EEM 若要覆盖前台就必须每轮采到它们的需求，否则 util_pct 恒 0、
                        // 前台线程永远不会进入放置决策（等于只对后台生效）。
                        if self.eem_placement {
                            let clk = clk_tck();
                            for tid in &seen {
                                if let Some(s) = sample_one_tid(*tid)
                                    && let Some(u) = self.window_util(*tid, s.ticks, now, clk)
                                    && let Some(st) = self.threads.get_mut(tid)
                                {
                                    st.util_pct = u;
                                    st.last_cpu = s.processor;
                                }
                            }
                        }

                        // —— default 小核高水位：非关键前台线程升核（限量采样）——
                        if key_pressure && !scan_pool.is_empty() {
                            self.promote_busy_foreground(&scan_pool, &perf_pool, fg_pid, now);
                        }
                    }
                    Err(_) => {
                        // 前台进程已退出：清理其全部线程
                        let gone: Vec<i32> = self
                            .threads
                            .iter()
                            .filter(|(_, st)| st.is_fg && st.pid == fg_pid)
                            .map(|(tid, _)| *tid)
                            .collect();
                        self.cleanup_threads(&gone, "gone");
                    }
                }
            }
        }

        // —— 后台：仅亮屏；已 promote 复查 + 分片候选 ——
        if screen_on {
            // 已 promote 线程 demote 复查（集合小，每 2 轮）
            if t % PROMOTED_REVIEW_EVERY_ROUNDS == 0 {
                let clk = clk_tck();
                let promoted: Vec<i32> = self
                    .threads
                    .iter()
                    .filter(|(_, st)| st.promoted && st.home >= 0)
                    .map(|(tid, _)| *tid)
                    .collect();
                for tid in promoted {
                    if let Some(s) = sample_one_tid(tid) {
                        let util = self.window_util(tid, s.ticks, now, clk).unwrap_or(0.0);
                        if let Some(st) = self.threads.get_mut(&tid) {
                            st.util_pct = util;
                            st.last_cpu = s.processor;
                            // [partition] boost 期后台深扫被跳过、无人刷 last_seen；本块 boost 期照跑，
                            // 必须在这里续命，否则 30s 后全量条目命中 stale 清理（批量 demote 迁移，
                            // 恰在降迁移的会话中段制造迁移瀑布）。
                            st.last_seen = now;
                        }
                        // tid 来自上方收集且循环内无删除，正常不可达；防御性跳过不中止整轮
                        let low = match self.threads.get_mut(&tid) {
                            Some(st) => {
                                if util < self.tuning.demote_util_pct {
                                    st.low_streak += 1;
                                } else {
                                    st.low_streak = 0;
                                }
                                st.low_streak
                            }
                            None => continue,
                        };
                        if low >= self.tuning.demote_streak {
                            self.demote(tid, util);
                        } else if util >= self.tuning.demote_util_pct {
                            // 线程仍忙且所在核过载：换低占用 big 核分散负载promoted 线程虽在 top-app 组但不属前台——保持 big 池与迁移防抖，不走前台路径
                            let (home, last_move) = match self.threads.get(&tid) {
                                Some(st) => (st.home, st.last_move),
                                None => continue,
                            };
                            // [partition] boost（含 fas）期重钉走保守参数：debounce ×3、阈值 0.70+0.15=0.85
                            // （锁频下换核不省能，过载分散压排队是唯一收益，门槛必须更确定才付迁移代价）；
                            // 同组内换核不破坏 cpuset 分区（promoted 线程已在 top-app=big∪prime），双滞回照旧。
                            // 2026-10-24 修订：早前版本对 boost 期一刀切禁掉，会造成 promoted 线程在饱和核排队无人解。
                            if home >= 0
                                && now.duration_since(last_move)
                                    >= repin_debounce * (if boost { BOOST_REPIN_SCALE } else { 1 })
                                && self.core_utils.get(home as usize).copied().unwrap_or(0.0)
                                    > (self.tuning.core_overload_util
                                        + if boost { BOOST_OVERLOAD_EXTRA } else { 0.0 })
                                    .min(0.98)
                            {
                                // 与前台同口径：全性能池最低分核 + 双滞回 + 反跳回冷却
                                if let Some(core) = self.pick_core(&perf_pool) {
                                    let (left_home, left_at) = match self.threads.get(&tid) {
                                        Some(st) => (st.prev_home, st.prev_home_at),
                                        None => continue,
                                    };
                                    let home_score = self.core_score(home as usize);
                                    let cand_score = self.core_score(core);
                                    let banned = left_home == core as i16
                                        && now.duration_since(left_at) < return_cooldown;
                                    if core != home as usize
                                        && cand_score <= home_score - self.tuning.overload_margin
                                        && self.core_utils.get(core).copied().unwrap_or(0.0)
                                            < self
                                                .core_utils
                                                .get(home as usize)
                                                .copied()
                                                .unwrap_or(0.0)
                                        && !banned
                                    {
                                        self.pin_core(tid, core, home, 0, "-", "bg_overload");
                                    } else if core != home as usize {
                                        // cand==home 不打点（同前台口径）；hold 按 HOLD_LOG_COOLDOWN 节流
                                        if let Some(st) = self.threads.get_mut(&tid) {
                                            if now.duration_since(st.last_hold_log)
                                                >= HOLD_LOG_COOLDOWN
                                            {
                                                st.last_hold_log = now;
                                                crate::logger::main_event(
                                                    "overload_hold",
                                                    "-",
                                                    &format!(
                                                        "tid={} home={} cand={} score_home={:.2} score_cand={:.2}",
                                                        tid, home, core, home_score, cand_score
                                                    ),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 候选刷新（低频）+ 分片深扫
            // [partition] boost（含 fas）期不跑后台动态 promote：boost 的 cpuset 分区已经把
            // 后台固定在 little、top-app/foreground 收窄在 big∪prime，此时再逐线程把后台
            // 线程 promote 到 big，等于每轮把分区重画一遍——每个 promote/demote 都是一次
            // group move + 掩码写（= 一次立即迁移），而 FAS 期间频率由 PID 锁档、这些迁移
            // 换不来能效（真机 A02-14：fas 段 9703~10007 次/s，是 default 段的 2~4 倍）。
            // 改为「boost 期由 cpuset 静态分区承担后台摆放，退出 boost 后恢复动态再平衡」。
            // 2026-10-24 修订边界：boost 期禁的只有 **move_group 型 promote**（会破坏分区不变式）；
            // 同组内换核的过载重钉（home_overload / bg_overload）放开成保守参数版——见上方各分支。
            // 注意只跳过**候选扫描**，不得 return：尾部失联清理仍要跑。
            if (t % BG_LIST_EVERY_ROUNDS == 0 || self.bg_cursor == 0) && !boost {
                let mut bg: Vec<(i32, &'static str)> = Vec::new();
                for group in BACKGROUND_GROUPS {
                    let exist = match group {
                        "background" => self.sys.cpuset_background_exist,
                        "system-background" => self.sys.cpuset_system_background_exist,
                        _ => self.sys.cpuset_restricted_exist,
                    };
                    if exist {
                        for tid in read_cpuset_tasks(group) {
                            if tid as u32 != self.self_pid {
                                bg.push((tid, group));
                            }
                        }
                    }
                }
                // bg 为空只跳过候选扫描，不得 return——否则跳过尾部失联清理（promoted 滞留大核）与 devimp 输出
                if !bg.is_empty() {
                    let little_max = ranges
                        .little
                        .by_ref()
                        .filter_map(|c| self.core_utils.get(c).copied())
                        .fold(0.0_f32, f32::max);
                    let big_max = ranges
                        .big
                        .by_ref()
                        .filter_map(|c| self.core_utils.get(c).copied())
                        .fold(0.0_f32, f32::max);
                    let (promote_base, little_promote) = (
                        self.tuning.promote_util_pct,
                        self.tuning.little_promote_util_pct,
                    );
                    let promote_thresh = if little_max > self.tuning.little_high_water {
                        little_promote
                    } else {
                        promote_base
                    };
                    // 防拥挤：big 实际已饱和即不 promote——big_pressure_any_mode=true 时不再绑定 boost（亮屏 default 也生效）；
                    // false = 回到仅 boost 态旧行为，判据只用排序/门槛，不引入新数值
                    let big_pressure = (self.big_pressure_any_mode || boost)
                        && big_max > self.tuning.big_high_water;
                    let clk = clk_tck();
                    let n = bg.len();
                    self.bg_cursor %= n;
                    let end = (self.bg_cursor + BG_SCAN_WINDOW).min(n);
                    for (tid, group) in &bg[self.bg_cursor..end] {
                        // 已 promote / 前台线程跳过（前者走复查，后者走前台路径），is_fg 而非 pid>0：后台候选 pid 已补真实 tgid，判据必须看归属
                        if let Some(st) = self.threads.get(tid) {
                            if st.promoted || st.is_fg {
                                continue;
                            }
                        }
                        let Some(s) = sample_one_tid(*tid) else {
                            continue;
                        };
                        if crate::common::is_affinity_blacklisted(&s.comm) {
                            continue;
                        }
                        // 进程级黑名单：首见读一次并缓存
                        if !self.bg_checked.contains(tid) {
                            if crate::common::is_affinity_blacklisted(&read_cmdline(*tid)) {
                                self.bg_checked.insert(*tid);
                                continue;
                            }
                            self.bg_checked.insert(*tid);
                        }
                        let busy = match self.threads.get_mut(tid) {
                            // 首见：建档并记基准，本轮无 util 不判 promote；pid 尽力补全（@S t 行归属）
                            None => {
                                // 首见建档判一次位宽；32 位落 elf32 帧（target = 线程所在后台组）
                                // bitwidth_aware=false 时不做判定（逐位回到不判位宽的路径：不读 /proc/<tid>/exe、不落帧）
                                let elf32 = self.bitwidth_aware && detect_elf32(*tid);
                                let pid = read_tgid(*tid).unwrap_or(0);
                                if elf32 && crate::logger::diag_active() {
                                    crate::logger::aff_action(
                                        "elf32", pid, *tid, "-", &s.comm, *group, "32", "ok",
                                        "bitwidth",
                                    );
                                }
                                self.threads.insert(
                                    *tid,
                                    ThreadState {
                                        pid,
                                        is_fg: false,
                                        comm: s.comm,
                                        is_key: false,
                                        home: -1,
                                        promoted: false,
                                        orig_group: *group,
                                        moved_group: false,
                                        last_busy: None,
                                        low_streak: 0,
                                        last_ticks: s.ticks,
                                        last_sample: now,
                                        last_move: now - min_mig,
                                        last_seen: now,
                                        group_bind: GroupBind::None,
                                        prev_home: -1,
                                        prev_home_at: now,
                                        last_hold_log: now - HOLD_LOG_COOLDOWN,
                                        pin_incapable: false,
                                        pin_fail_streak: 0,
                                        elf32,
                                        last_mask_write: None,
                                        starttime: s.starttime,
                                        normal_bind_retry: InvalidMaskRetry::default(),
                                        util_pct: 0.0,
                                        last_cpu: -1,
                                        eem_window: None,
                                        eem_cluster: None,
                                    },
                                );
                                continue;
                            }
                            // 再次命中：窗口 util = ticks 增量/间隔秒/CLK_TCK×100
                            Some(st) => {
                                st.last_seen = now;
                                let dt =
                                    now.duration_since(st.last_sample).as_secs_f32().max(0.001);
                                let util =
                                    ((s.ticks.saturating_sub(st.last_ticks)) as f32 / clk / dt
                                        * 100.0)
                                        .min(100.0);
                                st.last_ticks = s.ticks;
                                st.last_sample = now;
                                // 两窗防抖：本次忙且上次忙才 promote，期间采到低负载即清忙标记防瞬时忙被 promote；不依赖采样间隔，与前台 normal_busy 共用判定
                                let sustained_busy = busy_window_update(
                                    st,
                                    util,
                                    now,
                                    promote_thresh,
                                    self.tuning.demote_util_pct,
                                );
                                if sustained_busy && !big_pressure {
                                    Some(util)
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(util) = busy {
                            // 32 位线程不能上不含 AArch32 的性能簇（big/prime），直接放弃 promote（不 move_group，无副作用）
                            if !self.mask_write_allowed(*tid, &big_pool) {
                                continue;
                            }
                            // bg_promote_exclude_prime 时先按 big 池选核（overflow 传空），选核失败（big 全钉满）即放弃——
                            // 绝不先 move_group：normal 态 top-app cpuset 可能含 prime，滞留组内仍会被 EAS 放上 prime，破坏「后台永不上 prime」不变式
                            let pre_core = if self.bg_promote_exclude_prime {
                                match self.pick_core_pref(&big_pool, &[]) {
                                    Some(c) => Some(c),
                                    None => continue,
                                }
                            } else {
                                None
                            };
                            // 移入 top-app 使 big 核可见，再按当前核心占用选核钉定（原路径：先移组后选核；exclude 路径：已先选核）
                            let move_res = self.move_tid_group(*tid, GROUP_TOP_APP);
                            if crate::logger::diag_active() {
                                let comm = self.thread_comm(*tid);
                                crate::logger::aff_action(
                                    "move_group",
                                    0,
                                    *tid,
                                    "-",
                                    comm,
                                    GROUP_TOP_APP,
                                    "-",
                                    &io_result_tag(&move_res),
                                    "promote",
                                );
                            }
                            if let Some(st) = self.threads.get_mut(tid) {
                                // 写失败不置位（未真正迁入 top-app 就无需迁回）；原 try_write_file 恒 Ok 时该项恒 true，为观测化顺带的语义修正
                                st.moved_group = move_res.is_ok();
                                st.promoted = true;
                            }
                            // 选核与前台普通线程同口径：big 有未钉核只看 big，big 钉满才溢出prime（exclude 路径恒无 overflow）
                            let core = match pre_core {
                                Some(c) => Some(c),
                                None => self.pick_core_pref(&big_pool, &prime_pool),
                            };
                            if let Some(core) = core {
                                self.pin_core(*tid, core, -1, 0, "-", "bg_busy");
                            }
                            debug!(
                                "{}",
                                t_with_args(
                                    "affinity-promoted",
                                    &fluent_args!(
                                        "tid" => tid.to_string(),
                                        "util" => format!("{:.1}", util)
                                    )
                                )
                            );
                        }
                    }
                    self.bg_cursor = if end < n { end } else { 0 };
                    if self.bg_checked.len() > 512 {
                        self.bg_checked.clear();
                    }
                }
            }
        }

        // —— [eem] 相对能效放置：不分前后台，统一按「同算力取更低功耗」动态迁移 ——
        // 位置在失联清理之前：本轮刚采到的 util 立刻参与决策，本轮退出/失联的线程由清理兜底。
        // 与既有 pin/promote/demote 并存但互斥：EEM 接管时那些路径只做显式策略（boost/触摸/组绑定），
        // 普通放置决策一律走 EEM，避免两套机制对同一线程给出矛盾落点。
        if self.eem_placement && screen_on && !boost && !self.core_utils.is_empty() {
            self.eem_place_round(now);
        }

        // —— 失联清理 ——
        let stale: Vec<i32> = self
            .threads
            .iter()
            .filter(|(_, st)| now.duration_since(st.last_seen) > self.tuning.thread_stale())
            .map(|(tid, _)| *tid)
            .collect();
        self.cleanup_threads(&stale, "stale");

        // 每 25 tick 汇总掩码/cpuset/uclamp 写失败次数（有失败才输出；log_enabled! 门控省分配，不新增 @A 列）
        if t % WRITE_FAIL_SUMMARY_EVERY_TICKS == 0 {
            let mask_f = MASK_WRITE_FAILS.swap(0, Ordering::Relaxed);
            let cpuset_f = CPUSET_WRITE_FAILS.swap(0, Ordering::Relaxed);
            let uclamp_f = UCLAMP_WRITE_FAILS.swap(0, Ordering::Relaxed);
            if (mask_f | cpuset_f | uclamp_f) != 0 && log::log_enabled!(log::Level::Debug) {
                debug!(
                    "{}",
                    t_with_args(
                        "affinity-write-fail-summary",
                        &fluent_args!(
                            "mask" => mask_f.to_string(),
                            "cpuset" => cpuset_f.to_string(),
                            "uclamp" => uclamp_f.to_string()
                        )
                    )
                );
            }
        }
    }

    /// [eem] 相对能效放置的一轮：对**所有**已建档线程（不分前后台）做「同算力取更低功耗」的动态迁移。
    ///
    /// 与旧路径的关键差异：
    /// * **不分前后台**：因为有平滑迁移（先扩后缩），前台线程也能安全换核，不必只对后台动刀；
    /// * **迁移优先于升频**：越过甜点才发起迁移，甜点以内交给 CLG 的 `eem_target_freq` 封顶；
    /// * **先扩后缩**：所有区间变更都走 `placement::Engine` 的两拍协议，从不出现空窗口。
    ///
    /// 日志两级（用户口径）：
    /// * INFO：只写「线程 + 方向」，一行一次迁移（`affinity-migrate`）
    /// * devimp：`@A act=place` 帧写全量细节（簇、区间、成本、收益、原因）
    fn eem_place_round(&mut self, now: Instant) {
        // 容量表：进程内一次解析（真机 cpu_capacity 优先，回退 soc.yaml [capacity]）
        if self.core_caps.is_empty() {
            let max_cpu = crate::common::chiri_core_ranges()
                .prime
                .end
                .max(crate::common::chiri_core_ranges().big.end)
                .max(crate::common::chiri_core_ranges().little.end);
            let mut caps = Vec::with_capacity(max_cpu);
            for cpu in 0..max_cpu {
                let c = std::fs::read_to_string(format!(
                    "/sys/devices/system/cpu/cpu{cpu}/cpu_capacity"
                ))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|&v| v > 0)
                .or_else(|| {
                    crate::common::core_group_of(cpu as u32)
                        .and_then(crate::common::soc_capacity_for_group)
                });
                caps.push(c);
            }
            self.core_caps = caps;
        }
        let online = energy_model::online_clusters();
        if online.is_empty() {
            return;
        }
        // 本轮候选：util 达门槛、非关键线程、且**未被显式钉核**。
        // * 关键线程（`is_key`）由 boost / 组绑定独占；
        // * `home >= 0` = 被 `fg_pin` / `home_overload` 钉在单核上，那是显式策略，
        //   EEM 再改它的掩码只会与钉定路径互相覆盖。
        let cands: Vec<(i32, f32)> = self
            .threads
            .iter()
            .filter(|(_, st)| !st.is_key && st.home < 0 && st.util_pct >= self.eem_min_util_pct)
            .map(|(tid, st)| (*tid, st.util_pct))
            .collect();
        if cands.is_empty() {
            return;
        }
        // 三簇当前频率：整轮只读一次（每线程各读一次是纯浪费）
        let cur_freqs = energy_cost::read_cluster_freqs();
        // [ledger] 新一轮决策：清空「本轮已承诺」账。不清会让承诺跨轮累进、谁也迁不动。
        self.eem.begin_round();
        // 本轮已提交的迁移条数上限（保底闸门）：成本项只在跨轮才反映真实负载，
        // 同轮内几十条线程会一起判「同一个簇最便宜」而齐涌，必须按轮封顶。
        // 真正的修法在 placement 侧：见 `placement::Ledger`——每个决策把「我占掉了多少」
        // 记到核上，后续决策按含承诺的有效负载选核，齐涌自然摊平（截断只是兜底）。
        let mut budget = EEM_MAX_MOVES_PER_ROUND;
        let mut actions: Vec<placement::Action> = Vec::new();
        for (tid, util_pct) in cands {
            // 需求：util% × 该线程**当前所在核**的 capacity（缺失按本簇 capacity 兜底）
            let (cluster, home, last_cpu) = match self.threads.get(&tid) {
                Some(st) => (st.eem_cluster, st.home, st.last_cpu),
                None => continue,
            };
            // 起始核：显式钉核 > 记账窗口 > 采样到的当前所在核。
            // 最后一档不能省：首次接管的线程既无 `home` 也无记账簇，缺了它就会被
            // 直接跳过——EEM 对「新出现的线程」永不生效（2026-10-22 审查发现）。
            let core = if home >= 0 {
                home as usize
            } else if let Some(c) = cluster {
                placement::cluster_pool(c).first().copied().unwrap_or(0)
            } else if last_cpu >= 0 {
                last_cpu as usize
            } else {
                continue;
            };
            let cap = self
                .core_caps
                .get(core)
                .copied()
                .flatten()
                .map(|c| c as f32)
                .or_else(|| {
                    cluster
                        .and_then(crate::common::soc_capacity_for_group)
                        .map(|c| c as f32)
                });
            let Some(cap) = cap else { continue };
            let demand = energy_model::thread_demand(util_pct, cap);
            if demand <= 0.0 {
                continue;
            }
            // 当前所在簇：优先记账值，其次由 home 反推
            let cur_cluster = cluster.or_else(|| crate::common::core_group_of(core as u32));
            let Some(cur_cluster) = cur_cluster else {
                continue;
            };
            let pool = placement::cluster_pool(cur_cluster);
            if pool.is_empty() {
                continue;
            }
            // 当前窗口：记账值，否则以 home 为中心取 1 核
            let window = self
                .threads
                .get(&tid)
                .and_then(|st| st.eem_window)
                .unwrap_or_else(|| placement::Window::new(core, core));
            // 是否越过甜点：当前频点对应的档 > 甜点档
            let past_sweet = energy_model::curve(cur_cluster).is_some_and(|cv| {
                let now_khz = cur_freqs[match cur_cluster {
                    CoreGroup::Little => 0,
                    CoreGroup::Big => 1,
                    CoreGroup::Prime => 2,
                }]
                .unwrap_or(0);
                // 当前频点 → 档位下标（≤ 该频点的最大档）
                let gear = (0..cv.len())
                    .filter(|&i| cv.freq_khz(i) <= now_khz)
                    .next_back()
                    .unwrap_or(0);
                gear > cv.sweet()
            });
            let subject = placement::Subject {
                demand,
                cluster: cur_cluster,
                window,
                past_sweet,
            };
            // 建档（首次以当前窗口为稳态）
            self.eem.ensure(tid, cur_cluster, window, now);
            if let Some(a) = self.eem.tick(
                tid,
                &subject,
                &self.core_utils,
                &online,
                self.eem_hysteresis,
                now,
            ) {
                actions.push(a);
                budget = budget.saturating_sub(1);
                if budget == 0 {
                    break;
                }
            }
        }
        // 落盘：全部决策算完再写 sysfs（避免边算边改导致同轮输入漂移）
        for a in actions {
            let (pid, comm) = match self.threads.get(&a.tid) {
                Some(st) => (st.pid, st.comm.clone()),
                None => continue,
            };
            // 32 位线程不能上无 AArch32 的簇
            if !self.mask_write_allowed(a.tid, &a.mask) {
                continue;
            }
            // cpuset 前置：纯后台线程的组通常只含小核，掩码写到 big/prime 会被内核静默裁剪。
            // 故跨到更高能级簇时先把线程升组到 top-app（与既有 promote 同一目的地，
            // 退出由 demote / 失联清理负责还原）——不升组就是「白算一次、日志报 clipped」。
            let dst_group = a.switch_cluster.and_then(|c| match c {
                CoreGroup::Little => None,
                CoreGroup::Big | CoreGroup::Prime => Some(GROUP_TOP_APP),
            });
            if let Some(group) = dst_group
                && let Some(st) = self.threads.get(&a.tid)
                && !st.is_fg
                && !st.moved_group
            {
                let mv = self.move_tid_group(a.tid, group);
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "move_group",
                        pid,
                        a.tid,
                        "-",
                        &comm,
                        group,
                        "-",
                        &io_result_tag(&mv),
                        "eem_promote",
                    );
                }
                if mv.is_ok()
                    && let Some(st) = self.threads.get_mut(&a.tid)
                {
                    st.moved_group = true;
                } else if mv.is_err() {
                    // 升组失败：本线程本轮放弃（掩码一定被裁剪，写它没有意义）。
                    // 必须同时回滚引擎记账：本轮的 `tick` 已经把 window/cluster 改成目标，
                    // 不退回就会与内核实际掩码分叉，下一轮还按假窗口重新规划。
                    self.eem.abort(a.tid, now);
                    continue;
                }
            }
            // [narrow] 目标簇刚被这个进程拒过 → 本轮整条跳过（不发起 syscall、不落 @A 帧），
            // 但**必须回滚引擎记账**：`tick` 已把 window/cluster 改成目标，不回滚下一轮
            // 就会按假窗口重新规划（正是「每轮再失败一次」的来源）。
            let dst_core = match a.dir {
                energy_model::Dir::Up => a.mask.last().copied(),
                _ => a.mask.first().copied(),
            };
            let dst_group = dst_core.and_then(|c| crate::common::core_group_of(c as u32));
            if let Some(g) = dst_group
                && self.narrow_skip(pid, g, now)
            {
                self.eem.abort(a.tid, now);
                continue;
            }
            let res = set_tid_affinity(a.tid, &a.mask);
            // 写后读回：cgroup cpuset 会**静默裁剪**越界核（写返回 0 但掩码没生效）。
            // 不校验就会把「内核没答应」记成成功迁移，日志与事实分叉。
            let effective = res.as_ref().ok().and_then(|_| read_tid_mask(a.tid));
            let clipped = match (&res, &effective) {
                (Ok(()), Some(got)) => a.mask.iter().any(|c| !got.contains(c)),
                _ => false,
            };
            // INFO：只写线程 + 方向（用户口径）
            if res.is_ok() && !clipped {
                info!(
                    "{}",
                    t_with_args(
                        "affinity-migrate",
                        &fluent_args!(
                            "tid" => a.tid.to_string(),
                            "comm" => comm.clone(),
                            "dir" => a.dir.as_str().to_string(),
                            "cores" => format!("{}-{}", a.mask.first().copied().unwrap_or(0), a.mask.last().copied().unwrap_or(0))
                        )
                    )
                );
            }
            // devimp：全量细节
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "place",
                    pid,
                    a.tid,
                    "-",
                    &comm,
                    &format!(
                        "{}-{}",
                        a.mask.first().copied().unwrap_or(0),
                        a.mask.last().copied().unwrap_or(0)
                    ),
                    &format!(
                        "gain={:.3},settled={},reason={},clipped={},eff={}",
                        a.gain,
                        a.settled,
                        a.reason,
                        clipped,
                        // [eem-ack] 回读到的**实际**掩码（read_fail = 读回失败，多为线程已退出）。
                        // 与意图掩码 dst= 对照，即可在不加新 syscall 的前提下看出
                        // 「cpuset 裁剪 / 暂停核」到底吃掉了哪些核 —— 下一轮日志用它对账。
                        effective
                            .as_ref()
                            .map(|v| match (v.first(), v.last()) {
                                (Some(lo), Some(hi)) => format!("{lo}-{hi}"),
                                _ => "none".to_string(),
                            })
                            .unwrap_or_else(|| "read_fail".to_string())
                    ),
                    &io_result_tag(&res),
                    a.dir.as_str(),
                );
            }
            // [eem-ack] 写结果的两种归属（详见 `placement::Engine` 的 note_applied / abort）：
            // * **成功** → 以**回读掩码**为回滚锚点。意图掩码会被 cgroup cpuset 与暂停核
            //   静默摘核（写返回 0 不等于全生效），拿意图记账等于记了个假窗口。
            // * **失败** → 回滚到上一锚点并累加失败计数，退避在 `Engine::tick` 的冷却里生效。
            //   「先扩后缩」的前提是每拍都落地；失败不回滚会让引擎记「已在新核」而内核里
            //   线程还在老掩码上，下一轮按假窗口重新规划 → 每轮再失败一次（sm8550 实测
            //   14 批 `place` 失败 62,531 次、其中 35 个线程陷入连续重复）。
            // 仍只在**收敛步**更新 `eem_window`：过渡步写的是「旧∪新」并集，拿它当窗口镜像
            // 会让下一轮的 direction/宽度判断基于过宽的假窗口。
            match effective.as_ref().filter(|v| !v.is_empty()) {
                Some(eff) => {
                    let cl = eff
                        .first()
                        .copied()
                        .and_then(|c| crate::common::core_group_of(c as u32));
                    if let Some(cl) = cl {
                        self.eem.note_applied(a.tid, cl, eff, now);
                    }
                    // [strip] 意图里有、回读里**整簇没有**的簇：记窄域备忘，15s 内不再往那簇写。
                    // 与 EINVAL 的区别只在「内核用返回值说了」还是「用回读说了」，后果一样。
                    for cl in [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime] {
                        let wanted = a
                            .mask
                            .iter()
                            .any(|&c| crate::common::core_group_of(c as u32) == Some(cl));
                        let got = eff
                            .iter()
                            .any(|&c| crate::common::core_group_of(c as u32) == Some(cl));
                        if wanted && !got {
                            self.note_stripped(pid, cl, now);
                        }
                    }
                    if a.settled
                        && let Some(st) = self.threads.get_mut(&a.tid)
                    {
                        st.eem_window = Some(placement::Window::new(
                            eff.first().copied().unwrap_or(0),
                            eff.last().copied().unwrap_or(0),
                        ));
                        st.eem_cluster = cl;
                    }
                }
                None => {
                    // 失败：先记进程级窄域（仅 EINVAL 生效），再回滚引擎
                    if let Some(g) = dst_group {
                        let errno = res.as_ref().err().and_then(|e| e.raw_os_error());
                        self.note_narrow(pid, g, now, errno);
                    }
                    self.eem.abort(a.tid, now);
                }
            }
        }
    }

    /// [eem] 关闭 EEM 时把被接管的线程掩码还原成「全核」并清记账。
    ///
    /// 不还原的后果：EEM 把线程写成 1 核窗口后若开关被热重载关掉，既有 pin/promote
    /// 路径**不会主动扫**这些线程，它们会永久卡在窄掩码上（性能塌陷且无日志）。
    fn eem_release_all(&mut self) {
        let max_cpu = {
            let r = crate::common::chiri_core_ranges();
            r.prime.end.max(r.big.end)
        };
        let all: Vec<usize> = (0..max_cpu).collect();
        let tids: Vec<i32> = self
            .threads
            .iter()
            .filter(|(_, st)| st.eem_window.is_some())
            .map(|(tid, _)| *tid)
            .collect();
        let n = tids.len();
        for tid in tids {
            if self.mask_write_allowed(tid, &all) {
                let _ = set_tid_affinity(tid, &all);
            }
            self.eem.forget(tid);
            if let Some(st) = self.threads.get_mut(&tid) {
                st.eem_window = None;
                st.eem_cluster = None;
            }
        }
        info!(
            "{}",
            t_with_args("affinity-eem-released", &fluent_args!("n" => n.to_string()))
        );
    }

    /// default 小核高水位下的非关键前台线程升核（normal_busy），仅压力窗口内调用（窗口外零采样零写入）；每轮最多采样 FG_SCAN_WINDOW 个（游标轮转）限制文件 IO
    /// 判定复用busy_window_update 两窗防抖；绑定后 util 跌破释放水位连续 DEMOTE_STREAK 轮才回落（滞回）
    // [fg_promote]
    fn promote_busy_foreground(
        &mut self,
        scan_pool: &[i32],
        perf_pool: &[usize],
        fg_pid: i32,
        now: Instant,
    ) {
        let clk = clk_tck();
        let n = scan_pool.len();
        if n == 0 {
            return;
        }
        self.fg_cursor %= n;
        let end = (self.fg_cursor + FG_SCAN_WINDOW).min(n);
        let pkg = self.fg_cmdline.clone();
        for idx in self.fg_cursor..end {
            let tid = scan_pool[idx];
            if self.threads.get(&tid).is_some_and(|st| st.pin_incapable) {
                continue;
            }
            let Some(s) = sample_one_tid(tid) else {
                continue;
            };
            let util = self.window_util(tid, s.ticks, now, clk).unwrap_or(0.0);
            let (sustained, bound, low) = match self.threads.get_mut(&tid) {
                Some(st) => {
                    let sustained = busy_window_update(
                        st,
                        util,
                        now,
                        self.tuning.fg_busy_util_pct,
                        self.tuning.fg_busy_release_util_pct,
                    );
                    if util < self.tuning.fg_busy_release_util_pct {
                        st.low_streak = st.low_streak.saturating_add(1);
                    } else {
                        st.low_streak = 0;
                    }
                    (sustained, st.group_bind == GroupBind::Busy, st.low_streak)
                }
                None => continue,
            };
            if sustained
                && !bound
                && self.group_bind_precheck(tid, fg_pid, &pkg, perf_pool, "normal_busy")
            {
                // 抬到性能核组（不钉单核，组内交给 EAS 自调度）；perf_pool 本就是切片参数
                let now_bind = Instant::now();
                let res = set_tid_affinity(tid, perf_pool);
                self.record_normal_bind_result(tid, perf_pool, &res, now_bind);
                if crate::logger::diag_active() {
                    let comm = self.thread_comm(tid);
                    crate::logger::aff_action(
                        "pin",
                        fg_pid,
                        tid,
                        &pkg,
                        comm,
                        "group",
                        "-",
                        &io_result_tag(&res),
                        "normal_busy",
                    );
                }
                if res.is_ok() {
                    if let Some(st) = self.threads.get_mut(&tid) {
                        st.group_bind = GroupBind::Busy;
                    }
                    self.record_mask_write(tid, MASK_TAG_PERF, now_bind);
                }
            } else if bound && low >= self.tuning.demote_streak {
                // 空闲回落：恢复全核掩码（restore 内清 group_bind）
                let _ = self.restore_group_mask(tid, fg_pid, &pkg);
            }
        }
        self.fg_cursor = if end < n { end } else { 0 };
    }

    /// 窗口 util%（ticks 增量 / 上次采样至今秒 / CLK_TCK × 100）；首见返回 None
    fn window_util(&mut self, tid: i32, ticks: u64, now: Instant, clk: f32) -> Option<f32> {
        self.threads.get_mut(&tid).map(|st| {
            let dt = now.duration_since(st.last_sample).as_secs_f32().max(0.001);
            let d = ticks.saturating_sub(st.last_ticks);
            st.last_ticks = ticks;
            st.last_sample = now;
            (d as f32 / clk / dt * 100.0).min(100.0)
        })
    }

    /// demote：迁回原 cpuset 组 + 恢复全核（`util` 仅用于日志，由调用点传入当轮窗口 util%）
    fn demote(&mut self, tid: i32, util: f32) {
        let (moved, orig, home) = match self.threads.get(&tid) {
            Some(st) => (st.moved_group, st.orig_group, st.home),
            None => return,
        };
        if moved && !orig.is_empty() {
            let res = self.move_tid_group(tid, orig);
            if crate::logger::diag_active() {
                let comm = self.thread_comm(tid);
                crate::logger::aff_action(
                    "move_group",
                    0,
                    tid,
                    "-",
                    comm,
                    orig,
                    "-",
                    &io_result_tag(&res),
                    "demote",
                );
            }
            // 写成功才清 moved_group：失败时组未迁回，保留标记供下次重试，防状态表与内核分叉
            if res.is_ok() {
                if let Some(st) = self.threads.get_mut(&tid) {
                    st.moved_group = false;
                }
            }
        }
        if home >= 0 {
            let _ = self.unpin_core(tid, home, 0, "-");
        }
        debug!(
            "{}",
            t_with_args(
                "affinity-demoted",
                &fluent_args!("tid" => tid.to_string(), "util" => format!("{:.1}", util))
            )
        );
    }

    // 实验室静态分组（contingency/babel）

    /// 进入/纠偏静态分组：停线程迁移与动态分组，按模式写各业务组 cpus，同模式时仅重写（框架写回的周期纠偏），快照不重复记录
    pub fn lab_static_apply(&mut self, mode: &str) {
        if self.lab_static_mode.as_deref() != Some(mode) {
            // 先恢复接管的收窄（top-app cpus/uclamp/线程绑定回系统值），lab 快照记的才是真实原值
            self.release();
            self.lab_group_snapshot.clear();
            self.lab_static_mode = Some(mode.to_string());
        }
        match mode {
            "contingency" => {
                // 后台进程全部压到 0-1 两颗小核；前台/顶部用全部核心
                let bg = little_pair_list();
                let all = all_cores_list();
                set_groups_cpus_tracked(
                    &[GROUP_TOP_APP, GROUP_FOREGROUND],
                    &all,
                    &mut self.lab_group_snapshot,
                );
                set_groups_cpus_tracked(
                    BACKGROUND_GROUPS.as_slice(),
                    &bg,
                    &mut self.lab_group_snapshot,
                );
            }
            "babel" => {
                // 规整化：后台→小核，前台/顶部→大核+超大核，系统进程→大核
                let bg = little_list();
                let fg = big_plus_prime_list();
                let sys = big_list();
                set_groups_cpus_tracked(
                    &[GROUP_TOP_APP, GROUP_FOREGROUND],
                    &fg,
                    &mut self.lab_group_snapshot,
                );
                set_groups_cpus_tracked(&["system-background"], &sys, &mut self.lab_group_snapshot);
                set_groups_cpus_tracked(
                    &["background", "restricted"],
                    &bg,
                    &mut self.lab_group_snapshot,
                );
            }
            _ => {}
        }
    }

    /// 退出静态分组：恢复各组原值（此后接管方自行 apply）
    pub fn lab_static_deactivate(&mut self) {
        if self.lab_static_mode.take().is_none() {
            return;
        }
        let items: Vec<(String, String)> = self.lab_group_snapshot.drain(..).collect();
        write_cpuset_cpus_items(&items);
    }

    // cgroup 布局 / uclamp / 释放

    fn pin_background(&self, little_list: &str) {
        let mut writes: Vec<(String, String)> = Vec::new();
        for group in BACKGROUND_GROUPS {
            let exist = match group {
                "background" => self.sys.cpuset_background_exist,
                "system-background" => self.sys.cpuset_system_background_exist,
                _ => self.sys.cpuset_restricted_exist,
            };
            if exist {
                writes.push((group.to_string(), little_list.to_string()));
            }
        }
        write_cpuset_cpus_items(&writes);
    }

    fn apply_uclamp(&self, cfg: &AffinityConfig) {
        if cfg.top_app_uclamp_min_pct > 0 && self.sys.cpuctl_top_app_exist {
            let val = cfg.top_app_uclamp_min_pct.to_string();
            let res = logged_write("/dev/cpuctl/top-app/cpu.uclamp.min", &val);
            if res.is_err() {
                UCLAMP_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
            }
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "uclamp",
                    0,
                    0,
                    "-",
                    "-",
                    "/dev/cpuctl/top-app/cpu.uclamp.min",
                    &val,
                    &io_result_tag(&res),
                    "apply_min",
                );
            }
        }
    }

    fn restore_foreground_groups(&self) {
        if let Some(snap) = &self.snapshot {
            let items: Vec<(String, String)> = snap
                .iter()
                .filter(|(path, _)| path.contains(GROUP_TOP_APP) || path.contains(GROUP_FOREGROUND))
                .cloned()
                .collect();
            write_cpuset_paths_items(&items);
        }
    }

    fn restore_uclamp(&self) {
        if let Some(v) = &self.uclamp_snapshot {
            if !v.is_empty() {
                let res = logged_write("/dev/cpuctl/top-app/cpu.uclamp.min", v);
                if res.is_err() {
                    UCLAMP_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
                }
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "uclamp",
                        0,
                        0,
                        "-",
                        "-",
                        "/dev/cpuctl/top-app/cpu.uclamp.min",
                        v,
                        &io_result_tag(&res),
                        "restore_min",
                    );
                }
            }
        }
    }

    fn apply_uclamp_max(&mut self, cfg: &AffinityConfig) {
        let pct = cfg.top_app_uclamp_max_pct;
        if pct == 0 || self.uclamp_max_support == UclampSupport::Unsupported {
            return;
        }
        if self.uclamp_max_support == UclampSupport::Unknown {
            let ver = kernel_version();
            let ver_ok = ver.map_or(false, |(a, b, _)| a > 5 || (a == 5 && b >= 3));
            let node_ok = std::path::Path::new(UCLAMP_MAX_PATH).exists();
            if !ver_ok || !node_ok {
                self.uclamp_max_support = UclampSupport::Unsupported;
                warn!(
                    "{}",
                    t_with_args(
                        "affinity-uclamp-unavailable",
                        &fluent_args!(
                            "version" => ver
                                .map(|(a, b, _)| format!("{a}.{b}"))
                                .unwrap_or_else(|| "unknown".to_string()),
                            "reason" => if ver_ok { "node missing" } else { "kernel < 5.3" }
                        )
                    )
                );
                return;
            }
        }
        let val = format!("{pct}.00");
        let res = logged_write(UCLAMP_MAX_PATH, &val);
        if res.is_err() {
            UCLAMP_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
        }
        if crate::logger::diag_active() {
            crate::logger::aff_action(
                "uclamp",
                0,
                0,
                "-",
                "-",
                UCLAMP_MAX_PATH,
                &val,
                &io_result_tag(&res),
                "apply_max",
            );
        }
        // 回读验证保持原判定口径：不以写入返回值决定是否验证
        {
            let applied = std::fs::read_to_string(UCLAMP_MAX_PATH)
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok())
                .map(|v| (v - pct as f32).abs() < 0.01)
                .unwrap_or(false);
            if applied {
                self.uclamp_max_support = UclampSupport::Ok;
            } else {
                self.uclamp_max_support = UclampSupport::Unsupported;
                log::warn!(
                    "{}",
                    t_with_args(
                        "affinity-uclamp-unavailable",
                        &fluent_args!(
                            "version" => kernel_version()
                                .map(|(a, b, _)| format!("{a}.{b}"))
                                .unwrap_or_else(|| "unknown".to_string()),
                            "reason" => "write not applied"
                        )
                    )
                );
            }
        }
    }

    /// 后台组 uclamp.max 降权（每次 apply 调用，幂等）：钳低 background/restricted 的 util 需求，EAS 放置与 schedutil 频率随之回落、优先小核（不禁止用大核）
    fn apply_bg_uclamp_max(&self, cfg: &AffinityConfig) {
        let pct = cfg.background_uclamp_max_pct;
        if pct == 0 {
            return;
        }
        write_bg_uclamp_max(&format!("{pct}.00"), "bg_apply");
    }

    /// 还原后台组 uclamp.max 为内核默认（"max"）：释放与关闭总闸时调用
    fn restore_bg_uclamp_max(&self) {
        write_bg_uclamp_max("max", "bg_restore");
    }

    fn restore_uclamp_max(&self) {
        if let Some(v) = &self.uclamp_max_snapshot {
            if !v.is_empty() {
                let res = logged_write(UCLAMP_MAX_PATH, v);
                if res.is_err() {
                    UCLAMP_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
                }
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "uclamp",
                        0,
                        0,
                        "-",
                        "-",
                        UCLAMP_MAX_PATH,
                        v,
                        &io_result_tag(&res),
                        "restore_max",
                    );
                }
            }
        }
    }

    /// boost 期放开 top-app uclamp.max 为 100（fas_affinity_hook 与特调激活路径调用）：机型值 85 在真机 waltgov 路径走 effective_cpu_util 限频钳制、不进能量模型，激活期不需要它
    /// 去激活还原时序：激活点均在 boost 进入写 85 之后；去激活时若 boost 已退出则跳过写入（restore_uclamp_max 链已归位）；息屏释放不调本方法，由 boost 退出链兜底还原
    pub fn set_boost_uclamp_override(&mut self, active: bool) {
        if active {
            if self.uclamp_max_support == UclampSupport::Unsupported {
                return;
            }
            if self.uclamp_max_support == UclampSupport::Unknown {
                // 常规时序下 boost 进入已探测支持性；此处兜底同口径（<5.3 / 节点缺失 → 永久跳过）
                let ver_ok =
                    kernel_version().map_or(false, |(a, b, _)| a > 5 || (a == 5 && b >= 3));
                if !ver_ok || !std::path::Path::new(UCLAMP_MAX_PATH).exists() {
                    self.uclamp_max_support = UclampSupport::Unsupported;
                    return;
                }
                self.uclamp_max_support = UclampSupport::Ok;
            }
            if self.boost_uclamp_prev.is_none() {
                self.boost_uclamp_prev = std::fs::read_to_string(UCLAMP_MAX_PATH)
                    .ok()
                    .map(|s| s.trim().to_string());
            }
            // 每轮重写（非幂等短路）：与 mode file/fast_lock 周期重写同口径，兼作防篡改兜底再断言——特调期间每 2s 收敛回 100
            let res = logged_write(UCLAMP_MAX_PATH, "100.00");
            if res.is_err() {
                UCLAMP_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
            }
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "uclamp",
                    0,
                    0,
                    "-",
                    "-",
                    UCLAMP_MAX_PATH,
                    "100.00",
                    &io_result_tag(&res),
                    "override",
                );
            }
        } else if let Some(prev) = self.boost_uclamp_prev.take() {
            if self.applied_kind == KIND_BOOST && !prev.is_empty() {
                let res = logged_write(UCLAMP_MAX_PATH, &prev);
                if res.is_err() {
                    UCLAMP_WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
                }
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "uclamp",
                        0,
                        0,
                        "-",
                        "-",
                        UCLAMP_MAX_PATH,
                        &prev,
                        &io_result_tag(&res),
                        "override_restore",
                    );
                }
            }
        }
    }

    /// 是否持有接管（cpuset 收窄 / 线程迁移 / uclamp 任一已应用）release 前先查此标志，避免周期路径对已释放管理器重复 release（每次都无条件回写后台组 uclamp.max 并打点）
    pub fn is_active(&self) -> bool {
        self.applied_kind != KIND_NONE || self.lab_static_mode.is_some()
    }

    pub fn release(&mut self) {
        self.release_impl("release");
    }

    /// 配置重载清除失败抑制；同值守卫同步失效以允许立即写入
    pub fn invalidate_retry_caches(&mut self) {
        RESTRICTED_UCLAMP_MISSING.clear();
        BG_UCLAMP_CODE.store(0, Ordering::Relaxed);
        BG_UCLAMP_AT_MS.store(0, Ordering::Relaxed);
        for state in self.threads.values_mut() {
            state.normal_bind_retry.clear();
        }
    }

    /// 释放（reason 按场景区分：常规收尾 "release"、总闸关闭 "disabled"），各恢复动作的帧由 write_cpuset_paths_items / restore_* /
    /// cleanup_thread 自落，这里补一条act=bind_release 汇总帧（result = 各线程恢复写入的首个失败 errno，全成功 ok）
    fn release_impl(&mut self, reason: &str) {
        let tids: Vec<i32> = self.threads.keys().copied().collect();
        let err = self.cleanup_threads(&tids, reason);
        if let Some(snap) = self.snapshot.take() {
            write_cpuset_paths_items(&snap);
        }
        self.restore_uclamp();
        self.restore_uclamp_max();
        self.restore_bg_uclamp_max();
        self.applied_kind = KIND_NONE;
        self.last_fg_pid = 0;
        self.last_boost = false;
        self.bg_cursor = 0;
        self.fg_cursor = 0;
        self.bg_checked.clear();
        if crate::logger::diag_active() {
            let result = match err {
                None => "ok".to_string(),
                Some(n) => format!("e{n}"),
            };
            crate::logger::aff_action("bind_release", 0, 0, "-", "-", "-", "-", &result, reason);
        }
        info!("{}", t("affinity-released"));
    }

    /// 快照取数：导出线程状态表 (tid, pid, home, pinned)；pid 未知仍填 0（= 归属未知），
    /// pinned = 单核钉定或性能核组绑定
    pub fn thread_diag(&self) -> Vec<(i32, i32, i16, bool)> {
        self.threads
            .iter()
            .map(|(tid, st)| {
                (
                    *tid,
                    st.pid,
                    st.home,
                    st.home >= 0 || st.group_bind != GroupBind::None,
                )
            })
            .collect()
    }

    /// 快照取数：被管线程所属 pid 集合（去重排序）只收前台归属（is_fg）条目——后台候选虽带真实 tgid，但不是 @S 的「被管进程」口径
    pub fn managed_pids(&self) -> Vec<i32> {
        let mut pids: Vec<i32> = self
            .threads
            .values()
            .filter(|st| st.is_fg)
            .map(|st| st.pid)
            .filter(|&p| p > 0)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::CoreGroupRanges;

    /// 8550 布局：little 0-3 / big 4-6 / prime 7
    fn ranges() -> CoreGroupRanges {
        CoreGroupRanges {
            little: 0..4,
            big: 4..7,
            prime: 7..8,
        }
    }

    #[test]
    fn cpu_group_boundaries() {
        let r = ranges();
        assert_eq!(cpu_group_of(0, &r), Some(CoreGroup::Little));
        assert_eq!(cpu_group_of(3, &r), Some(CoreGroup::Little));
        assert_eq!(cpu_group_of(4, &r), Some(CoreGroup::Big));
        assert_eq!(cpu_group_of(6, &r), Some(CoreGroup::Big));
        assert_eq!(cpu_group_of(7, &r), Some(CoreGroup::Prime));
        // 越界（不在任何簇）
        assert_eq!(cpu_group_of(8, &r), None);
        assert_eq!(cpu_group_of(64, &r), None);
    }

    #[test]
    fn aarch32_little_only_rejects_big_prime() {
        let clusters = vec!["little".to_string()];
        assert!(cluster_is_aarch32(&clusters, CoreGroup::Little));
        assert!(!cluster_is_aarch32(&clusters, CoreGroup::Big));
        assert!(!cluster_is_aarch32(&clusters, CoreGroup::Prime));
    }
}
