//! common.rs: [events] [proc_snap] [paths] [soc_detect] [core_ranges] [soc_config] [governor] [special_tuned] [fas_whitelist]
//! [aff_blacklist] [embedded] [external_meta] [frontier_table]

use crate::chiri::config::PowerBaseConfig;
use crate::monitor::config::RulesConfig;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// [events]
/// 守护进程全局事件总线
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// 低频事件：前台应用切换或环境温度变化引起的模式改变
    ModeChange {
        package_name: String,
        pid: i32,
        mode: String,
        temperature: f64,
    },
    /// 同模式前台包切换（模式不变、应用变化，ChiRi 侧 FAS fas→fas 热切换消费）
    PackageSwitch {
        package_name: String,
        pid: i32,
    },
    /// 高频事件：eBPF 捕获到的底层渲染帧数据
    FrameUpdate {
        /// 纳秒级帧间隔
        frame_delta_ns: u64,
    },
    /// eBPF 全局系统负载更新 (每 X 毫秒触发一次)
    SystemLoadUpdate {
        /// 每个 CPU 核心的真实利用率 (0.0 ~ 1.0)，数组索引即 cpu_id
        core_utils: Vec<f32>,
        /// 如果当前有前台应用，这是该应用最吃 CPU 的那 1 个线程的利用率
        foreground_max_util: f32,
    },

/// 装箱：RulesConfig 是最大变体，内联会让 40ms 一次的 SystemLoadUpdate 按整枚举尺寸搬内存
    ConfigReload(Box<RulesConfig>),

    ScreenStateChange(bool),

/// eBPF 扩展探针周期统计（仅 ChiRi SoC 的 cpu_monitor 可选探针产生），字段为 2s 发送周期内增量
    BpfStats {
        /// sched_wakeup 唤醒次数：调度唤醒链活跃度
        wakeups: u32,
        /// sched_migrate_task 线程迁移次数：亲和策略的实际迁移观测
        migrations: u32,
        /// cpufreq_transition 频率切换次数：调频活跃度（含热限频切换）
        freq_transitions: u32,
    },
}

// [proc_snap]
/// 每秒进程快照条目（aff @S 帧供数）：全系统 TGID 级，util 来自 eBPF TGID_RUN_TIME 的 1s 窗口差分（不扫 /proc）
pub struct ProcSnap {
    /// 进程 TGID
    pub pid: u32,
    /// 进程名（cmdline 首段优先，退化 comm，见 cpu_monitor::proc_name）
    pub comm: String,
    /// 1s 窗口内总 CPU 时间占比**百分比**：多核并行可 >100（320.0 ≈ 3.2 核满载）
    pub util: f32,
}

// [paths]
/// 获取模块根目录的绝对路径
pub fn get_module_root() -> PathBuf {
    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from("/"));

// 回溯两级：core/bin/chiri -> <模块根>
    exe_path
        .parent()
        .unwrap_or(&exe_path) // .../core/bin
        .parent()
        .unwrap_or(&exe_path) // .../core
        .parent()
        .unwrap_or(&exe_path) // .../chiri (Root)
        .to_path_buf()
}

/// 读取文件首行并去除空白（用于 SoC 型号探测）
fn read_first_line(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

/// 触发 Chiri 专用调度的处理器型号片段，命中任一即启用；片段须能互相区分，新增机型在此追加即可8998 暂时下线：从名单移除即不接管 CPU 仅监控，config/8998/ 与兜底 match 保留，
/// 恢复时把片段加回来
// [soc_detect]
const CHIRI_SOC_HINTS: &[&str] = &["8550", "8475", "8650", "zumapro"];

/// 读取单个 Android 系统属性（getprop key），失败/为空返回空串跨分区属性只能走 getprop 拿合并视图，直接读 /system/build.prop 会读空
pub(crate) fn getprop(key: &str) -> String {
    std::process::Command::new("getprop")
        .arg(key)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// 型号片段是否命中任一特定处理器：从多个权威来源取硬件标识统一比较，避免机型只暴露部分来源而漏检来源：soc0/machine、soc0/plat_name、getprop ro.soc.model / ro.board
/// platform / ro.product.board / ro.hardware、/proc/cpuinfo 兜底结果统一转小写后做子串匹配，兼容 "SM8550" / "sm8550" / "8550"
fn soc_hint_matches(hints: &[&str]) -> bool {
    hints.iter().any(|h| hint_matches(h))
}

/// 设备硬件标识全集（小写、多源拼接）：只探测一次并缓存，供各片段子串匹配复用
static SOC_HINT_HAYSTACK: OnceLock<String> = OnceLock::new();
fn soc_hint_haystack() -> &'static str {
    SOC_HINT_HAYSTACK.get_or_init(|| {
        let haystacks: Vec<String> = vec![
            read_first_line("/sys/devices/soc0/machine"),
            read_first_line("/sys/devices/soc0/plat_name"),
            getprop("ro.soc.model"),
            getprop("ro.board.platform"),
            getprop("ro.product.board"),
            getprop("ro.hardware"),
            std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(),
        ];
        haystacks.join("\n").to_lowercase()
    })
}

/// 单个片段是否命中设备硬件标识；片段至少 3 字符，避免过短片段误匹配
fn hint_matches(hint: &str) -> bool {
    let hl = hint.to_lowercase();
    hl.len() >= 3 && soc_hint_haystack().contains(&hl)
}

/// 是否应启用 Chiri 专用调度器（检测到列表中的处理器时为 true），结果只探测一次并缓存，避免反复读 /proc 与 sysfs
static CHIRI_SOC: OnceLock<bool> = OnceLock::new();
pub fn is_chiri_soc() -> bool {
    *CHIRI_SOC.get_or_init(|| soc_hint_matches(CHIRI_SOC_HINTS))
}

/// 返回第一个命中的处理器片段（顺序与 CHIRI_SOC_HINTS 一致）配置已编译进二进制，匹配只看硬件标识，磁盘目录缺失不影响识别结果只探测一次并缓存：被 chiri_core_ranges() 等大量周期路径调用，
/// 重算需逐片段匹配数 KB 标识，纯属重复劳动
static MATCHED_SOC_HINT: OnceLock<Option<&'static str>> = OnceLock::new();
pub(crate) fn matched_soc_hint() -> Option<&'static str> {
    *MATCHED_SOC_HINT.get_or_init(|| {
        if !is_chiri_soc() {
            return None;
        }
        CHIRI_SOC_HINTS
            .iter()
            .copied()
            .find(|hint| hint_matches(hint))
    })
}

/// 命中 Chiri 目标 SoC 时，返回其处理器专属配置目录 `config/{命中片段}/`（存在则返回）
fn matched_soc_config_dir() -> Option<PathBuf> {
    matched_soc_hint().map(|hint| get_module_root().join("config").join(hint))
}

/// 处理器核心组区间（little/big/prime 的 CPU ID，左闭右开）：akmode 忙/闲统计、CLG 触摸升频判定大核簇使用各 SoC 簇布局不同，按命中片段区分，未命中回退 8550 布局兜底
// [core_ranges]
#[derive(Debug, Clone)]
pub struct CoreGroupRanges {
    /// 小核组
    pub little: std::ops::Range<usize>,
    /// 大核组
    pub big: std::ops::Range<usize>,
    /// 超大核组：无超大核的 SoC 为空区间（start == end），统计时自动跳过
    pub prime: std::ops::Range<usize>,
}

/// 按命中片段返回核心组区间：数据源 = soc.yaml [topology]（编译期嵌入，见 [soc_config]），缺失/损坏回退下方硬编码兜底
/// 新增 SoC：加 config/{片段}/soc.yaml + 片段进 CHIRI_SOC_HINTS，无需改 .rs
pub fn chiri_core_ranges() -> CoreGroupRanges {
    // 热路径（20+ 调用点，每轮周期块都走）：OnceLock 读 + 区间小拷贝，不重解析 yaml
    if let Some(topo) = soc_config().and_then(|c| c.topology.as_ref()) {
        if let (Some(l), Some(b), Some(p)) = (&topo.little, &topo.big, &topo.prime) {
            return CoreGroupRanges {
                little: l.start..l.end,
                big: b.start..b.end,
                prime: p.start..p.end,
            };
        }
    }
    match matched_soc_hint() {
        Some("8475") => CoreGroupRanges {
            little: 0..4,
            big: 4..7,
            prime: 7..8,
        },
        Some("8998") => CoreGroupRanges {
            little: 0..4,
            big: 4..8,
            prime: 7..7,
        },
        _ => CoreGroupRanges {
            little: 0..3,
            big: 3..7,
            prime: 7..8,
        },
    }
}

// [soc_config]
/// CPU 所属核心组（[`core_group_of`] 的返回，供按 CPU/policy 映射 soc.yaml 分段；
/// Hash 供 [`frontier_aligned`] 的「组 + 频率表」缓存作键）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreGroup {
    Little,
    Big,
    Prime,
}

/// cpu id 落在哪个核心组，都不在返回 None（8998 空 prime 区间不会命中 prime）；区间以 chiri_core_ranges 为准
pub fn core_group_of(cpu: u32) -> Option<CoreGroup> {
    let r = chiri_core_ranges();
    let c = cpu as usize;
    if r.little.contains(&c) {
        Some(CoreGroup::Little)
    } else if r.big.contains(&c) {
        Some(CoreGroup::Big)
    } else if r.prime.contains(&c) {
        Some(CoreGroup::Prime)
    } else {
        None
    }
}

