//! energy_model.rs: [curve] [sweet] [frontier] [range] [tests]
//!
//! 相对能效模型 —— 全仓**唯一**的能效决策数据源。
//!
//! # 为什么是相对向量
//! 绝对功耗曲线的量纲不可信（第三方图表、单核 vs 整簇口径、漏电未扣净），
//! 但**形状**可靠。故先把单核功耗与单核算力各自归一成 0→1 的相对向量，
//! 此后所有比较都在这把无量纲尺子上做：
//!
//! * 簇内：`cap_rel[i] = f[i]/f_max`，`pow_rel[i] = (p[i]−p[0])/(p_max−p[0])`
//!   两者同域 0→1，于是「1 单位功耗换 1 单位算力」就是**斜率 1.0**，
//!   斜率跌破 1.0 处即[甜点](Curve::sweet)。乘以任何正常数都不改变结论。
//! * 跨簇：功耗以本 SoC 最大单核动态范围为 `P_REF`、算力以最大单核 capacity
//!   为 `CAP_REF`，仍是相对量，只保证**同一把尺子**下可比。
//!
//! # 为什么按忙核数计成本
//! 调度器迁移的是**线程**，不是整簇。一个线程跑在 6 核簇上是「1 核忙、5 核空闲」，
//! 簇频照样拉到顶。若按整簇核数计成本，同一决策会被放大约 N 倍
//! （sm8750 big 顶档 6× 高估），且需求轴也会被抬到单线程够不着的高度。
//! 故 `cost(gear, k) = p[gear] × k`，`k` = 真正要承接工作的核数。
//!
//! 数据源：`soc.yaml` 的 `frontier_policy.per_cluster`（逐档频点 + 逐档**簇**功耗）。
//! 单核功耗 = 簇功耗 ÷ 簇核数（纯除法）；若该段给了 `single_core_power_w` 则直接采用。
//! 任一环节缺数据一律 `None` 回退，绝不猜。

use crate::common::{self, CoreGroup};
use std::sync::OnceLock;

// [curve]

/// 单簇相对能效曲线（构造期一次算定，运行期只做下标访问）
pub struct Curve {
    /// 该簇真实频档（kHz 升序）
    freqs: Box<[u32]>,
    /// 跨簇标定算力（CAP_REF 单位）
    cap_glob: Box<[f32]>,
    /// 跨簇标定功耗（P_REF 单位）
    pow_glob: Box<[f32]>,
    /// 甜点档下标（`Δcap_rel/Δpow_rel` 最后一次 ≥ 1.0 的档）
    sweet: usize,
    /// 该簇拓扑核数（放置时 `k` 的上限）
    n_cores: usize,
}

impl Curve {
    /// 频档数
    pub fn len(&self) -> usize {
        self.freqs.len()
    }

    /// 该簇拓扑核数
    pub fn cores(&self) -> usize {
        self.n_cores
    }

    /// 该档频点 kHz
    pub fn freq_khz(&self, i: usize) -> u32 {
        self.freqs.get(i).copied().unwrap_or(0)
    }

    /// 甜点档下标（能效拐点）
    pub fn sweet(&self) -> usize {
        self.sweet
    }

    /// 甜点档频点 kHz
    pub fn sweet_khz(&self) -> u32 {
        self.freq_khz(self.sweet)
    }

    /// 该档单核算力（CAP_REF 单位）
    pub fn cap_at(&self, i: usize) -> f32 {
        self.cap_glob.get(i).copied().unwrap_or(0.0)
    }

    /// 该档单核功耗（P_REF 单位）
    pub fn pow_at(&self, i: usize) -> f32 {
        self.pow_glob.get(i).copied().unwrap_or(f32::INFINITY)
    }

    /// k 核在档 i 的算力供给（CAP_REF 单位）
    pub fn supply(&self, i: usize, k: usize) -> f32 {
        self.cap_at(i) * k.max(1) as f32
    }

    /// k 核在档 i 的成本（P_REF 单位），**按忙核数计**
    pub fn cost(&self, i: usize, k: usize) -> f32 {
        self.pow_at(i) * k.max(1) as f32
    }

    /// 最高档下标
    pub fn top(&self) -> usize {
        self.len().saturating_sub(1)
    }

