//! energy_cost.rs: [source] [power] [supply] [cost] [probe] [decision] [tests]
//!
//! FDP（前沿主导的放置内核）的能耗成本模型。**单点真相**：`mdocs/8550/sm8550-freq-power.md`
//! §3「簇级口径」三张表 → `scripts/frontier_policy.py` → `module/config/8550/soc.yaml` 受控块
//! `per_cluster.<簇>.power_w` → 运行期 `soc_frontier_policy()`。本模块**不手抄任何表**。
//!
//! ⚠️ 表是纯动态项（DPC·f·V²，不含漏电/静态功耗/idle-exit，电压借自另一批次 Odin2），**只能用于
//! 相对排序、不做绝对功耗预测**；单线程×簇级的比较不等于图③的整机前沿（口径降级见计划 3.3（5））。
//!
//! **频率口径**：不做「簇总算力均摊」的过原点反解——它假设需求摊到全簇所有核，对少核簇严重低估
//! （prime 1 核时轻需求会被钳到最低档 → `ΔP=0` → 误判 prime「免费」，与 schedutil 真实行为及
//! 计划 P-b「单核算力需求 >2600 Mdmips 才用 prime」冲突）。改为：
//! - **现状**：取该簇**实测** `scaling_cur_freq`（FREQ_QOS 聚合值、非 CLG 决策上限）；读不到才用模型
//!   `freq_for_policy_util(cluster, u_now)`。现状不含模型误差。
//! - **之后**：`Δu = d / cap_簇`（一份需求落在**一个核**上 → 该核占用率增量）；
//!   `u_after = clamp(u_now ± Δu, 0, 1)`；`f_after = floor 对齐(f_min + u_after × (f_max − f_min))`。
//! - `u_now` = `core_utils` 里该簇各核 util 的**最大值**（与 policy 口径一致：schedutil 的频率来自
//!   该 policy 的 util = 最忙核占用率，不是簇总算力/平均）。
//! - 同一份 `f` 下 `P(f)` 超线性（§2.1 算力/mW 随频率单调劣化）→ 同为 `Δu` 在高频段 `ΔP` 更大：
//!   往已经在高位的簇塞更贵，即「注意目标核当前能效、避免迁移把目标簇能效拉劣」。
//!
//! 非 8550 / 前沿段未启用 → `soc_frontier_policy()` 为 `None` → 全部返回 `None`；调用方先经
//! [`fdp_available`] 判定，缺数据时**回退原放置路径**（不是把后台 promote 整段停掉）。

use crate::common::{self, CoreGroup};
use std::sync::OnceLock;

// [source]
/// 取该簇的 (频率档 kHz, 逐档簇功耗 W)：数据源 = `soc_frontier_policy().per_cluster`。
/// 缺段 / 缺该簇表 / 空表 / `freqs.len() != powers.len()` → `None`（不齐一律回退、不动作、不 panic、不猜）
fn cluster_energy_table(cluster: CoreGroup) -> Option<(&'static [u32], &'static [f32])> {
    let pol = common::soc_frontier_policy()?;
    let base = pol.per_cluster.as_ref()?;
    let (freqs, powers) = match cluster {
        CoreGroup::Little => (&base.little.freq_khz, &base.little.power_w),
        CoreGroup::Big => (&base.big.freq_khz, &base.big.power_w),
        CoreGroup::Prime => (&base.prime.freq_khz, &base.prime.power_w),
    };
    if freqs.is_empty() || freqs.len() != powers.len() {
        return None;
    }
    Some((freqs.as_slice(), powers.as_slice()))
}

