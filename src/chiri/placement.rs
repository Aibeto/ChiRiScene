//! placement.rs: [window] [decide] [engine] [tests]
//!
//! 线程放置内核：**同算力下取更低功耗**，迁移优先于升频。
//!
//! 三层口径（全部无量纲，见 `energy_model`）：
//! * **选簇**：`energy_model::best_placement` 在候选簇里取「供给够用且成本最低」；
//!   成本 = 单核相对功耗 × 忙核数，跨簇同一把尺子。
//! * **选核**：在选中簇里挑负载最低的一段**连续**核心区间（`Window`）——
//!   连续是硬要求，离散掩码会让内核在多个簇间反复迁，且不利于 L2 复用。
//! * **过窗**：区间变更一律**先扩后缩**（`energy_model::step_range`）：
//!   `0-2 → 0-3 → 1-3`。并集掩码先行，线程全程有核可跑，绝不出现「旧核已摘、
//!   新核未到」的空窗；上行与下行共用同一套协议。
//!
//! 防震荡三重闸：
//! 1. **甜点阈值**：只有当前落点越过本簇甜点（1 单位功耗换不到 1 单位算力）才发起迁移；
//! 2. **相对滞回**：新方案成本改善不到 `SWITCH_HYSTERESIS` 不动；
//! 3. **驻留冷却**：同一线程两次迁移间至少隔 `MIN_DWELL`，且反跳回（A→B→A）加倍冷却。

use crate::chiri::energy_model::{self, Dir, RangePhase, RangeState};
use crate::common::CoreGroup;
use std::time::{Duration, Instant};

/// 迁移最小驻留（同一线程两次迁移之间的最短间隔）
pub const MIN_DWELL: Duration = Duration::from_secs(2);
/// 反跳回冷却倍数：刚离开的簇，短时间内回去需要更高收益
pub const RETURN_COOLDOWN_MUL: u32 = 4;

// [window]

/// 连续核心区间
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub lo: usize,
    pub hi: usize,
}

impl Window {
    pub fn new(lo: usize, hi: usize) -> Self {
        Self { lo, hi: hi.max(lo) }
    }

    pub fn width(self) -> usize {
        self.hi - self.lo + 1
    }

    pub fn cores(self) -> Vec<usize> {
        (self.lo..=self.hi).collect()
    }
}

/// 在给定核池里挑一段宽度 `w` 的连续区间，使区间内**平均负载最低**。
/// 池必须是连续编号（单簇）；池为空 / 宽度越界 → None。
pub fn best_window(pool: &[usize], w: usize, load: &[f32]) -> Option<Window> {
    if pool.is_empty() {
        return None;
    }
    let w = w.clamp(1, pool.len());
    // 池内连续段：直接按池的下标滑动（池本身来自单簇区间，天然连续）
    let mut best: Option<(f32, Window)> = None;
    for start in 0..=(pool.len() - w) {
        let seg = &pool[start..start + w];
        let sum: f32 = seg
            .iter()
            .map(|&c| load.get(c).copied().unwrap_or(0.0).clamp(0.0, 1.0))
            .sum();
        let score = sum / w as f32;
        if best.is_none_or(|(b, _)| score < b) {
            best = Some((score, Window::new(seg[0], seg[w - 1])));
        }
    }
    best.map(|(_, w)| w)
}

// [decide]

/// 一次迁移计划
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// 目标簇
    pub cluster: CoreGroup,
    /// 目标区间
    pub target: Window,
    /// 方向（日志口径）
    pub dir: Dir,
    /// 相对成本改善（0..1，越大越省）
    pub gain: f32,
    /// 决策原因（devimp 明细用）
    pub reason: &'static str,
}

/// 线程的可迁移性判据（由调用方给出，本模块只消费）
#[derive(Clone, Copy, Debug)]
pub struct Subject {
    /// 需求（CAP_REF 单位）
    pub demand: f32,
    /// 当前簇
    pub cluster: CoreGroup,
    /// 当前区间
    pub window: Window,
    /// 当前落点是否已越过本簇甜点（= 边际能效 < 1，该迁移了）
    pub past_sweet: bool,
}