/// SoC 硬件基线（{soc}/soc.yaml，随 module/config 编译期嵌入、磁盘不落盘）：运行时 sysfs 探测的兜底与校准基准，不是调优输入
/// 全部字段可缺省（缺段 = None → 调用方走原有回退路径）；新增 SoC = 加 config/{片段}/soc.yaml + 片段进 CHIRI_SOC_HINTS
/// 注意：serde 无 deny_unknown_fields，未知键被静默忽略，键名拼错只会整段变 None 走兜底，排障留意 warn 日志
#[derive(Debug, Clone, Deserialize)]
pub struct SocConfig {
    /// 核心组 CPU ID 区间（[chiri_core_ranges] 的数据源）
    #[serde(default)]
    pub topology: Option<SocTopology>,
    /// EAS capacity 兜底（真机 cpu_capacity 读不到时用；真机 sysfs 永远优先）
    #[serde(default)]
    pub capacity: Option<SocTriValues>,
    /// 频率档位兜底（kHz 升序；scaling_available_frequencies 读不到时用）
    #[serde(default)]
    pub freq_khz: Option<SocFreqTable>,
    /// 空闲态代价 µs（校准基准，本期无运行时消费方）
    #[serde(default)]
    // 解析留存备用，暂无读取方（与 dpc 同口径）
    #[allow(dead_code)]
    pub idle_us: Option<SocIdleUs>,
    /// dynamic-power-coefficient（校准基准，本期无运行时消费方）
    #[serde(default)]
    #[allow(dead_code)]
    pub dpc: Option<SocTriValues>,
    /// 帕累托前沿查表段（仅 8550 具备采用条件；其它 SoC 的 soc.yaml 无本段 → 天然 None → 走原比例路径）
    /// [frontier_lenient] 用宽松反序列化：本段损坏不得拖垮整份 soc.yaml（topology/capacity/freq 兜底是更贵的资产）
    #[serde(default, deserialize_with = "deserialize_frontier_lenient")]
    pub frontier_policy: Option<FrontierPolicy>,
    /// cpufreq 调速器优先级表（[governor]，从左到右优先；与真机 scaling_available_governors 求交后取首个）
    #[serde(default)]
    pub cpu_governors: Option<Vec<String>>,
}

/// 核心组 CPU ID 区间（左闭右开）
#[derive(Debug, Clone, Deserialize)]
pub struct SocRange {
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub end: usize,
}

/// 核心组 topology：字段缺失 = 整段视为不完整（不启用 topology），而非空区间
#[derive(Debug, Clone, Deserialize)]
pub struct SocTopology {
    #[serde(default)]
    pub little: Option<SocRange>,
    #[serde(default)]
    pub big: Option<SocRange>,
    #[serde(default)]
    pub prime: Option<SocRange>,
}

/// 按核心组的三值段（capacity / dpc 共用；单值缺失 = None，调用方回退）
#[derive(Debug, Clone, Deserialize)]
pub struct SocTriValues {
    #[serde(default)]
    pub little: Option<u32>,
    #[serde(default)]
    pub big: Option<u32>,
    #[serde(default)]
    pub prime: Option<u32>,
}

/// 频率档位表（kHz，升序；写 scaling_max 前仍须 floor 对齐到表内档位）
#[derive(Debug, Clone, Deserialize)]
pub struct SocFreqTable {
    #[serde(default)]
    pub little: Vec<u32>,
    #[serde(default)]
    pub big: Vec<u32>,
    #[serde(default)]
    pub prime: Vec<u32>,
}

/// 空闲态代价（µs；校准基准，本期无运行时消费方，字段解析留存备用）
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SocIdleUs {
    #[serde(default)]
    pub exit_latency: Option<SocTriValues>,
    #[serde(default)]
    pub min_residency: Option<SocTriValues>,
    #[serde(default)]
    pub cluster_exit_latency: Vec<u32>,
}

// [frontier] 帕累托前沿策略段（8550 专用，soc.yaml 内受控生成块；scripts/frontier_policy.py 产出）
// 运行期消费方：CLG 的 PF 落点（cpu_load_governor.rs [frontier_pf]，只允许下调；查表经 [frontier_table]
// 摊平成进程级表，勿在 tick 路径直接扫 demand_buckets）与 FDP 的逐档功耗（chiri/energy_cost.rs）。
// 字段名与生成脚本 / 定稿 schema 逐字对齐（唯一例外：soc.yaml 桶里的 margin_mdmips_per_w 本版不消费，
// 靠 serde 忽略未知键通过）；解析失败由 [frontier_lenient] 兜底为 None（不回退他人逻辑、不 panic）。

/// 前沿策略段。段缺失或 `enabled` 非 true → `soc_frontier_policy()` 返回 None → 原比例路径
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FrontierPolicy {
    /// 运行期开关（在块内）：false / 缺失 = 不启用
    #[serde(default)]
    pub enabled: bool,
    /// 输入内容哈希（生成脚本产出，运行期仅用于日志标识）
    #[serde(default)]
    pub model_fingerprint: Option<String>,
    /// 复算对账锚点（运行期不消费，落表自检留痕）
    #[serde(default)]
    #[allow(dead_code)]
    pub anchors: Vec<FrontierAnchor>,
    /// 前沿点（DT 口径，运行期不消费）
    #[serde(default)]
    #[allow(dead_code)]
    pub frontier: Vec<FrontierPoint>,
    /// 各簇硬件基线与频表（与 soc.yaml 其余段同源，仅作参照）
    #[serde(default)]
    #[allow(dead_code)]
    pub per_cluster: Option<FrontierPerCluster>,
    /// 需求桶（真机容量单位边界 + 该前沿点的三簇目标 kHz）：PF 查表的唯一数据源
    #[serde(default)]
    pub demand_buckets: Vec<FrontierBucket>,
    /// 三模式结果向量（预判带；本版运行期未消费，留待后续）
    #[serde(default)]
    #[allow(dead_code)]
    pub modes: HashMap<String, FrontierMode>,
    /// 预判参数（本版运行期未消费）
    #[serde(default)]
    #[allow(dead_code)]
    pub anticipate: Option<FrontierAnticipate>,
}

/// 复算对账锚点（运行期不消费）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierAnchor {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub got: f32,
    #[serde(default)]
    pub want: f32,
    #[serde(default)]
    pub ok: bool,
}

/// 前沿点（DT 口径 Mdmips / W，运行期不消费）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierPoint {
    #[serde(default)]
    pub compute: f32,
    #[serde(default)]
    pub power_w: f32,
}

/// 各簇硬件基线（参照，运行期不消费）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierPerCluster {
    #[serde(default)]
    pub little: FrontierClusterBase,
    #[serde(default)]
    pub big: FrontierClusterBase,
    #[serde(default)]
    pub prime: FrontierClusterBase,
}

/// 单簇基线：capacity、频率表（kHz 升序）与**逐档簇功耗**（W，与 `freq_khz` 逐档同下标对齐）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierClusterBase {
    #[serde(default)]
    pub capacity: Option<u32>,
    #[serde(default)]
    pub freq_khz: Vec<u32>,
    /// 逐档簇功耗 W（文档 §3 簇级表，只含动态项、不含漏电）：放置成本模型（FDP）的边际比较输入；
    /// 长度与 `freq_khz` 对齐，缺省为空 = 该簇无成本数据（FDP 对该簇回退、不动作）
    #[serde(default)]
    pub power_w: Vec<f32>,
}

/// 需求桶：`lo_units`/`hi_units` 为**真机容量单位**边界；`*_khz` 为该前沿点的三簇目标频点
/// （收到后仍须 floor 对齐到 `available_freqs`；0 = 表未提供 → 回退比例路径）
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FrontierBucket {
    #[serde(default)]
    #[allow(dead_code)]
    pub point: u32,
    #[serde(default)]
    pub lo_units: f32,
    #[serde(default)]
    #[allow(dead_code)]
    pub hi_units: f32,
    #[serde(default)]
    #[allow(dead_code)]
    pub target_compute: f32,
    #[serde(default)]
    #[allow(dead_code)]
    pub target_power_w: f32,
    #[serde(default)]
    pub little_khz: u32,
    #[serde(default)]
    pub big_khz: u32,
    #[serde(default)]
    pub prime_khz: u32,
}

/// 单模式结果向量（运行期未消费）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierMode {
    #[serde(default)]
    pub lead_mult: f32,
    #[serde(default)]
    pub lead_frac: f32,
    #[serde(default)]
    pub reserve_pct: f32,
    #[serde(default)]
    pub buckets: Vec<FrontierModeBucket>,
}

/// 单模式单桶（运行期未消费）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierModeBucket {
    #[serde(default)]
    pub point: u32,
    #[serde(default)]
    pub target_compute: f32,
    #[serde(default)]
    pub target_power_w: f32,
    #[serde(default)]
    pub enter_units: f32,
    #[serde(default)]
    pub exit_units: f32,
}

/// 预判参数（运行期未消费）：`mode_mul` 为 reduce/default/boost 三模式乘子
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierAnticipate {
    #[serde(default)]
    pub min_dwell_ticks: u32,
    #[serde(default)]
    pub adjacent_only: bool,
    #[serde(default)]
    pub mode_mul: Option<FrontierModeMul>,
}

