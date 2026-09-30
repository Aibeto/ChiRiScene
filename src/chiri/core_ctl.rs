//! core_ctl.rs: [types] [helpers] [state] [boost] [max_cpus] [scenemode] [online_reader] [restore]

/// 核心在线控制器接管（ChiRi 专属）。三态状态机（互斥，按「最近一次 apply」切换，内部去重）：
/// - **Boost**（boost/vector/特调）：各 cluster 的 core_ctl `min_cpus` 抬到全组常在线，防止低负载时
/// 热插拔回滞把大核下线、与 ChiRi 升降频决策打架；
/// - **Scenemode 离线**（息屏深度省电）：**逐核 `online=0` 下线 little 簇除引导核外的全部核**
/// （2026-09-28 由「只下线 prime」改来，原因见 `scenemode_targets`），再用 WALT core_ctl 的 `max_cpus`
/// 钳住该簇在线核数上界（little 写 `1`，不含引导核的簇才用 `0`——0 是 walt_halt_cpus 整簇停摆语义，
/// 见 `scenemode_keep_cpus`），否则 core_ctl 会按自己的 `[min_cpus, max_cpus]` 区间把核重新拉起（2s 拉锯
/// = hotplug churn）。big / prime 常驻低频（频率上限由 scenemode CLG 配置压制）；另将**编号最大的大核**
/// **独占**给调度服务（从业务 cpuset 组移除 + 自身线程移入根组 + 全线程自钉）；周期重入纠偏（被异常拉起
/// 的核重新下线——节点被改写属异常态；max_cpus 被改回按 60s 冷却重写一次，退出按快照恢复）；
/// - **Normal**：恢复全部快照（min_cpus / online）
/// 用 min_cpus/online 而非逐核「按需唤醒」：点亮大核净亏能，且直接写 online 会与热插拔回滞打架——
/// 需要更多在线核时的正确姿势是抬 min_cpus（Boost 态），让内核按自己的回滞策略管理唤醒
/// cluster 发现：遍历 cpufreq policy → related_cpus 首个 CPU 的 core_ctl 目录（每 policy 一份，天然去重）；
/// 直接 sysfs 离线不依赖 core_ctl 节点（由 `core_ctl.enabled` 配置门控）
use crate::chiri::affinity::{io_result_tag, set_tid_affinity};
use crate::chiri::get_cpu_policies;
use crate::utils::FastReader;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [types]
/// 状态：无接管
const STATE_NONE: u8 = 0;
/// 状态：boost（min_cpus 全组常在线）
const STATE_BOOST: u8 = 1;
/// 状态：scenemode 离线（little 簇除引导核外全部下线，big / prime 常驻低频，1 颗大核独占给调度服务）
const STATE_SCENEMODE: u8 = 2;

/// max_cpus 维持期纠偏冷却：60s 内只纠偏一次——逐 2s 重写会与厂商 pipeline 拉锯产生写放大，60s 足以感知并收敛误改
const MAX_CPUS_REASSERT_COOLDOWN: Duration = Duration::from_secs(60);

/// `max_cpus` 写回失败的宽限：核都回来了、只剩 max_cpus 迟迟写不回去（节点被厂商锁写）时，超过本时长
/// 即释放独占（cpuset 排除 + 全线程自钉）——否则亮屏后调度服务会长期钉在大核上、业务侧少一颗 big，
/// 而钳制残留由 `restore_max_cpus` 继续重试（日志可见）。2026-09-28 审查发现
const MAX_CPUS_RELEASE_GRACE: Duration = Duration::from_secs(5);

/// 单个 cluster 的 core_ctl 控制节点
struct CoreCtlCluster {
    /// core_ctl 目录（如 /sys/devices/system/cpu/cpu3/core_ctl）
    dir: String,
    /// cluster 首核号（scenemode 目标簇匹配用：目标是哪个簇就按它的首核号找；含 0 的簇不走 halt 语义）
    first_cpu: u32,
    /// cluster 内 CPU 数（boost 时 min_cpus 的目标值）
    cluster_size: u32,
    /// 快照的原始 min_cpus
    min_cpus: String,
/// 快照的原始 max_cpus；None = 节点不可读，该簇走逐核 offline 兜底
    max_cpus: Option<String>,
    /// 快照的 core_ctl enable；Some(false) = core_ctl 不执行压制——apply_limits() 直接 `return cluster->num_cpus`
    /// （见 kernel/sched/walt/core_ctl.c），min_cpus / max_cpus 写入虽被接受但不生效，跳过无效写走兜底（仅快照期读一次，不做周期重读——厂商可能动态改 enable）
    enable: Option<bool>,
}

