//! common.rs: [events] [proc_snap] [paths] [soc_detect] [core_ranges] [soc_config] [governor] [special_tuned] [fas_whitelist]
//! [aff_blacklist] [embedded] [external_meta] [frontier]

use crate::monitor::config::RulesConfig;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
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
        detection_generation: u64,
    },
    /// 同模式前台包切换（模式不变、应用变化，ChiRi 侧 FAS fas→fas 热切换消费）
    PackageSwitch {
        package_name: String,
        pid: i32,
        detection_generation: u64,
    },
    /// 高频事件：eBPF 捕获到的底层渲染帧数据
    FrameUpdate {
        /// 纳秒级帧间隔
        frame_delta_ns: u64,
        /// 采集时绑定的游戏进程与控制会话
        source_pid: u32,
        source_generation: u64,
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

/// 触发 Chiri 专用调度的处理器型号片段，命中任一即启用；片段须能互相区分，新增机型在此追加即可。
/// 片段统一带厂牌前缀（sm/msm），兼作更安全的探测器：子串匹配下 sm8550 不会误命中无关硬件，原先的裸数字 8550 则会。
/// 2026-10-20 批量适配后本表扩到 39 条（覆盖 freqbench_tensor 全部可用机型，含 34 个新机型）；
/// msm8998 原曾从名单移除（「暂时下线」），本轮已随帕累托前沿一起恢复：其 freqbench 实测折算出了
/// capacity/freq_khz 与前沿表，不再等 dtsi。
// [soc_detect]
// 顺序有语义：片段是「子串匹配、首个命中即停」，故任何**更短**的片段必须排在它的超集之后
// （zuma ⊂ zumapro、sm6150 ⊂ sm6150ac、sm7150 ⊂ sm7150ab/ac、sm8150 ⊂ sm8150ac），
// 否则长名机型会解析到短名的配置目录，静默拿错 topology/capacity。新增机型同理：放进冲突组内更长的那个之前。
const CHIRI_SOC_HINTS: &[&str] = &[
    "sm8550",
    "sm8475",
    "sm8650",
    // SM8750（骁龙 8 Elite / OnePlus 13 PJZ110）：标准 SoC 名，命中 ro.soc.model /
    // soc0/machine 等来源；配置目录同名 config/sm8750/。
    "sm8750",
    "msm8998",
    "zumapro",
    "exynos5250",
    "exynos7880",
    "exynos7904",
    "exynos9611",
    "gs201",
    "lga",
    "msm8917",
    "msm8940",
    "msm8953",
    "msm8996",
    "mt6853t",
    "sdm439",
    "sdm630",
    "sdm632",
    "sdm636",
    "sdm660",
    "sdm670",
    "sdm710",
    "sdm712",
    "sdm845",
    "sm6115",
    "sm6125",
    "sm6150ac",
    "sm7125",
    "sm6150",
    "sm7150ab",
    "sm7150ac",
    "sm7150",
    "sm7250ab",
    "sm8150ac",
    "sm8250",
    "sm8150",
    "sm8350",
    "zuma",
    // "sun" = SM8750 的平台代号（真机 ro.board.platform，devimp main_ 头 `board=sun` 可证）。
    // 它是 sm8750 的**别名**，不是独立机型：配置目录由 [soc_hint_config_name] 统一映射到
    // config/sm8750/。保留它是因为并非所有来源都暴露 ro.soc.model（有些机型只有 board 名）。
    // 放末位 + 整词边界匹配（见 SOC_HINT_TOKEN_BOUNDARY）：短片段若用子串匹配会把
    // sunfish（Pixel 4a / sm7150）误判成本机；末位保证任何具体 SoC 名先命中。
    "sun",
];

/// 需要**整词边界**匹配的片段（其余仍为子串匹配）：只有短且易撞车的 board 代号才进本表
const SOC_HINT_TOKEN_BOUNDARY: &[&str] = &["sun"];

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
/// platform / ro.product.board / ro.hardware、/proc/cpuinfo 兜底结果统一转小写后做子串匹配，兼容 "SM8550" / "sm8550"（裸数字 8550 不再命中——片段带厂牌前缀才能避免误匹配无关硬件）
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
/// `SOC_HINT_TOKEN_BOUNDARY` 内的片段要求**整词命中**（haystack 按非字母数字切分后逐词相等），
/// 其余片段沿用子串匹配（`qcom,sm8550-mtp` 这类带前缀的标识必须靠子串才命中）
fn hint_matches(hint: &str) -> bool {
    let hl = hint.to_lowercase();
    if hl.len() < 3 {
        return false;
    }
    let hay = soc_hint_haystack();
    if SOC_HINT_TOKEN_BOUNDARY.contains(&hl.as_str()) {
        return hay
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|tok| tok == hl);
    }
    hay.contains(&hl)
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
/// 命中片段 → 配置目录名（`config/<name>/`）：**别名在此归一**
///
/// 绝大多数片段与目录同名；只有平台代号需要映射（`sun` → `sm8750`），
/// 这样目录仍按 SoC 型号命名（与既有 39 个目录同构），而 board 名也能命中。
/// 新增别名 = 在本函数加一条 match 分支，不要复制配置目录。
pub(crate) fn soc_hint_config_name(hint: &str) -> &str {
    match hint {
        "sun" => "sm8750",
        other => other,
    }
}

/// 命中片段的**配置目录名**（已归一别名）；未命中 → None
pub(crate) fn matched_soc_config_name() -> Option<&'static str> {
    matched_soc_hint().map(soc_hint_config_name)
}