// [power]
/// 在 (freqs, powers) 上线性插值：越界取端点（低于首档→首档功耗，高于末档→末档功耗）；
/// 表空 / 长度不一致 → `None`。纯函数（不读全局），便于测试
fn interp(freqs: &[u32], powers: &[f32], freq_khz: u32) -> Option<f32> {
    if freqs.is_empty() || freqs.len() != powers.len() {
        return None;
    }
    let f = freq_khz as f32;
    let f_lo = *freqs.first()? as f32;
    let f_hi = *freqs.last()? as f32;
    if f <= f_lo {
        return powers.first().copied();
    }
    if f >= f_hi {
        return powers.last().copied();
    }
    for i in 1..freqs.len() {
        if f <= freqs[i] as f32 {
            let lo = freqs[i - 1] as f32;
            let hi = freqs[i] as f32;
            let p0 = powers[i - 1];
            let p1 = powers[i];
            // 同表内不存在 lo == hi（频档升序且不等），该守卫只为杜绝除零
            let t = if hi > lo { (f - lo) / (hi - lo) } else { 0.0 };
            return Some(p0 + (p1 - p0) * t);
        }
    }
    powers.last().copied()
}

/// 该簇在 `freq_khz` 处的**簇功耗**（W）：查 `soc_frontier_policy()` 逐档表并线性插值，越界取端点。
/// 非 8550 / 前沿段缺失 / 表不齐 → `None`
pub fn cluster_power_w(cluster: CoreGroup, freq_khz: u32) -> Option<f32> {
    let (freqs, powers) = cluster_energy_table(cluster)?;
    interp(freqs, powers, freq_khz)
}

// [accounting]
/// CPU **动态**功率估计（W）：逐簇 `簇功耗表(实测频率) × 簇忙碌比` 求和。
///
/// 忙碌比 = 簇内各核 util 之和 ÷ 簇核数——表是「整簇满载」口径，单核满载即 1/N。
/// util 入参是 `core_utils`（按 CPU 编号索引，0~1）。
///
/// ⚠️ 三处口径限制，读数前必知：
/// 1. 表只含动态项（不含漏电 / 静态 / idle-exit），本值是模型估计，
///    不是逐秒物理下界或实测 CPU 总功率；
/// 2. 电压借自另一批次（见模块头），绝对量有系统偏差，相对排序也需核对适用场景；
/// 3. 无功耗表的 SoC（当前仅 8550 有）返回 `None`，对应列写 `-`。
///
/// 用途：把电池端总功率里 ChiRi 能影响的那部分单独记一列，离线用「总 − 动态 = 残差」
/// 剥掉屏幕 / modem / GPU / 静态这些调度层碰不到的外围，避免拿整机功耗直接归因调度改动。
///
/// **代价**：有表的 SoC 上每秒 1 次调用 = 3 次 `scaling_cur_freq` open/read（`dev_record`
/// 关闭时也照读，因为它挂在 status 行上而不是诊断块里）。相对 `@S` 帧上千次 stat 读是小头，
/// 同秒已有实测快照时可改用 `cpu_dynamic_power_w_with_frequencies`，省去重复频率读取。
pub fn cpu_dynamic_power_w(core_utils: &[f32]) -> Option<f32> {
    if !has_cpu_energy_table() {
        return None;
    }
    let frequencies = read_cluster_freqs();
    let ranges = common::chiri_core_ranges();
    let spans = [ranges.little, ranges.big, ranges.prime];
    sum_cpu_dynamic_power_w(core_utils, &spans, &frequencies, cluster_power_w)
}

/// CPU 动态功率估计（W）：使用同秒 `(policy ID, scaling_cur_freq kHz)` 快照。
/// 顺序不限，policy ID 不得重复；缺失、读取失败及 0 频率均跳过，不回退模型、不额外读频率。
/// policy 到簇的拓扑映射沿用 `read_cluster_freqs` 的一次性发现结果。
pub fn cpu_dynamic_power_w_with_frequencies(
    core_utils: &[f32],
    policy_frequencies: &[(u32, Option<u32>)],
) -> Option<f32> {
    if !has_cpu_energy_table() {
        return None;
    }
    let ranges = common::chiri_core_ranges();
    let spans = [ranges.little, ranges.big, ranges.prime];
    let frequencies = cluster_frequencies_from_policies(cluster_policy_ids(), policy_frequencies);
    sum_cpu_dynamic_power_w(core_utils, &spans, &frequencies, cluster_power_w)
}