/// 核心在线接管器
pub struct CoreCtlManager {
    /// 可控 cluster 列表；惰性发现（首次进入 boost/scenemode 前枚举一次）
    clusters: Vec<CoreCtlCluster>,
    /// 是否已完成发现（含发现结果为空的情况，避免重复枚举）
    discovered: bool,
    /// 当前状态（去重写）
    state: u8,
    /// scenemode 下被本模块下线的 CPU 及其原始 online 值（恢复用）
    offlined: Vec<(u32, String)>,
/// scenemode 被 core_ctl 钳过 `max_cpus` 的簇下标（写入值见 `scenemode_keep_cpus`）；None = 当前无钳制记账，
/// 恢复按该簇快照写回与 `offlined` **可并存**（core_ctl 只钳上界、逐核写负责落地，两者各自记账各自恢复）
    max_cpus_applied: Option<usize>,
    /// `max_cpus` 首次写回失败的记账时刻：配合 `MAX_CPUS_RELEASE_GRACE` 决定「写不回去也要释放独占」
    max_cpus_restore_since: Option<Instant>,
    /// scenemode 压过 `min_cpus` 的簇下标（仅当快照 `min_cpus > scenemode_keep_cpus()` 时才压）。
    /// 必须独立记账：`min_cpus` 的写回**不依赖** `max_cpus` 是否写回成功（写回失败只说明内核此刻以
    /// `max < min` 拒绝，保留记账下周期重试即可）——早先按「max_cpus 记账为空才恢复」的写法在
    /// `max_cpus` 被厂商锁写时会让 min_cpus 永久停在 1（元审查发现）
    min_cpus_lowered: Option<usize>,
    /// max_cpus 节点不可用/被厂商改写的 warn 一次性标记（防 2s 周期纠偏刷屏）
    max_cpus_warned: bool,
    /// boost 期遇到 enable=0 簇的 warn 一次性标记（core_ctl 不执行压制、整簇本就常在线，跳过写入并打 e0）
    boost_enable_warned: bool,
/// 上次 max_cpus 纠偏（重写目标值）时刻：冷却内不重复，防与厂商 pipeline 逐周期拉锯
    max_cpus_reassert_at: Option<Instant>,
    /// scenemode 下守护进程自身线程是否已**全部**钉到专用核（部分失败为 false，下次触发重试钉定）
    self_pinned: bool,
/// 实际钉住的自身 tid 清单（**只记成功**）：unpin 按它逐个恢复，避免部分失败时已钉线程永久滞留单核掩码
    self_pinned_tids: Vec<i32>,
    /// scenemode 独占给调度服务的核（取编号最大的大核——小核已整簇下线；None = 未独占/设备无 cpuset 时降级）
    reserved_core: Option<usize>,
    /// 独占时被移除核的业务 cpuset 组快照（(组名, 原始 cpus)，退出恢复用）
    reserved_cpusets: Vec<(String, String)>,
    /// 自身线程被移入根组前的原 cpuset 组相对路径（退出恢复用）
    self_cpuset_group: Option<String>,
/// [fast_reader] cpuN/online 的 keep-open 读取器（缓存路径，周期纠偏/恢复回读免每 2s format! + open/close），按核号惰性建立
    online_readers: HashMap<u32, FastReader>,
}

// [helpers]
/// 枚举守护进程自身全部线程 TID（/proc/self/task）
fn self_tids() -> Vec<i32> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            if let Some(t) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
                out.push(t);
            }
        }
    }
    out
}

/// 计算 scenemode 下线目标：**little 簇**（息屏深度省电 = 下线小核，big / prime 常驻低频。
/// 2026-09-28 由「下线 prime」改来：原策略把后台负载全挤到 3 个小核上，实测息屏期 little 反复被顶满
/// 100%，反过来触发 saturation 保护退回 reduce，息屏省电被反复自我撤销）。
/// 2026-10-01：本策略仍会把负载挤到唯一在线的引导核 CPU0（同样顶满 → 同样被撤销），故改为**按机型可选**——
/// 由 `CoreCtl.scenemode_offline` 控制，8550 置 false 走「不下线、只压频」（见 8550/feature.yaml [corectl]）。
/// 引导核 CPU0 无法热拔出（内核拒写 `cpu0/online=0`），故「关闭所有小核」实为 little 簇除 CPU0 外的全部核。
/// 保留 CPU0 还顺带消除一个坑：little 的 cpufreq policy 不会因整簇消失而让 worker 失去接管
/// （prime 整簇下线时 policy 目录消失，见 chiri/mod.rs 饱和退出处的 reload 顺序说明）
fn scenemode_targets() -> Vec<u32> {
    let ranges = crate::common::chiri_core_ranges();
    let mut targets: Vec<u32> = ranges.little.clone().map(|c| c as u32).collect();
    // 引导核（CPU0）永远在线；写 online=0 会被内核拒绝，登记进来只会记一条假失败
    targets.retain(|&c| c != 0);
    targets
}

/// scenemode 目标簇要保留的在线核数（core_ctl `max_cpus` 写入值）：
/// - 含引导核的簇（little）保留 **1**：**绝不能写 0**——core_ctl 的 0 是 walt_halt_cpus「整簇停摆」语义
///   （`is_active = cpu_active && !cpu_halted`），把引导核一起停摆是系统级事故；
/// - 不含引导核的簇沿用 0（整簇 halt，即原 prime 路径的语义）
fn scenemode_keep_cpus(cluster: &CoreCtlCluster) -> u32 {
    if cluster.first_cpu == 0 {
        1
    } else {
        0
    }
}