    /// 满足需求 `d` 的最低档（**不设甜点上限**）：硬件够不着时返回最高档
    pub fn gear_for(&self, d: f32, k: usize) -> usize {
        if !d.is_finite() || d <= 0.0 {
            return 0;
        }
        (0..self.len())
            .find(|&i| self.supply(i, k) >= d)
            .unwrap_or_else(|| self.top())
    }

    /// 甜点档以下（含）满足 `d` 的最低档；甜点档都不够 → `None`（= 该靠加核或迁移了）
    pub fn gear_below_sweet_for(&self, d: f32, k: usize) -> Option<usize> {
        if !d.is_finite() || d <= 0.0 {
            return Some(0);
        }
        (0..=self.sweet.min(self.top())).find(|&i| self.supply(i, k) >= d)
    }
}

// [sweet]

/// 甜点下标：`Δcap_rel/Δpow_rel` 最后一次 ≥ 1.0 的档；该段 Δpow == 0 视为无限边际能效。
/// 全程斜率都 < 1.0 → 0（首档即最优）。空/单元素输入 → 0。
///
/// 公开给离线回归（`scripts/tests/energy_model_regression.rs`）直接验证判据，
/// 不必构造完整 SoC 数据。
pub fn sweet_index(cap_rel: &[f32], pow_rel: &[f32]) -> usize {
    let mut sweet = 0usize;
    for i in 1..cap_rel.len().min(pow_rel.len()) {
        let dc = cap_rel[i] - cap_rel[i - 1];
        if dc <= 0.0 {
            continue;
        }
        let dp = pow_rel[i] - pow_rel[i - 1];
        // 严格 `<= 0` 才视为「零功耗增量」。**不能用 `f32::EPSILON` 这类绝对阈值**：
        // 相对向量整体缩放后增量会同比例变小，绝对阈值会把「不免费」误判成「免费」，
        // 甜点随缩放漂移（回归 `sweet_point_is_scale_invariant` 已证）。
        let eff = if dp <= 0.0 { f32::INFINITY } else { dc / dp };
        if eff >= 1.0 {
            sweet = i;
        }
    }
    sweet
}

// [frontier]

/// 候选放置：某簇、某档、k 个忙核
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub cluster: CoreGroup,
    /// 档位下标
    pub gear: usize,
    /// 目标频点 kHz
    pub freq_khz: u32,
    /// 忙核数（≥1）
    pub cores: usize,
    /// 成本（P_REF 单位）
    pub cost: f32,
    /// 算力供给（CAP_REF 单位）
    pub supply: f32,
}

/// 该簇单核容量（真机 `cpu_capacity` 优先，缺则 `soc.yaml [capacity]`）
fn cluster_cap(cluster: CoreGroup) -> Option<f32> {
    let r = common::chiri_core_ranges();
    let first = match cluster {
        CoreGroup::Little => r.little.start,
        CoreGroup::Big => r.big.start,
        CoreGroup::Prime => r.prime.start,
    };
    let from_sys =
        std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{first}/cpu_capacity"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|&v| v > 0);
    let cap = from_sys.or_else(|| common::soc_capacity_for_group(cluster))? as f32;
    (cap > 0.0).then_some(cap)
}

/// 该簇拓扑核数（空区间按 1，避免除零；空簇本来就没有 curve）
fn cluster_cores(cluster: CoreGroup) -> usize {
    let r = common::chiri_core_ranges();
    let n = match cluster {
        CoreGroup::Little => r.little.len(),
        CoreGroup::Big => r.big.len(),
        CoreGroup::Prime => r.prime.len(),
    };
    n.max(1)
}

/// 该簇逐档**单核**动态功耗：`single_core_power_w` 优先，否则 `power_w ÷ n`。
/// 长度不齐 / 全零 / 缺段 → None（不猜、不回退绝对值）
fn single_core_power(cluster: CoreGroup, n: usize) -> Option<(Vec<u32>, Vec<f32>)> {
    let base = common::soc_frontier_policy()?.per_cluster.as_ref()?;
    let c = match cluster {
        CoreGroup::Little => &base.little,
        CoreGroup::Big => &base.big,
        CoreGroup::Prime => &base.prime,
    };
    if c.freq_khz.is_empty() {
        return None;
    }
    let freqs = c.freq_khz.clone();
    if c.single_core_power_w.len() == freqs.len() {
        let v = c.single_core_power_w.clone();
        return (!v.iter().all(|&p| p <= 0.0)).then_some((freqs, v));
    }
    if c.power_w.len() != freqs.len() {
        return None;
    }
    let n = n.max(1) as f32;
    let v: Vec<f32> = c.power_w.iter().map(|&p| p / n).collect();
    (!v.iter().all(|&p| p <= 0.0)).then_some((freqs, v))
}