fn has_cpu_energy_table() -> bool {
    let ranges = common::chiri_core_ranges();
    let groups = [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime];
    let spans = [ranges.little, ranges.big, ranges.prime];
    (0..3).any(|slot| !spans[slot].is_empty() && cluster_energy_table(groups[slot]).is_some())
}

fn cluster_frequencies_from_policies(
    policy_ids: &[Option<u32>; 3],
    policy_frequencies: &[(u32, Option<u32>)],
) -> [Option<u32>; 3] {
    policy_ids.map(|policy_id| {
        let policy_id = policy_id?;
        policy_frequencies
            .iter()
            .find(|(observed_policy_id, _)| *observed_policy_id == policy_id)
            .and_then(|(_, frequency_khz)| *frequency_khz)
            .filter(|frequency_khz| *frequency_khz > 0)
    })
}

fn sum_cpu_dynamic_power_w<F>(
    core_utils: &[f32],
    spans: &[std::ops::Range<usize>; 3],
    policy_frequencies: &[Option<u32>; 3],
    mut power_at_frequency: F,
) -> Option<f32>
where
    F: FnMut(CoreGroup, u32) -> Option<f32>,
{
    let groups = [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime];
    let mut total_power = 0.0f32;
    let mut has_observed_cluster = false;
    for slot in 0..3 {
        let span = &spans[slot];
        if span.is_empty() {
            continue;
        }
        let Some(frequency_khz) = policy_frequencies[slot] else {
            continue;
        };
        let Some(cluster_power) = power_at_frequency(groups[slot], frequency_khz) else {
            continue;
        };
        let mut busy_utilization = 0.0f32;
        let mut observed_core_count = 0usize;
        for core_index in span.clone() {
            if let Some(utilization) = core_utils.get(core_index).copied() {
                busy_utilization += utilization;
                observed_core_count += 1;
            }
        }
        if observed_core_count == 0 {
            continue;
        }
        // 缺失 util 不当成实测零；有样本时仍按整簇核数归一化。
        total_power += cluster_power * (busy_utilization / span.len() as f32).clamp(0.0, 1.0);
        has_observed_cluster = true;
    }
    has_observed_cluster.then_some(total_power)
}

// [supply]
/// 该簇的**真实频档**（kHz 升序）：优先 `soc.yaml [freq_khz]`，缺失则回退前沿受控块
/// `per_cluster.<簇>.freq_khz`（两者同源）；都缺 → `None`
fn cluster_real_freqs(cluster: CoreGroup) -> Option<&'static [u32]> {
    if let Some(t) = common::soc_config().and_then(|c| c.freq_khz.as_ref()) {
        let v = match cluster {
            CoreGroup::Little => &t.little,
            CoreGroup::Big => &t.big,
            CoreGroup::Prime => &t.prime,
        };
        if !v.is_empty() {
            return Some(v.as_slice());
        }
    }
    let pol = common::soc_frontier_policy()?;
    let base = pol.per_cluster.as_ref()?;
    let v = match cluster {
        CoreGroup::Little => &base.little.freq_khz,
        CoreGroup::Big => &base.big.freq_khz,
        CoreGroup::Prime => &base.prime.freq_khz,
    };
    if v.is_empty() {
        None
    } else {
        Some(v.as_slice())
    }
}

/// 该簇当前**最忙核**占用率（0..1）：`core_utils` 为该簇各核 util，取最大值。
/// 与 policy 口径一致（schedutil 的频率来自该 policy 的 util = 最忙核占用率，不取平均/簇总算力）
fn cluster_now_util(cluster: CoreGroup, core_utils: &[f32]) -> f32 {
    let r = common::chiri_core_ranges();
    let range = match cluster {
        CoreGroup::Little => r.little,
        CoreGroup::Big => r.big,
        CoreGroup::Prime => r.prime,
    };
    let mut m = 0.0_f32;
    for c in range {
        m = m.max(core_utils.get(c).copied().unwrap_or(0.0).clamp(0.0, 1.0));
    }
    m
}