// [state]
impl CoreCtlManager {
    pub fn new() -> Self {
        Self {
            clusters: Vec::new(),
            discovered: false,
            state: STATE_NONE,
            offlined: Vec::new(),
            max_cpus_applied: None,
            max_cpus_restore_since: None,
            min_cpus_lowered: None,
            max_cpus_warned: false,
            boost_enable_warned: false,
            max_cpus_reassert_at: None,
            self_pinned: false,
            self_pinned_tids: Vec::new(),
            reserved_core: None,
            reserved_cpusets: Vec::new(),
            self_cpuset_group: None,
            online_readers: HashMap::new(),
        }
    }

/// 枚举各 cluster 的 core_ctl 控制节点并快照 min_cpus；无任何节点（内核不支持/未启用）时打点一次，之后保持空表
    fn discover(&mut self) {
        if self.discovered {
            return;
        }
        self.discovered = true;
        for policy in get_cpu_policies() {
            // related_cpus 首个 CPU 即该 cluster 的代表（core_ctl 挂在 cluster 首 CPU 下）
            let related = fs::read_to_string(format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/related_cpus",
                policy.id
            ))
            .or_else(|_| {
                fs::read_to_string(format!(
                    "/sys/devices/system/cpu/cpufreq/policy{}/affected_cpus",
                    policy.id
                ))
            })
            .unwrap_or_default();
            let cpus: Vec<u32> = related
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            let Some(first) = cpus.first() else {
                continue;
            };
            let dir = format!("/sys/devices/system/cpu/cpu{}/core_ctl", first);
            let min_path = format!("{}/min_cpus", dir);
            if let Ok(val) = fs::read_to_string(&min_path) {
                let val = val.trim().to_string();
                if val.is_empty() {
                    continue;
                }
                let mut max_cpus = fs::read_to_string(format!("{dir}/max_cpus"))
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty());
// [residual] 崩溃残留自愈：上次可能死在 scenemode 中途，max_cpus=0 残留会让 core_ctl 持续 halt 整簇到重启内核约束 max_cpus ≥ min_cpus ≥ 1，
// 合法原值不可能为 0——快照解析为 0 / 低于下界即视为残留，直接写满配核数解除 halt、快照同步为写回值；写/回读失败记 None（节点不可用口径），restore 不把 "0" 当原值恢复快照为磁盘现读，
// 不做此自愈则「快照 == 磁盘」双重比对恒成立、残留永远跳过写回
// [residual_min] 已知残留（2026-09-28 审查记录，**刻意不做自动修复**）：本判据以**磁盘 min_cpus** 为下界，
// 而 scenemode 新增了「把 min_cpus 压到 keep」这一动作——若进程在 scenemode 中途被强杀，磁盘会留下
// `min_cpus == max_cpus == keep`（如 1），此时 `n < val` 恒假 → 这里不再自愈，且 min_cpus 没有任何自愈路径
// （`force_online_all` 只写 max_cpus / online；**不能盲写 cluster_size**：厂商把 little 的 min_cpus 配成小于
// 簇规模是正常省电，盲写会破坏它）。触发需同时满足「快照 min_cpus > keep（即压过 min_cpus）」+「崩溃」，
// 边界见 07-todos；彻底解决需落一个持久化的「scenemode 进行中」标记
                if let Some(v) = max_cpus.as_deref() {
                    let residual = match v.parse::<u32>() {
                        Ok(n) => n == 0 || n < val.parse::<u32>().unwrap_or(1),
                        // 非数值内容不是 halt 残留形态，按原样快照
                        Err(_) => false,
                    };
                    if residual {
                        let path = format!("{dir}/max_cpus");
                        let fix = cpus.len().to_string();
                        let res = fs::write(&path, &fix);
                        if crate::logger::diag_active() {
                            crate::logger::aff_action(
                                "corectl",
                                0,
                                0,
                                "-",
                                "-",
                                "max_cpus",
                                &fix,
                                &io_result_tag(&res),
                                &path,
                            );
                        }
                        max_cpus = if res.is_ok() {
                            // 回读确认写回生效；确认失败按节点不可用降级
                            fs::read_to_string(&path)
                                .ok()
                                .map(|s| s.trim().to_string())
                                .filter(|s| s == &fix)
                        } else {
                            warn!(
                                "{}",
                                t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                            );
                            None
                        };
                    }
                }
                // enable 快照（读失败记 None = 保持原有 max_cpus 压制尝试）：enable=0 时 core_ctl 不执行压制
                // （apply_limits 直接返回整簇核数，写入虽被接受但不生效），记 false 供 scenemode 省掉无效写（见 clamp_scenemode_cluster_via_max_cpus）
                let enable = fs::read_to_string(format!("{dir}/enable")).ok().map(|v| {
                        let v = v.trim();
                        v == "1" || v.eq_ignore_ascii_case("true")
                    });
                self.clusters.push(CoreCtlCluster {
                    dir,
                    first_cpu: *first,
                    cluster_size: cpus.len() as u32,
                    min_cpus: val,
                    max_cpus,
                    enable,
                });
// TODO: 快照为启动期单次读取，接管后厂商改写 max_cpus 时恢复会按旧快照覆盖；纠偏冷却已部分缓解，彻底解决需恢复前重读比对
            }
        }
        if self.clusters.is_empty() {
            info!("{}", t("corectl-unavailable"));
        }
    }

/// 统一状态入口（内部去重，可被 2s 周期安全调用）：boost = min_cpus 全组常在线；scenemode = little 簇除引导核外下线 + 独占一颗大核给调度服务；两者互斥，切换时先退出旧状态（恢复快照）；
/// scenemode 维持期每次纠偏；NONE / BOOST 稳态下每 2s 重试恢复失败残留核（restore 失败不 clear），否则被下线的核会离线到下次模式切换
    pub fn set_power_state(&mut self, boost: bool, scenemode: bool) {
        let target = if boost {
            STATE_BOOST
        } else if scenemode {
            STATE_SCENEMODE
        } else {
            STATE_NONE
        };
        if target == self.state {
            if target == STATE_SCENEMODE {
// 维持期纠偏：被异常拉起的核（残留旧模块/手动调试/内核异常等异常态，ChiRi 默认无竞争者）重新收敛回离线
                self.reassert_offline();
            } else if !self.offlined.is_empty()
                || self.max_cpus_applied.is_some()
                // BOOST 稳态不恢复 min_cpus：boost 自己把 min_cpus 写成全组常在线，恢复快照会与它拉锯
                // （进入 BOOST 时会清掉 scenemode 的记账，见下方 BOOST 分支）
                || (self.min_cpus_lowered.is_some() && target != STATE_BOOST)
            {
// 恢复失败残留核 / max_cpus / 被压低的 min_cpus 的周期重试：BOOST 期不重试则 prime 持续离线到模式切换（如整场游戏无超大核）
                self.restore_online();
            }
            return;
        }

        // 先退出当前状态（恢复快照）
        match self.state {
            STATE_BOOST => self.restore_min_cpus(),
            STATE_SCENEMODE => self.restore_online(),
            _ => {}
        }
// 残留核兜底：进入新状态前先拉回在线，否则重进 scenemode 时读到 online=0 会跳过登记、永远失去恢复记录（max_cpus / 被压低的 min_cpus 记账残留同样先恢复）
        if !self.offlined.is_empty()
            || self.max_cpus_applied.is_some()
            || self.min_cpus_lowered.is_some()
        {
            self.restore_online();
        }

        // 进入目标状态
        match target {
            // [boost]
            STATE_BOOST => {
                self.discover();
                // boost 会把各簇 min_cpus 写成全组常在线，scenemode 的压低意图已被取代 → 清记账，
                // 免得稳态重试把它写回小值、与 boost 保核拉锯。即便某簇 enable=0 时 boost 跳过写入
                // （磁盘仍留 keep），退出 boost 的 restore_min_cpus() 也会按快照全量纠正 → 清账不留错配
                self.min_cpus_lowered = None;
                for i in 0..self.clusters.len() {
                    let path = format!("{}/min_cpus", self.clusters[i].dir);
                    let val = self.clusters[i].cluster_size.to_string();
                    // enable=0（快照期读一次）时 core_ctl 的 apply_limits() 直接 return num_cpus——
                    // 见 kernel/sched/walt/core_ctl.c 的 `if (!cluster->enable) return cluster->num_cpus;`：
                    // min_cpus 写入虽被接受但不参与压制，且整簇本就常在线（正是 boost 保核想要的目标状态），
                    // 故不写、按「该簇实际未生效」记 e0；原来无脑 fs::write 会把这种静默失效的 Ok 记成 ok，
                    // 打点与真实能力分叉
                    if self.clusters[i].enable == Some(false) {
                        if crate::logger::diag_active() {
                            crate::logger::aff_action(
                                "corectl", 0, 0, "-", "-", "min_cpus", &val, "e0", &path,
                            );
                        }
                        if !self.boost_enable_warned {
                            self.boost_enable_warned = true;
                            warn!(
                                "{}",
                                t_with_args(
                                    "corectl-enable-off-boost",
                                    &fluent_args!("path" => format!("{}/enable", self.clusters[i].dir))
                                )
                            );
                        }
                        continue;
                    }
                    let res = fs::write(&path, &val);
                    if crate::logger::diag_active() {
                        crate::logger::aff_action(
                            "corectl",
                            0,
                            0,
                            "-",
                            "-",
                            "min_cpus",
                            &val,
                            &io_result_tag(&res),
                            &path,
                        );
                    }
                    if res.is_err() {
                        warn!(
                            "{}",
                            t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                        );
                    }
                }
                if !self.clusters.is_empty() {
                    info!(
                        "{}",
                        t_with_args(
                            "corectl-boost-on",
                            &fluent_args!("count" => self.clusters.len().to_string())
                        )
                    );
                }
            }
            STATE_SCENEMODE => {
                self.offline_cores();
            }
            _ => {}
        }
        self.state = target;
    }