/// 预判模式乘子（reduce/default/boost 三模式）
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FrontierModeMul {
    #[serde(default)]
    pub reduce: f32,
    #[serde(default)]
    pub default: f32,
    #[serde(default)]
    pub boost: f32,
}

// [frontier_table] 进程级预计算的前沿查表：桶表来自 soc.yaml（编译期嵌入、SoC 硬件固定），
// 全程只摊平一次——worker 首次接管建表，之后每 tick 只做「二分找桶 + 数组取频点」；
// 热切换 / 息屏等引起的 worker 重建不再重走结构体、重扫桶表（`frontier_aligned` 另有对齐缓存）

/// 前沿查表（摊平后的桶表）：`lo_units` 升序，三簇目标 kHz 与它逐桶同索引（0 = 表未提供该簇）
pub struct FrontierTable {
    lo_units: Box<[f32]>,
    little: Box<[u32]>,
    big: Box<[u32]>,
    prime: Box<[u32]>,
}

impl FrontierTable {
    /// 由 `demand_buckets` 摊平；桶表为空 → None（调用方走原比例路径，与段缺失同口径）
    fn from_policy(policy: &FrontierPolicy) -> Option<Self> {
        if policy.demand_buckets.is_empty() {
            return None;
        }
        let n = policy.demand_buckets.len();
        let mut lo_units = Vec::with_capacity(n);
        let mut little = Vec::with_capacity(n);
        let mut big = Vec::with_capacity(n);
        let mut prime = Vec::with_capacity(n);
        for b in &policy.demand_buckets {
            lo_units.push(b.lo_units);
            little.push(b.little_khz);
            big.push(b.big_khz);
            prime.push(b.prime_khz);
        }
        Some(Self {
            lo_units: lo_units.into_boxed_slice(),
            little: little.into_boxed_slice(),
            big: big.into_boxed_slice(),
            prime: prime.into_boxed_slice(),
        })
    }

    /// 需求（真机容量单位）落在哪个桶：`lo_units <= demand` 的最大桶索引（二分）；
    /// 桶按 `lo_units` 升序（生成脚本的 hull 天然有序），需求低于首桶 → None（负载极低，几何边界不是表错）
    pub fn bucket_for_demand(&self, demand_units: f32) -> Option<usize> {
        if !demand_units.is_finite() {
            return None;
        }
        let idx = self.lo_units.partition_point(|&lo| lo <= demand_units);
        (idx > 0).then_some(idx - 1)
    }

    /// 该簇逐桶的**原始**目标 kHz（未 floor 对齐；0 = 表未提供该桶该簇）
    fn targets(&self, group: CoreGroup) -> &[u32] {
        match group {
            CoreGroup::Little => &self.little,
            CoreGroup::Big => &self.big,
            CoreGroup::Prime => &self.prime,
        }
    }
}

/// 当前 SoC 的前沿查表（OnceLock 惰性构建）：段缺失 / `enabled != true` / 桶表为空 → None
pub fn frontier_table() -> Option<&'static FrontierTable> {
    static TABLE: OnceLock<Option<FrontierTable>> = OnceLock::new();
    TABLE
        .get_or_init(|| soc_frontier_policy().and_then(FrontierTable::from_policy))
        .as_ref()
}

// [frontier_aligned] 逐桶落点表（已 floor 对齐到**某簇真实频率表**）：进程级缓存，键 = 核心组 + 频率表，
// 同一频率表下的所有 worker、所有次重建（热切换 / 息屏切换）共用同一份，重建不重算
/// 缓存条目：(该簇真实频率表, 逐桶落点数组)
type FrontierAlignedEntry = (Vec<u32>, Arc<Vec<u32>>);
static FRONTIER_ALIGNED: OnceLock<Mutex<HashMap<CoreGroup, Vec<FrontierAlignedEntry>>>> =
    OnceLock::new();

/// 该簇的逐桶落点：索引 = 需求桶（与 [`frontier_table`] 桶序一致），值 = 频率表中 `<= 表给目标` 的最大档 kHz，
/// 0 = 该桶该簇无目标 → 调用方回退比例路径。非 PF SoC（无查表）同样 None（调用方静默走比例路径）
pub fn frontier_aligned(group: CoreGroup, available_freqs: &[u32]) -> Option<Arc<Vec<u32>>> {
    let targets = frontier_table()?.targets(group);
    let mut cache = FRONTIER_ALIGNED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let slot = cache.entry(group).or_default();
    if let Some((_, hit)) = slot
        .iter()
        .find(|(freqs, _)| freqs.as_slice() == available_freqs)
    {
        return Some(Arc::clone(hit));
    }
    let aligned: Arc<Vec<u32>> = Arc::new(
        targets
            .iter()
            .map(|&khz| {
                if khz == 0 {
                    0
                } else {
                    floor_khz(available_freqs, khz)
                }
            })
            .collect(),
    );
    slot.push((available_freqs.to_vec(), Arc::clone(&aligned)));
    Some(aligned)
}

/// kHz → 频率表中 `<= 目标` 的最大档（floor 对齐，频率表为升序的真实档位）：
/// 落点不得高于表给目标；目标低于首档时取首档（与比例路径同口径）
fn floor_khz(available_freqs: &[u32], khz: u32) -> u32 {
    let idx = available_freqs.partition_point(|&f| f <= khz);
    if idx == 0 {
        available_freqs.first().copied().unwrap_or(0)
    } else {
        available_freqs[idx - 1]
    }
}

/// [frontier_lenient] 前沿段解析失败不得拖垮整份 soc.yaml：单字段兜底为 None + warn 一次
/// （topology/capacity/freq 兜底是更贵资产，误配必须自愈）
fn deserialize_frontier_lenient<'de, D>(d: D) -> Result<Option<FrontierPolicy>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<serde_yaml::Value>::deserialize(d)?;
    match raw {
        None => Ok(None),
        Some(v) => match serde_yaml::from_value::<FrontierPolicy>(v) {
            Ok(p) => Ok(Some(p)),
            Err(e) => {
                log::warn!("frontier-policy-parse-failed: {e}");
                Ok(None)
            }
        },
    }
}

/// 当前 SoC 的帕累托前沿策略：仅当 soc.yaml 存在 `frontier_policy` 段且 `enabled == true` 时返回 Some；
/// 其它 SoC（无本段）天然 None → CLG 走原比例路径，行为逐位不变
pub fn soc_frontier_policy() -> Option<&'static FrontierPolicy> {
    soc_config()?.frontier_policy.as_ref().filter(|p| p.enabled)
}

/// 命中 SoC 的硬件基线（{soc}/soc.yaml），OnceLock 惰性解析一次
/// None = 非 ChiRi SoC / 嵌入缺失 / yaml 损坏（warn 一次，不 panic），调用方一律走原有回退路径
static SOC_CONFIG: OnceLock<Option<SocConfig>> = OnceLock::new();