/// 全局标定 `(P_REF, CAP_REF)`：三簇各自单核动态范围的**最大者**与单核 capacity 的最大者。
/// 只求「同一把尺子」，不求物理真值。
fn refs() -> Option<(f32, f32)> {
    static REFS: OnceLock<Option<(f32, f32)>> = OnceLock::new();
    *REFS.get_or_init(|| {
        const ALL: [CoreGroup; 3] = [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime];
        let mut p_ref = 0.0f32;
        let mut cap_ref = 0.0f32;
        for c in ALL {
            if let Some((_, p)) = single_core_power(c, cluster_cores(c)) {
                // 参考 = 全 SoC **最大单核绝对功耗**（不是动态跨度）：跨簇比较必须保留静态项，
                // 否则「开一颗核」在便宜簇上近乎免费，比较失去意义
                p_ref = p_ref.max(p.iter().copied().fold(0.0f32, f32::max));
            }
            cap_ref = cap_ref.max(cluster_cap(c).unwrap_or(0.0));
        }
        (p_ref > 0.0 && cap_ref > 0.0).then_some((p_ref, cap_ref))
    })
}

/// 本 SoC 的算力标定（CAP_REF）：需求折算的分母
pub fn cap_ref() -> f32 {
    refs().map(|(_, c)| c).unwrap_or(0.0)
}

/// 构造单簇曲线
fn build(cluster: CoreGroup, p_ref: f32, cap_ref: f32) -> Option<Curve> {
    let n = cluster_cores(cluster);
    let (freqs, power) = single_core_power(cluster, n)?;
    let cap = cluster_cap(cluster)?;
    if freqs.len() < 2 || power.len() != freqs.len() {
        return None;
    }
    let f_max = *freqs.last()? as f32;
    if f_max <= 0.0 || p_ref <= 0.0 || cap_ref <= 0.0 {
        return None;
    }
    // 两套向量，用途严格分开：
    // * `*_rel`：**簇内**归一（首档扣到 0、末档拉到 1），只用于求甜点——它是纯形状量，
    //   任何正常数缩放都不改变结论。
    // * `*_glob`：**跨簇**统一标定（功耗 ÷ 全 SoC 最大单核功耗，算力 ÷ 最大单核 capacity），
    //   用于「同算力下取更低功耗」的跨簇比较。
    //   ⚠ 这里**绝不能扣首档基线**：首档功耗就是该簇的静态开销，扣掉等于让「开一颗核」免费，
    //   prime 会因为动态增量小而永远胜出（回归已验证过这个错误口径）。
    let p_base = power[0];
    let p_span = (power[power.len() - 1] - p_base).max(f32::EPSILON);
    let mut cap_rel = Vec::with_capacity(freqs.len());
    let mut pow_rel = Vec::with_capacity(freqs.len());
    let mut cap_glob = Vec::with_capacity(freqs.len());
    let mut pow_glob = Vec::with_capacity(freqs.len());
    for (i, &f) in freqs.iter().enumerate() {
        let rel = f as f32 / f_max;
        cap_rel.push(rel);
        pow_rel.push((power[i] - p_base).max(0.0) / p_span);
        cap_glob.push(cap * rel / cap_ref);
        // 绝对单核功耗直接除以全局参考：静态项（首档）如实保留在向量里
        pow_glob.push(power[i].max(0.0) / p_ref);
    }
    // `cap_rel`/`pow_rel` 是**构建期中间量**：只用于求甜点，求完即弃。
    // 不进结构体——它们没有运行期消费者，留着就是两个永不读取的 Box（死重）。
    let sweet = sweet_index(&cap_rel, &pow_rel);
    Some(Curve {
        freqs: freqs.into_boxed_slice(),
        cap_glob: cap_glob.into_boxed_slice(),
        pow_glob: pow_glob.into_boxed_slice(),
        sweet,
        n_cores: n,
    })
}