/// scenemode 压制入口：**逐核写 `online=0` 下线 little 簇除引导核外的核**，回读验证、失败/被拒的核跳过记 warn，
/// 成功下线的核连同原始 online 记入 offlined；随后独占一颗**大核**给调度服务（选编号最大大核，从业务 cpuset
/// 组移除 + 自身线程移入根组 + 全线程自钉，设备无 cpuset 时降级为仅自钉），保证后台任务堵塞不了调度服务
    // [max_cpus]
// [max_cpus] scenemode 的 core_ctl 钳制：把目标簇 WALT core_ctl 的 `max_cpus` 写成 `scenemode_keep_cpus()`
// （含引导核的 little 簇 = 1；不含引导核的簇 = 0，即 walt_halt_cpus 整簇停摆语义）。
// 事件驱动单次写 + 记账防重复写；返回 false = core_ctl 路径不可用/未接受——无目标簇 / 无 max_cpus 节点、
// enable=0（core_ctl 不执行压制，跳过无效写）、写失败、读回不等于目标值（厂商并发改写）——均首次 warn 一次，
// 不重试不拉锯。**返回 false 不影响逐核 offline 落地**（调用方两步都做，逐核写是主力、core_ctl 只负责钳上界）
// 读回失败按「写已发出」记账 Some 防恢复漏记，交维持期 reassert 纠偏
    fn clamp_scenemode_cluster_via_max_cpus(&mut self) -> bool {
        // 已走 core_ctl 路径且未恢复（恢复失败残留重入）：不重复写
        if self.max_cpus_applied.is_some() {
            return true;
        }
        self.discover();
        // 目标簇 = **含**下线目标的簇，按「首核号 + 簇内核数」区间定位。**不能**拿
        // `scenemode_targets().first()` 去比 `first_cpu`：目标已 retain 掉引导核 CPU0，而 CPU0 正是
        // little 簇首核 → `0 == 1` 恒为假、永不命中（2026-09-28 审查发现：目标由「下线 prime」改成
        // little 时漏改定位，整条 core_ctl 路径静默失效——max_cpus 钳制、引导核护栏、min_cpus 压低与
        // 两处记账全成死代码，scenemode 只剩逐核 offline 与 core_ctl 每 2s 拉锯）
        let Some(target) = scenemode_targets().first().copied() else {
            return false;
        };
        let Some(idx) = self.clusters.iter().position(|c| {
            let first = c.first_cpu as usize;
            (target as usize) >= first && (target as usize) < first + c.cluster_size as usize
        }) else {
            return false;
        };
        if self.clusters[idx].max_cpus.is_none() {
            if !self.max_cpus_warned {
                self.max_cpus_warned = true;
                warn!(
                    "{}",
                    t_with_args(
                        "corectl-node-missing",
                        &fluent_args!("path" => format!("{}/max_cpus", self.clusters[idx].dir))
                    )
                );
            }
            return false;
        }
        // enable 感知（快照期读一次，仅省写优化）：enable=0 时 core_ctl 不执行压制（写入被接受但不生效），直接降级兜底（warn 复用 max_cpus_warned 去重）；enable == None（读不到）保持现行为
        if self.clusters[idx].enable == Some(false) {
            if !self.max_cpus_warned {
                self.max_cpus_warned = true;
                warn!(
                    "{}",
                    t_with_args(
                        "corectl-enable-off",
                        &fluent_args!("path" => format!("{}/enable", self.clusters[idx].dir))
                    )
                );
            }
            return false;
        }
        let path = format!("{}/max_cpus", self.clusters[idx].dir);
        let keep = scenemode_keep_cpus(&self.clusters[idx]);
        let keep_s = keep.to_string();
        // 内核约束 `max_cpus >= min_cpus`：快照 min_cpus 高于 keep 时写 max_cpus 会被拒（读回不符 → 本函数
        // 记「未生效」），而 core_ctl 仍会按 min_cpus 把核拉回 → 与逐核 offline 每 2s 拉锯。故先把该簇
        // min_cpus 压到 keep（**降低**下限，合法方向）；退出由 restore_online 里的 restore_min_cpus 按快照写回
        if self.clusters[idx]
            .min_cpus
            .parse::<u32>()
            .map(|m| m > keep)
            .unwrap_or(false)
        {
            let min_path = format!("{}/min_cpus", self.clusters[idx].dir);
            let res = fs::write(&min_path, &keep_s);
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "corectl", 0, 0, "-", "-", "min_cpus", &keep_s, &io_result_tag(&res), &min_path,
                );
            }
            if res.is_ok() {
                // 记账（与 max_cpus 独立）：退出时由 restore_scenemode_min_cpus 按快照写回，
                // 写回失败保留记账、下周期重试——否则 min_cpus 会永久停在 1（业务组少核、core_ctl 也可能照它下线）
                self.min_cpus_lowered = Some(idx);
            }
            if res.is_err() {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => min_path))
                );
            }
        }
        let res = fs::write(&path, &keep_s);
        if crate::logger::diag_active() {
            crate::logger::aff_action(
                "corectl",
                0,
                0,
                "-",
                "-",
                "max_cpus",
                &keep_s,
                &io_result_tag(&res),
                &path,
            );
        }
        // 护栏：keep==1 的整个前提是「1 是停摆档（0）之外的合法值、引导核不会掉线」。万一某内核把
        // 「<= 1」也当成 walt_halt_cpus 的停摆条件，含引导核的簇写 1 就等于把 CPU0 停摆（系统级事故）。
        // 写后回读 `cpu0/online`（节点不存在 = 该机型 CPU0 本就不可热插拔，视为安全），一旦读到 0
        // 立刻回滚 max_cpus 并放弃 core_ctl 路径（逐核 offline 兜底永远不碰 CPU0）
        if keep == 1 && self.clusters[idx].first_cpu == 0 {
            let cpu0 = "/sys/devices/system/cpu/cpu0/online";
            if fs::read_to_string(cpu0)
                .map(|s| s.trim() == "0")
                .unwrap_or(false)
            {
                if let Some(snap) = self.clusters[idx].max_cpus.clone() {
                    let _ = fs::write(&path, &snap);
                }
                warn!("{}", t("corectl-reserved-core-halted"));
                return false;
            }
        }
        if res.is_err() {
            warn!(
                "{}",
                t_with_args("corectl-write-failed", &fluent_args!("path" => path))
            );
            return false;
        }