/// 当前命中 SoC 的硬件基线；解析结果全程缓存，重复调用零开销
pub fn soc_config() -> Option<&'static SocConfig> {
    SOC_CONFIG
        .get_or_init(|| {
            let Some(hint) = matched_soc_hint() else {
                return None;
            };
            let Some(text) = embedded_config_file(&format!("{hint}/soc.yaml")) else {
                return None;
            };
            match serde_yaml::from_str::<SocConfig>(text) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::warn!("soc-config-parse-failed ({hint}): {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// capacity 兜底：按核心组取 soc.yaml [capacity] 值（真机 sysfs 由调用方优先读）
pub(crate) fn soc_capacity_for_group(group: CoreGroup) -> Option<u32> {
    let cap = soc_config()?.capacity.as_ref()?;
    match group {
        CoreGroup::Little => cap.little,
        CoreGroup::Big => cap.big,
        CoreGroup::Prime => cap.prime,
    }
}

/// policy 的首个相关 CPU（related_cpus 优先，退 affected_cpus），供按 policy 映射核心组
pub(crate) fn policy_first_cpu(policy_id: i32) -> Option<u32> {
    let base = format!("/sys/devices/system/cpu/cpufreq/policy{policy_id}");
    let text = std::fs::read_to_string(format!("{base}/related_cpus"))
        .or_else(|_| std::fs::read_to_string(format!("{base}/affected_cpus")))
        .ok()?;
    text.split_whitespace().next()?.parse().ok()
}

/// 频率档位兜底：scaling_available_frequencies 读不到/空表时，按 policy 首核所属核心组回退 soc.yaml [freq_khz]（升序拷贝）
/// 无表 / 非 ChiRi SoC / 首核不在任何核心组时返回 None，调用方维持原失败路径
pub(crate) fn soc_freq_fallback_for_policy(policy_id: i32) -> Option<Vec<u32>> {
    let table = soc_config()?.freq_khz.as_ref()?;
    let v = match core_group_of(policy_first_cpu(policy_id)?)? {
        CoreGroup::Little => &table.little,
        CoreGroup::Big => &table.big,
        CoreGroup::Prime => &table.prime,
    };
    (!v.is_empty()).then(|| v.to_vec())
}

// [governor]
/// 兜底调速器：soc.yaml 无 [cpu_governors] / 该 policy 一个都没匹配上时沿用（与历史写死值同）
const DEFAULT_CPU_GOVERNOR: &str = "schedutil";

/// 各 policy 选定的调速器：启动时 [`select_cpu_governors`] 批量算定，此后只读。
/// 用 Mutex 而非纯 OnceLock 是为了**缺谁补谁**：批量算定跑在 `force_online_all` 之前，
/// 启动期离线的簇那时还没有 cpufreq 目录、进不了表，取值时按同一口径补算（此时核已上线）
static POLICY_GOVERNORS: OnceLock<Mutex<HashMap<i32, String>>> = OnceLock::new();

fn governor_map() -> &'static Mutex<HashMap<i32, String>> {
    POLICY_GOVERNORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// soc.yaml [cpu_governors] 优先级表（去空白、丢空串）；无该段 → 空表（全落兜底值）
fn governor_prefs() -> Vec<String> {
    soc_config()
        .and_then(|c| c.cpu_governors.as_ref())
        .map(|v| {
            v.iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 单 policy 选型：优先级表从左到右，取第一个出现在 `scaling_available_governors` 里的；
/// 读不到列表（含 /sys 不可访问）或全不匹配 → 兜底值，行为与改造前逐位一致
fn pick_governor(policy_id: i32, prefs: &[String]) -> String {
    let available: Vec<String> = std::fs::read_to_string(format!(
        "/sys/devices/system/cpu/cpufreq/policy{}/scaling_available_governors",
        policy_id
    ))
    .map(|s| s.split_whitespace().map(|w| w.to_string()).collect())
    .unwrap_or_default();
    prefs
        .iter()
        .find(|g| available.iter().any(|a| a == *g))
        .cloned()
        .unwrap_or_else(|| DEFAULT_CPU_GOVERNOR.to_string())
}

/// 调度启动（及每次热重载）时调用：批量算定当前可见的 policy 并回填表，返回全表副本（供日志）。
/// 已在表内则不重复读 sysfs。所有「默认调速器」写入点（CLG / 特调 / PowerBase / FastLock /
/// 残留清理）都按这里的值写，不再各自写死 schedutil
pub fn select_cpu_governors() -> HashMap<i32, String> {
    let mut m = governor_map().lock().unwrap_or_else(|e| e.into_inner());
    if m.is_empty() {
        let prefs = governor_prefs();
        for p in crate::chiri::get_cpu_policies() {
            m.insert(p.id, pick_governor(p.id, &prefs));
        }
    }
    m.clone()
}

/// 该 policy 应写入的默认调速器。表里没有的（批量算定时该簇离线、或选型被 DOWN 跳过）
/// 当场按同一口径补算一次并回填；补算结果不进 `governor-selected` 日志
pub fn cpu_governor_for_policy(policy_id: i32) -> String {
    let mut m = governor_map().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(g) = m.get(&policy_id) {
        return g.clone();
    }
    let g = pick_governor(policy_id, &governor_prefs());
    m.insert(policy_id, g.clone());
    g
}

/// 特调可用性共享标志：chiri Config 合并 tuned_profiles.yaml 成功后置 true，缺失/损坏置 false
/// determine_mode 据此决定白名单应用进入特调还是回退 CLG（缺 tuned_profiles.yaml 的机型按普通模式调度）
static SPECIAL_TUNED_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// 特调是否可用（tuned_profiles.yaml 已成功加载）
pub fn is_special_tuned_available() -> bool {
    SPECIAL_TUNED_AVAILABLE.load(Ordering::Acquire)
}

/// 设置特调可用性：chiri Config::load 合并嵌入的 tuned_profiles.yaml 时调用
pub fn set_special_tuned_available(available: bool) {
    SPECIAL_TUNED_AVAILABLE.store(available, Ordering::Release);
}

// 功能总开关（fas_enabled / scenemode_enabled，meta.yaml 顶层字段，缺省 true）：Config::load（启动 + 热重载）同步到原子标志，
// 高频路径（fas_available / scenemode 进入判定）只读原子量，不触碰磁盘与锁
static FAS_ENABLED: AtomicBool = AtomicBool::new(true);
static SCENEMODE_ENABLED: AtomicBool = AtomicBool::new(true);
/// PowerBase 总开关（meta.yaml `powerbase_enabled`，**缺省 false**：默认由 CLG 接管）
static POWERBASE_ENABLED: AtomicBool = AtomicBool::new(false);

/// 设置 PowerBase 总开关（meta.yaml 的 powerbase_enabled，Config::load 时调用）
pub fn set_powerbase_enabled(enabled: bool) {
    POWERBASE_ENABLED.store(enabled, Ordering::Release);
}

/// PowerBase 是否开启（powerbase_enabled，缺省 false）：高频路径（determine_mode 模式替换、affinity promote 阈值）只读原子量，不读磁盘与锁
pub fn powerbase_enabled() -> bool {
    POWERBASE_ENABLED.load(Ordering::Acquire)
}

/// PowerBase 参数（feature.yaml `powerbase` 段，Config::load 下发）：CLG 的接管入口在 Cluster 那一刻
/// 才读一次（选中它当后端时才会用到），不在负载 tick 路径取锁
static POWERBASE_CFG: OnceLock<Mutex<PowerBaseConfig>> = OnceLock::new();

/// 下发 PowerBase 参数（Config::load 时与总开关同步调用）
pub fn set_powerbase_config(cfg: PowerBaseConfig) {
    let lock = POWERBASE_CFG.get_or_init(|| Mutex::new(PowerBaseConfig::default()));
    if let Ok(mut g) = lock.lock() {
        *g = cfg;
    }
}

/// 取 PowerBase 参数副本（未下发过则代码默认值）
pub fn powerbase_config() -> PowerBaseConfig {
    match POWERBASE_CFG.get() {
        Some(l) => l.lock().map(|g| g.clone()).unwrap_or_default(),
        None => PowerBaseConfig::default(),
    }
}

/// 当前是否亮屏（屏幕事件同步，缺省 true）：接管判决需要在 CLG 内部取值——PowerBase 是它的后端之一，
/// 息屏必须交 doze（息屏接管会把夜间功耗抬高），而屏幕事件的处理在调度循环而不在 CLG 内部
static SCREEN_ON: AtomicBool = AtomicBool::new(true);

/// 同步亮屏状态（ScreenStateChange 事件与启动时各同步一次）
pub fn set_screen_on(on: bool) {
    SCREEN_ON.store(on, Ordering::Release);
}

/// 当前是否亮屏
pub fn screen_on() -> bool {
    SCREEN_ON.load(Ordering::Acquire)
}

/// 息屏判定值（screen_off_value`，缺省 1）：debug.tracing.screen_state 等于该值视为息屏，其余数字视为亮屏。Config::load 同步原子量，
/// 屏幕检测（monitor/screen_detect.rs [prop]）只读原子量，不在 tick 内读磁盘
static SCREEN_OFF_VALUE: AtomicU32 = AtomicU32::new(1);

/// 设置息屏判定值（meta.yaml 的 screen_off_value，Config::load 时调用）
pub fn set_screen_off_value(value: u32) {
    SCREEN_OFF_VALUE.store(value, Ordering::Release);
}

/// 息屏判定值（meta.yaml 的 screen_off_value，缺省 1）
pub fn screen_off_value() -> u32 {
    SCREEN_OFF_VALUE.load(Ordering::Acquire)
}

/// 设置 FAS 总开关（meta.yaml 的 fas_enabled，Config::load 时调用）
pub fn set_fas_enabled(enabled: bool) {
    FAS_ENABLED.store(enabled, Ordering::Release);
}

/// 设置 scenemode 总开关（meta.yaml 的 scenemode_enabled，Config::load 时调用）
pub fn set_scenemode_enabled(enabled: bool) {
    SCENEMODE_ENABLED.store(enabled, Ordering::Release);
}

/// FAS 总开关是否开启（meta.yaml 的 fas_enabled，缺省 true）
pub fn fas_enabled() -> bool {
    FAS_ENABLED.load(Ordering::Acquire)
}

/// scenemode 总开关是否开启（meta.yaml 的 scenemode_enabled，缺省 true）
pub fn scenemode_enabled() -> bool {
    SCENEMODE_ENABLED.load(Ordering::Acquire)
}

// 实验室（rhine）运行时覆盖层：把两项没有持久化载体的影响收在这里——global_mode 在 rules.yaml 是编译期嵌入、special_tuned 白名单是 include_str! 嵌入，均无外部开关
// fas_enabled / scenemode_enabled 有 meta.yaml 载体不走这里；读取方都在高频路径，只读原子量不碰磁盘
static LAB_GLOBAL_MODE: Mutex<Option<String>> = Mutex::new(None);
static LAB_SPECIAL_TUNED_DISABLED: AtomicBool = AtomicBool::new(false);

/// 实验室覆盖的全局模式（None = 无覆盖，按 rules.yaml 的 global_mode）
pub fn lab_global_mode() -> Option<String> {
    LAB_GLOBAL_MODE.lock().ok().and_then(|g| g.clone())
}

/// 设置或清除实验室的全局模式覆盖
pub fn set_lab_global_mode(mode: Option<String>) {
    if let Ok(mut g) = LAB_GLOBAL_MODE.lock() {
        *g = mode;
    }
}

/// 实验室是否关闭了所有场景特调
pub fn lab_special_tuned_disabled() -> bool {
    LAB_SPECIAL_TUNED_DISABLED.load(Ordering::Acquire)
}

/// 设置实验室的场景特调总闸（true = 所有特调判定失效）
pub fn set_lab_special_tuned_disabled(disabled: bool) {
    LAB_SPECIAL_TUNED_DISABLED.store(disabled, Ordering::Release);
}

/// 返回当前应加载的配置文件路径：命中 SoC 且存在 config/{命中片段}/meta.yaml 时用之，否则回退默认 config/meta.yaml
/// 所有配置加载/热重载入口（main.rs 与 chiri config_watcher）统一走这里，保证目标机型用处理器独立配置
pub fn get_config_path() -> PathBuf {
    matched_soc_config_dir()
        .map(|dir| dir.join("meta.yaml"))
        .unwrap_or_else(|| get_module_root().join("config").join("meta.yaml"))
}

// [special_tuned]
/// 特调白名单条目（src/chiri/special_tuned.yaml 编译期嵌入并解析，用户/WebUI 不可修改；磁盘上的 special_tuned.yaml 仅是运行时导出快照）
pub struct SpecialTunedEntry {
    /// 匹配器原文：精确包名，或 "re:" 前缀的正则表达式
    pub package: String,
    /// 正则条目的预编译结果（精确条目为 None）
    pub regex: Option<regex::Regex>,
    /// 该应用可用的特调模式列表（须在 chiri Config::get_mode 注册）
    pub modes: Vec<String>,
    /// 优先回退模式：用户未显式配置该应用时默认采用（必须在 modes 内）
    pub fallback: String,
}

impl SpecialTunedEntry {
    /// 包名是否命中本条目：精确条目全等比较，正则条目 is_match
    fn matches(&self, pkg: &str) -> bool {
        match &self.regex {
            Some(re) => re.is_match(pkg),
            None => self.package == pkg,
        }
    }
}

/// 嵌入的白名单原文（include_str! 相对 src/common.rs）
const SPECIAL_TUNED_TEXT: &str = include_str!("chiri/special_tuned.yaml");

/// 解析结果只算一次，之后全部走缓存
static SPECIAL_TUNED: OnceLock<Vec<SpecialTunedEntry>> = OnceLock::new();

/// 解析嵌入文本：跳过空行与 # 注释行，按 匹配器:模式列表:回退模式 切分；"re:" 前缀预编译正则，编译失败跳过该条并告警
fn parse_special_tuned(text: &str) -> Vec<SpecialTunedEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
// 注意：先摘 "re:" 前缀再切分——直接整行 splitn(3,':') 会让包名变字面量 "re" 永不命中并混入假模式名（正则体按 yaml 约定不含冒号）
        let (is_regex, body) = match line.strip_prefix("re:") {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let mut parts = body.splitn(3, ':');
        let (Some(pkg), Some(modes), Some(fallback)) = (parts.next(), parts.next(), parts.next())
        else {
            log::warn!("special-tuned: malformed entry skipped: {}", line);
            continue;
        };
        let modes: Vec<String> = modes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let fallback = fallback.trim().to_string();
        if modes.is_empty() || fallback.is_empty() {
            log::warn!(
                "special-tuned: entry with empty modes/fallback skipped: {}",
                line
            );
            continue;
        }
        let (package, regex) = if is_regex {
            match regex::Regex::new(pkg) {
                Ok(re) => (format!("re:{pkg}"), Some(re)),
                Err(e) => {
                    log::warn!("special-tuned: invalid regex '{}' skipped: {}", pkg, e);
                    continue;
                }
            }
        } else {
            (pkg.to_string(), None)
        };
        out.push(SpecialTunedEntry {
            package,
            regex,
            modes,
            fallback,
        });
    }
    out
}

/// 全部白名单条目（精确 + 正则，按文件顺序）；main.rs 导出 special_tuned.yaml 时只取 regex.is_none() 的精确条目
pub fn special_tuned_entries() -> &'static [SpecialTunedEntry] {
    SPECIAL_TUNED.get_or_init(|| parse_special_tuned(SPECIAL_TUNED_TEXT))
}

// [exact_index]
/// 精确条目（regex.is_none()）的查找索引：只加速精确段查询，不改遍历/导出行为建表按文件顺序 or_insert，同名保留第一条，与原线性 find 的「文件顺序第一条」语义等价
static SPECIAL_TUNED_EXACT: OnceLock<HashMap<&'static str, &'static SpecialTunedEntry>> =
    OnceLock::new();

fn special_tuned_exact() -> &'static HashMap<&'static str, &'static SpecialTunedEntry> {
    SPECIAL_TUNED_EXACT.get_or_init(|| {
        let mut map = HashMap::new();
        for e in special_tuned_entries() {
            if e.regex.is_none() {
                map.entry(e.package.as_str()).or_insert(e);
            }
        }
        map
    })
}

/// 查询包名命中的白名单条目：先精确包名（文件顺序），未命中再按正则条目优先级：rules.yaml 用户自定义 app_modes > 特调白名单回退模式 > global_mode；实验室关闭特调期间恒返回 None
pub fn special_tuned_entry(pkg: &str) -> Option<&'static SpecialTunedEntry> {
    if lab_special_tuned_disabled() {
        return None;
    }
// [exact_index] 两段式查找顺序不变：先精确段（等价原「文件顺序第一条」），未命中再按文件顺序线性匹配正则条目，行为不变
    special_tuned_exact()
        .get(pkg)
        .copied()
        .or_else(|| special_tuned_entries().iter().find(|e| e.matches(pkg)))
}

/// 查询包名是否命中特调白名单，命中返回优先回退模式
pub fn special_tuned_mode(pkg: &str) -> Option<String> {
    special_tuned_entry(pkg).map(|e| e.fallback.clone())
}

/// 判断模式名是否为特调模式（任一条目 modes 中出现）；实验室关闭特调期间恒 false此时 chiri 主循环所有 is_special_mode 分支走普通模式路径，正在跑的特调按普通模式收尾
pub fn is_special_mode(mode: &str) -> bool {
    if lab_special_tuned_disabled() {
        return false;
    }
    special_tuned_entries()
        .iter()
        .any(|e| e.modes.iter().any(|m| m == mode))
}

/// 白名单注册的全部特调模式名（精确 + 正则条目 modes 并集，去重）用途：Config 合并时校验「注册了模式但没有参数组」的错配——那种情况会静默回退 akmode 段，省电场景反效果，必须在日志暴露
pub fn special_tuned_mode_names() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in special_tuned_entries() {
        for m in &e.modes {
            if !out.iter().any(|x| x == m) {
                out.push(m.clone());
            }
        }
    }
    out
}

/// 包名是否被允许使用指定特调模式（包名命中白名单且模式在该条目 modes 列表中）
pub fn is_special_mode_allowed(pkg: &str, mode: &str) -> bool {
    special_tuned_entry(pkg)
        .map(|e| e.modes.iter().any(|m| m == mode))
        .unwrap_or(false)
}

// [fas_whitelist]
// FAS（帧感知调度）白名单与每应用配置（编译期嵌入，用户/WebUI 不可修改）
// 白名单运行时导出到模块根 fas_whitelist.yaml 供 WebUI 只读展示；每应用配置不导出

// 白名单与每应用配置（normal/fas.yaml 与 normal/fas/<配置名>.yaml）随 module/config 整体构建期嵌入（见 [embedded]），不在此逐文件硬编码

/// 按配置名返回嵌入 FAS 配置文本（构建期自动收录 normal/fas/*.yaml）。新增 FAS 游戏：fas.yaml 白名单加一行 + 新建 normal/fas/<配置名>.yaml，无需改 .rs
pub fn embedded_fas_app_str(name: &str) -> Option<&'static str> {
    embedded_config_file(&format!("normal/fas/{name}.yaml"))
}