fn matched_soc_config_dir() -> Option<PathBuf> {
    matched_soc_config_name().map(|name| get_module_root().join("config").join(name))
}

/// 处理器核心组区间（little/big/prime 的 CPU ID，左闭右开）：akmode 忙/闲统计、CLG 触摸升频判定大核簇使用各 SoC 簇布局不同，按命中片段区分，未命中回退 sm8550 布局兜底
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
        Some("sm8475") => CoreGroupRanges {
            little: 0..4,
            big: 4..7,
            prime: 7..8,
        },
        Some("msm8998") => CoreGroupRanges {
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

/// cpu id 落在哪个核心组，都不在返回 None（msm8998 空 prime 区间不会命中 prime）；区间以 chiri_core_ranges 为准
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
/// serde 无 deny_unknown_fields，未知键被静默忽略，键名拼错只会整段变 None 走兜底
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
    /// 相对能效模型段（第二代帕累托前沿 CLG-EEM 的数据源：sm8550 / zumapro / sm8750 等 38 机型已具备；
    /// 无本段的 SoC（sm8475 / sm8650）→ 天然 None → 走原比例路径）
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

// [frontier] 相对能效模型策略段（第二代帕累托前沿 CLG-EEM），由 scripts/frontier_policy.py --soc <机型> 生成到对应 soc.yaml
// 消费方：① CLG 的 EEM 甜点封顶（`energy_model::curve`）；② 功耗遥测的逐档簇功耗 `per_cluster.<簇>.power_w`
// （上一代 FDP 的边际成本决策链已于 2026-10-23 移除，`per_cluster` 的 freq_khz 仅供遥测与对账留痕）
// 字段名与生成脚本逐字对齐，解析失败由 [frontier_lenient] 兜底为 None

/// 前沿策略段，段缺失或 `enabled` 非 true → `soc_frontier_policy()` 返回 None → 原比例路径
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FrontierPolicy {
    /// 运行期开关（在块内）：false / 缺失 = 不启用
    #[serde(default)]
    pub enabled: bool,
    /// 输入内容哈希（生成脚本产出，运行期仅用于日志标识）
    #[serde(default)]
    pub model_fingerprint: Option<String>,
    /// 各簇**单核**能效曲线（频点 + 单核动态功耗 + capacity）：相对能效模型的唯一数据源。
    /// 这是本段运行期**唯一**被消费的字段——`demand_buckets` / `modes` / `anticipate` 等
    /// 预计算桶表已随重构移除（决策改由 `energy_model` 在相对向量空间直接算）。
    #[serde(default)]
    pub per_cluster: Option<FrontierPerCluster>,
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
    /// 逐档**簇**功耗 W（文档 §3 簇级表，只含动态项、不含漏电）。
    /// 仅供分析工具与溯源；运行期**不再直接消费**——`energy_model` 只用它的**形状**，
    /// 且必须先除以簇核数折算成单核口径（`single_core_power_w` 缺失时的兜底路径）。
    #[serde(default)]
    pub power_w: Vec<f32>,
    /// 逐档**单核**功耗 W（已按单核口径折算、已扣首档静态项）：相对能效模型的唯一功耗输入。
    /// 长度与 `freq_khz` 对齐；缺省为空 → `energy_model` 回退到 `power_w ÷ 簇核数`。
    /// 与 `power_w` 的口径差异是历史事故源（单核/整簇混用），故单列一个字段显式区分。
    #[serde(default)]
    pub single_core_power_w: Vec<f32>,
}

/// [frontier_lenient] 前沿段解析失败不得拖垮整份 soc.yaml：单字段兜底为 None + warn 一次
/// （topology/capacity/freq 兜底是更贵的资产，误配必须自愈）
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

/// 当前 SoC 的相对能效模型策略：仅当 soc.yaml 存在 `frontier_policy` 段且 `enabled == true` 时返回 Some；
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
            let Some(name) = matched_soc_config_name() else {
                return None;
            };
            let Some(text) = embedded_config_file(&format!("{name}/soc.yaml")) else {
                return None;
            };
            match serde_yaml::from_str::<SocConfig>(text) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::warn!("soc-config-parse-failed ({name}): {e}");
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

/// 各 policy 选定的调速器：启动时 [`select_cpu_governors`] 批量算定，此后只读
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

/// 调度启动（及每次热重载）时调用：批量算定当前可见的 policy 并回填表，返回全表副本（供日志）
/// 已在表内则不重复读 sysfs，所有「默认调速器」写入点（CLG / 特调 / FastLock /
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

/// 该 policy 应写入的默认调速器，表里没有的（批量算定时该簇离线、或选型被 DOWN 跳过）
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

/// 当前是否亮屏（屏幕事件同步，缺省 true）：状态块据此填 screen_on 列、择模式列与决定功率采样口径（平均档只采亮屏）
/// 屏幕事件的处理在调度循环而不在这里，必须经原子量跨线程同步
static SCREEN_ON: AtomicBool = AtomicBool::new(true);

/// 同步亮屏状态（ScreenStateChange 事件与启动时各同步一次）
pub fn set_screen_on(on: bool) {
    SCREEN_ON.store(on, Ordering::Release);
}

/// 当前是否亮屏
pub fn screen_on() -> bool {
    SCREEN_ON.load(Ordering::Acquire)
}

/// 息屏判定值（screen_off_value`，缺省 1）：debug.tracing.screen_state 等于该值视为息屏，其余数字视为亮屏，Config::load 同步原子量，
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
        // 先摘 "re:" 前缀再切分，直接整行 splitn(3,':') 会让包名变字面量 "re" 永不命中
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

/// 按配置名返回嵌入 FAS 配置文本（构建期自动收录 normal/fas/*.yaml），新增 FAS 游戏：fas.yaml 白名单加一行 + 新建 normal/fas/<配置名>.yaml，无需改 .rs
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

/// 进程 cmdline（或线程 comm）是否命中亲和黑名单，另含两条内置兜底（不经文件、不可关闭）：空 cmdline = 内核线程 → 黑名单；'/' 开头 = native 二进制路径 → 黑名单
pub fn is_affinity_blacklisted(cmdline: &str) -> bool {
    if cmdline.is_empty() || cmdline.starts_with('/') {
        return true;
    }
    affinity_blacklist_entries()
        .iter()
        .any(|e| e.matches(cmdline))
}

// 编译期嵌入 module/config 整目录（防篡改），原 config.yaml 拆成 meta.yaml（用户可改）+ feature.yaml（不可改调优段）
// 二者只存于二进制不落盘，目录整体嵌入，新增 yaml 无需改 .rs，缺文件由 build.rs 断言编译失败
// ***-example.yaml 参考文件不放本目录，一律放 mdocs/

// [embedded]
static CONFIG_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/module/config");

/// 按相对 module/config 的路径取嵌入文本（如 "sm8550/meta.yaml"），缺失返回 None；必需文件存在性由 build.rs 编译期断言，运行时不会静默空转
pub(crate) fn embedded_config_file(rel: &str) -> Option<&'static str> {
    CONFIG_DIR.get_file(rel).and_then(|f| f.contents_utf8())
}