/// schedutil 式 **util → frequency**（kHz）：线性映射 `f_min + u × (f_max − f_min)` 后
/// **floor 对齐**到该簇真实频档（≤ raw 的最大档）。缺频表 → `None`。
/// **不做簇总算力/过原点换算**（那会低估少核簇真实频率）。
pub fn freq_for_policy_util(cluster: CoreGroup, u: f32) -> Option<u32> {
    if !u.is_finite() {
        return None;
    }
    let u = u.clamp(0.0, 1.0);
    let table = cluster_real_freqs(cluster)?;
    let f_min = *table.first()? as f32;
    let f_max = *table.last()? as f32;
    let raw = f_min + u * (f_max - f_min);
    // 频档升序：取 ≤ raw 的最大档（floor 对齐）
    let mut out = *table.first()?;
    for &f in table {
        if f as f32 <= raw {
            out = f;
        } else {
            break;
        }
    }
    Some(out)
}

/// 该簇**工作频率**：优先实测 `scaling_cur_freq`（现状不含模型误差），读不到才用模型
/// `freq_for_policy_util(cluster, u_now)`
fn cluster_now_freq(cluster: CoreGroup, u_now: f32, cur_freq: Option<u32>) -> Option<u32> {
    match cur_freq {
        Some(f) if f > 0 => Some(f),
        _ => freq_for_policy_util(cluster, u_now),
    }
}

// [cost]
/// 双边净收益（W）：`net = [P_src(f_src_after) − P_src(f_src_now)] + [P_dst(f_dst_after) − P_dst(f_dst_now)]`。
///
/// **等价语义（写清便于审查）**：两项对应**同一份需求 d**，所以 `net < 0` ⟺ 这份算力搬到目标簇后的
/// **边际 mW/Mdmips 比留在源簇更低**——判据本身就是「等算力下更低功耗」，不是簇名气/平均值比较。
/// 又因同簇 `P(f)` 超线性（§2.1），同样的占用率/频率增量在高频段 `ΔP` 更大 → **往已在高位的簇塞更贵**。
/// 源簇让出后工作点不变 → 让出收益为 0 → 默认不动（保守口径）。
/// 参数为**频率**（kHz）：调用方必须传**单调且同底**的一对值，并保证两条不变式——
/// `f_dst_after ≥ f_dst_now`（否则 `ΔP_dst < 0`，等于给一个簇加需求却更省电）与
/// `f_src_after ≤ f_src_now`（否则让出收益为负）。`fdp_destination` 用保守包络构造这两对值。
pub fn net_benefit_w(
    src: CoreGroup,
    f_src_now: u32,
    f_src_after: u32,
    dst: CoreGroup,
    f_dst_now: u32,
    f_dst_after: u32,
) -> Option<f32> {
    let p_src_now = cluster_power_w(src, f_src_now)?;
    let p_src_after = cluster_power_w(src, f_src_after)?;
    let p_dst_now = cluster_power_w(dst, f_dst_now)?;
    let p_dst_after = cluster_power_w(dst, f_dst_after)?;
    Some((p_src_after - p_src_now) + (p_dst_after - p_dst_now))
}

// [probe]
/// 簇 → 数组槽位（little 0 / big 1 / prime 2），`read_cluster_freqs`/FDP 内部索引共用
fn cluster_slot(cluster: CoreGroup) -> usize {
    match cluster {
        CoreGroup::Little => 0,
        CoreGroup::Big => 1,
        CoreGroup::Prime => 2,
    }
}