#[derive(Debug, Deserialize)]
struct FasWhitelistFile {
    #[serde(default)]
    fas: FasWhitelistSection,
}

#[derive(Debug, Default, Deserialize)]
struct FasWhitelistSection {
    #[serde(default)]
    apps: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct FasAppFile {
    #[serde(default)]
    fas_rules: crate::fas_types::FasRulesConfig,
}

static FAS_WHITELIST: OnceLock<HashMap<String, String>> = OnceLock::new();
static FAS_APP_CONFIGS: OnceLock<HashMap<String, crate::fas_types::FasRulesConfig>> =
    OnceLock::new();

/// FAS 白名单（精确包名 → 配置名），编译期嵌入，解析失败回退空表
pub fn fas_whitelist() -> &'static HashMap<String, String> {
    FAS_WHITELIST.get_or_init(|| {
        // 构建期嵌入的 normal/fas.yaml（缺失时按空表处理）
        let text = embedded_config_file("normal/fas.yaml").unwrap_or_default();
        match serde_yaml::from_str::<FasWhitelistFile>(text) {
            Ok(f) => f.fas.apps,
            Err(e) => {
                log::warn!("fas-config-parse-failed: {e}");
                HashMap::new()
            }
        }
    })
}

/// 精确匹配 FAS 白名单
pub fn fas_whitelist_entry(pkg: &str) -> Option<&'static String> {
    fas_whitelist().get(pkg)
}

/// 按配置名取该应用的 FAS 规则（首次调用时解析全部白名单应用，normalize 后缓存）
pub fn fas_app_config(name: &str) -> Option<&'static crate::fas_types::FasRulesConfig> {
    FAS_APP_CONFIGS
        .get_or_init(|| {
            let mut map = HashMap::new();
            let mut names: Vec<&String> = fas_whitelist().values().collect();
            names.sort();
            names.dedup();
            for name in names {
                let Some(text) = embedded_fas_app_str(name) else {
                    log::warn!("fas-config-missing: {name}");
                    continue;
                };
                match serde_yaml::from_str::<FasAppFile>(text) {
                    Ok(mut f) => {
                        f.fas_rules.normalize();
                        f.fas_rules.migrate_legacy_margins();
                        map.insert(name.clone(), f.fas_rules);
                    }
                    Err(e) => log::warn!("fas-config-parse-failed ({name}): {e}"),
                }
            }
            map
        })
        .get(name)
}