// 读回校验须区分「读失败」与「读回不等于目标值」：读失败按已生效记账 Some（若按未生效降级，退出恢复时 core_ctl 仍持压制值会继续压，压制泄漏到 scenemode 之外）；
// 仅读回确认不等于目标值才视为未生效、记账保持 None
        let mut read_back = None;
        for _ in 0..3 {
            match fs::read_to_string(&path) {
                Ok(s) => {
                    read_back = Some(s.trim().to_string());
                    break;
                }
                Err(_) => continue,
            }
        }
        match read_back {
            Some(v) if v == keep_s => {
                self.max_cpus_applied = Some(idx);
                true
            }
            Some(_) => {
                if !self.max_cpus_warned {
                    self.max_cpus_warned = true;
                    warn!(
                        "{}",
                        t_with_args("corectl-vendor-override", &fluent_args!("path" => path))
                    );
                }
                false
            }
            None => {
                // 读回持续失败：写已发出、生效状态未知，按已生效记账防恢复漏记（restore_max_cpus 按快照写回；即使 halt 未生效，写回快照原值也无害）。维持期 reassert 会重读并纠偏
                warn!(
                    "{}",
                    t_with_args("corectl-verify-failed", &fluent_args!("path" => path))
                );
                self.max_cpus_applied = Some(idx);
                true
            }
        }
    }

/// scenemode 退出：恢复目标簇 max_cpus 快照（记账 + 读回双重比对，磁盘已是快照值则零写跳过）；恢复失败保留记账，
/// 由 set_power_state 周期重试路径重入（与 offlined 残留核同款）
    fn restore_max_cpus(&mut self) {
        let Some(idx) = self.max_cpus_applied else {
            return;
        };
        let Some(snap) = self.clusters[idx].max_cpus.clone() else {
            self.max_cpus_applied = None;
            self.max_cpus_restore_since = None;
            return;
        };
        let path = format!("{}/max_cpus", self.clusters[idx].dir);
        // 双重比对：磁盘内容已是快照值（厂商已改回/上一轮已写成功）只读跳过
        if fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim().to_string())
            .as_deref()
            == Some(snap.as_str())
        {
            self.max_cpus_applied = None;
            self.max_cpus_restore_since = None;
            return;
        }
        let res = fs::write(&path, &snap);
        if crate::logger::diag_active() {
            crate::logger::aff_action(
                "corectl",
                0,
                0,
                "-",
                "-",
                "max_cpus",
                &snap,
                &io_result_tag(&res),
                &path,
            );
        }
        // 读回验证；失败保留记账待下个周期重试，降 debug 防刷屏
        let ok = fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim() == snap)
            .unwrap_or(false);
        if ok {
            self.max_cpus_applied = None;
            self.max_cpus_restore_since = None;
        } else {
            // 首次失败起计时：超 MAX_CPUS_RELEASE_GRACE 仍未写回 → 释放独占（见 restore_online）
            self.max_cpus_restore_since.get_or_insert_with(Instant::now);
            debug!(
                "{}",
                t_with_args("corectl-write-failed", &fluent_args!("path" => path))
            );
        }
    }

    // [scenemode]