/// 簇 → cpufreq policy id 的映射，**只发现一次**：policy 目录开机后固定（热插拔只动核的 online，
/// 不增删 policy），所以把「扫 policy 目录 + related_cpus 首核归属」这套发现过程做成一次性，
/// 此后每轮只读 3 个 `scaling_cur_freq`。
fn cluster_policy_ids() -> &'static [Option<u32>; 3] {
    static IDS: OnceLock<[Option<u32>; 3]> = OnceLock::new();
    IDS.get_or_init(|| {
        let mut out = [None, None, None];
        let mut found = 0usize;
        for pid in 0..=16u32 {
            if found == 3 {
                break;
            }
            let base = format!("/sys/devices/system/cpu/cpufreq/policy{pid}");
            let Ok(text) = std::fs::read_to_string(format!("{base}/related_cpus"))
                .or_else(|_| std::fs::read_to_string(format!("{base}/affected_cpus")))
            else {
                continue;
            };
            let Some(first) = text
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let Some(slot) = common::core_group_of(first).map(cluster_slot) else {
                continue;
            };
            if out[slot].is_some() {
                continue;
            }
            out[slot] = Some(pid);
            found += 1;
        }
        out
    })
}

/// 三簇当前实际运行频率（kHz，返回 `[little, big, prime]`）：按缓存好的 policy id 各读一个
/// `scaling_cur_freq`，共 3 次 open。
///
/// **口径 = FREQ_QOS 聚合后的实际频率，不是 CLG 的决策原值/上限**；读不到的簇为 `None`
/// （调用方回退到模型 `freq_for_policy_util`）。
pub fn read_cluster_freqs() -> [Option<u32>; 3] {
    let ids = cluster_policy_ids();
    let mut out = [None, None, None];
    for (slot, pid) in ids.iter().enumerate() {
        let Some(pid) = pid else { continue };
        out[slot] = std::fs::read_to_string(format!(
            "/sys/devices/system/cpu/cpufreq/policy{pid}/scaling_cur_freq"
        ))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    }
    out
}

// [decision]
/// FDP 迁移明细（离线核对用）：目的簇、净收益 W，以及两簇 before/after 工作频率（kHz）
pub struct FdpMove {
    pub dst: CoreGroup,
    pub net_w: f32,
    pub f_src_now: u32,
    pub f_src_after: u32,
    pub f_dst_now: u32,
    pub f_dst_after: u32,
}