/// FAS 是否可用：总开关开启（meta.yaml fas_enabled）、白名单非空且至少一个应用配置解析成功
pub fn fas_available() -> bool {
    if !fas_enabled() {
        return false;
    }
    if fas_whitelist().is_empty() {
        return false;
    }
    // 触发惰性解析（fas_app_config 首次调用会解析全部白名单应用）
    if let Some(name) = fas_whitelist().values().next() {
        let _ = fas_app_config(name);
    }
    FAS_APP_CONFIGS.get().map_or(false, |m| !m.is_empty())
}

/// 模式名是否为 FAS
pub fn is_fas_mode(mode: &str) -> bool {
    mode == "fas"
}

// 线程亲和黑名单（src/chiri/affinity_blacklist.yaml，编译期嵌入，用户/WebUI 不可修改，独立成文件便于维护）命中黑名单的进程：全部线程保持全核运行，
// AffinityManager 不做任何迁移与亲和

// [aff_blacklist]
/// 黑名单条目：精确进程名/包名，或 "re:" 前缀的正则（同特调白名单格式）
pub struct AffinityBlacklistEntry {
    pub pattern: String,
    pub regex: Option<regex::Regex>,
}

impl AffinityBlacklistEntry {
    fn matches(&self, cmdline: &str) -> bool {
        match &self.regex {
            Some(re) => re.is_match(cmdline),
            None => self.pattern == cmdline,
        }
    }
}

const AFFINITY_BLACKLIST_TEXT: &str = include_str!("chiri/affinity_blacklist.yaml");

static AFFINITY_BLACKLIST: OnceLock<Vec<AffinityBlacklistEntry>> = OnceLock::new();

/// 解析黑名单文本：跳过空行与 # 注释行；"re:" 前缀预编译正则，编译失败跳过该条（不影响其余）
fn parse_affinity_blacklist(text: &str) -> Vec<AffinityBlacklistEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_prefix("re:") {
            Some(pat) => match regex::Regex::new(pat) {
                Ok(re) => out.push(AffinityBlacklistEntry {
                    pattern: line.to_string(),
                    regex: Some(re),
                }),
                Err(e) => log::warn!("affinity-blacklist: invalid regex '{}' skipped: {}", pat, e),
            },
            None => out.push(AffinityBlacklistEntry {
                pattern: line.to_string(),
                regex: None,
            }),
        }
    }
    out
}

/// 全部黑名单条目（精确 + 正则，按文件顺序），OnceLock 缓存
pub fn affinity_blacklist_entries() -> &'static [AffinityBlacklistEntry] {
    AFFINITY_BLACKLIST.get_or_init(|| parse_affinity_blacklist(AFFINITY_BLACKLIST_TEXT))
}

/// 进程 cmdline（或线程 comm）是否命中亲和黑名单。另含两条内置兜底（不经文件、不可关闭）：空 cmdline = 内核线程 → 黑名单；'/' 开头 = native 二进制路径 → 黑名单
pub fn is_affinity_blacklisted(cmdline: &str) -> bool {
    if cmdline.is_empty() || cmdline.starts_with('/') {
        return true;
    }
    affinity_blacklist_entries()
        .iter()
        .any(|e| e.matches(cmdline))
}

// 编译期嵌入的配置（module/config 整目录 include_dir!，防篡改）原 config.yaml 已拆分为 meta.yaml（用户可修改抬头）+ feature.yaml（不可修改调优段），各 SoC 目录与默认 config/ 下各一份
// 二者仅存于二进制，磁盘不落盘（feature 无外部读取方；meta 磁盘副本由 sync_meta_snapshot 自愈）
// 目录整体构建期嵌入：新增/删除 yaml 无需改 .rs；必需文件缺失由 build.rs 断言编译失败；文件改动由 include_bytes! 依赖跟踪、目录增删由 rerun-if-changed=module/config 触发重编译
// ***-example.yaml 参考文件不放本目录（会随之嵌入二进制），一律放 mdocs/

// [embedded]
static CONFIG_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/module/config");

/// 按相对 module/config 的路径取嵌入文本（如 "8550/meta.yaml"），缺失返回 None；必需文件存在性由 build.rs 编译期断言，运行时不会静默空转
pub(crate) fn embedded_config_file(rel: &str) -> Option<&'static str> {
    CONFIG_DIR.get_file(rel).and_then(|f| f.contents_utf8())
}

/// 嵌入的 meta.yaml（用户可修改字段默认值）：按命中处理器取 {soc}/meta.yaml，未命中取 meta.yaml
pub fn embedded_meta_str() -> &'static str {
    if let Some(soc) = matched_soc_hint() {
        if let Some(text) = embedded_config_file(&format!("{soc}/meta.yaml")) {
            return text;
        }
    }
    embedded_config_file("meta.yaml").unwrap_or_default()
}

/// 嵌入的 feature.yaml（不可修改调优段）：以根 `feature.yaml` 为**基底**，用命中处理器的
/// `{soc}/feature.yaml` **按字段深合并覆盖**——map 递归合并（SoC 少写的字段继承根值），
/// 标量/序列整体替换（不拼接）。未命中处理器时即根文件本身。仅存于二进制，磁盘不落盘、不监听。
/// ⚠️ 全量合并意味着 SoC 未写的**整段**也会从根继承（如 8550/8998 无 `vector` 段 → 继承根的 vector）。
fn merge_yaml(base: &mut serde_yaml::Value, over: &serde_yaml::Value) {
    if let (serde_yaml::Value::Mapping(b), serde_yaml::Value::Mapping(o)) = (&mut *base, over) {
        for (k, v) in o {
            match b.get_mut(k) {
                Some(slot) => merge_yaml(slot, v),
                None => {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
    } else {
        // 任一侧非 map（标量/序列/类型不符）：整体替换，避免把两种形状混在一起
        *base = over.clone();
    }
}

pub fn embedded_feature_str() -> &'static str {
    static MERGED: OnceLock<String> = OnceLock::new();
    MERGED.get_or_init(|| {
        let root = embedded_config_file("feature.yaml").unwrap_or_default();
        let Some(soc_text) =
            matched_soc_hint().and_then(|soc| embedded_config_file(&format!("{soc}/feature.yaml")))
        else {
            return root.to_string();
        };
        // 任一侧解析失败：退回 SoC 原文（与旧行为一致，后续 Config 反序列化会报出具体问题）
        let (Ok(mut base), Ok(over)) = (
            serde_yaml::from_str::<serde_yaml::Value>(root),
            serde_yaml::from_str::<serde_yaml::Value>(soc_text),
        ) else {
            return soc_text.to_string();
        };
        merge_yaml(&mut base, &over);
        serde_yaml::to_string(&base).unwrap_or_else(|_| soc_text.to_string())
    })
}

/// 嵌入的 tuned_profiles.yaml（config/normal/）：特调参数组——缺省段 `akmode`（游戏特调兼未注册模式回退）+ `tuned_profiles` 段按模式名分派
pub fn embedded_tuned_profiles_str() -> &'static str {
    embedded_config_file("normal/tuned_profiles.yaml").unwrap_or_default()
}

/// 嵌入的 rhine-init.yaml（实验室模式定义）：只给守护进程读，不落盘；WebUI 模式列表与文案硬编码
pub fn embedded_rhine_init_str() -> &'static str {
    embedded_config_file("rhine-init.yaml").unwrap_or_default()
}

/// 嵌入的 rules.yaml（模块根）：与其他只读文件同口径——运行时一律读嵌入内容，磁盘文件仅作对外展示副本，被篡改不影响调度行为
pub fn embedded_rules_str() -> &'static str {
    include_str!("../module/rules.yaml")
}

/// 解析嵌入的 rules.yaml（运行时唯一规则来源）：解析失败回退 Default 并告警，绝不 panic
pub fn embedded_rules() -> crate::monitor::config::RulesConfig {
    match serde_yaml::from_str(embedded_rules_str()) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[Rules] Embedded rules parse failed: {e}. Default.");
            crate::monitor::config::RulesConfig::default()
        }
    }
}

/// 嵌入的 scenemode.yaml（config/normal/）
pub fn embedded_scenemode_str() -> &'static str {
    embedded_config_file("normal/scenemode.yaml").unwrap_or_default()
}

/// 嵌入的语言包：zh → i18n/zh.ftl，其余语言一律回退 i18n/en.ftl
pub fn embedded_ftl_str(lang: &str) -> &'static str {
    let rel = if lang.eq_ignore_ascii_case("zh") {
        "i18n/zh.ftl"
    } else {
        "i18n/en.ftl"
    };
    embedded_config_file(rel).unwrap_or_default()
}