/// 嵌入的 meta.yaml（用户可修改字段默认值）：按命中处理器取 {soc}/meta.yaml，未命中取 meta.yaml
pub fn embedded_meta_str() -> &'static str {
    if let Some(soc) = matched_soc_config_name() {
        match embedded_config_file(&format!("{soc}/meta.yaml")) {
            Some(text) => return text,
            // [silent-fallback] 命中 SoC 却没有它的 meta.yaml：静默回退根文件后，该机型会拿到
            // 通用默认（如 power_max_w / 日志级别基准），与「SoC 专属默认正常生效」完全同形
            // 编译期嵌入，运行时只可能打包缺文件 → 一次性告警（本函数仅启动期少量调用）
            None => log::warn!(
                "[Meta] SoC '{}' 命中但无专属 meta.yaml，回退根默认（该机型默认值可能不符）",
                soc
            ),
        }
    }
    embedded_config_file("meta.yaml").unwrap_or_default()
}

/// 嵌入的 feature.yaml：以根文件为基底，按字段深合并命中处理器的 {soc}/feature.yaml，map 递归合并、标量与序列整体替换
/// 未命中处理器时即根文件本身，SoC 未写的整段也会从根继承（如 sm8550/msm8998 无 vector 段）
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
        let Some(soc_text) = matched_soc_config_name()
            .and_then(|soc| embedded_config_file(&format!("{soc}/feature.yaml")))
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
/// [activity_params] 解析成功后统一钳制参数覆盖：`activity_params` 是唯一会被调度层直接消费的规则数值，
/// 越界值（如 headroom=9、down_hold_ms=99999）若不在此收口，会一路带进写频决策；热重载同走本函数，故钳制只有一处
pub fn embedded_rules() -> crate::monitor::config::RulesConfig {
    match serde_yaml::from_str::<crate::monitor::config::RulesConfig>(embedded_rules_str()) {
        Ok(mut r) => {
            for ov in r.activity_params.values_mut() {
                ov.normalize();
            }
            // 空覆盖（字段全被钳制丢弃 / 只写了 `{}`）与「未配置」等价，清掉避免快照层误判为有效覆盖
            r.activity_params.retain(|_, ov| !ov.is_empty());
            r
        }
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

/// 磁盘 meta.yaml 的严格结构：字段全部可选，类型不符或未知键即整文件判非法，由 sync_meta_snapshot 逐字段恢复（仅 YAML 读不出时整体重建）
/// 新增字段五处必须同步：本结构、ExternalMetaOverrides、chiri::config::Meta 与 Config::load、六个 meta.yaml 模板、WebUI META_FIELDS
// [external_meta]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaYamlFile {
    name: Option<String>,
    author: Option<String>,
    language: Option<String>,
    loglevel: Option<String>,
    dev_record: Option<bool>,
    /// aff `@S` 每秒快照帧的 top-N 进程数（手改字段，WebUI 无开关），见 ExternalMetaOverrides 同名字段
    devimp_top_n: Option<usize>,
    /// aff `@S` 快照帧的采样间隔秒数（手改字段，WebUI 无开关），见 ExternalMetaOverrides 同名字段
    devimp_aff_secs: Option<usize>,
    fas_enabled: Option<bool>,
    scenemode_enabled: Option<bool>,
    /// 线程摆放总开关（affinity + core_ctl），见 ExternalMetaOverrides
    thread_bind: Option<bool>,
    /// 功耗口径开关（PowerAVG.chr），见 ExternalMetaOverrides（缺省 false）
    power_avg: Option<bool>,
    /// 常驻状态通知开关，见 ExternalMetaOverrides（缺省 true）
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
    /// 「不改」开关（模板不写），见 ExternalMetaOverrides
    nofix: Option<bool>,
    /// 耗电读数满量程 W（缺省 12），见 ExternalMetaOverrides
    power_max_w: Option<f32>,
    /// 息屏判定值（0/1，缺省 1），见 ExternalMetaOverrides
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
    /// aff @S 快照帧采样间隔秒数（缺省 1）：每 N 秒采一帧，息屏再乘 `AFF_SNAP_OFFSCREEN_FACTOR`
    /// 调大即线性减少 stat 采样与写盘——守护进程自身的常驻开销主要就花在前台线程逐帧下钻上
    pub devimp_aff_secs: Option<usize>,
    pub fas_enabled: Option<bool>,
    pub scenemode_enabled: Option<bool>,
    /// 线程摆放总闸（`thread_bind`）：实验室 frozen 专用机制——用户侧开关已移除，仅 frozen 模式写 false 交还线程亲和/绑核与 core_ctl， 与机型内嵌 feature
    /// yaml 的两个子开关取「与」，见 chiri/config.rs::Config::load
    pub thread_bind: Option<bool>,
    /// 功耗口径开关（PowerAVG.chr）：false（默认）= 参考值（旧值先乘 10 再按 10:1 加权递推，偏历史，含息屏样本）；true = 累计平均（等权全史，仅亮屏放电样本）两者都只在放电时取样；
    /// 仅 ChiRi 1s 状态采样消费；写侧走单次读-改-写顶层行替换，见 WebUI contract/meta.ts
    pub power_avg: Option<bool>,
    /// 【已废弃/遗留】常驻状态通知开关（notify）：原属「每 5s 写常驻调度通知」子系统，该子系统已随
    /// `notify.rs` 重构删除，daemon 内**无读取点**；仅存字段供 meta.yaml 兼容与 WebUI 读写契约
    /// 5 个机型 meta.yaml 均写 `notify: false`，将来要复用须先移除那些行
    pub notify: Option<bool>,
    /// OPlus 私有电压/电流节点（`oplus_chg`，默认 false）：true = 优先读 `/sys/class/oplus_chg/battery/bcc_parms`（下标 6 电芯电压0、8 电流、11 电芯电压1，
    /// mV/mA），读不到回退标准 power_supply 节点，仅 OPlus 机型有意义
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
    /// Config::load 合并后 set_screen_off_value 同步原子量，customize.sh [screen-detect] 安装期按属性值翻转该字段
    pub screen_off_value: Option<u32>,
}

/// 读磁盘 meta.yaml（先经 sync_meta_snapshot 校验/纠正）：文件缺失或仍非法返回 None，调用方沿用嵌入默认——绝不 panic，不让坏文件拖垮配置加载
pub fn read_external_meta(path: &Path) -> Option<ExternalMetaOverrides> {
    // [silent-fallback] 读不到与「读到了但解析/校验判非法」都返回 None、调用方无从区分
    // 前者是首装常态，后者是结构层仍非法（自愈已尽力逐字段修）；debug 下按原因分开留痕
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!("[Meta] {} unreadable: {}", path.display(), e);
            }
            return None;
        }
    };
    let parsed = parse_disk_meta(&text);
    if parsed.is_none() && log::log_enabled!(log::Level::Debug) {
        log::debug!(
            "[Meta] {} rejected (invalid or unknown keys), using embedded defaults",
            path.display()
        );
    }
    parsed
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