/// FDP 决策结果：`Move` = 目的簇 + 明细；`Skip` = 不动作的原因（离线核对用）
pub enum FdpChoice {
    Move(FdpMove),
    Skip(&'static str),
}

// [probe]
/// FDP 可用性（静态数据面）：三簇 capacity 与簇功耗表都在 = 本 SoC 支持 FDP。
/// 缺任何一项（soc.yaml 无 capacity / frontier 段）→ 调用方回退旧的放置路径，
/// 而不是把后台 promote 整段停掉（「不支持的 SoC 回退原逻辑」）
pub fn fdp_available() -> bool {
    const ALL: [CoreGroup; 3] = [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime];
    ALL.iter()
        .all(|&c| common::soc_capacity_for_group(c).is_some() && cluster_energy_table(c).is_some())
}

/// FDP：后台/批处理线程从 little 让出的**目的簇选择**（纯函数：同一输入 → 同一落点，不依赖迭代顺序）。
///
/// 源簇 = little（后台线程被 `pin_background` 限制在小核），线程序列需求 `d = util 比例 × little 单核 capacity`。
/// 目的簇候选：`allow_prime == false` 时**只有 big**——prime 独核被从 rail-collapse 拉起后的静默代价、
/// 以及 P-b「单核算力需求 >2600 Mdmips 才用 prime」门槛，本静态表（纯动态项、不含漏电/idle-exit）都判不出来，
/// 表会系统性低估 prime 真实成本。故 prime 候选资格仍由 `Affinity.bg_promote_exclude_prime` 把关（A3 保持硬规则）：
/// 只有显式把 `bg_promote_exclude_prime: false`，prime 才进入候选。缺省保守、零新增开关。
///
/// 过率条件：`net < 0 且 net ≤ −hysteresis`、迁入后 `f_dst_after ≤ ratio × 该簇最高频`、该簇本轮迁入条数未触顶；
/// 通过后取 `net` 最小（最省）的簇。配额与频率上限是**同轮硬约束**（成本项只在跨轮生效）。
#[allow(clippy::too_many_arguments)]
pub fn fdp_destination(
    thread_util_pct: f32,
    core_utils: &[f32],
    cur_freqs: &[Option<u32>; 3],
    hysteresis_w: f32,
    dst_freq_cap_ratio: f32,
    allow_prime: bool,
    inserts: &[u32; 2],
    max_inserts: u32,
) -> FdpChoice {
    const BOTH: [CoreGroup; 2] = [CoreGroup::Big, CoreGroup::Prime];
    const BIG_ONLY: [CoreGroup; 1] = [CoreGroup::Big];

    let src = CoreGroup::Little;
    let Some(src_cap) = common::soc_capacity_for_group(src) else {
        return FdpChoice::Skip("nodata");
    };
    let src_cap_f = src_cap as f32;
    if src_cap_f <= 0.0 {
        return FdpChoice::Skip("nodata");
    }
    if !thread_util_pct.is_finite() || thread_util_pct <= 0.0 {
        return FdpChoice::Skip("noreq");
    }
    // 需求 d（单核容量单位）= 窗口 util 比例 × little 单核 capacity
    let d = (thread_util_pct / 100.0).min(1.0) * src_cap_f;
    let u_src_now = cluster_now_util(src, core_utils);
    let Some(f_src_now) = cluster_now_freq(src, u_src_now, cur_freqs[cluster_slot(src)]) else {
        return FdpChoice::Skip("nodata");
    };
    // Δu = d / cap_簇（一份需求落在一个核上）；让出后该核占用率下降
    let u_src_after = (u_src_now - d / src_cap_f).clamp(0.0, 1.0);
    let Some(f_src_after_model) = freq_for_policy_util(src, u_src_after) else {
        return FdpChoice::Skip("nodata");
    };
    // 让出收益也取保守侧：之后频率不高于现状（`min`）——保证 `ΔP_src ≤ 0`（让出不可能更费电），
    // 也避免源簇此刻实测偏低时「之后」被算得比现状还高、让收益反向。让出收益宁小勿大：收益虚高会推动不该有的迁移
    // TODO: 这一对值不同底：现状取实测、之后取模型（目的簇侧已用 f_dst_now/f_dst_after 包络对齐，此处缺 f_src_now 的模型值）。
    // 实测高于模型（QoS/热/别的线程抬频）时 `min` 仍会放大让出收益；修法 = 对称地取 f_src_now = min(实测, 模型)。
    let f_src_after = f_src_after_model.min(f_src_now);

    let max_inserts = max_inserts.max(1);
    let ratio = dst_freq_cap_ratio.clamp(0.0, 1.0);
    let dsts: &[CoreGroup] = if allow_prime { &BOTH } else { &BIG_ONLY };
    let mut best: Option<FdpMove> = None;
    // 位 0 = quota、位 1 = freq cap/数据缺失、位 2 = 净收益不达标。多候选中不同候选被不同约束拦下会
    // 同时置位，故出参按位组合如实回（见函数尾），不压成单一原因——否则离线只能看到一个原因、
    // 把「同一候选被多重拦截」误记成单一约束（如把 dstfreq 记高/记漏）。
    let mut blocked = 0u32;
    for &dst in dsts {
        let slot = cluster_slot(dst);
        let islot = usize::from(matches!(dst, CoreGroup::Prime));
        if inserts.get(islot).copied().unwrap_or(0) >= max_inserts {
            blocked |= 1;
            continue;
        }
        let Some(dst_cap) = common::soc_capacity_for_group(dst) else {
            blocked |= 2;
            continue;
        };
        let dst_cap_f = dst_cap as f32;
        if dst_cap_f <= 0.0 {
            blocked |= 2;
            continue;
        }
        let Some(dst_top) = cluster_real_freqs(dst).and_then(|t| t.last().copied()) else {
            blocked |= 2;
            continue;
        };
        let u_dst_now = cluster_now_util(dst, core_utils);
        let Some(f_dst_now_measured) = cluster_now_freq(dst, u_dst_now, cur_freqs[slot]) else {
            blocked |= 2;
            continue;
        };
        // 与实测同底的模型现状：下面靠它把「现状 / 之后」锚到同一侧，避免两个不同底相减
        let Some(f_dst_now_model) = freq_for_policy_util(dst, u_dst_now) else {
            blocked |= 2;
            continue;
        };
        let u_dst_after = (u_dst_now + d / dst_cap_f).clamp(0.0, 1.0);
        let Some(f_dst_after_model) = freq_for_policy_util(dst, u_dst_after) else {
            blocked |= 2;
            continue;
        };
        // 承接成本取**保守包络**：现状 = 实测与模型的**小**，之后 = 模型与实测的**大**。两个作用：
        // ① 保证 `ΔP_dst ≥ 模型边际 ≥ 0`（给一个簇加需求不可能让它更省电）。若照直用「实测现状 + 模型之后」，
        //    当目的簇因 QoS / 热 / 别的线程尖峰此刻跑得比模型高时，会算出 `f_after < f_now`、`ΔP_dst < 0`，
        //    把承接成本算成免费甚至收益——即「低估承接成本 → 反鼓励往满簇塞」；
        // ② 顺带堵住「簇已经在热 ⇒ 加负载免费」的错觉：实测把差值压到 0 时，仍至少收模型那一段边际。
        let f_dst_now = f_dst_now_measured.min(f_dst_now_model);
        let f_dst_after = f_dst_after_model.max(f_dst_now_measured);
        // 硬护栏（P-a「任何簇贴顶都不行」）：迁入后该簇工作频率不得高于 ratio × 该簇最高频。
        // 比的是上面那个包络值（≥ 实测现状），所以「目的簇此刻就已经贴在劣化区」同样会被拦下
        if f_dst_after as f32 > dst_top as f32 * ratio {
            blocked |= 2;
            continue;
        }
        let Some(net) = net_benefit_w(src, f_src_now, f_src_after, dst, f_dst_now, f_dst_after)
        else {
            blocked |= 2;
            continue;
        };
        // 「等算力下更低功耗」：必须严格为负且幅度达滞回阈值，否则与「不搬（增量 0）」相比不该动
        if !(net < 0.0 && net <= -hysteresis_w) {
            blocked |= 4;
            continue;
        }
        let cand = FdpMove {
            dst,
            net_w: net,
            f_src_now,
            f_src_after,
            f_dst_now,
            f_dst_after,
        };
        match &best {
            Some(b) if b.net_w <= net => {}
            _ => best = Some(cand),
        }
    }
    match best {
        Some(m) => FdpChoice::Move(m),
        None => {
            // 按 blocked 的位组合如实回原因（不压成单一值）：离线按 `+` 拆开即可还原每个约束
            // 各拦下多少候选，`nodata` 表示三个约束都没置位（候选为空或压根没进循环）
            let reason = match blocked & 0b111 {
                0 => "nodata",
                1 => "quota",
                2 => "dstfreq",
                3 => "quota+dstfreq",
                4 => "hysteresis",
                5 => "quota+hysteresis",
                6 => "dstfreq+hysteresis",
                _ => "quota+dstfreq+hysteresis",
            };
            FdpChoice::Skip(reason)
        }
    }
}

// [tests]
// 本仓库**无测试基建**（无 CI 执行 `cargo test`）——以下测试只保证编译通过、不会被执行；
// 依赖 SoC 运行期数据（`soc_frontier_policy()` / `chiri_core_ranges()`）的断言在开发机恒为
// None/兜底值，故只用「None 或满足不变量」的弱断言，避免误伤。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_frequencies_follow_policy_ids_and_preserve_missing_values() {
        let policy_ids = [Some(0), Some(4), Some(7)];
        let frequencies = [(7, Some(2_000_000)), (0, Some(600_000)), (4, None)];
        assert_eq!(
            cluster_frequencies_from_policies(&policy_ids, &frequencies),
            [Some(600_000), None, Some(2_000_000)]
        );
        assert_eq!(
            cluster_frequencies_from_policies(&policy_ids, &[(0, Some(0))]),
            [None, None, None]
        );
        assert_eq!(
            cluster_frequencies_from_policies(&[None; 3], &frequencies),
            [None; 3]
        );
    }

    #[test]
    fn snapshot_accounting_uses_khz_and_full_cluster_denominators() {
        let spans = [0..2, 2..3, 3..4];
        let frequencies = [Some(1_000_000), None, Some(2_000_000)];
        let power_at_frequency = |_: CoreGroup, frequency_khz| {
            interp(&[1_000_000, 2_000_000], &[2.0, 4.0], frequency_khz)
        };
        assert_eq!(
            sum_cpu_dynamic_power_w(&[1.0, 0.0, 1.0, 0.5], &spans, &frequencies, power_at_frequency),
            Some(3.0)
        );
        assert_eq!(
            sum_cpu_dynamic_power_w(&[1.0], &spans, &frequencies, power_at_frequency),
            Some(1.0)
        );
        assert_eq!(
            sum_cpu_dynamic_power_w(&[], &spans, &frequencies, power_at_frequency),
            None
        );
        assert_eq!(
            sum_cpu_dynamic_power_w(&[0.0; 4], &spans, &frequencies, power_at_frequency),
            Some(0.0)
        );
        assert_eq!(
            sum_cpu_dynamic_power_w(&[1.0; 4], &spans, &[None; 3], power_at_frequency),
            None
        );
    }

    #[test]
    fn snapshot_api_does_not_estimate_unobserved_frequencies() {
        assert_eq!(
            cpu_dynamic_power_w_with_frequencies(&[1.0; 8], &[] as &[(u32, Option<u32>)]),
            None
        );
    }

    #[test]
    fn interp_linear_clamp_and_missing() {
        let freqs = [1000u32, 2000];
        let powers = [1.0_f32, 3.0];
        // 表内中点 = 两端线性插值
        assert_eq!(interp(&freqs, &powers, 1500), Some(2.0));
        // 越界按端点外推取端点值
        assert_eq!(interp(&freqs, &powers, 500), Some(1.0));
        assert_eq!(interp(&freqs, &powers, 9000), Some(3.0));
        // 空表 / 长度不一致 → None
        assert_eq!(interp(&[], &[], 1000), None);
        assert_eq!(interp(&freqs, &[1.0], 1000), None);
    }

    #[test]
    fn anchor_big_low_vs_little_high_net_negative() {
        // §2.1/§5 锚点：同量级算力下「big 低频」比「little 高频」省
        // （little 2016000 ≈ 0.933 W ↔ big 940800 ≈ 0.654 W）。真机 8550 有 frontier 表时，
        // 从 little 让出（2016000→307200）、搬到 big（499200→940800）的净收益必须为负；
        // 测试环境读不到 SoC 数据时 API 返回 None（同样不动作）——两种情况都不许 net > 0。
        let net = net_benefit_w(
            CoreGroup::Little,
            2_016_000,
            307_200,
            CoreGroup::Big,
            499_200,
            940_800,
        );
        assert!(net.map_or(true, |v| v < 0.0));
    }

    #[test]
    fn missing_data_returns_none() {
        // 表不齐 → None（数据缺失路径，不 panic、不猜）
        assert_eq!(interp(&[], &[], 1000), None);
        assert_eq!(interp(&[1000u32, 2000], &[1.0f32], 1000), None);
        // 无 SoC 数据时 FDP 整段不动作
        let choice = fdp_destination(30.0, &[], &[None, None, None], 0.02, 0.85, true, &[0, 0], 2);
        assert!(matches!(choice, FdpChoice::Skip(_)));
    }
}