/// 磁盘 meta.yaml 的严格结构：字段全部可选（缺省 = 沿用内嵌默认，见 ExternalMetaOverrides 的 None = 不变更语义）、拒绝未知字段后加字段一律设计成可选（serde default），
/// 老文件缺行仍合法；出现即校验：类型不符 / 取值不在白名单 / 未知键 → 整文件判非法，由 sync_meta_snapshot 用内嵌默认整体覆盖（缺字段不算非法）注意：
/// 新增字段四处必须同步——本结构 + ExternalMetaOverrides、chiri::config::Meta 与 Config::load 合并、四个 meta.yaml 模板、
/// WebUI META_FIELDS/WRITABLE_FIELDS（含布尔校验列表），漏改会让新键被判未知字段整体重置
// [external_meta]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaYamlFile {
    name: Option<String>,
    author: Option<String>,
    language: Option<String>,
    loglevel: Option<String>,
    dev_record: Option<bool>,
/// aff `@S` 每秒快照帧的 top-N 进程数（手改字段，WebUI 无开关），详见 ExternalMetaOverrides 同名字段
    devimp_top_n: Option<usize>,
    fas_enabled: Option<bool>,
    scenemode_enabled: Option<bool>,
    /// 线程摆放总开关（affinity + core_ctl），详见 ExternalMetaOverrides
    thread_bind: Option<bool>,
/// PowerBase 总开关（缺省 false）必须与本结构同步注册：deny_unknown_fields 下 WebUI 直写键不在这里会让整个 meta.yaml 被判非法并整体重置
    powerbase_enabled: Option<bool>,
    /// 功耗口径开关（PowerAVG.chr），详见 ExternalMetaOverrides（缺省 false）
    power_avg: Option<bool>,
    /// 常驻状态通知开关，详见 ExternalMetaOverrides（缺省 true）
    notify: Option<bool>,
    /// 电池读数：OPlus 私有节点 / 双电芯 / 倍电压 / 倍电流 / 单位校准，缺省见结构注释
    oplus_chg: Option<bool>,
    oplus_dual_cell: Option<bool>,
    voltage_double: Option<bool>,
    current_double: Option<bool>,
    voltage_divisor: Option<f32>,
    current_divisor: Option<f32>,
    /// 旧键（曾把电压/电流校准合成一个）：仍接收，等价于只设电压校准
    unit_divisor: Option<f32>,
    /// 「不改」开关（模板不写）：详见 ExternalMetaOverrides
    nofix: Option<bool>,
    /// 耗电读数满量程 W（缺省 12）：详见 ExternalMetaOverrides
    power_max_w: Option<f32>,
    /// 息屏判定值（0/1，缺省 1）：详见 ExternalMetaOverrides
    screen_off_value: Option<u32>,
}

/// 磁盘 meta.yaml 交给 Config::load 的覆盖值：None = 文件里没写这个键 → 沿用内嵌默认（字段全部可选，老文件/精简文件都合法）
/// daemon 只消费其中 7 项（name/author 仅 WebUI 展示，直接读文件即可）
#[derive(Debug, Clone, Default)]
pub struct ExternalMetaOverrides {
    pub loglevel: Option<String>,
    pub language: Option<String>,
    pub dev_record: Option<bool>,
/// aff @S 每秒快照 top-N 进程数（缺省 10）：每秒按 util 降序落盘前 N 个进程；前台树与被管进程不受 N 截断、恒定落盘消费点：Config::load 合并后由 Meta::
/// normalize 钳到 1..=64（超限 clamp，不判文件非法）
    pub devimp_top_n: Option<usize>,
    pub fas_enabled: Option<bool>,
    pub scenemode_enabled: Option<bool>,
/// 线程摆放总闸（`thread_bind`）：实验室 frozen 专用机制——用户侧开关已移除，仅 frozen 模式写 false 交还线程亲和/绑核与 core_ctl。 与机型内嵌 feature
/// yaml 的两个子开关取「与」，见 chiri/config.rs::Config::load
    pub thread_bind: Option<bool>,
/// PowerBase 总开关（缺省 false）：开启后原由 CLG 接管的亮屏日常场合改由 PowerBase 接管（以放电功耗为指标）消费点：Config::
/// load 合并后 set_powerbase_enabled 同步原子量
    pub powerbase_enabled: Option<bool>,
/// 功耗口径开关（PowerAVG.chr）：false（默认）= 参考值（旧值先乘 10 再按 10:1 加权递推，偏历史，含息屏样本）；true = 累计平均（等权全史，仅亮屏放电样本）两者都只在放电时取样；
/// 仅 ChiRi 1s 状态采样消费；写侧走单次读-改-写顶层行替换，见 WebUI contract/meta.ts
    pub power_avg: Option<bool>,
/// 常驻状态通知开关（notify，默认 true）：daemon 每 5s 把调度状态（前台包名/模式/家族/子模式/温度/功耗）写进常驻通知，见 src/notify.rs
/// false = 不投递并撤销已投递通知；仅 ChiRi 1s 循环消费
    pub notify: Option<bool>,
/// OPlus 私有电压/电流节点（`oplus_chg`，默认 false）：true = 优先读 `/sys/class/oplus_chg/battery/bcc_parms`（下标 6 电芯电压0、8 电流、11 电芯电压1，
/// mV/mA），读不到回退标准 power_supply 节点。仅 OPlus 机型有意义
    pub oplus_chg: Option<bool>,
/// OPlus 双电芯（默认 false）：私有节点按两节并联读——电压取两节平均（下标 6 与 11）、电流 ×2（下标 8 为单节支路）；仅在 oplus_chg 打开时生效
    pub oplus_dual_cell: Option<bool>,
/// 倍电压（默认 false）：标准节点电压 ×2（双电芯机型标准节点只报单节值）；与 oplus_chg 互斥，私有开关打开时被强制关闭
    pub voltage_double: Option<bool>,
    /// 倍电流（`current_double`，默认 false）：标准节点路径电流 ×2，互斥关系同上
    pub current_double: Option<bool>,
/// 电压校准除数（默认 1000000，须 > 0）：节点原始值 ÷ 该值 = V，读取层不做换算缺省 = 标准 Android ABI 的 µV 口径；OPlus 私有节点报 mV，
/// 安装脚本检测到该节点时写入 1000（customize.sh [battery-detect]）
    pub voltage_divisor: Option<f32>,
/// 电流校准除数（默认 1000000，须 > 0）：节点原始值 ÷ 该值 = 安培；标准节点 µA → 1000000，OPlus 私有节点 mA → 1000（安装脚本自动写入）
/// 与电压分开：两个量未必同时错单位，分开才能单独校正（W = |A| × V 保持自洽）
    pub current_divisor: Option<f32>,
/// 「不改」开关（nofix，默认 false 且模板不写）：true = 启动时跳过所有覆盖类操作——webui 资产还原（restore_webroot）与 meta
/// yaml 快照自愈（sync_meta_snapshot）用户自担文件被篡改风险；rhine 实验与 WebUI 写入不受影响
    pub nofix: Option<bool>,
/// 耗电读数满量程 W（power_max_w，缺省 12）：只影响 WebUI 状态页仪表盘进度换算，不参与任何调度决策
    pub power_max_w: Option<f32>,
/// 息屏判定值（screen_off_value，缺省 1）：debug.tracing.screen_state 等于该值视为息屏；取值仅 0/1，其它值在 parse_disk_meta 回退默认（不判整文件非法）消费点：
/// Config::load 合并后 set_screen_off_value 同步原子量；customize.sh [screen-detect] 安装期按属性实测值翻转该字段
    pub screen_off_value: Option<u32>,
}

/// 读磁盘 meta.yaml（先经 sync_meta_snapshot 校验/纠正）：文件缺失或仍非法返回 None，调用方沿用嵌入默认——绝不 panic，不让坏文件拖垮配置加载
pub fn read_external_meta(path: &Path) -> Option<ExternalMetaOverrides> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_disk_meta(&text)
}

/// 提前读「不改」开关（nofix）：必须在 sync_meta_snapshot 之前调用——晚了文件可能已被内嵌默认覆盖（该覆盖本身就是待跳过操作之一）文件缺失/非法返回 false（照常自愈）
pub fn read_nofix_flag(path: &Path) -> bool {
    read_external_meta(path)
        .and_then(|m| m.nofix)
        .unwrap_or(false)
}

// 「不改」进程级标志：main 启动期判定后置位，此后所有覆盖类操作入口（webui 资产还原、meta/rules 快照自愈，含热重载路径）只读原子量高频路径不许读磁盘，与 FAS_ENABLED 同范式
static NOFIX: AtomicBool = AtomicBool::new(false);

/// 记录启动期判定的 nofix 状态（在 read_nofix_flag 之后、任何覆盖类操作之前调用）
pub fn set_nofix(active: bool) {
    NOFIX.store(active, Ordering::SeqCst);
}

/// 是否跳过「二进制内容对外部文件的覆盖类操作」（webui 资产还原 / meta、rules 快照自愈）
pub fn nofix_active() -> bool {
    NOFIX.load(Ordering::SeqCst)
}

/// 嵌入 meta.yaml 的默认覆盖值（编译期内容，构建期断言文件存在，正常必合法）
pub fn embedded_meta_defaults() -> ExternalMetaOverrides {
    parse_disk_meta(embedded_meta_str()).unwrap_or_default()
}

/// 日志等级白名单（大小写不敏感，与 logger 支持的档位一致）
const LOGLEVELS: [&str; 6] = ["OFF", "ERROR", "WARN", "INFO", "DEBUG", "TRACE"];