/// scenemode 压制入口：**两步都做**——① core_ctl 钳住目标簇在线核数上界（max_cpus_applied 记账），
/// ② 逐核写 online=0 落地（offlined 记账）；恢复时两条各自按快照回写，随后独占一颗大核（共用）
    fn offline_cores(&mut self) {
        // ① 首选 core_ctl 钳住目标簇的在线核数上界（little → `max_cpus=1`，保留引导核）：只靠逐核写 online
        //    会被 core_ctl 按自己的 [min_cpus, max_cpus] 区间重新拉起 → 2s 周期拉锯 = hotplug churn
        let clamped = self.clamp_scenemode_cluster_via_max_cpus();
        // ② 逐核 offline 落地：core_ctl 是**异步**收敛，逐核写让当下立即生效；若 core_ctl 已先收敛，
        //    逐核读到的 orig 不是 "1"，自然跳过（不登记、退出也不把核拉回——此时核由 core_ctl 自己管）
        self.offline_cores_direct();
        if clamped && self.offlined.is_empty() {
            // core_ctl 路径已收敛、逐核写全部跳过：补一条成功日志表明压制已生效
            info!("{}", t("corectl-scenemode-halt"));
        }
// 独占给调度服务的核：little 已整簇下线，改取**编号最大的大核**（大核空出 1 颗换调度服务不被后台堵塞，
// 代价是业务侧少 1 颗 big）sched_setaffinity 自钉不排他，须同时把该核移出业务 cpuset 组 +
// 自身线程移入根组（否则被组掩码二次过滤）
        let ranges = crate::common::chiri_core_ranges();
        let reserved = ranges.big.clone().last().or_else(|| ranges.little.clone().last());
        if let Some(core) = reserved {
            crate::chiri::affinity::exclude_core_from_cpusets(core, &mut self.reserved_cpusets);
            self.self_cpuset_group = crate::chiri::affinity::move_self_to_cpuset_root();
            self.reserved_core = Some(core);
        }
        // 专用核自钉
        self.pin_self_dedicated();
        if !self.offlined.is_empty() {
            info!(
                "{}",
                t_with_args(
                    "corectl-scenemode-on",
                    &fluent_args!("count" => self.offlined.len().to_string())
                )
            );
        }
    }

/// 主力路径：逐核写 online=0 下线 little 簇除引导核外的核回读验证，失败跳过（记 warn）；成功下线的核连同原始 online 值记入 offlined 供恢复。
/// **已离线的核（orig != "1"）不登记也不恢复**——那种形态说明 core_ctl 自己收敛了，核归它管
    fn offline_cores_direct(&mut self) {
        for cpu in scenemode_targets() {
            // 防重复：上轮恢复失败的残留核（已在 offlined 中）跳过重复登记
            if self.offlined.iter().any(|(c, _)| *c == cpu) {
                continue;
            }
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            let orig = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            // 本来就离线（或节点不存在/不可读）：不记录、不恢复
            if orig != "1" {
                continue;
            }
            let res = fs::write(&path, "0");
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "corectl",
                    0,
                    0,
                    "-",
                    "-",
                    "online",
                    "0",
                    &io_result_tag(&res),
                    &path,
                );
            }
            if res.is_err() {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
                continue;
            }
            // 回读验证：防内核静默拒绝（热插拔锁等异常态）
            let now = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if now != "0" {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
                continue;
            }
            self.offlined.push((cpu, orig));
        }
    }

/// 把守护进程自身全部线程钉到专用核（offline_cores 已独占的 reserved_core：已移出业务 cpuset、自身线程已移根组，sched_setaffinity 由此实现真独占），调度服务独享该核、
/// 后台任务堵塞不了
    fn pin_self_dedicated(&mut self) {
        if self.self_pinned {
            return;
        }
        let Some(core) = self.reserved_core else {
            return;
        };
        let mut all_ok = true;
        // 成功清单每次重建（防重复累积）：失败 tid 不入清单（掩码未变无需恢复）
        let mut pinned = Vec::new();
        for tid in self_tids() {
            let res = set_tid_affinity(tid, &[core]);
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "self_pin",
                    std::process::id() as i32,
                    tid,
                    "-",
                    "-",
                    &core.to_string(),
                    "-",
                    &io_result_tag(&res),
                    "scenemode",
                );
            }
            match res {
                Ok(()) => pinned.push(tid),
                Err(_) => all_ok = false,
            }
        }
        self.self_pinned = all_ok;
        self.self_pinned_tids = pinned;
        debug!(
            "{}",
            t_with_args(
                "corectl-self-pinned",
                &fluent_args!("core" => core.to_string())
            )
        );
    }

/// 解除专用核钉定：把实际钉住的线程逐个恢复全核掩码（不以 self_pinned 总开关短路——部分失败时已钉 tid 不恢复会永久滞留单核掩码）；失败且线程仍在（非 ESRCH）的 tid 留清单，下次重试
    fn unpin_self(&mut self) {
        if self.self_pinned_tids.is_empty() {
            self.self_pinned = false;
            return;
        }
        let ranges = crate::common::chiri_core_ranges();
        let all: Vec<usize> = (0..ranges.prime.end.max(ranges.big.end)).collect();
        for tid in std::mem::take(&mut self.self_pinned_tids) {
            let res = set_tid_affinity(tid, &all);
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "self_unpin",
                    std::process::id() as i32,
                    tid,
                    "-",
                    "-",
                    "full",
                    "-",
                    &io_result_tag(&res),
                    "scenemode",
                );
            }
            // ESRCH = 线程已消亡，无从重试也不需要恢复；其余失败留清单重试
            if let Err(e) = &res {
                if e.raw_os_error() != Some(libc::ESRCH) {
                    self.self_pinned_tids.push(tid);
                }
            }
        }
        self.self_pinned = false;
    }

    // [online_reader]
/// cpuN/online 的 keep-open 读取器：按核号惰性建立；节点常驻（核下线后文件仍在、值变 0），符合 FastReader 稳定节点口径
    fn online_reader(&mut self, cpu: u32) -> &mut FastReader {
        self.online_readers
            .entry(cpu)
            .or_insert_with(|| FastReader::new(format!("/sys/devices/system/cpu/cpu{cpu}/online")))
    }