/// 该簇曲线（进程级 OnceLock，三簇各自惰性构造）
pub fn curve(cluster: CoreGroup) -> Option<&'static Curve> {
    static LITTLE: OnceLock<Option<Curve>> = OnceLock::new();
    static BIG: OnceLock<Option<Curve>> = OnceLock::new();
    static PRIME: OnceLock<Option<Curve>> = OnceLock::new();
    let cell = match cluster {
        CoreGroup::Little => &LITTLE,
        CoreGroup::Big => &BIG,
        CoreGroup::Prime => &PRIME,
    };
    cell.get_or_init(|| refs().and_then(|(p, c)| build(cluster, p, c)))
        .as_ref()
}

/// 能效模型是否可用（任一簇有曲线）
pub fn available() -> bool {
    const ALL: [CoreGroup; 3] = [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime];
    ALL.iter().any(|&c| curve(c).is_some())
}

/// 需求折算：**只累加 `cpus` 指定的核**，逐核 `util × capacity ÷ CAP_REF`（CAP_REF 单位）。
///
/// `cpus` 必须由调用方给出本簇的核集合（CLG 的 `affected_cpus`）。**不能遍历整个
/// `core_utils`**：CLG worker 是按 cpufreq policy（= 单簇）划分的，若把整机 util 都算进来，
/// 每个簇都会以为自己要承担全 SoC 的需求——三簇 SoC 上需求会被放大近三倍，
/// 全部簇一起冲到顶档（2026-10-22 审查发现并修正）。
///
/// 任一贡献核容量缺失 → `None`（整段回退，不猜）。
pub fn demand_units(cpus: &[usize], core_utils: &[f32], core_caps: &[Option<u32>]) -> Option<f32> {
    let cap_ref = cap_ref();
    if cap_ref <= 0.0 || cpus.is_empty() {
        return None;
    }
    let mut sum = 0.0f32;
    for &cpu in cpus {
        let Some(&u) = core_utils.get(cpu) else {
            continue;
        };
        if !(u > 0.0) {
            continue;
        }
        let cap = core_caps.get(cpu).copied().flatten()? as f32;
        sum += u.min(1.0) * cap / cap_ref;
    }
    Some(sum)
}

/// 单线程需求折算：`util% × 该核 capacity ÷ CAP_REF`
pub fn thread_demand(util_pct: f32, core_cap: f32) -> f32 {
    let cr = cap_ref();
    if cr <= 0.0 || !util_pct.is_finite() || !core_cap.is_finite() || util_pct <= 0.0 {
        return 0.0;
    }
    (util_pct / 100.0).min(1.0) * core_cap.max(0.0) / cr
}

/// 该簇「整簇升频」的占用门槛（**核当量**）：`ceil(n/2)`。
///
/// 口径 = 用户要求的「单个线程占据了该核心簇的 50%（向上取整）」：
/// 以**核当量**衡量本簇被占掉多少算力（`Σ util`，1.0 = 占满一颗核），
/// 达到 `ceil(n/2)` 即视为「这簇已经被吃了一半以上」。
///
/// * 未达门槛 → 占用还稀，升频会让其余空闲核白吃漏电；
///   正确动作是**把线程迁走 / 拉别的核进来分摊**（优先迁移，其次升频）。
/// * 达门槛 → 迁移已无处可去，此时抬整簇频率才是唯一有效动作。
///
/// 6 核 big 簇 = 3.0 核当量（单线程最多 1.0，永远不会单独触发 → 单线程一律先迁移）；
/// 2 核 prime 簇 = 1.0 核当量（单线程跑满即触发 → prime 上直接升频，符合物理：
/// 频率域共享，别无他法）。
pub fn cluster_busy_gate(cluster: CoreGroup) -> f32 {
    cluster_cores(cluster).max(1).div_ceil(2) as f32
}

/// 该簇在给定需求下的目标档（相对前沿的调频落点）。
///
/// * `allow_above_sweet == false`：只在甜点及以下找档；甜点档不够 → 返回甜点档
///   （**压住频率**，把「不够」交给迁移去解决 —— 这正是「优先迁移而非升频」）
/// * `allow_above_sweet == true`：允许越过甜点，取刚好够用的最低档
pub fn gear_for_demand(
    cluster: CoreGroup,
    demand: f32,
    cores: usize,
    allow_above_sweet: bool,
) -> Option<usize> {
    let c = curve(cluster)?;
    if allow_above_sweet {
        Some(c.gear_for(demand, cores))
    } else {
        Some(c.gear_below_sweet_for(demand, cores).unwrap_or(c.sweet()))
    }
}

