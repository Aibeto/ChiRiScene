//! core_ctl.rs: [types] [helpers] [state] [max_cpus] [scenemode] [online_reader] [restore]

/// 核心在线控制器接管（ChiRi 专属）。
///
/// 三态状态机（互斥，按「最近一次 apply」切换，内部去重）：
/// - **Boost**（boost/vector/特调）：各 cluster 的 core_ctl `min_cpus` 抬到
///   全组常在线，防止低负载时热插拔回滞把大核下线、与 ChiRi 升降频决策打架；
/// - **Scenemode 离线**（息屏 5 分钟后的深度省电）：解除 boost 后压制
///   prime 超大核簇——**首选 WALT core_ctl sysfs**：写簇首核
///   `core_ctl/max_cpus=0` 让内核 `walt_halt_cpus` 整簇停摆（core_ctl.c:1354；
///   is_active = cpu_active && !cpu_halted，被 halt 的核 getaffinity 自动扣掉、
///   选核避开，与 ChiRi 亲和互不踩）。写后读回校验：被厂商并发改写（OPLUS
///   pipeline scene 会放行/锁 max_cpus，core_ctl.c:847,861,918-919）记 warn
///   一次不重试，降级逐核 offline。core_ctl 节点不可用的机型**兜底直接写
///   `/sys/devices/system/cpu/cpuN/online`**。**小核 + 大核全开常驻**（频率
///   上限由 scenemode CLG 配置压制），仅 prime 整簇断电消除空转漏电流；
///   另将编号最大的小核**独占**给调度服务（从业务 cpuset 组移除 + 自身线程
///   移入根组 + 全线程自钉）；逐核写后回读验证，失败的核跳过；周期重入时
///   纠偏（被异常拉起的核重新下线、被加回的保留核重新移除——ChiRi 默认是全局
///   唯一调度程序，节点被改写属异常态：残留旧模块/手动调试/内核异常；
///   max_cpus 被厂商改回只 warn 不重写，退出时按快照恢复）；
/// - **Normal**：恢复全部快照（min_cpus / online）。
///
/// 为什么用 min_cpus/online 而不是逐核"按需唤醒"：唤醒大核要拉电压轨、重建
/// L2，为一个后台线程点亮大核净亏能；且直接写 online 会与内核热插拔回滞策略
/// 打架（低负载判定会再下线）。需要更多在线核时的正确姿势是抬 core_ctl min_cpus
/// （Boost 态），让内核按自己的回滞策略管理唤醒。
///
/// cluster 发现：遍历 cpufreq policy → related_cpus 首个 CPU 的
/// `/sys/devices/system/cpu/cpuN/core_ctl`（每个 policy 只注册一份，天然去重）。
/// 直接 sysfs 离线不依赖 core_ctl 节点（core_ctl 不可用的机型也能用，
/// 由 `core_ctl.enabled` 配置门控）。
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
/// 状态：scenemode 离线（小核+大核常驻，prime 断电，1 颗小核独占给调度服务）
const STATE_SCENEMODE: u8 = 2;

/// max_cpus 维持期纠偏冷却：60s 内只纠偏一次。依据：OPLUS pipeline scene 会
/// 按自己的节奏放行/锁 max_cpus（core_ctl.c:847,861,918-919），逐 2s 周期
/// 重写会与厂商 pipeline 互相拉锯、产生写放大；60s 足以感知并收敛误改，
/// 同时把拉锯代价压到最低。
const MAX_CPUS_REASSERT_COOLDOWN: Duration = Duration::from_secs(60);