/// scenemode 维持期纠偏：被外部拉起的核重新写 0；被框架 CpusetManager 加回业务组的专用核重新移除只读 online + 组 cpus（每 2s 数次小读），无写发生时零开销
    fn reassert_offline(&mut self) {
// 逐核收集待下线核一次批量写：单核失败 debug、全部失败 warn（utils::write_nodes），不留静默失败
        let mut items: Vec<(String, String)> = Vec::new();
// [fast_reader] 按下标迭代：online_reader 要 &mut self，与 &self.offlined 借用冲突（cpu 是 Copy，取完即释放）
        for i in 0..self.offlined.len() {
            let cpu = self.offlined[i].0;
            let reader = self.online_reader(cpu);
            // 三态口径不变：读失败/节点缺失（None）跳过该核，读到原文才比较
            let tampered = matches!(reader.read_raw(), Some(v) if v.trim() != "0");
            if tampered {
                items.push((
                    reader.path().to_string_lossy().into_owned(),
                    "0".to_string(),
                ));
            }
        }
        let written = crate::utils::write_nodes(&items, "corectl-reassert-offline");
        if crate::logger::diag_active() {
            for (path, value) in &items {
                let result = if written.iter().any(|w| w == path) {
                    "ok"
                } else {
                    "e0"
                };
                crate::logger::aff_action(
                    "corectl", 0, 0, "-", "-", "reassert", value, result, path,
                );
            }
        }
// core_ctl max_cpus 路径纠偏：被厂商改回（OPLUS pipeline scene 并发放行/锁 max_cpus）则重写目标值收敛
// 带冷却（MAX_CPUS_REASSERT_COOLDOWN，60s 一次）防逐 2s 拉锯写放大；首次改写 warn 一次，恢复由退出路径兜底
        if let Some(idx) = self.max_cpus_applied {
            let path = format!("{}/max_cpus", self.clusters[idx].dir);
            let keep_s = scenemode_keep_cpus(&self.clusters[idx]).to_string();
            let tampered = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .map(|v| v != keep_s)
                .unwrap_or(false);
            if tampered
                && self
                .max_cpus_reassert_at
                .map(|t| t.elapsed() >= MAX_CPUS_REASSERT_COOLDOWN)
                .unwrap_or(true)
            {
                self.max_cpus_reassert_at = Some(Instant::now());
                if !self.max_cpus_warned {
                    self.max_cpus_warned = true;
                    warn!(
                        "{}",
                        t_with_args(
                            "corectl-vendor-override",
                            &fluent_args!("path" => path.clone())
                        )
                    );
                }
                let res = fs::write(&path, &keep_s);
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "corectl",
                        0,
                        0,
                        "-",
                        "-",
                        "reassert_max_cpus",
                        &keep_s,
                        &io_result_tag(&res),
                        &path,
                    );
                }
            }
        }
        if let Some(core) = self.reserved_core {
            crate::chiri::affinity::exclude_core_from_cpusets(core, &mut self.reserved_cpusets);
        }
    }

/// 恢复被下线的核：按快照值写回 online，带回读 + 一次重试。**恢复失败的核保留在 offlined 中**（clear 掉则状态机回 NONE 后再无重试路径，被内核拒绝 / 异步热插拔未完成的核会永久离线），
/// 由每 2s 周期重试
    fn restore_online(&mut self) {
        // 顺序不可反：① 先解除 max_cpus 钳制（内核约束 `max_cpus >= min_cpus`，反了会被拒），
        // ② 再把 min_cpus 快照写回（scenemode 在 `min_cpus > keep` 的机型上压过它；写不进去就保留记账，
        // 下一轮再试——不因 max_cpus 没解除而放弃），③ 最后逐核写回 online（见下方循环）
        self.restore_max_cpus();
        self.restore_scenemode_min_cpus();
        let offlined = std::mem::take(&mut self.offlined);
        let total = offlined.len();
        let mut failed = Vec::new();
        for (cpu, orig) in offlined {
            // [fast_reader] keep-open 回读 + 路径缓存（恢复重试期免每次 format!+open/close）
            let reader = self.online_reader(cpu);
            let path = reader.path().to_string_lossy().into_owned();
// 判定以回读为真值（原 try_write_file 恒返 Ok），写入结果只落 @A 帧观测
            let mut write_back = |p: &str, v: &str| {
                let res = fs::write(p, v);
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "corectl",
                        0,
                        0,
                        "-",
                        "-",
                        "online",
                        v,
                        &io_result_tag(&res),
                        p,
                    );
                }
                // 三态口径不变：读失败/缺失（None）计 false，读到原文才 trim 比较
                reader.read_raw().map(|s| s.trim() == v).unwrap_or(false)
            };
            if !write_back(&path, &orig) && !write_back(&path, &orig) {
// 周期重试期每次尝试都会经过，降 debug 防刷屏；失败汇总由下方 restore-pending 打点
                debug!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
                failed.push((cpu, orig));
            }
        }
        let recovered = total - failed.len();
        if recovered > 0 {
            info!(
                "{}",
                t_with_args(
                    "corectl-scenemode-off",
                    &fluent_args!("count" => recovered.to_string())
                )
            );
        }
        // 失败项保留，等待下个周期重试；成功项自然丢弃
        self.offlined = failed;
        if !self.offlined.is_empty() {
            warn!(
                "{}",
                t_with_args(
                    "corectl-restore-pending",
                    &fluent_args!("count" => self.offlined.len().to_string())
                )
            );
        }
// 全部恢复后才解除专用核钉定与独占；有残留时保持钉定——守护线程仍应避开离线核池。
// 宽限：max_cpus 节点被厂商锁写时永远写不回去，若把独占死绑在它上面，亮屏后调度服务会长期钉在
// 大核上、业务侧少一颗 big（2026-09-28 审查发现）→ 核都回来了、只剩 max_cpus 写不回且超过
// MAX_CPUS_RELEASE_GRACE 时照样释放（钳制残留由 restore_max_cpus 继续重试）
        let max_cpus_stuck = self.max_cpus_applied.is_some()
            && self
                .max_cpus_restore_since
                .map(|t| t.elapsed() >= MAX_CPUS_RELEASE_GRACE)
                .unwrap_or(false);
        if self.offlined.is_empty() && (self.max_cpus_applied.is_none() || max_cpus_stuck) {
// 释放独占核：cpuset 组恢复原始值（保留核还给业务组）、自身线程移回原组、解除全线程自钉
            crate::chiri::affinity::restore_excluded_cpusets(std::mem::take(
                &mut self.reserved_cpusets,
            ));
            if let Some(group) = self.self_cpuset_group.take() {
                crate::chiri::affinity::move_self_to_cpuset_group(&group);
            }
            self.reserved_core = None;
            self.unpin_self();
        }
    }