/// 相对前沿查询：在候选簇中取「同算力下成本最低」的放置。
///
/// * 供给必须 ≥ 需求；都不够时返回供给最大的那个（调用方据此判断「已到硬件上限」）
/// * `hysteresis`：相对成本改善不足该比例则不切换（防震荡）；0 = 只要有改善就切
pub fn best_placement(
    demand: f32,
    online: &[(CoreGroup, usize)],
    only: Option<&[CoreGroup]>,
    hysteresis: f32,
) -> Option<Placement> {
    if !demand.is_finite() || demand <= 0.0 {
        return None;
    }
    let mut best: Option<Placement> = None;
    for &(cluster, n_online) in online {
        if let Some(allow) = only
            && !allow.contains(&cluster)
        {
            continue;
        }
        let Some(c) = curve(cluster) else {
            continue;
        };
        // 单线程只占 1 核（`n_online` 只用于判断该簇是否在线，不参与供给计算）：
        // 先问「1 核在甜点档够不够」，不够才升档
        let _ = n_online;
        let gear = c
            .gear_below_sweet_for(demand, 1)
            .unwrap_or_else(|| c.gear_for(demand, 1));
        let cand = Placement {
            cluster,
            gear,
            freq_khz: c.freq_khz(gear),
            cores: 1,
            cost: c.cost(gear, 1),
            supply: c.supply(gear, 1),
        };
        best = Some(match best {
            None => cand,
            Some(b) => {
                let b_ok = b.supply >= demand;
                let c_ok = cand.supply >= demand;
                match (b_ok, c_ok) {
                    (true, false) => b,
                    (false, true) => cand,
                    _ => {
                        let gain = (b.cost - cand.cost) / b.cost.max(f32::EPSILON);
                        if cand.cost < b.cost && gain >= hysteresis {
                            cand
                        } else {
                            b
                        }
                    }
                }
            }
        });
    }
    best
}

/// 跨簇切换的默认滞回（相对成本改善比例）：省不到 15% 不动，防两簇边界抖动
pub const SWITCH_HYSTERESIS: f32 = 0.15;

/// 当前在线且拓扑非空的三簇（`(簇, 在线核数)`）：放置/迁移共用的候选集
pub fn online_clusters() -> Vec<(CoreGroup, usize)> {
    const ALL: [CoreGroup; 3] = [CoreGroup::Little, CoreGroup::Big, CoreGroup::Prime];
    let r = common::chiri_core_ranges();
    let mut out = Vec::with_capacity(3);
    for c in ALL {
        let span = match c {
            CoreGroup::Little => &r.little,
            CoreGroup::Big => &r.big,
            CoreGroup::Prime => &r.prime,
        };
        if span.is_empty() || curve(c).is_none() {
            continue;
        }
        out.push((c, span.len()));
    }
    out
}

// [range]

/// 线程可占核心区间的迁移阶段
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangePhase {
    /// 稳态：掩码 = 目标区间
    Steady,
    /// 正在**扩大**：掩码 = 旧区间 ∪ 新区间（先并集，保证迁移途中不被饿死）
    Expanding,
}

/// 线程的核心区间状态：`[lo, hi]` 为**目标**区间，`phase` 决定当前实际掩码
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeState {
    pub lo: usize,
    pub hi: usize,
    pub phase: RangePhase,
}

impl RangeState {
    pub fn new(lo: usize, hi: usize) -> Self {
        Self {
            lo,
            hi: hi.max(lo),
            phase: RangePhase::Steady,
        }
    }

    /// 当前应写入的掩码
    pub fn mask(&self) -> Vec<usize> {
        (self.lo..=self.hi).collect()
    }
}