/// 单个 cluster 的 core_ctl 控制节点
struct CoreCtlCluster {
    /// core_ctl 目录（如 /sys/devices/system/cpu/cpu3/core_ctl）
    dir: String,
    /// cluster 首核号（prime 簇匹配用：scenemode 只压 prime 簇）
    first_cpu: u32,
    /// cluster 内 CPU 数（boost 时 min_cpus 的目标值）
    cluster_size: u32,
    /// 快照的原始 min_cpus
    min_cpus: String,
    /// 快照的原始 max_cpus（None = 节点不可读，该簇不支持 max_cpus 压制，
    /// scenemode 走逐核 offline 兜底）
    max_cpus: Option<String>,
    /// 快照的 core_ctl enable（None = 节点不可读，保持原有 max_cpus 压制
    /// 尝试；Some(false) = 该簇 core_ctl 未启用，内核不受理 max_cpus 写入，
    /// 直接跳过这次无效写走逐核 offline 兜底。仅快照期读一次作省写优化，
    /// 不做周期重读——厂商可能动态改 enable，纠偏路径仍按原兜底）
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
    /// scenemode 经 core_ctl 路径写过 `max_cpus=0` 的 cluster 下标记账：
    /// 恢复时按该簇 max_cpus 快照还原；None = 未走 core_ctl 路径（兜底
    /// offline 或未进入 scenemode）。与 offlined 互斥（两压制路径二选一）
    max_cpus_applied: Option<usize>,
    /// max_cpus 节点不可用/被厂商改写的 warn 一次性标记（防 2s 周期纠偏刷屏）
    max_cpus_warned: bool,
    /// max_cpus 维持期纠偏（重写 "0"）的上次时刻：冷却内不重复纠偏，防与
    /// 厂商 pipeline 逐周期拉锯（MAX_CPUS_REASSERT_COOLDOWN）
    max_cpus_reassert_at: Option<Instant>,
    /// scenemode 下守护进程自身线程是否已**全部**钉到专用小核（部分失败为
    /// false，下次触发重试钉定）
    self_pinned: bool,
    /// 实际钉住的自身 tid 清单（**只记成功**）：unpin_self 按它逐个恢复——
    /// 旧实现以 self_pinned 总开关跳过恢复，部分钉定失败时成功钉住的线程
    /// 会永久滞留单核掩码
    self_pinned_tids: Vec<i32>,
    /// scenemode 独占给调度服务的小核（None = 未独占/设备无 cpuset 时降级）
    reserved_core: Option<usize>,
    /// 独占时被移除核的业务 cpuset 组快照（(组名, 原始 cpus)，退出恢复用）
    reserved_cpusets: Vec<(String, String)>,
    /// 自身线程被移入根组前的原 cpuset 组相对路径（退出恢复用）
    self_cpuset_group: Option<String>,
    /// [fast_reader] cpuN/online 节点的 keep-open 读取器（路径构造期缓存，
    /// 周期纠偏/恢复回读免每 2s format! + open/close），按核号惰性建立
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

/// 计算 scenemode 下线目标：**小核 + 大核全开常驻**（频率上限由 scenemode
/// CLG 配置统一压制），仅 prime（超大核）整簇下线。此外 scenemode 期间会
/// 把编号最大的小核独占给调度服务（从业务 cpuset 组移除 + 自身线程移入
/// 根组 + 自钉，见 offline_cores 与 affinity 的专用核独占助手）。
fn scenemode_targets() -> Vec<u32> {
    let ranges = crate::common::chiri_core_ranges();
    let mut targets: Vec<u32> = ranges.prime.clone().map(|c| c as u32).collect();
    // 防御：引导核（CPU0）无法热拔出，永不下线（prime 不含 0，双保险）
    targets.retain(|&c| c != 0);
    targets
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
            max_cpus_warned: false,
            max_cpus_reassert_at: None,
            self_pinned: false,
            self_pinned_tids: Vec::new(),
            reserved_core: None,
            reserved_cpusets: Vec::new(),
            self_cpuset_group: None,
            online_readers: HashMap::new(),
        }
    }

    /// 枚举各 cluster 的 core_ctl 控制节点并快照 min_cpus。
    /// 无任何 core_ctl 节点（内核不支持/未启用）时打点一次，之后保持空表。
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
                // [residual] 崩溃残留自愈：上次运行可能死在 scenemode 中途，
                // max_cpus=0 残留会让 core_ctl 持续 halt 整簇到重启。判定依据：
                // core_ctl 约束 max_cpus ≥ min_cpus ≥ 1（内核 core_ctl.c 的
                // store 钳制），合法原值不可能为 0——快照解析为 0（或数值低于
                // min_cpus 合法下界）即视为残留，不是「原值」。直接写满配核数
                // 回节点解除 halt，内存快照同步为写回后的值；写/回读失败则该簇
                // max_cpus 快照记 None（节点不可用口径），scenemode 走逐核
                // online 兜底，restore 不再把 "0" 当原值恢复。快照为磁盘现读，
                // 若不做此自愈，「快照 == 磁盘」双重比对恒成立，残留永远跳过写回。
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
                                t_with_args(
                                    "corectl-write-failed",
                                    &fluent_args!("path" => path)
                                )
                            );
                            None
                        };
                    }
                }
                // enable 快照（读失败记 None = 保持原有 max_cpus 压制尝试）：
                // core_ctl enable=0 时内核不受理 max_cpus 写入，快照记 false 供
                // scenemode 省掉这次无效写（见 shrink_prime_via_max_cpus）
                let enable = fs::read_to_string(format!("{dir}/enable"))
                    .ok()
                    .map(|v| {
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
                // TODO(discover): 快照为进程启动时的单次读取，若厂商在接管后
                // 改写 max_cpus，restore_max_cpus 会按旧快照覆盖厂商值。F3 的
                // 维持期纠偏冷却已部分缓解（压制期被改回会被纠偏重写），彻底
                // 解决需恢复前重读比对，暂不展开。
            }
        }
        if self.clusters.is_empty() {
            info!("{}", t("corectl-unavailable"));
        }
    }

    /// 统一状态入口（内部去重，可被 2s 周期安全调用）。
    /// - `boost`：min_cpus 抬到全组常在线（性能模式）；
    /// - `scenemode`：prime 整簇下线（小核+大核常驻），独占一颗小核给调度服务。
    /// 两者互斥；切换时先退出旧状态（恢复快照）再进入新状态。
    /// scenemode 维持期每次调用都会纠偏（重新下线被外部拉起的核）。
    /// NONE / BOOST 稳态下若仍有恢复失败的核残留（restore_online 失败不
    /// clear，见其注释），每 2s 周期重试恢复——BOOST 期不重试会让 scenemode
    /// 退出的残留核（典型 = prime 超大核）在整段 boost 会话中保持离线。
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
                // 维持期纠偏：下线的核可能被异常拉回（残留旧模块/手动调试/内核
                // 异常态；ChiRi 默认无竞争者），重新收敛回离线
                self.reassert_offline();
            } else if !self.offlined.is_empty() || self.max_cpus_applied.is_some() {
                // 恢复失败残留核/max_cpus 的周期重试（NONE 与 BOOST 稳态）：
                // 亮屏恢复/boost 进入前的那次 restore 失败后，若 BOOST 期不再
                // 重试，prime 会持续离线直到模式切换——游戏整场无超大核
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
        // 残留核兜底：进入新状态前先把上次恢复失败的核拉回在线——
        // 否则重新进入 scenemode 时 offline_cores 读到 online=0 会跳过记录，
        // 这些核永远失去恢复登记（max_cpus 记账残留同样先恢复）
        if !self.offlined.is_empty() || self.max_cpus_applied.is_some() {
            self.restore_online();
        }

        // 进入目标状态
        match target {
            STATE_BOOST => {
                self.discover();
                for c in &self.clusters {
                    let path = format!("{}/min_cpus", c.dir);
                    let val = c.cluster_size.to_string();
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

    /// scenemode：下线 prime（超大核）整簇——小核 + 大核全开常驻，频率上限
    /// 由 scenemode CLG 配置统一压制。逐核写 online=0 并回读验证；写失败/
    /// 内核拒绝的核跳过（记录 warn）。成功下线的核连同原始 online 值记入
    /// offlined，供恢复。
    /// 随后**独占一颗小核给调度服务**：选编号最大的小核，从全部业务 cpuset
    /// 组移除（其他进程不可调度到该核）+ 自身线程移入根组 + 全线程自钉，
    /// 保证后台任务堵塞不了调度服务（设备无 cpuset 时降级为仅自钉）。
    // [max_cpus]
    /// scenemode 首选压制路径：写 prime 簇 WALT core_ctl `max_cpus=0`，让内核
    /// walt_halt_cpus 整簇停摆（core_ctl.c:1354；is_active = cpu_active &&
    /// !cpu_halted，被 halt 的核 getaffinity 自动扣掉、选核避开，与 ChiRi 亲和
    /// 互不踩）。事件驱动单次写 + 记账（max_cpus_applied）防重复写。返回
    /// false = core_ctl 路径不可用，调用方降级逐核 online 兜底：
    /// - 无 prime 簇 / 该簇无 max_cpus 节点（快照缺失）——首次 warn 一次防刷屏；
    /// - 该簇 core_ctl `enable=0`（快照期读到，内核不受理 max_cpus 写入）——
    ///   跳过无效写直接兜底，首次 warn 一次（复用 max_cpus_warned 去重）；
    /// - 写失败：core_ctl 写入被拒，记 warn 一次不重试；
    /// - 读回确认为非 0：OPLUS pipeline scene 会放行/锁 max_cpus（厂商并发
    ///   写，core_ctl.c:847,861,918-919），记 warn 一次不重试，不与厂商拉锯；
    /// - 读回失败（瞬时/持续）：写已发出，按已生效记账 Some（防恢复漏记），
    ///   交由维持期 reassert 纠偏——见函数内注释。
    fn shrink_prime_via_max_cpus(&mut self) -> bool {
        // 已走 core_ctl 路径且未恢复（恢复失败残留重入）：不重复写
        if self.max_cpus_applied.is_some() {
            return true;
        }
        self.discover();
        // prime 簇 = 首核号等于 prime 首核的 cluster（policy 首核即簇首核）
        let Some(prime_first) = scenemode_targets().first().copied() else {
            return false;
        };
        let Some(idx) = self
            .clusters
            .iter()
            .position(|c| c.first_cpu == prime_first)
        else {
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
        // enable 感知（快照期读一次，仅作省写优化）：core_ctl enable=0 时内核
        // 不受理 max_cpus 写入，写也是无效 sysfs 写；直接降级逐核 offline 兜底
        // （warn 复用 max_cpus_warned 去重）。enable == None（读不到）保持现行为
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
        let res = fs::write(&path, "0");
        if crate::logger::diag_active() {
            crate::logger::aff_action(
                "corectl",
                0,
                0,
                "-",
                "-",
                "max_cpus",
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
            return false;
        }
        // 读回校验，且必须区分「读失败」与「读回非 0」：写已发出，读回瞬时
        // 失败不能断定 halt 未生效——若按未生效降级逐核兜底，scenemode 退出后
        // 逐核恢复 online 时 core_ctl 仍持 max_cpus=0 再次 halt，压制泄漏到
        // scenemode 之外。处理：读失败按「写已发出」对待，记账 Some（恢复路径
        // 覆盖，重点是不能漏记）交由维持期 reassert 纠偏；只有读回确认为非 0
        // （内核钳制或厂商改写，halt 确未生效）才降级逐核兜底、记账保持 None。
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
            Some(v) if v == "0" => {
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
                // 读回持续失败：写已发出、生效状态未知，按已生效记账防恢复
                // 漏记（restore_max_cpus 按快照写回；即使 halt 未生效，写回
                // 快照原值也无害）。维持期 reassert 会重读并纠偏
                warn!(
                    "{}",
                    t_with_args("corectl-verify-failed", &fluent_args!("path" => path))
                );
                self.max_cpus_applied = Some(idx);
                true
            }
        }
    }

    /// scenemode 退出：恢复 prime 簇 max_cpus 快照（记账 + 读回双重比对，同
    /// current_mode.chr 口径——磁盘已是快照值则零写跳过）。恢复失败保留记账，
    /// 由 set_power_state 的周期重试路径重入（与 offlined 残留核同款）。
    fn restore_max_cpus(&mut self) {
        let Some(idx) = self.max_cpus_applied else {
            return;
        };
        let Some(snap) = self.clusters[idx].max_cpus.clone() else {
            self.max_cpus_applied = None;
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
        } else {
            debug!(
                "{}",
                t_with_args("corectl-write-failed", &fluent_args!("path" => path))
            );
        }
    }

    // [scenemode]
    /// scenemode 压制入口：首选 core_ctl max_cpus 路径（整簇 halt），节点
    /// 不可用/写失败/被厂商并发改写时降级逐核 online 兜底——两路径记账互斥
    /// （max_cpus_applied / offlined），恢复各走各的。随后独占一颗小核给调度
    /// 服务（两路径共用）。
    fn offline_cores(&mut self) {
        if self.shrink_prime_via_max_cpus() {
            // core_ctl halt 路径成功：offlined 为空，corectl-scenemode-on 不会
            // 打——补一条成功日志表明压制已通过 max_cpus 生效
            if self.offlined.is_empty() {
                info!("{}", t("corectl-scenemode-halt"));
            }
        } else {
            self.offline_cores_direct();
        }
        // 独占小核：sched_setaffinity 自钉不排他，须同时把该核从业务 cpuset
        // 组移除 + 自身线程移入根组（钉定才不会被组掩码二次过滤）
        let ranges = crate::common::chiri_core_ranges();
        if let Some(core) = ranges.little.clone().last() {
            crate::chiri::affinity::exclude_core_from_cpusets(core, &mut self.reserved_cpusets);
            self.self_cpuset_group = crate::chiri::affinity::move_self_to_cpuset_root();
            self.reserved_core = Some(core);
        }
        // 专用小核自钉
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

    /// 兜底路径：逐核写 online=0 下线 prime 簇（core_ctl 节点不可用的机型）。
    /// 逐核写后回读验证，失败的核跳过（记录 warn）；成功下线的核连同原始
    /// online 值记入 offlined，供恢复。
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

    /// 把守护进程自身全部线程钉到专用小核（offline_cores 已独占的
    /// reserved_core——该核已从业务 cpuset 组移除、自身线程已移入根组，
    /// sched_setaffinity 由此实现真独占）。scenemode 下调度服务
    /// （scheduler_ipc / telemetry / 触摸检测等）独占该核，后台任务堵塞不了。
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

    /// 解除专用核钉定：把 pin_self_dedicated **实际钉住**的线程恢复全核掩码。
    /// 按成功清单逐个恢复（不再以 self_pinned 总开关短路）：部分钉定失败时
    /// self_pinned 为 false，但已钉住的 tid 若不恢复会永久滞留单核掩码。
    /// 恢复失败且线程仍在（非 ESRCH）的 tid 留在清单里，下次 unpin 重试。
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
    /// cpuN/online 的 keep-open 读取器：按核号惰性建立（路径构造期缓存，周期
    /// 纠偏/恢复回读免每次 format! + open/close）。节点常驻（核下线后文件仍在、
    /// 值变 0），符合 FastReader 的稳定节点口径。
    fn online_reader(&mut self, cpu: u32) -> &mut FastReader {
        self.online_readers
            .entry(cpu)
            .or_insert_with(|| FastReader::new(format!("/sys/devices/system/cpu/cpu{cpu}/online")))
    }

    /// scenemode 维持期纠偏：已下线核若被外部重新拉起，重新写 0；被独占的
    /// 专用小核若被框架 CpusetManager 加回业务组（top-app 的 cpus 由框架
    /// 动态管理），重新从组内移除。只读 online 文件 + 组 cpus（每 2s 数次
    /// 小读），无写发生时零开销。
    fn reassert_offline(&mut self) {
        // 逐核收集需要重新下线的核，一次批量写：单核失败 debug、全部失败 warn
        // （见 utils::write_nodes）——此前静默 `let _ =`，全核写不回去也毫无痕迹
        let mut items: Vec<(String, String)> = Vec::new();
        // [fast_reader] 按下标迭代：online_reader 要 &mut self，与 &self.offlined
        // 的跨迭代借用冲突（cpu 是 u32 Copy，取完即释放）
        for i in 0..self.offlined.len() {
            let cpu = self.offlined[i].0;
            let reader = self.online_reader(cpu);
            // 三态口径不变：读失败/节点缺失（None）跳过该核，读到原文才比较
            let tampered = matches!(reader.read_raw(), Some(v) if v.trim() != "0");
            if tampered {
                items.push((reader.path().to_string_lossy().into_owned(), "0".to_string()));
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
        // core_ctl max_cpus 路径纠偏：记账生效（Some）时读回节点，被厂商改回
        // 非 0（OPLUS pipeline scene 会并发放行/锁 max_cpus，
        // core_ctl.c:847,861,918-919）则重写 "0" 收敛回压制态。带冷却
        // （MAX_CPUS_REASSERT_COOLDOWN，60s 内只纠偏一次）：防与厂商 pipeline
        // 逐 2s 周期互相拉锯产生写放大。首次改写 warn 一次，重写后恢复失败
        // 由退出路径 restore_max_cpus 的记账 + 读回兜底
        if let Some(idx) = self.max_cpus_applied {
            let path = format!("{}/max_cpus", self.clusters[idx].dir);
            let tampered = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .map(|v| v != "0")
                .unwrap_or(false);
            if tampered && self
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
                let res = fs::write(&path, "0");
                if crate::logger::diag_active() {
                    crate::logger::aff_action(
                        "corectl",
                        0,
                        0,
                        "-",
                        "-",
                        "reassert_max_cpus",
                        "0",
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

    /// 恢复被下线的核：按快照值写回 online，带回读 + 一次重试。
    /// **恢复失败的核保留在 offlined 中**（此前 clear 掉后状态机回 NONE 再无
    /// 重试路径，写回被内核拒绝/异步热插拔未完成的核会永久离线——实测亮屏后
    /// 大核 4-7 一直不上线）。残留核由 set_power_state 的 STATE_NONE 分支在
    /// 每 2s 周期调用中重试，直到全部恢复。
    fn restore_online(&mut self) {
        // 先恢复 core_ctl max_cpus 记账（与 offlined 互斥，两路径只会有一个生效）
        self.restore_max_cpus();
        let offlined = std::mem::take(&mut self.offlined);
        let total = offlined.len();
        let mut failed = Vec::new();
        for (cpu, orig) in offlined {
            // [fast_reader] keep-open 回读 + 路径缓存（恢复重试期免每次 format!+open/close）
            let reader = self.online_reader(cpu);
            let path = reader.path().to_string_lossy().into_owned();
            // 判定保持原口径：以回读为真值（原 try_write_file 恒返 Ok，
            // is_ok() 项恒真），写入结果只落 @A 帧观测
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
                // 周期重试期间每次尝试都会经过这里，降为 debug 防刷屏；
                // 失败汇总由下方 restore-pending 打点
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
        // 全部恢复后才解除专用核钉定与独占（守护进程线程恢复全核）；
        // 有残留时保持钉定——守护线程仍应避开离线核池
        if self.offlined.is_empty() && self.max_cpus_applied.is_none() {
            // 释放独占小核：cpuset 组恢复原始值（保留核还给业务组）、自身
            // 线程移回原组、解除全线程自钉
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

    /// 启动时强制上线全部核心：上次运行可能在 scenemode 中途被杀，残留的
    /// 离线核会让其 cpufreq policy 目录消失（CLG/akmode/fast_lock 启动初始化
    /// 枚举不到该集群，永久失去 worker），且核本身永久离线。调度线程启动
    /// 阶段在任何 governor 接管之前调用，把调度范围内的核全部写 online=1，
    /// 同时清空 offlined 残留快照（快照对应的恢复语义已由本调用替代）。
    // [restore]
    pub fn force_online_all(&mut self) {
        // [restore] core_ctl max_cpus 残留自愈：上次运行可能死在 scenemode
        // 中途，prime 簇 max_cpus=0 残留会让 core_ctl 持续 halt 整簇，下方
        // 逐核 online=1 会被内核拒绝。枚举快照后与磁盘双重比对，不一致才写
        // （记账本进程为空，此处直接按快照恢复；失败仅 warn，逐核 online 仍兜底）
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
        // 状态机同步复位：panic 恢复路径可能在 scenemode 激活中调用本方法，
        // 只清 max_cpus_applied 不清 state 会让状态机仍认为压制在生效
        // （STATE_SCENEMODE），维持期纠偏/退出恢复全部指向已被本方法取代的
        // 语义——压制静默失效。一并清纠偏冷却计时
        self.state = STATE_NONE;
        self.max_cpus_reassert_at = None;
        let ranges = crate::common::chiri_core_ranges();
        // 判定保持原口径：以回读为真值（原 try_write_file 恒返 Ok），写入
        // 结果只落 @A 帧观测（与 restore_online 同款）
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
                // 启动期失败打 warn：热插拔锁等异常态可能拒绝写入，
                // 后续亮屏/ModeChange 路径的 restore_online 仍会按快照兜底
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
            }
        }
        self.offlined.clear();
    }

    /// 恢复各 cluster 的 min_cpus 快照（boost 退出）。
    fn restore_min_cpus(&mut self) {
        for c in &self.clusters {
            let path = format!("{}/min_cpus", c.dir);
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

    /// 释放接管：恢复全部快照（调度线程收尾时调用）。
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