/// 整体校验磁盘 meta.yaml：结构严格（无未知键、类型正确）+ 取值白名单；任一字段异常返回 None——调用方先逐字段自愈（heal_disk_meta），自愈失败才整体重建
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
        devimp_aff_secs: f.devimp_aff_secs,
        fas_enabled: f.fas_enabled,
        scenemode_enabled: f.scenemode_enabled,
        thread_bind: f.thread_bind,
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

/// 逐字段自愈结果：Unlocalizable = 连 YAML 结构都读不出（语法错）或结构异常，只能整份重建；
/// Clean = 全部合法（不写盘）；Repaired = 已就地修好几行，其余内容逐字保留
/// 自愈追加在文件末尾的警告注释（固定文案，用于「削掉上一轮同名行再补本轮」，避免多轮自愈累积多行）
const META_HEAL_NOTE: &str =
    "# 上一次修改存在非法字段，仅该字段已恢复为默认值；详情见 logs/daemon.log";
/// 整份重建分支的警告注释：与逐字段自愈是两条不同路径，文案刻意不同（前者只动一行、后者整份重置），
/// 但幂等削行必须同时认这两条——否则「先整份重建、后逐字段自愈」会在文件末尾留下两行且每轮再叠
const META_HEAL_NOTE_WHOLE: &str =
    "# 上一次修改存在非法字段，已整体恢复为默认值；详情见 logs/daemon.log";

/// 削掉上一轮自愈留在末尾的警告注释（两条文案都认），返回不含它的正文
fn strip_heal_note(body: &str) -> &str {
    let body = body.trim_end();
    for note in [META_HEAL_NOTE, META_HEAL_NOTE_WHOLE] {
        if let Some(stripped) = body.strip_suffix(note) {
            return stripped.trim_end();
        }
    }
    body
}

enum MetaHeal {
    Unlocalizable,
    Clean,
    Repaired(String),
}

/// 值域校验（结构层面之外的白名单，与 parse_disk_meta 同口径）：name/author 非空、loglevel 六档、language en/zh；
/// 类型不匹配与未知键已由结构层拦截，此处只判「类型对但取值非法」的那几项
fn meta_value_domain_ok(key: &str, v: &serde_yaml::Value) -> bool {
    match key.to_ascii_lowercase().as_str() {
        "name" | "author" => v.as_str().is_some_and(|s| !s.trim().is_empty()),
        "loglevel" => v.as_str().is_some_and(|s| sanitize_loglevel(s).is_some()),
        "language" => v.as_str().is_some_and(|s| sanitize_language(s).is_some()),
        _ => true,
    }
}