/// 区间迁移一步（**先扩大再缩小**）。
///
/// 目标区间从 `[lo,hi]` 变到 `[want_lo,want_hi]`，按三段走：
/// 1. `Steady` + 目标不同 → 写入**并集**掩码，进入 `Expanding`
///    （例 `0-2 → 0-3`：先给 `0-3`，让调度器有地方搬）
/// 2. `Expanding` 且并集掩码已写入 → 本步收敛到目标区间（`0-3 → 1-3`）
///    （旧核此时才被摘掉；摘之前线程早已有机会迁走，不会卡在「无核可用」）
/// 3. 目标未变 → `Steady`，掩码 = 目标
///
/// 返回 `(新状态, 是否发生了阶段推进)`；调用方按「推进了才写 sysfs」去重。
/// 上行（低位→高位）与下行（高位→低位）共用本函数，只是 `want_lo/want_hi` 方向不同。
pub fn step_range(state: &mut RangeState, want_lo: usize, want_hi: usize) -> bool {
    let want_hi = want_hi.max(want_lo);
    match state.phase {
        RangePhase::Expanding => {
            // 并集已下发，收敛到目标（缩小到目标区间 = 摘掉旧核）
            state.lo = want_lo;
            state.hi = want_hi;
            state.phase = RangePhase::Steady;
            true
        }
        RangePhase::Steady => {
            if state.lo == want_lo && state.hi == want_hi {
                false
            } else {
                // 先扩：把新旧区间的并集作为过渡掩码
                state.lo = state.lo.min(want_lo);
                state.hi = state.hi.max(want_hi);
                state.phase = RangePhase::Expanding;
                true
            }
        }
    }
}

/// 迁移方向（仅用于日志口径：INFO 只写「线程 + 方向」）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// 低位核心 → 高位核心（如 0-2 → 1-3）
    Up,
    /// 高位核心 → 低位核心（如 2-4 → 1-3）
    Down,
    /// 区间边界不变（仅扩缩核数）
    Hold,
}

impl Dir {
    pub fn as_str(self) -> &'static str {
        match self {
            Dir::Up => "up",
            Dir::Down => "down",
            Dir::Hold => "hold",
        }
    }
}