/// 去掉成对的单层引号（meta.yaml 里字符串值常带引号）
fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// 校验并归一化日志等级：必须整体等于某个档位（去引号后），含空白/注释/换行的脏值拒绝
fn sanitize_loglevel(raw: &str) -> Option<String> {
    let v = unquote(raw.trim());
    LOGLEVELS
        .iter()
        .find(|l| l.eq_ignore_ascii_case(v))
        .map(|l| l.to_string())
}

/// 校验并归一化日志语言：仅接受 en / zh（嵌入语言包只打包这两种）
fn sanitize_language(raw: &str) -> Option<String> {
    match unquote(raw.trim()).to_ascii_lowercase().as_str() {
        "zh" => Some("zh".to_string()),
        "en" => Some("en".to_string()),
        _ => None,
    }
}

/// 整体校验磁盘 meta.yaml：结构严格（无未知键、类型正确）+ 取值白名单；任一字段异常返回 None——调用方以内嵌默认覆盖修正，用户乱改不生效
fn parse_disk_meta(text: &str) -> Option<ExternalMetaOverrides> {
    let f: MetaYamlFile = serde_yaml::from_str(text).ok()?;
    // 出现即校验（缺省跳过）：name/author 非空
    if f.name.as_deref().is_some_and(|v| v.trim().is_empty())
        || f.author.as_deref().is_some_and(|v| v.trim().is_empty())
    {
        return None;
    }
// 满量程：非有限/非正/超 200 视为写错，回退内嵌默认 12——单个数笔误不判整文件非法（WebUI 写入侧另有 1..=200 校验）
    let power_max_w = f.power_max_w.map(|v| {
        if v.is_finite() && v > 0.0 && v <= 200.0 {
            v
        } else {
            crate::utils::default_power_max_w()
        }
    });
// 单位校准：非有限/非正视为写错，回退内嵌默认（同 power_max_w 口径，单个数笔误不判整文件非法；WebUI 侧另有 > 0 校验）；旧键 unit_divisor 仍接收作电压校准兜底
    let sane = |v: f32| {
        if v.is_finite() && v > 0.0 {
            v
        } else {
            crate::utils::DEFAULT_UNIT_DIVISOR
        }
    };
    let voltage_divisor = f.voltage_divisor.or(f.unit_divisor).map(sane);
    let current_divisor = f.current_divisor.map(sane);
// 息屏判定值：仅 0/1 合法，其它回退默认 1（同 power_max_w 口径，单字段笔误不判整文件非法）
    let screen_off_value = f.screen_off_value.map(|v| if v <= 1 { v } else { 1 });
    Some(ExternalMetaOverrides {
        loglevel: match f.loglevel.as_deref() {
            Some(v) => Some(sanitize_loglevel(v)?),
            None => None,
        },
        language: match f.language.as_deref() {
            Some(v) => Some(sanitize_language(v)?),
            None => None,
        },
        dev_record: f.dev_record,
        devimp_top_n: f.devimp_top_n,
        fas_enabled: f.fas_enabled,
        scenemode_enabled: f.scenemode_enabled,
        thread_bind: f.thread_bind,
        powerbase_enabled: f.powerbase_enabled,
        power_avg: f.power_avg,
        notify: f.notify,
        oplus_chg: f.oplus_chg,
        oplus_dual_cell: f.oplus_dual_cell,
        voltage_double: f.voltage_double,
        current_double: f.current_double,
        voltage_divisor,
        current_divisor,
        nofix: f.nofix,
        power_max_w,
        screen_off_value,
    })
}

/// meta.yaml 快照自愈：main.rs 启动时与 chiri config_watcher 触发热重载前调用文件合法 → 跳过写入（返回 false，防 config_watcher 事件成环）；
/// 文件缺失/不可读 → 嵌入原文原子重建（不留警告注释）-任一字段非法 → 嵌入原文整体覆盖 + 末尾追加警告注释 + warn 日志（完全丢失重建与「修改出错纠正不同，前者不留言）
pub fn sync_meta_snapshot(meta_path: &Path) -> bool {
    let embedded = embedded_meta_str();
    let corrected = match std::fs::read_to_string(meta_path) {
        Ok(text) => {
            if parse_disk_meta(&text).is_some() {
                return false;
            }
            log::warn!(
                "[Meta] meta.yaml fields invalid, reset to embedded defaults: {}",
                meta_path.display()
            );
            format!(
                "{embedded}\n# 上一次修改存在非法字段，已恢复为默认值；详情见 logs/daemon.log\n"
            )
        }
        Err(_) => {
            log::info!(
                "[Meta] meta.yaml missing, rebuilt from embedded defaults: {}",
                meta_path.display()
            );
            embedded.to_string()
        }
    };
    if let Some(parent) = meta_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_file_no_panic(meta_path, corrected.as_bytes())
}

/// rules.yaml 快照复制：把嵌入 rules.yaml 复制到模块根（嵌入内容唯一基准，磁盘副本仅供展示/备份，被篡改不影响调度），内容一致跳过写入防 panic 约定：
/// 对外写文件失败绝不 panic——原子写失败后补建父目录重写一次，仍失败记 warn 并跳过，不影响守护进程启动运行
pub fn sync_rules_snapshot(path: &Path) -> bool {
    let content = embedded_rules_str();
    // 内容一致就跳过：防止 config_watcher/inotify 事件循环
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return false;
    }
    if write_file_no_panic(path, content.as_bytes()) {
        return true;
    }
    // 第一次写入失败（目录缺失/权限/IO 抖动）：补建父目录后重写一次
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if write_file_no_panic(path, content.as_bytes()) {
        log::warn!("[Rules] Snapshot rewrite recovered: {}", path.display());
        return true;
    }
    // 重写仍无效：跳过（磁盘副本保持原样），绝不 panic
    log::warn!(
        "[Rules] Snapshot write failed, skip (embedded content stays authoritative): {}",
        path.display()
    );
    false
}

/// 无 panic 的原子文件写：tmp + rename 失败回退 try_write_file，全程不 panic；供快照复制使用（调用方自定失败策略）
pub(crate) fn write_file_no_panic(path: &Path, bytes: &[u8]) -> bool {
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rules".to_string());
    let tmp_path = path.with_file_name(format!("{}.tmp", file_name));
    let atomic_ok =
        std::fs::write(&tmp_path, bytes).is_ok() && std::fs::rename(&tmp_path, path).is_ok();
    if !atomic_ok {
        let _ = std::fs::remove_file(&tmp_path);
    }
    atomic_ok || crate::utils::try_write_file(path, bytes).is_ok()
}

/// meta.yaml 顶层行替换：只匹配缩进为 0 的 `键: 值` 行，保留键名大小写、分隔空白与行内注释；字段不存在返回 None，调用方放弃写入，绝不退化成整文件重排不走 serde 反序列化再序列化：
/// 整文件重排会吃掉用户注释，且 `MetaYamlFile` 带 deny_unknown_fields，漏一个新字段就会让下次 sync_meta_snapshot 判非法整体覆盖
/// 口径与 WebUI `contract/meta.ts::replaceTopLevelField` 一致，两边不要各写一套
pub(crate) fn replace_top_level_bool(content: &str, field: &str, value: bool) -> Option<String> {
    let want = field.to_ascii_lowercase();
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        // 只认顶层：有缩进的行（嵌套段）跳过
        if line.len() != trimmed.len() {
            continue;
        }
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim();
        if key.is_empty() || key.contains(char::is_whitespace) || key.contains('#') {
            continue;
        }
        if key.to_ascii_lowercase() != want {
            continue;
        }
        let rest = &trimmed[colon + 1..];
        let sep: String = rest.chars().take_while(|c| c.is_whitespace()).collect();
        let tail = &rest[sep.len()..];
        let comment = match tail.find(" #") {
            Some(i) => &tail[i..],
            None => "",
        };
        *line = format!("{key}:{sep}{value}{comment}");
        return Some(lines.join("\n"));
    }
    None
}

/// 一次写盘改掉 meta.yaml 的总开关（None = 该项不动；thread_bind 仅实验室使用）。返回 false 表示文件没被改到期望状态（字段缺失 / 读写失败），调用方放弃本次实验室套用
/// 多个开关必须一次写完：分两次写会触发两轮 config_watcher 热重载，中间态会被 apply_system_tweaks 真的执行一遍
pub(crate) fn rewrite_meta_toggles(
    path: &Path,
    fas: Option<bool>,
    scenemode: Option<bool>,
    thread_bind: Option<bool>,
) -> bool {
    if fas.is_none() && scenemode.is_none() && thread_bind.is_none() {
        return true;
    }
    let Ok(original) = std::fs::read_to_string(path) else {
        log::warn!("[Rhine] meta.yaml unreadable: {}", path.display());
        return false;
    };
    let mut text = original.clone();
    for (field, value) in [
        ("fas_enabled", fas),
        ("scenemode_enabled", scenemode),
        ("thread_bind", thread_bind),
    ] {
        let Some(v) = value else { continue };
        match replace_top_level_bool(&text, field, v) {
            Some(next) => text = next,
            None => {
                log::warn!("[Rhine] meta.yaml has no `{field}` field, give up writing");
                return false;
            }
        }
    }
    // 值本来就对：不写盘，省掉一次无谓的热重载
    if text == original {
        return true;
    }
    if !write_file_no_panic(path, text.as_bytes()) {
        log::warn!("[Rhine] meta.yaml write failed: {}", path.display());
        return false;
    }
    true
}