/// meta.yaml 逐字段自愈：只把非法字段所在的那一行恢复为内嵌默认（模板未带的键则整行删除 = 沿用代码默认），
/// 合法字段、用户注释与其余内容逐字保留。返回 Unlocalizable 时调用方才退回整份重建。
/// 结构判定复用 MetaYamlFile 本体（未知键/类型错即报错），故字段名与类型只有那一处权威
fn heal_disk_meta(text: &str) -> MetaHeal {
    let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(text)
    else {
        return MetaHeal::Unlocalizable;
    };
    // 模板本身读不出时不做逐字段推断（否则每个非法键都会退化成「删除」，可能连合法字段一起误伤）——保守走整份重建
    let Ok(template) = serde_yaml::from_str::<serde_yaml::Value>(embedded_meta_str()) else {
        return MetaHeal::Unlocalizable;
    };
    let Some(template_map) = template.as_mapping() else {
        return MetaHeal::Unlocalizable;
    };

    let mut out = text.to_string();
    let mut changed = false;
    for (k, v) in &map {
        // 顶层键不是字符串（YAML 允许 `1: x` 这类）：无法按行定位，保守走整份重建，别让它落到「全部合法」而静默不修
        let Some(key) = k.as_str() else {
            return MetaHeal::Unlocalizable;
        };
        // 本段所有字段都是标量；出现嵌套值即结构异常，交给整份重建，避免只删首行留下悬空块
        if !(v.is_null() || v.is_bool() || v.is_string() || v.is_number()) {
            return MetaHeal::Unlocalizable;
        }
        // 结构层：单键回灌 MetaYamlFile——未知键报 unknown field、类型错报 invalid type，两者都算非法
        let structurally_ok = {
            let mut one = serde_yaml::Mapping::new();
            one.insert(k.clone(), v.clone());
            serde_yaml::from_value::<MetaYamlFile>(serde_yaml::Value::Mapping(one)).is_ok()
        };
        if structurally_ok && meta_value_domain_ok(key, v) {
            continue;
        }
        match template_map.get(k) {
            // 模板带该键：就地回填模板值（保留键名大小写、分隔空白与行内注释）
            Some(default) => {
                let rendered = serde_yaml::to_string(default).unwrap_or_default();
                if let Some(next) = replace_top_level_value(&out, key, rendered.trim_end()) {
                    out = next;
                    changed = true;
                } else {
                    return MetaHeal::Unlocalizable;
                }
            }
            // 模板不带（devimp_top_n / devimp_aff_secs / unit_divisor / nofix 这类可选键，以及已废弃键）：整行删除 = 沿用代码默认
            None => match remove_top_level_key(&out, key) {
                Some(next) => {
                    out = next;
                    changed = true;
                }
                None => return MetaHeal::Unlocalizable,
            },
        }
    }
    if !changed {
        return MetaHeal::Clean;
    }
    // 自愈后必须真的能通过严格校验，否则宁可整份重建（不把半修好的文件留给加载侧）
    if parse_disk_meta(&out).is_some() {
        MetaHeal::Repaired(out)
    } else {
        MetaHeal::Unlocalizable
    }
}

/// meta.yaml 快照自愈：main.rs 启动时与 chiri config_watcher 触发热重载前调用文件合法 → 跳过写入（返回 false，防 config_watcher 事件成环）；
/// 文件缺失/不可读 → 嵌入原文原子重建（不留警告注释）；字段非法 → **只恢复非法字段所在行**，合法字段与用户注释保留 + 末尾追加警告注释 + warn 日志；
/// 仅当连 YAML 结构都读不出（语法错/嵌套异常值/修完仍不合法）才退回整份重建（完全丢失重建与「修改出错纠正不同，前者不留言）
pub fn sync_meta_snapshot(meta_path: &Path) -> bool {
    let embedded = embedded_meta_str();
    let corrected = match std::fs::read_to_string(meta_path) {
        Ok(text) => {
            if parse_disk_meta(&text).is_some() {
                return false;
            }
            match heal_disk_meta(&text) {
                MetaHeal::Clean => return false,
                MetaHeal::Repaired(fixed) => {
                    log::warn!(
                        "[Meta] meta.yaml has invalid fields, restored those lines to defaults (rest kept): {}",
                        meta_path.display()
                    );
                    // 幂等追加：先削掉上一轮留下的注释行（逐字段与整份重建两种文案都削），再补本轮
                    let body = strip_heal_note(fixed.trim_end());
                    format!("{body}\n{META_HEAL_NOTE}\n")
                }
                MetaHeal::Unlocalizable => {
                    log::warn!(
                        "[Meta] meta.yaml unreadable as YAML or still invalid after per-field repair, reset to embedded defaults: {}",
                        meta_path.display()
                    );
                    format!("{embedded}\n{META_HEAL_NOTE_WHOLE}\n")
                }
            }
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
        "[Rules] Snapshot write failed, skip, embedded content stays authoritative: {}",
        path.display()
    );
    false
}

/// 无 panic 的原子文件写：tmp + rename 失败回退直写（utils::write_to_file），全程不 panic；供快照复制使用（调用方自定失败策略）
pub(crate) fn write_file_no_panic(path: &Path, bytes: &[u8]) -> bool {
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rules".to_string());
    let tmp_path = path.with_file_name(format!("{}.tmp", file_name));
    let atomic_ok =
        std::fs::write(&tmp_path, bytes).is_ok() && std::fs::rename(&tmp_path, path).is_ok();
    if atomic_ok {
        return true;
    }
    let _ = std::fs::remove_file(&tmp_path);
    // [silent-fallback] 原子写失败后回退直写，两条路都失败才返回 false，但调用方普遍丢弃返回值
    // 这里在最终失败点兜底留痕（debug 级，成功路径绝不能刷屏）
    // [fix-1017-c2] 判定成败必须用真实返回 Result 的原语，try_write_file 恒 Ok(())，用它会让 false 不可达
    if crate::utils::write_to_file(path, bytes).is_ok() {
        return true;
    }
    if log::log_enabled!(log::Level::Debug) {
        log::debug!(
            "[Common] file write failed (atomic + fallback): {}",
            path.display()
        );
    }
    false
}

/// meta.yaml 顶层行的定位（键名大小写不敏感）：返回 (行下标, 冒号位置, 键名原文, 冒号后的分隔空白, 值+行内注释)；
/// 只认缩进为 0 的 `键: 值` 行，跳过空行/注释/序列项与所有嵌套行。下面两个写入口共用它，避免定位口径各写一套
fn find_top_level_line(
    content: &str,
    field: &str,
) -> Option<(usize, usize, String, String, String)> {
    let want = field.to_ascii_lowercase();
    for (i, line) in content.split('\n').enumerate() {
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
        let tail = rest[sep.len()..].to_string();
        return Some((i, colon, key.to_string(), sep, tail));
    }
    None
}

/// 顶层行替换（值可为任意已渲染的 YAML 标量）：保留键名原文、分隔空白与行内注释；字段不存在返回 None，绝不退化成整文件重排
/// 与 `replace_top_level_bool` 同一定位口径，语义见其注释；供逐字段自愈回填内嵌默认值
pub(crate) fn replace_top_level_value(content: &str, field: &str, value: &str) -> Option<String> {
    let (idx, _, key, sep, tail) = find_top_level_line(content, field)?;
    let comment = match tail.find(" #") {
        Some(i) => &tail[i..],
        None => "",
    };
    let mut out = String::with_capacity(content.len() + value.len());
    for (i, line) in content.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if i == idx {
            out.push_str(&format!("{key}:{sep}{value}{comment}"));
        } else {
            out.push_str(line);
        }
    }
    Some(out)
}