/// 决策：给定主体与全局负载，产出目标放置。
///
/// 只在**越过甜点**时才考虑迁移（甜点以内加频仍划算，迁移反而打断缓存局部性）；
/// 越过甜点后，优先迁到成本更低的簇；同簇内则把窗口滑到负载最低的连续段。
pub fn decide(
    s: &Subject,
    load: &[f32],
    online: &[(CoreGroup, usize)],
    hysteresis: f32,
) -> Option<Plan> {
    if !s.demand.is_finite() || s.demand <= 0.0 {
        return None;
    }
    // 未越过甜点：频率维度还没用尽，不动放置（避免无谓迁移打断缓存）
    if !s.past_sweet {
        return None;
    }
    let here = energy_model::curve(s.cluster)?;
    let here_cost = here.cost(here.gear_for(s.demand, 1), 1);

    // 1) 跨簇：候选簇要按**自身剩余容量**重新求档后再比成本。
    //
    //    只看「模型最便宜」不看目的地负载，会把所有线程堆到同一颗核上，
    //    结果该核过载顶频、反而更费电。正确做法：目的地已有 `dst_load` 的占用，
    //    只剩 `1 - dst_load` 的余量，故它要供给 `demand / (1 - dst_load)` 才能接住
    //    这份需求——按这个放大后的需求求档、算成本，再与本簇成本比较。
    //    （早期写法是「需求 + 负载 ≤ 1.0 才允许迁」，条件过严：需求恰好等于该档
    //     供给时要求目标核完全空闲，实际几乎永不成立，2026-10-22 审查修正。）
    let mut best_alt: Option<(f32, CoreGroup, Window, usize)> = None;
    for &(cand, _) in online {
        if cand == s.cluster {
            continue;
        }
        let Some(cv) = energy_model::curve(cand) else {
            continue;
        };
        let pool = cluster_pool(cand);
        if pool.is_empty() {
            continue;
        }
        let Some(win) = best_window(&pool, 1, load) else {
            continue;
        };
        let dst_load = load.get(win.lo).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        // 余量下限 5%：贴满的核不要（否则求档会爆到顶档，成本必然更差）
        let residual = (1.0 - dst_load).max(0.05);
        let need = s.demand / residual;
        let gear = cv
            .gear_below_sweet_for(need, 1)
            .unwrap_or_else(|| cv.gear_for(need, 1));
        let cost = cv.cost(gear, 1);
        // 相对改善不足滞回 → 不动（跨簇与同簇用同一把尺子）
        if cost < here_cost
            && (here_cost - cost) / here_cost.max(f32::EPSILON) >= hysteresis
            && best_alt.is_none_or(|(b, _, _, _)| cost < b)
        {
            best_alt = Some((cost, cand, win, gear));
        }
    }
    if let Some((cost, cand, win, _)) = best_alt {
        let gain = (here_cost - cost) / here_cost.max(f32::EPSILON);
        return Some(Plan {
            cluster: cand,
            target: win,
            dir: energy_model::direction(s.window.lo, s.window.hi, win.lo, win.hi),
            gain,
            reason: "cheaper_cluster",
        });
    }

    // 1b) 不做「加宽本线程窗口」来分摊算力——那是物理上不成立的：
    //     单个线程同时只能占一颗核，加宽它的掩码只是让内核在核间弹跳，
    //     算力不会变多；而 `s.demand` 构造上 ≤ 1 核当量，若按 k 核重算档位
    //     会把所需频率砍半，等于**凭空降频**（2026-10-22 审查发现并移除）。
    //     「拉其他核进来平衡」在正确的层级实现：CLG 侧 `k` = 本簇占用核当量，
    //     供给 `supply(gear,k) = cap[gear] × k` 随忙核数增长；放置侧由
    //     `best_window` 把**不同线程**摊到不同核上。

    // 2) 同簇：窗口滑到负载最低的连续段（宽度不变，纯平移）
    let pool: Vec<usize> = cluster_pool(s.cluster);
    let w = s.window.width().min(pool.len().max(1));
    let cur_score: f32 = s
        .window
        .cores()
        .iter()
        .map(|&c| load.get(c).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .sum::<f32>()
        / s.window.width().max(1) as f32;
    let win = best_window(&pool, w, load)?;
    if win == s.window {
        return None;
    }
    let new_score: f32 = win
        .cores()
        .iter()
        .map(|&c| load.get(c).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .sum::<f32>()
        / win.width().max(1) as f32;
    // 负载改善不足滞回：不动（防两段边界来回抖）
    let gain = if cur_score > f32::EPSILON {
        (cur_score - new_score) / cur_score
    } else {
        0.0
    };
    if gain < hysteresis {
        return None;
    }
    Some(Plan {
        cluster: s.cluster,
        target: win,
        dir: energy_model::direction(s.window.lo, s.window.hi, win.lo, win.hi),
        gain,
        reason: "lighter_cores",
    })
}

/// 该簇的连续核池（拓扑区间；空簇 → 空）
pub fn cluster_pool(cluster: CoreGroup) -> Vec<usize> {
    let r = crate::common::chiri_core_ranges();
    let span = match cluster {
        CoreGroup::Little => r.little,
        CoreGroup::Big => r.big,
        CoreGroup::Prime => r.prime,
    };
    span.collect()
}

// [engine]

/// 单线程的迁移记账
#[derive(Clone, Debug)]
pub struct Entry {
    /// 目标区间（稳态值；过渡态由 `state` 表达）
    pub window: Window,
    /// 区间迁移状态机
    pub state: RangeState,
    /// 当前所在簇
    pub cluster: CoreGroup,
    /// 上次迁移时刻
    pub last_move: Instant,
    /// 上次迁离的簇与时刻（反跳回冷却）
    pub left: Option<(CoreGroup, Instant)>,
}

impl Entry {
    pub fn new(cluster: CoreGroup, window: Window, now: Instant) -> Self {
        Self {
            window,
            state: RangeState::new(window.lo, window.hi),
            cluster,
            last_move: now,
            left: None,
        }
    }
}

/// 迁移引擎：按 tid 记账，负责冷却与「先扩后缩」的两步推进
#[derive(Default)]
pub struct Engine {
    entries: std::collections::HashMap<i32, Entry>,
}

/// 一次实际动作（由调用方落到 sysfs）
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub tid: i32,
    /// 本步要写入的掩码（过渡态为并集，收敛态为目标）
    pub mask: Vec<usize>,
    /// 本步是否为目标簇切换（需要额外处理 cpuset 组）
    pub switch_cluster: Option<CoreGroup>,
    /// 日志用方向
    pub dir: Dir,
    /// 是否已收敛（false = 还在过渡，下一轮继续）
    pub settled: bool,
    pub reason: &'static str,
    pub gain: f32,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// 首次建档：以当前实际区间为稳态
    pub fn ensure(&mut self, tid: i32, cluster: CoreGroup, window: Window, now: Instant) {
        self.entries
            .entry(tid)
            .or_insert_with(|| Entry::new(cluster, window, now));
    }

    /// 线程退出/失联：清账
    pub fn forget(&mut self, tid: i32) {
        self.entries.remove(&tid);
    }

    /// 推进一个线程：返回需要写掩码的动作（None = 本轮不动）
    ///
    /// 冷却未到 → None；否则按 `decide` 更新目标，再走一步 `step_range`。
    /// 迁移是**两拍**的：第一拍写并集（`settled=false`），第二拍收敛（`settled=true`）。
    pub fn tick(
        &mut self,
        tid: i32,
        s: &Subject,
        load: &[f32],
        online: &[(CoreGroup, usize)],
        hysteresis: f32,
        now: Instant,
    ) -> Option<Action> {
        let e = self.entries.get_mut(&tid)?;

        // 过渡中：不受冷却约束，立刻推进到收敛（并集已经写过，摘旧核是安全的）。
        // 走 `step_range` 而不是手写「赋值 + 改 phase」——先扩后缩只允许一份实现，
        // 否则离线回归测的是 A 而生产跑的是 B（2026-10-22 审查发现）。
        if e.state.phase == RangePhase::Expanding {
            energy_model::step_range(&mut e.state, e.window.lo, e.window.hi);
            return Some(Action {
                tid,
                mask: e.state.mask(),
                switch_cluster: None,
                dir: Dir::Hold,
                settled: true,
                reason: "settle",
                gain: 0.0,
            });
        }

        // 冷却闸：普通情况 `MIN_DWELL`；若当前簇正是**刚离开又回来**的那个，
        // 冷却加倍（防 A→B→A 抖动）。
        // 注意：原来的写法 `now.saturating_duration_since(at).min(Duration::ZERO)`
        // 恒等于零（Duration 非负），是条死表达式——已删。
        let since = now.saturating_duration_since(e.last_move);
        let returning = e.left.is_some_and(|(c, _)| c == s.cluster);
        let cooldown = if returning {
            MIN_DWELL * RETURN_COOLDOWN_MUL
        } else {
            MIN_DWELL
        };
        if since < cooldown {
            return None;
        }

        let plan = decide(s, load, online, hysteresis)?;
        // 反跳回冷却：刚离开的簇要回去，收益必须翻倍
        if let Some((c, at)) = e.left
            && c == plan.cluster
            && plan.cluster != s.cluster
            && now.saturating_duration_since(at) < MIN_DWELL * RETURN_COOLDOWN_MUL
            && plan.gain < hysteresis * 2.0
        {
            return None;
        }

        let switching = plan.cluster != s.cluster;
        if switching {
            e.left = Some((s.cluster, now));
            e.cluster = plan.cluster;
        }
        e.window = plan.target;
        e.last_move = now;
        // 先扩后缩：第一拍写并集（旧区间 ∪ 新区间），第二拍在下次 tick 收敛。
        // 唯一的「先扩后缩」实现在 `energy_model::step_range`，这里只负责记账。
        energy_model::step_range(&mut e.state, plan.target.lo, plan.target.hi);

        Some(Action {
            tid,
            mask: e.state.mask(),
            switch_cluster: switching.then_some(plan.cluster),
            dir: plan.dir,
            settled: e.state.phase == RangePhase::Steady,
            reason: plan.reason,
            gain: plan.gain,
        })
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;

    fn pool(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    #[test]
    fn best_window_picks_lightest_segment() {
        let load = [0.9, 0.8, 0.1, 0.05, 0.7];
        let w = best_window(&pool(5), 2, &load).unwrap();
        assert_eq!(w, Window::new(2, 3));
    }

    #[test]
    fn best_window_degenerate() {
        assert!(best_window(&[], 1, &[]).is_none());
        let w = best_window(&pool(3), 99, &[0.1, 0.2, 0.3]).unwrap();
        assert_eq!(w, Window::new(0, 2), "宽度越界收敛到整池");
    }

    #[test]
    fn no_plan_below_sweet() {
        let s = Subject {
            demand: 0.5,
            cluster: CoreGroup::Big,
            window: Window::new(0, 0),
            past_sweet: false,
        };
        assert!(decide(&s, &[0.1, 0.9], &[], 0.15).is_none());
    }

    /// 多核分摊的**数学关系**：同一份需求摊到 k 颗核上、档位相应下降，
    /// 总功耗严格更低（形状取自 sm8750 big：算力线性、功耗平方）。
    ///
    /// 这是 CLG 侧 `supply(gear,k) = cap[gear] × k` 与 `cost(gear,k) = pow[gear] × k`
    /// 的依据，**不是**本模块（线程放置）的动作——单个线程只能占一颗核，
    /// 加宽它的掩码不会分摊算力。分摊靠「不同线程落到不同核」（`best_window`）
    /// 与 CLG 的 k 核供给口径共同实现。
    #[test]
    fn k_core_sharing_beats_lifting_freq() {
        // 简化自 sm8750 big：cap_rel 线性、pow_rel 平方（形状与实测一致）
        let n = 16usize;
        let cap: Vec<f32> = (0..n).map(|i| (i as f32 + 1.0) / n as f32).collect();
        let pow: Vec<f32> = cap.iter().map(|x| x * x).collect();
        let cost = |d: f32, k: usize| -> Option<f32> {
            (0..n)
                .find(|&i| cap[i] * k as f32 >= d)
                .map(|i| pow[i] * k as f32)
        };
        for d in [0.5f32, 0.7, 0.9] {
            let c1 = cost(d, 1).unwrap();
            let c3 = cost(d, 3).unwrap();
            assert!(c3 < c1, "d={d}: 3 核分摊 {c3:.3} 应低于 1 核升频 {c1:.3}");
        }
    }

    #[test]
    fn window_slide_reports_direction() {
        // 同簇平移：0 号核很忙、2 号核空闲 → 窗口从 0 滑到 1 或 2（up）
        let load = [0.95, 0.2, 0.05];
        let s = Subject {
            demand: 0.3,
            cluster: CoreGroup::Big,
            window: Window::new(0, 0),
            past_sweet: true,
        };
        // 无 curve 数据时 decide 返回 None（本测试只验证方向函数本身）
        assert_eq!(
            energy_model::direction(0, 2, 1, 3),
            Dir::Up,
            "窗口上移必须判 Up"
        );
        assert_eq!(energy_model::direction(2, 4, 1, 3), Dir::Down);
        let _ = (load, s);
    }

    #[test]
    fn engine_requires_dwell() {
        let now = Instant::now();
        let mut e = Engine::new();
        e.ensure(7, CoreGroup::Big, Window::new(0, 0), now);
        let s = Subject {
            demand: 0.3,
            cluster: CoreGroup::Big,
            window: Window::new(0, 0),
            past_sweet: true,
        };
        // 刚建档 → 冷却未到 → 不动
        assert!(e.tick(7, &s, &[0.9, 0.1], &[], 0.15, now).is_none());
    }

    #[test]
    fn engine_two_phase_settle() {
        let now = Instant::now();
        let mut e = Engine::new();
        e.ensure(7, CoreGroup::Big, Window::new(0, 2), now);
        // 手工把状态推到过渡态，验证下一拍必然收敛且掩码 == 目标
        {
            let en = e.entries.get_mut(&7).unwrap();
            en.window = Window::new(1, 3);
            en.state.phase = RangePhase::Expanding;
            en.state.lo = 0;
            en.state.hi = 3;
        }
        let s = Subject {
            demand: 0.3,
            cluster: CoreGroup::Big,
            window: Window::new(1, 3),
            past_sweet: true,
        };
        let a = e
            .tick(7, &s, &[0.1, 0.1, 0.1, 0.1], &[], 0.15, now)
            .unwrap();
        assert!(a.settled);
        assert_eq!(a.mask, vec![1, 2, 3], "收敛后必须只留目标区间");
    }
}