/// 启动时强制上线全部核心：上次可能死在 scenemode 中途，残留离线核会让其 cpufreq policy 目录消失（CLG/akmode/fast_lock 枚举不到该集群、永久失去 worker）且核本身永久离线
/// 在 governor 接管前调用，写 online=1 并清空 offlined 残留快照（快照恢复语义已由本调用替代）
    // [restore]
    pub fn force_online_all(&mut self) {
// [restore] core_ctl max_cpus 残留自愈：残留 0 会让 core_ctl 持续 halt 整簇、下方逐核 online=1 被内核拒绝枚举快照后与磁盘双重比对，不一致才写（本进程记账为空，
// 按快照恢复；失败仅 warn，逐核 online 仍兜底）
        self.discover();
        for c in &self.clusters {
            let Some(snap) = c.max_cpus.clone() else {
                continue;
            };
            let path = format!("{}/max_cpus", c.dir);
            if fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .as_deref()
                == Some(snap.as_str())
            {
                continue;
            }
            let res = fs::write(&path, &snap);
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "corectl",
                    0,
                    0,
                    "-",
                    "-",
                    "max_cpus",
                    &snap,
                    &io_result_tag(&res),
                    &path,
                );
            }
            if res.is_err() {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
            }
        }
        self.max_cpus_applied = None;
// 状态机同步复位：panic 恢复可能在 scenemode 激活中调用，只清 max_cpus_applied 不清 state 会让维持期纠偏 / 退出恢复指向已被本方法取代的语义（压制静默失效）一并清纠偏冷却计时
        self.state = STATE_NONE;
        self.max_cpus_reassert_at = None;
        let ranges = crate::common::chiri_core_ranges();
// 判定以回读为真值（原 try_write_file 恒返 Ok），写入结果只落 @A 帧观测（与 restore_online 同款）
        let write_back = |path: &str| {
            let res = fs::write(path, "1");
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "corectl",
                    0,
                    0,
                    "-",
                    "-",
                    "online",
                    "1",
                    &io_result_tag(&res),
                    path,
                );
            }
            fs::read_to_string(path)
                .ok()
                .map(|s| s.trim() == "1")
                .unwrap_or(false)
        };
        for cpu in ranges
            .little
            .clone()
            .chain(ranges.big.clone())
            .chain(ranges.prime.clone())
        {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            // 已在线（或节点不可读——核数少于布局的机型）直接跳过
            if fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim() == "1")
                .unwrap_or(true)
            {
                continue;
            }
            if !write_back(&path) && !write_back(&path) {
// 启动期失败打 warn：热插拔锁等异常态可能拒绝写入，后续 restore_online 仍会按快照兜底
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
            }
        }
        self.offlined.clear();
    }

    /// scenemode 退出：把被压低的 `min_cpus` 按快照写回（只针对记账的那一簇，写后读回校验）。
    /// **独立于 `max_cpus`**：内核约束 `max_cpus >= min_cpus`——若钳制尚未解除，这里的写回会被拒，
    /// 那就保留记账、下周期重试（`set_power_state` 的稳态重试条件已纳入它），不得因此放弃：
    /// 早先「max_cpus 记账为空才恢复」的写法在 `max_cpus` 被厂商锁写时会让 min_cpus 永久停在 1
    /// （元审查发现：业务组少核、core_ctl 还会照 min_cpus=1 自行下线）
    fn restore_scenemode_min_cpus(&mut self) {
        let Some(idx) = self.min_cpus_lowered else {
            return;
        };
        let path = format!("{}/min_cpus", self.clusters[idx].dir);
        let snap = self.clusters[idx].min_cpus.clone();
        let res = fs::write(&path, &snap);
        if crate::logger::diag_active() {
            crate::logger::aff_action(
                "corectl", 0, 0, "-", "-", "min_cpus", &snap, &io_result_tag(&res), &path,
            );
        }
        // 回读为真值（原 restore_min_cpus 无校验）：写失败但已是快照值（厂商已改回/上一轮已写成功）同样算恢复完成
        if fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim() == snap.as_str())
            .unwrap_or(false)
        {
            self.min_cpus_lowered = None;
        } else {
            debug!(
                "{}",
                t_with_args("corectl-write-failed", &fluent_args!("path" => path))
            );
        }
    }

    /// 恢复各 cluster 的 min_cpus 快照（boost 退出）
    // [boost]
    fn restore_min_cpus(&mut self) {
        for c in &self.clusters {
            let path = format!("{}/min_cpus", c.dir);
            // enable=0 的簇 boost 期就没写过（core_ctl 不执行压制），恢复自然无需写；按「未生效」记 e0，与 boost 期口径一致
            if c.enable == Some(false) {
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "corectl",
                        0,
                        0,
                        "-",
                        "-",
                        "min_cpus",
                        &c.min_cpus,
                        "e0",
                        &path,
                    );
                }
                continue;
            }
            let res = fs::write(&path, &c.min_cpus);
            if crate::logger::diag_active() {
                crate::logger::aff_action(
                    "corectl",
                    0,
                    0,
                    "-",
                    "-",
                    "min_cpus",
                    &c.min_cpus,
                    &io_result_tag(&res),
                    &path,
                );
            }
        }
        if !self.clusters.is_empty() {
            info!("{}", t("corectl-boost-off"));
        }
    }

    /// 释放接管：恢复全部快照（调度线程收尾时调用）
    pub fn release(&mut self) {
        if self.state != STATE_NONE {
            match self.state {
                STATE_BOOST => self.restore_min_cpus(),
                STATE_SCENEMODE => self.restore_online(),
                _ => {}
            }
            self.state = STATE_NONE;
        }
    }
}