/// 顶层行删除（模板不带的可选键：删掉即「沿用代码默认」，比保留脏行更干净）；字段不存在返回 None，绝不整文件重排
fn remove_top_level_key(content: &str, field: &str) -> Option<String> {
    let (idx, ..) = find_top_level_line(content, field)?;
    let kept: Vec<&str> = content
        .split('\n')
        .enumerate()
        .filter_map(|(i, l)| (i != idx).then_some(l))
        .collect();
    Some(kept.join("\n"))
}

/// meta.yaml 顶层行替换：只匹配缩进为 0 的 `键: 值` 行，保留键名大小写、分隔空白与行内注释；字段不存在返回 None，调用方放弃写入，绝不退化成整文件重排不走 serde 反序列化再序列化：
/// 整文件重排会吃掉用户注释，且 `MetaYamlFile` 带 deny_unknown_fields，漏一个新字段就会让下次 sync_meta_snapshot 把该行判非法并恢复默认
/// 口径与 WebUI `contract/meta.ts::replaceTopLevelField` 一致，两边不要各写一套
pub(crate) fn replace_top_level_bool(content: &str, field: &str, value: bool) -> Option<String> {
    replace_top_level_value(content, field, if value { "true" } else { "false" })
}

/// 一次写盘改掉 meta.yaml 的总开关（None = 该项不动；thread_bind 仅实验室使用），返回 false 表示文件没被改到期望状态（字段缺失 / 读写失败），调用方放弃本次实验室套用
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

// [meta_heal_tests] 逐字段自愈契约：只修非法行，合法字段、用户注释与未知键之外的内容逐字保留
#[cfg(test)]
mod meta_heal_tests {
    use super::*;

    fn healed(t: &str) -> Option<String> {
        match heal_disk_meta(t) {
            MetaHeal::Repaired(s) => Some(s),
            _ => None,
        }
    }

    // 模板自带的键（loglevel）非法：只回填该行，其余行含注释逐字保留
    #[test]
    fn invalid_template_key_only_its_line_changes() {
        let src = "# 我的注释\nname: \"mine\"\nloglevel: NOPE\nauthor: \"me\"\npower_max_w: 12\n";
        let out = healed(src).expect("should repair loglevel");
        assert!(out.contains("# 我的注释"), "comment lost: {out}");
        assert!(out.contains("name: \"mine\""), "valid field lost: {out}");
        assert!(out.contains("author: \"me\""), "valid field lost: {out}");
        assert!(out.contains("power_max_w: 12"), "valid field lost: {out}");
        assert!(!out.contains("NOPE"), "bad value kept: {out}");
        assert!(parse_disk_meta(&out).is_some(), "still invalid: {out}");
    }

    // 行内注释与键名原文/空白要保留
    #[test]
    fn inline_comment_and_key_spacing_preserved() {
        let src = "loglevel:   NOPE   # 这里是注释\n";
        let out = healed(src).unwrap();
        assert!(out.contains("# 这里是注释"), "inline comment lost: {out}");
        assert!(out.contains("loglevel:"), "key lost: {out}");
        assert!(!out.contains("NOPE"), "bad value kept: {out}");
    }

    // 未知键（升级残留，如已删的 powerbase_enabled）：模板不带 → 整行删除，其余保留
    #[test]
    fn unknown_key_line_removed_others_intact() {
        let src = "name: \"mine\"\npowerbase_enabled: false\nloglevel: \"INFO\"\n";
        let out = healed(src).unwrap();
        assert!(
            !out.contains("powerbase_enabled"),
            "unknown key kept: {out}"
        );
        assert!(out.contains("name: \"mine\""), "valid field lost: {out}");
        assert!(
            out.contains("loglevel: \"INFO\""),
            "valid field lost: {out}"
        );
        assert!(parse_disk_meta(&out).is_some(), "still invalid: {out}");
    }