/// 由新旧区间判方向：**两端同向移动**才算 Up/Down，单端移动（纯扩张/纯收缩）= Hold。
///
/// 口径：日志里的方向描述的是「线程往哪个方向搬了」，不是「区间变宽了」。
/// `0-2 → 1-3` 两端各 +1 → Up；`2-4 → 1-3` 两端各 −1 → Down；
/// `0-2 → 0-3`（只扩上界）→ Hold，它没搬，只是窗口变大。
pub fn direction(from_lo: usize, from_hi: usize, to_lo: usize, to_hi: usize) -> Dir {
    let d_lo = (to_lo as isize - from_lo as isize).signum();
    let d_hi = (to_hi as isize - from_hi as isize).signum();
    match (d_lo, d_hi) {
        (1, 1) => Dir::Up,
        (-1, -1) => Dir::Down,
        _ => Dir::Hold,
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用曲线：`cap_rel`/`pow_rel` 只是**构建期中间量**，不进结构体，
    /// 这里按同样口径（cap 直接用、pow 用传入形状）构造等价对象。
    fn mk(cap: &[f32], pow: &[f32]) -> Curve {
        let freqs: Vec<u32> = (0..cap.len()).map(|i| 1000 * (i as u32 + 1)).collect();
        Curve {
            freqs: freqs.into_boxed_slice(),
            cap_glob: cap.to_vec().into_boxed_slice(),
            pow_glob: pow.to_vec().into_boxed_slice(),
            sweet: sweet_index(cap, pow),
            n_cores: 4,
        }
    }

    /// 算力线性 + 功耗平方 → 边际斜率 1/(2x)，在 x=0.5 处 == 1 → 甜点落在中点档
    #[test]
    fn sweet_is_where_unit_slope_turns() {
        let n = 9usize;
        let cap: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        let pow: Vec<f32> = cap.iter().map(|x| x * x).collect();
        assert_eq!(sweet_index(&cap, &pow), 4);
    }

    /// 相对向量的核心性质：**归一化后**，原始瓦数的任意正常数缩放都不改变甜点
    /// （只缩放功耗而不缩放算力会改变两轴币值，结论本就该变——不是不变性）
    #[test]
    fn sweet_is_scale_invariant() {
        let norm = |p: &[f32]| -> Vec<f32> {
            let base = p[0];
            let span = (p[p.len() - 1] - base).max(f32::EPSILON);
            p.iter().map(|&x| (x - base).max(0.0) / span).collect()
        };
        let cap: Vec<f32> = (0..8).map(|i| i as f32 / 7.0).collect();
        let pow: Vec<f32> = cap.iter().map(|x| x * x).collect();
        let base = sweet_index(&cap, &norm(&pow));
        for k in [37.5f32, 0.001, 1024.0] {
            let scaled: Vec<f32> = pow.iter().map(|p| p * k).collect();
            assert_eq!(
                base,
                sweet_index(&cap, &norm(&scaled)),
                "缩放 {k} 改变了甜点"
            );
        }
    }

    /// 平坦功耗段 = 无限边际能效（免费提速），甜点会越过它
    #[test]
    fn flat_power_segment_is_free() {
        // 前两段零功耗增量 → 免费；第三段 dc/dp = 0.25/0.5 = 0.5 < 1 → 甜点停在第 2 档
        let cap = [0.0, 0.25, 0.5, 0.75, 1.0];
        let pow = [0.0, 0.0, 0.0, 0.5, 1.0];
        assert_eq!(sweet_index(&cap, &pow), 2);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let c = mk(&[0.25, 0.5, 1.0], &[0.0, 0.4, 1.0]);
        assert_eq!(c.gear_for(0.0, 1), 0);
        assert_eq!(c.gear_for(f32::NAN, 1), 0);
        assert!(c.gear_for(99.0, 1) <= c.top());
        assert_eq!(c.gear_below_sweet_for(99.0, 1), None);
        assert_eq!(c.cost(2, 3), 3.0);
        assert_eq!(c.supply(2, 3), 3.0);
        assert_eq!(c.freq_khz(999), 0);
    }

    /// 甜点以下不加频：需求小的时候落点必须 ≤ 甜点档
    #[test]
    fn never_exceeds_sweet_for_small_demand() {
        let c = mk(&[0.25, 0.5, 0.75, 1.0], &[0.0, 0.25, 0.75, 1.0]);
        let sweet = c.sweet();
        for d in [0.1f32, 0.2, 0.3, 0.4] {
            let g = c.gear_below_sweet_for(d, 1).unwrap_or(sweet);
            assert!(g <= sweet, "d={d} g={g} sweet={sweet}");
        }
    }

    /// 区间滑窗：上行必须先并集再收敛（0-2 → 0-3 → 1-3）
    #[test]
    fn range_expands_then_shrinks_upward() {
        let mut s = RangeState::new(0, 2);
        assert!(step_range(&mut s, 1, 3));
        assert_eq!(s.mask(), vec![0, 1, 2, 3], "过渡掩码必须是并集");
        assert_eq!(s.phase, RangePhase::Expanding);
        assert!(step_range(&mut s, 1, 3));
        assert_eq!(s.mask(), vec![1, 2, 3], "收敛后才摘掉旧核 0");
        assert_eq!(s.phase, RangePhase::Steady);
        assert!(!step_range(&mut s, 1, 3), "目标未变不再写");
    }

    /// 区间滑窗：下行同样先并集再收敛（2-4 → 1-4 → 1-3）
    #[test]
    fn range_expands_then_shrinks_downward() {
        let mut s = RangeState::new(2, 4);
        assert!(step_range(&mut s, 1, 3));
        assert_eq!(s.mask(), vec![1, 2, 3, 4]);
        assert!(step_range(&mut s, 1, 3));
        assert_eq!(s.mask(), vec![1, 2, 3]);
    }

    /// 中间不允许出现「空掩码」或「丢掉旧核又还没拿到新核」的窗口
    #[test]
    fn transition_mask_is_always_superset() {
        let mut s = RangeState::new(0, 1);
        for target in [(0usize, 2usize), (0, 3), (1, 3), (1, 2)] {
            let before: Vec<usize> = s.mask();
            step_range(&mut s, target.0, target.1);
            let during = s.mask();
            for c in &before {
                assert!(during.contains(c), "过渡丢了旧核 {c}: {during:?}");
            }
            step_range(&mut s, target.0, target.1);
            assert_eq!(s.mask(), (target.0..=target.1).collect::<Vec<_>>());
        }
    }

    #[test]
    fn direction_detects_up_and_down() {
        assert_eq!(direction(0, 2, 1, 3), Dir::Up);
        assert_eq!(direction(2, 4, 1, 3), Dir::Down);
        assert_eq!(direction(0, 2, 0, 3), Dir::Hold);
    }
}