    // 多个非法字段一次修完
    #[test]
    fn multiple_bad_fields_all_repaired() {
        let src = "loglevel: NOPE\nlanguage: ja\nname: \"keep\"\n";
        let out = healed(src).unwrap();
        assert!(
            !out.contains("NOPE") && !out.contains("ja"),
            "bad kept: {out}"
        );
        assert!(out.contains("name: \"keep\""), "valid field lost: {out}");
        assert!(parse_disk_meta(&out).is_some(), "still invalid: {out}");
    }

    // 全合法：不写盘、不改内容
    #[test]
    fn all_valid_is_clean() {
        let src = "name: \"mine\"\nloglevel: \"INFO\"\nlanguage: \"zh\"\n";
        assert!(matches!(heal_disk_meta(src), MetaHeal::Clean));
    }

    // 语法错：无法逐行定位 → 交回整份重建
    #[test]
    fn syntax_error_is_unlocalizable() {
        let src = "name: x\n\tdev_record: true\n";
        assert!(matches!(heal_disk_meta(src), MetaHeal::Unlocalizable));
    }

    // 嵌套值（结构异常）→ 交回整份重建，不留下悬空块
    #[test]
    fn nested_value_is_unlocalizable() {
        let src = "name: \"x\"\nloglevel:\n  a: 1\n";
        assert!(matches!(heal_disk_meta(src), MetaHeal::Unlocalizable));
    }

    // 顶层键不是字符串（如 `1: x`）：无法按行定位，必须走整份重建，不能静默当作「全部合法」
    #[test]
    fn non_string_top_key_is_unlocalizable() {
        let src = "name: \"mine\"\n1: x\n";
        assert!(matches!(heal_disk_meta(src), MetaHeal::Unlocalizable));
    }

    // 块标量（`loglevel: |` 带缩进体）被判定非法时：就地改行会留下悬空缩进体，
    // 收尾的 parse_disk_meta 不变量必须拦住它并改走整份重建
    #[test]
    fn block_scalar_body_never_left_dangling() {
        let src = "name: \"mine\"\nloglevel: |\n  NOPE\n";
        match heal_disk_meta(src) {
            // 要么成功修好且结果真的合法，要么明确交回整份重建；不许产出半修好的文件
            MetaHeal::Repaired(out) => {
                assert!(parse_disk_meta(&out).is_some(), "half-repaired: {out}")
            }
            MetaHeal::Unlocalizable => {}
            MetaHeal::Clean => panic!("must not report clean for an invalid loglevel"),
        }
    }

    // 类型错（布尔写成字符串）也要修掉，且不牵连其它行
    #[test]
    fn type_mismatch_repaired_not_whole_file() {
        let src = "# keep me\nname: \"mine\"\ndev_record: \"true\"\npower_avg: false\n";
        let out = healed(src).unwrap();
        assert!(out.contains("# keep me"), "comment lost: {out}");
        assert!(out.contains("name: \"mine\""), "valid lost: {out}");
        assert!(out.contains("power_avg: false"), "valid lost: {out}");
        assert!(!out.contains("\"true\""), "bad value kept: {out}");
        assert!(parse_disk_meta(&out).is_some(), "still invalid: {out}");
    }

    // 端到端（仍属 meta_heal_tests）：走真实文件路径的 sync_meta_snapshot，验证落盘结果
    // 每个用例只保留自己那一臂——原先写成 match "字面量" 会留下 2/3 恒不可达分支

    #[test]
    fn e2e_stale_powerbase_key() {
        let dir = std::env::temp_dir().join("chiri_heal_stale_powerbase_key");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("meta.yaml");
        std::fs::write(
            &f,
            "# 用户注释\nname: \"mine\"\nloglevel: \"DEBUG\"\npowerbase_enabled: false\npower_avg: false\n",
        )
        .unwrap();
        sync_meta_snapshot(&f);
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(!after.contains("powerbase_enabled"), "stale key kept");
        assert!(after.contains("name: \"mine\""), "user name kept");
        assert!(after.contains("loglevel: \"DEBUG\""), "user loglevel kept");
        assert!(after.contains("power_avg: false"), "user switch kept");
        assert!(after.contains("# 用户注释"), "comment kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn e2e_bad_loglevel_keeps_rest() {
        let dir = std::env::temp_dir().join("chiri_heal_bad_loglevel_keeps_rest");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("meta.yaml");
        std::fs::write(
            &f,
            "# 用户注释\nname: \"mine\"\nloglevel: NOPE\npower_avg: false\n",
        )
        .unwrap();
        sync_meta_snapshot(&f);
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(!after.contains("NOPE"), "bad kept");
        assert!(after.contains("name: \"mine\""), "user name lost");
        assert!(after.contains("power_avg: false"), "user switch lost");
        assert!(after.contains("# 用户注释"), "comment lost");
        assert!(after.contains("loglevel:"), "key lost");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn e2e_syntax_error_whole_rebuild() {
        let dir = std::env::temp_dir().join("chiri_heal_syntax_error_whole_rebuild");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("meta.yaml");
        std::fs::write(&f, "name: x\n\tdev_record: true\n").unwrap();
        sync_meta_snapshot(&f);
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(
            after.contains("default_config"),
            "should be embedded rebuild"
        );
        assert!(!after.contains("\t"), "tab still present");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 幂等：连续两轮自愈后末尾只能留一行警告注释（整份重建的文案也要能被削掉）
    #[test]
    fn e2e_heal_note_not_duplicated() {
        let dir = std::env::temp_dir().join("chiri_heal_note_idempotent");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("meta.yaml");
        std::fs::write(&f, "name: x\n\tdev_record: true\n").unwrap();
        sync_meta_snapshot(&f);
        std::fs::write(
            &f,
            "# 用户注释\nname: \"mine\"\nloglevel: NOPE\npower_avg: false\n",
        )
        .unwrap();
        sync_meta_snapshot(&f);
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(
            after.matches(META_HEAL_NOTE).count(),
            1,
            "heal note duplicated: {after}"
        );
        assert!(
            !after.contains(META_HEAL_NOTE_WHOLE),
            "stale whole-rebuild note kept: {after}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// [frontier] 机型 frontier_policy 段的结构回归：`deserialize_frontier_lenient` 会把解析错误
/// 静默降级成 None（顶层 soc.yaml 解析失败同样静默回退整份配置），故各机型数据必须显式断言
/// 「解析出来且可用」，否则误配不会在任何地方报错、EEM 只是悄悄不生效。
#[cfg(test)]
mod frontier_config_tests {
    use super::*;

    /// 对单个机型的 soc.yaml 校验 frontier 段的可用性；返回各簇甜点 kHz。
    fn check(soc: &str, text: &str) -> Vec<(&'static str, u32)> {
        let c: SocConfig = serde_yaml::from_str(text)
            .unwrap_or_else(|e| panic!("{soc}: 整份 soc.yaml 解析失败: {e}"));
        let p = c
            .frontier_policy
            .unwrap_or_else(|| panic!("{soc}: frontier_policy 段缺失或解析失败"));
        assert!(p.enabled, "{soc}: enabled 必须为 true");
        assert!(
            p.model_fingerprint
                .as_deref()
                .is_some_and(|s| !s.is_empty()),
            "{soc}: 缺 model_fingerprint"
        );
        let pc = p
            .per_cluster
            .unwrap_or_else(|| panic!("{soc}: per_cluster 缺失"));
        let mut out = Vec::new();
        for (name, b) in [
            ("little", &pc.little),
            ("big", &pc.big),
            ("prime", &pc.prime),
        ] {
            // 空簇是**合法**配置（如 8750 没有小核簇，little 空区间 + 空表）：跳过而非报错。
            // 非空即必须自洽——有频点就得有等长功耗，否则归一化会静默产出退化向量。
            if b.freq_khz.is_empty() {
                assert!(
                    b.single_core_power_w.is_empty(),
                    "{soc}/{name}: freq_khz 空但 single_core_power_w 非空"
                );
                continue;
            }
            assert_eq!(
                b.freq_khz.len(),
                b.single_core_power_w.len(),
                "{soc}/{name}: freq_khz 与 single_core_power_w 长度不齐"
            );
            assert!(
                b.freq_khz.windows(2).all(|w| w[0] < w[1]),
                "{soc}/{name}: freq_khz 非严格升序"
            );
            assert!(
                b.single_core_power_w.windows(2).all(|w| w[0] <= w[1]),
                "{soc}/{name}: single_core_power_w 非单调非减（相对向量的跨度项会失真）"
            );
            // 复现 build()：cap_rel = f/f_max，pow_rel 先扣首档再按跨度归一
            let fmax = *b.freq_khz.last().unwrap() as f32;
            let cap: Vec<f32> = b.freq_khz.iter().map(|&f| f as f32 / fmax).collect();
            let p0 = b.single_core_power_w[0];
            let span = (b.single_core_power_w.last().unwrap() - p0).max(f32::EPSILON);
            let pr: Vec<f32> = b
                .single_core_power_w
                .iter()
                .map(|&x| (x - p0).max(0.0) / span)
                .collect();
            let sweet = crate::chiri::energy_model::sweet_index(&cap, &pr);
            // 甜点不能落在首档（= 判据一次都没满足，EEM 退化成锁最低频）也不能在顶档
            assert!(
                sweet > 0,
                "{soc}/{name}: 甜点落在首档，功耗曲线在归一化后没有超线性段"
            );
            assert!(sweet < b.freq_khz.len() - 1, "{soc}/{name}: 甜点落在顶档");
            out.push((name, b.freq_khz[sweet]));
        }
        out
    }

    /// 8650：8 Gen 3 实测能效曲线折算（读图 ≤2 档漂移按体质误差接受）
    #[test]
    fn sm8650_frontier_usable() {
        let got = check("sm8650", include_str!("../module/config/sm8650/soc.yaml"));
        assert_eq!(
            got,
            vec![("little", 1843200), ("big", 2323200), ("prime", 2246400)],
            "sm8650 甜点漂移超出预期（改了锚点数据就需要同步更新 expectations）"
        );
    }

    /// 8750：EEM 首个适配机型，行为不得被后续改动带偏
    #[test]
    fn sm8750_frontier_usable() {
        // 8750 是两簇布局（无小核簇，little 空区间 + 空表）→ 只有 big/prime 两簇
        let got = check("sm8750", include_str!("../module/config/sm8750/soc.yaml"));
        assert_eq!(got.len(), 2, "sm8750 应只有 big/prime 两簇（无小核）");
    }

    /// 8550：完整实测表机型，同上
    #[test]
    fn sm8550_frontier_usable() {
        let got = check("sm8550", include_str!("../module/config/sm8550/soc.yaml"));
        assert_eq!(got.len(), 3, "sm8550 三簇都应给出甜点");
    }
}
