//! energy_model_regression.rs: [shim] [sweet] [range] [window]
//!
//! 相对能效模型与放置内核的离线回归：全部断言针对**纯函数**（甜点判据 / 区间滑窗 /
//! 选核），不触 sysfs。用最小 `common` 桩把两个真实源文件拉进来，保证测的是生产代码
//! 本身而不是副本。
//!
//! Run: `rustc --edition=2024 --test scripts/tests/energy_model_regression.rs -o /tmp/em_test`

// [shim]
/// 最小 `common` 桩：只提供 `energy_model` / `placement` 引用到的符号。
/// `soc_frontier_policy()` 恒 None = 「本 SoC 无 frontier 段」，故 `curve()` 全 None；
/// 被测函数都是不依赖曲线的纯逻辑。
pub mod common {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CoreGroup {
        Little,
        Big,
        Prime,
    }

    #[derive(Clone, Debug, Default)]
    pub struct CoreGroupRanges {
        pub little: std::ops::Range<usize>,
        pub big: std::ops::Range<usize>,
        pub prime: std::ops::Range<usize>,
    }

    /// 6 核 big + 2 核 prime、无 little（sm8750 形状）
    pub fn chiri_core_ranges() -> CoreGroupRanges {
        CoreGroupRanges {
            little: 0..0,
            big: 0..6,
            prime: 6..8,
        }
    }

    pub fn soc_capacity_for_group(g: CoreGroup) -> Option<u32> {
        match g {
            CoreGroup::Big => Some(762),
            CoreGroup::Prime => Some(1024),
            CoreGroup::Little => None,
        }
    }

    #[derive(Debug, Clone, Default)]
    pub struct FrontierClusterBase {
        pub capacity: Option<u32>,
        pub freq_khz: Vec<u32>,
        pub power_w: Vec<f32>,
        pub single_core_power_w: Vec<f32>,
    }

    #[derive(Debug, Clone, Default)]
    pub struct FrontierPerCluster {
        pub little: FrontierClusterBase,
        pub big: FrontierClusterBase,
        pub prime: FrontierClusterBase,
    }

    #[derive(Debug, Clone, Default)]
    pub struct FrontierPolicy {
        pub per_cluster: Option<FrontierPerCluster>,
    }

    /// 虚构一台 6×big + 2×prime 的 SoC（与 `chiri_core_ranges` 同形状）：
    /// 原先恒 `None` 让 `curve()` 全 None，只能测不依赖曲线的纯函数；给了这张表之后
    /// `build()`/`decide()`/`Engine::tick()` 都能离线跑，用于 [replay] 段的重放回归。
    /// 数值取自 sm8750 量级（Oryon-M 762 / Oryon-L 1024），不求物理真值、只求形状可比。
    pub fn soc_frontier_policy() -> Option<&'static FrontierPolicy> {
        static P: std::sync::OnceLock<Option<FrontierPolicy>> = std::sync::OnceLock::new();
        P.get_or_init(|| {
            let big = FrontierClusterBase {
                capacity: Some(762),
                freq_khz: vec![300_000, 600_000, 900_000, 1_200_000, 1_500_000, 1_800_000, 2_100_000],
                power_w: Vec::new(),
                single_core_power_w: vec![0.05, 0.09, 0.15, 0.24, 0.36, 0.52, 0.72],
            };
            let prime = FrontierClusterBase {
                capacity: Some(1024),
                freq_khz: vec![
                    400_000, 800_000, 1_200_000, 1_600_000, 2_000_000, 2_400_000, 2_800_000, 3_200_000,
                ],
                power_w: Vec::new(),
                single_core_power_w: vec![0.08, 0.15, 0.26, 0.42, 0.62, 0.88, 1.20, 1.60],
            };
            Some(FrontierPolicy {
                per_cluster: Some(FrontierPerCluster {
                    little: FrontierClusterBase::default(),
                    big,
                    prime,
                }),
            })
        })
        .as_ref()
    }
}

#[path = "../../src/chiri/energy_model.rs"]
pub mod energy_model;

#[path = "../../src/chiri/placement.rs"]
pub mod placement;

/// `placement` 内部按 `crate::chiri::energy_model` 引用同侪模块，这里补一层别名
pub mod chiri {
    pub use crate::energy_model;
}

use energy_model::{self as em, Dir, RangePhase, RangeState, direction, step_range};
use placement::{Window, best_window};

// [sweet]
/// 甜点 = `Δ算力/Δ功耗` 最后一次 ≥ 1.0 的档。算力线性、功耗平方时斜率 = 1/(2x)，
/// 在 x = 0.5 处穿过 1 → 落在中点档。
#[test]
fn sweet_point_is_unit_slope_boundary() {
    let n = 9usize;
    let cap: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
    let pow: Vec<f32> = cap.iter().map(|x| x * x).collect();
    assert_eq!(em::sweet_index(&cap, &pow), 4);
}

/// 相对向量的存在意义：**先归一化**之后，原始瓦数的任意正常数缩放都不改变甜点。
/// （注意：只缩放功耗向量而不缩放算力向量会改变两轴的币值，结论本就该变，
///   那是物理上不同的取舍，不是不变性。）
#[test]
fn sweet_point_is_scale_invariant() {
    // 模拟 build()：功耗先扣首档、再按跨度归一；算力按 f/f_max 归一
    let normalize = |p: &[f32]| -> Vec<f32> {
        let base = p[0];
        let span = (p[p.len() - 1] - base).max(f32::EPSILON);
        p.iter().map(|&x| (x - base).max(0.0) / span).collect()
    };
    let n = 8usize;
    let cap: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
    let pow: Vec<f32> = cap.iter().map(|x| x * x).collect();
    let pow_rel = normalize(&pow);
    let base = em::sweet_index(&cap, &pow_rel);
    for k in [0.001f32, 1.0, 37.5, 1024.0, 1e6] {
        let scaled: Vec<f32> = pow.iter().map(|p| p * k).collect();
        assert_eq!(
            base,
            em::sweet_index(&cap, &normalize(&scaled)),
            "瓦数量纲缩放 {k} 改变了甜点"
        );
    }
}

/// 完全平坦的功耗段 = 无限边际能效（免费提速），甜点应越过它停在该段末档
#[test]
fn flat_power_segment_is_free() {
    let cap = [0.0, 0.25, 0.5, 0.75, 1.0];
    let pow = [0.0, 0.0, 0.0, 0.5, 1.0];
    // 第 1、2 段 dp == 0 → 免费，甜点推到 2；第 3 段 dc/dp = 0.25/0.5 = 0.5 < 1 → 停
    assert_eq!(em::sweet_index(&cap, &pow), 2);
}

/// 退化输入不得 panic
#[test]
fn sweet_point_degenerate() {
    assert_eq!(em::sweet_index(&[], &[]), 0);
    assert_eq!(em::sweet_index(&[1.0], &[1.0]), 0);
    // 首段 0.5/0.9 = 0.56 < 1，次段 0.5/0.1 = 5 ≥ 1 → 甜点落在末档
    assert_eq!(
        em::sweet_index(&[0.0, 0.5, 1.0], &[0.0, 0.9, 1.0]),
        2
    );
    // 全程斜率 < 1（功耗涨得比算力快）→ 首档即最优
    assert_eq!(
        em::sweet_index(&[0.0, 0.5, 1.0], &[0.0, 0.9, 1.0]),
        2
    );
}

// [range]
/// 先扩后缩：过渡掩码必须始终是旧区间的**超集**，绝不出现「旧核已摘、新核未到」
#[test]
fn range_transition_never_drops_old_cores() {
    let mut s = RangeState::new(0, 1);
    for (lo, hi) in [(0usize, 2usize), (0, 3), (1, 3), (1, 2), (0, 0)] {
        let before = s.mask();
        step_range(&mut s, lo, hi);
        let during = s.mask();
        for c in &before {
            assert!(during.contains(c), "过渡丢核 {c}: {during:?} → 目标 {lo}-{hi}");
        }
        assert_eq!(s.phase, RangePhase::Expanding);
        step_range(&mut s, lo, hi);
        assert_eq!(s.mask(), (lo..=hi).collect::<Vec<_>>());
        assert_eq!(s.phase, RangePhase::Steady);
    }
}

/// 上行示例流程：0-2 → 0-3（并集）→ 1-3（收敛）
#[test]
fn range_slides_upward_as_specified() {
    let mut s = RangeState::new(0, 2);
    assert!(step_range(&mut s, 1, 3));
    assert_eq!(s.mask(), vec![0, 1, 2, 3], "第一步必须是并集 0-3");
    assert!(step_range(&mut s, 1, 3));
    assert_eq!(s.mask(), vec![1, 2, 3], "第二步才收敛到 1-3");
    assert!(!step_range(&mut s, 1, 3), "稳态重复调用不得再写");
}

/// 下行镜像流程：2-4 → 1-4（并集）→ 1-3（收敛）
#[test]
fn range_slides_downward_symmetrically() {
    let mut s = RangeState::new(2, 4);
    assert!(step_range(&mut s, 1, 3));
    assert_eq!(s.mask(), vec![1, 2, 3, 4]);
    assert!(step_range(&mut s, 1, 3));
    assert_eq!(s.mask(), vec![1, 2, 3]);
}

/// 方向判定（日志口径）：比较区间中点
#[test]
fn direction_reports_up_down_hold() {
    assert_eq!(direction(0, 2, 1, 3), Dir::Up);
    assert_eq!(direction(2, 4, 1, 3), Dir::Down);
    assert_eq!(direction(0, 2, 0, 3), Dir::Hold, "只扩不移动 = Hold");
    assert_eq!(Dir::Up.as_str(), "up");
    assert_eq!(Dir::Down.as_str(), "down");
}

// [window]
/// 选核：落在负载最低的连续段
#[test]
fn window_picks_lightest_contiguous_segment() {
    let load = [0.9, 0.85, 0.15, 0.05, 0.6];
    let w = best_window(&(0..5).collect::<Vec<_>>(), 2, &load).unwrap();
    assert_eq!(w, Window::new(2, 3));
}

/// 宽度越界收敛到整池；空池返回 None
#[test]
fn window_degenerate() {
    assert!(best_window(&[], 1, &[]).is_none());
    let w = best_window(&(0..3).collect::<Vec<_>>(), 99, &[0.1, 0.2, 0.3]).unwrap();
    assert_eq!(w, Window::new(0, 2));
}

/// 窗口几何
#[test]
fn window_geometry() {
    let w = Window::new(2, 4);
    assert_eq!(w.width(), 3);
    assert_eq!(w.cores(), vec![2, 3, 4]);
}

// [caliber]
/// 跨簇比较必须**保留静态项**（首档功耗），否则「开一颗核」在动态增量小的簇上近乎免费，
/// 该簇会永远胜出。这里用两个形状相同、静态基座不同的簇验证：
/// low 首档 0.05 W、high 首档 0.40 W，同频同算力 → 轻载必须选 low。
#[test]
fn cross_cluster_keeps_static_term() {
    // 两簇：算力相同（f/f_max 一致），功耗增量也一致，唯一差别是首档基座
    let freqs = [500u32, 1000, 1500, 2000];
    let low = [0.05f32, 0.15, 0.30, 0.50];
    let high = [0.40f32, 0.50, 0.65, 0.85];
    // 全局标定：P_REF = 最大单核绝对功耗
    let p_ref = low
        .iter()
        .chain(high.iter())
        .copied()
        .fold(0.0f32, f32::max);
    let cap_ref = 1024.0f32;
    let cap = 1024.0f32;
    let build = |p: &[f32]| -> Vec<f32> {
        let f_max = *freqs.last().unwrap() as f32;
        freqs
            .iter()
            .zip(p.iter())
            .map(|(&f, &pw)| cap * (f as f32 / f_max) / cap_ref * (pw / p_ref))
            .collect()
    };
    let cost = |p: &[f32], d: f32| -> Option<f32> {
        let f_max = *freqs.last().unwrap() as f32;
        for (i, &f) in freqs.iter().enumerate() {
            let sup = cap * (f as f32 / f_max) / cap_ref;
            if sup >= d {
                return Some(p[i] / p_ref);
            }
        }
        None
    };
    let _ = build(&low);
    for d in [0.25f32, 0.5, 0.75] {
        let cl = cost(&low, d).unwrap();
        let ch = cost(&high, d).unwrap();
        assert!(cl < ch, "d={d}: low={cl} 应低于 high={ch}（静态基座必须计入）");
    }
}

/// 回归护栏：`pow_glob` 必须**包含**首档（静态）项，不能像 `pow_rel` 那样扣到 0。
/// 用真实构建路径无法在本机取到 soc.yaml，故直接检验归一化契约：
/// 同一簇、同频，`pow_glob[0] > 0`（若扣了基线会恒为 0）。
#[test]
fn glob_vector_keeps_nonzero_base() {
    let first = 0.0451f32; // sm8750 big 首档单核功耗
    let p_ref = 6.1959f32; // 全局最大单核功耗
    let v = first / p_ref;
    assert!(v > 0.0, "静态项被扣掉了：{v}");
    assert!((v - 0.00728).abs() < 1e-4, "静态项比例应约 0.0073，实得 {v}");
}

// [replay]
// 离线重放（不触 sysfs、不依赖真机）：把「一轮决策 → 落盘 → 失败」编排成最小闭环，
// 回归 2026-10-26 真机取证暴露的三条失效。背景数字（sm8550/PHB110 14 批 + sm8750 22 批）：
//   * `place` 写失败 62,531 次（80%）、其中 54% 的 ok 写又被 cpuset/暂停核摘核 → 真正生效仅 ≈9.5%；
//   * 写失败后原实现既不记账也不限速 → 同一线程对同一颗不可用核按调度周期无限重打；
//   * 两拍协议假定每拍都落地，失败时不回滚 → 引擎记账 / 内核掩码 / `eem_window` 三方分叉。

use crate::common::CoreGroup;
use std::time::{Duration, Instant};

/// 0..5 = big（核 5 最闲）、6..7 = prime（都忙）：复现「一轮里多个线程盯上同一颗最闲核」
fn herding_load() -> Vec<f32> {
    vec![0.90, 0.90, 0.90, 0.90, 0.90, 0.02, 0.90, 0.90]
}

fn replay_online() -> Vec<(CoreGroup, usize)> {
    vec![(CoreGroup::Big, 6), (CoreGroup::Prime, 2)]
}

fn replay_subject(window: usize, demand: f32) -> placement::Subject {
    placement::Subject {
        demand,
        cluster: CoreGroup::Big,
        window: Window::new(window, window),
        past_sweet: true,
    }
}

/// 一轮重放的**核心不变量**：落到同一颗核的所有线程，其需求总和加上该核原有负载不得越 1。
///
/// 这正是 `Ledger` 相对「整轮共用一份裸 load 快照」的差别：齐涌不会因为多个线程同时判定
/// 同一颗最闲核而超载 —— 后续线程看到的是**含本轮承诺**的有效负载（改去刚被腾空的源核，
/// 或被「到达后不超载」闸拒掉）。同时校验先扩后缩：旧核必须留在并集里。
#[test]
fn replay_round_keeps_arrivals_within_capacity() {
    use std::collections::BTreeMap;
    let load = herding_load();
    let online = replay_online();
    let hyst = 0.15f32;
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(5);
    let demand = 0.4f32;
    let n = 6usize;
    let mut eng = placement::Engine::new();
    for w in 0..n {
        eng.ensure(w as i32, CoreGroup::Big, Window::new(w, w), t0);
    }
    eng.begin_round();
    let mut arrivals: BTreeMap<usize, f32> = BTreeMap::new();
    let mut departures: BTreeMap<usize, f32> = BTreeMap::new();
    let mut moved = 0usize;
    for w in 0..n {
        if let Some(a) = eng.tick(w as i32, &replay_subject(w, demand), &load, &online, hyst, t1) {
            assert!(!a.mask.is_empty(), "掩码绝不空（先扩后缩第一拍是并集）");
            // 并集的两端不总代表目标：向上迁目标在高位、向下迁目标在低位，
            // 判方向只能看 `dir`（Hold = 收敛拍，目标就在掩码内）。
            let dst = match a.dir {
                Dir::Up => *a.mask.last().expect("掩码非空"),
                _ => *a.mask.first().expect("掩码非空"),
            };
            let (lo, hi) = (a.mask[0], a.mask[a.mask.len() - 1]);
            assert!(
                (lo..=hi).contains(&w),
                "旧核 {w} 必须留在并集 {lo}-{hi} 里（目标 {dst}，dir={:?}）",
                a.dir
            );
            assert!(!a.settled, "第一拍是过渡态，收敛在下一拍");
            *arrivals.entry(dst).or_default() += demand;
            if dst != w {
                // 源核同时卸载：账本让源核变轻正是靠这一半，只算迁入会得出假超载
                *departures.entry(w).or_default() += demand;
            }
            moved += 1;
        }
    }
    assert!(moved >= 2, "重放没覆盖到决策路径（moved={moved}）");
    // 守恒口径：一轮结束时每颗核的状态 = 原有负载 − 本轮迁出 + 本轮迁入，不得越 1。
    for core in 0..load.len() {
        let landed = load[core] - departures.get(&core).copied().unwrap_or(0.0)
            + arrivals.get(&core).copied().unwrap_or(0.0);
        assert!(
            landed <= 1.0 + 1e-6,
            "核 {core} 一轮后超载：原有 {} − 迁出 {} + 迁入 {} = {landed}",
            load[core],
            departures.get(&core).copied().unwrap_or(0.0),
            arrivals.get(&core).copied().unwrap_or(0.0)
        );
    }
}

/// 写失败 → 回滚到锚点 + 指数退避：失败是**核此刻不可用**的状态，不是重试信号。
/// 原实现下这条线程会按 MIN_DWELL(2s) 每轮重打（真机实测 1 分钟 390~417 次）。
#[test]
fn replay_abort_restores_anchor_and_backs_off() {
    let load = herding_load();
    let online = replay_online();
    let hyst = 0.15f32;
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(5);
    let mut eng = placement::Engine::new();
    eng.ensure(0, CoreGroup::Big, Window::new(2, 2), t0);
    eng.begin_round();
    let a = eng
        .tick(0, &replay_subject(2, 0.4), &load, &online, hyst, t1)
        .expect("首轮应判定迁移");
    assert_eq!(a.mask, vec![2, 3, 4, 5]);
    // 内核拒了这一拍（目标核当时不可用）
    assert_eq!(eng.abort(0, t1), Some(1), "失败计数必须 +1");
    let e = eng.entry(0).expect("条目存在");
    assert_eq!(
        e.window,
        Window::new(2, 2),
        "失败后必须退回最后**确认生效**的区间，否则引擎记的是假窗口"
    );
    assert_eq!(e.state.phase, RangePhase::Steady, "回滚后回到稳态，不再滞留过渡态");
    eng.begin_round();
    assert!(
        eng.tick(
            0,
            &replay_subject(2, 0.4),
            &load,
            &online,
            hyst,
            t1 + Duration::from_secs(1)
        )
        .is_none(),
        "退避期内（FAIL_BACKOFF_BASE=4s）不得重打"
    );
    eng.begin_round();
    assert!(
        eng.tick(
            0,
            &replay_subject(2, 0.4),
            &load,
            &online,
            hyst,
            t1 + Duration::from_secs(5)
        )
        .is_some(),
        "退避期满应重新参与竞争（核恢复后至多等一个退避周期）"
    );
}

/// 回执必须记**回读掩码**：写返回 0 但内核摘了核（cpuset 裁剪 / 核被暂停）时，
/// 按意图记账等于把「没生效」记成「已迁移」。
#[test]
fn replay_note_applied_records_readback_not_intent() {
    let t0 = Instant::now();
    let mut eng = placement::Engine::new();
    eng.ensure(7, CoreGroup::Big, Window::new(0, 0), t0);
    // 意图写 {0,1}，回读只剩 {1}
    eng.note_applied(7, CoreGroup::Big, &[1], t0);
    let e = eng.entry(7).expect("条目存在");
    assert_eq!(e.applied, Window::new(1, 1), "锚点必须取回读掩码");
    assert_eq!(e.fail_streak, 0, "成功写回执要清零失败计数");
}


// [core_floor]
/// 载荷不均时，**逐核**供给下限必须把目标档抬起来。
///
/// 4 核 (0.85, 0.1, 0.1, 0.1)：Σ = 1.15 → «k = ceil(Σ) = 2» → 平均口径只要每核 0.575 就判够，
/// 而最忙那颗核要 0.85，实际只拿到 68%。更糟的是 «supply(i,2) >= Σ» 仍成立，
/// 抗饥饿判据也看不出来 → 封顶永不释放。本用例锁住「按最忙核折算后必须抬档」。
#[test]
fn core_floor_lifts_gear_for_uneven_load() {
    let big = em::curve(CoreGroup::Big).expect("虚构 SoC 有 big 曲线");
    let caps = [Some(762u32); 6];
    let utils = [0.85f32, 0.1, 0.1, 0.1, 0.0, 0.0];
    let cpus: Vec<usize> = (0..6).collect();

    let total = em::demand_units(&cpus, &utils, &caps).expect("容量齐备");
    // k 走的是**原始 util 之和**（与 CLG `let k = occupied.ceil()` 同口径），
    // 不是容量归一后的 demand：两者口径不同正是缺陷 A 的来源之一。
    let occupied: f32 = utils.iter().map(|u| u.clamp(0.0, 1.0)).sum();
    let k = (occupied.ceil() as usize).max(1);
    assert_eq!(k, 2, "Σutil=1.15 只占 2 颗核当量（问题正在这里）");
    let busiest = em::demand_units(&cpus[..1], &utils, &caps).expect("最忙核需求");
    let floored = em::demand_with_core_floor(total, busiest, k);
    assert!(floored > total, "不均载荷必须抬高总需求：{floored} vs {total}");

    // 平均口径选出的档：按 2 核摊薄，最忙核拿不到自己需要的份额
    let avg_gear = big.gear_for(total, k);
    let per_core_avg = big.supply(avg_gear, k) / k as f32;
    assert!(
        per_core_avg < busiest,
        "平均口径应确实欠供最忙核（{per_core_avg} < {busiest}）"
    );
    // 逐核口径选出的档：每核供给覆盖最忙核
    let floor_gear = big.gear_for(floored, k);
    let per_core_floor = big.supply(floor_gear, k) / k as f32;
    assert!(
        per_core_floor >= busiest,
        "逐核口径必须让每核供给覆盖最忙核（{per_core_floor} >= {busiest}）"
    );
    assert!(floor_gear >= avg_gear, "只许抬档，不许降档");
}

/// 均匀载荷下 «demand_with_core_floor» 必须逐位不变：此时 «busiest ≈ Σ/k»，
/// 折算不会引入任何额外需求，既有行为与甜点落点完全不受影响（等价性保护）。
#[test]
fn core_floor_is_noop_for_even_load() {
    let caps = [Some(762u32); 6];
    for u in [0.05f32, 0.2, 0.5, 0.9] {
        let utils = [u; 6];
        let cpus: Vec<usize> = (0..6).collect();
        let total = em::demand_units(&cpus, &utils, &caps).expect("容量齐备");
        let k = (total.ceil() as usize).max(1);
        let busiest = em::demand_units(&cpus[..1], &utils, &caps).expect("单核需求");
        let floored = em::demand_with_core_floor(total, busiest, k);
        assert!(
            (floored - total).abs() <= f32::EPSILON * total.max(1.0),
            "均匀 util={u} 不应改变需求：{floored} vs {total}"
        );
    }
    // 非有限值原样返回（NaN 不允许被「修」成一个可用的数）
    assert!(em::demand_with_core_floor(f32::NAN, 0.5, 2).is_nan());
}

/// 放置候选**不许虚报供给**：需求超过单核硬件上限时，只能返回「供给最大的那个」，
/// 且其 supply 必须等于该簇该档的真实供给（调用方据此判「已到硬件上限」而不是以为搬得动）。
#[test]
fn placement_never_claims_supply_beyond_hardware() {
    let online = em::online_clusters();
    assert!(!online.is_empty(), "虚构 SoC 至少有一簇在线");
    let demand = 2.0f32; // 远超任一双簇单核供给
    let p = em::best_placement(demand, &online, None, 0.0, 1).expect("有候选");
    let c = em::curve(p.cluster).expect("候选簇有曲线");
    assert_eq!(p.cores, 1, "单线程口径：只占 1 核");
    assert!(
        (p.supply - c.supply(p.gear, 1)).abs() <= f32::EPSILON,
        "供给必须等于真实档位供给"
    );
    assert!(p.supply < demand, "供不上就是供不上，不许凑数");
    assert_eq!(p.gear, c.top(), "供不上时应取本簇顶档");
}

/// 成本更低的簇如果**供不上**需求，放置必须选能供上的那个（哪怕更贵）。
/// 这条锁的是「别处更便宜」判据的前置条件：供给不足的候选不构成迁移理由。
#[test]
fn placement_picks_cluster_that_can_supply_even_if_dearer() {
    let online = em::online_clusters();
    let demand = 0.9f32; // big 单核最高供给 < 0.9，prime 够
    let p = em::best_placement(demand, &online, None, 0.0, 1).expect("有候选");
    assert!(
        p.supply >= demand,
        "存在能供上的簇时必须选它（选中 {:?} supply={}）",
        p.cluster,
        p.supply
    );
}


// [k_symmetric]
/// `best_placement` 必须按**调用方给的核数**比价：单线程（cores=1）供不上的需求，
/// 在簇级口径（cores=k）下可能正好落得下——这是「k 核 vs k 核」而非「k 核 vs 1 核」的意义。
#[test]
fn placement_scales_with_requested_cores() {
    let online = em::online_clusters();
    let demand = 1.2f32;
    let single = em::best_placement(demand, &online, None, 0.0, 1).expect("有候选");
    assert!(single.supply < demand, "单核口径下 1.2 供不上（应取顶档）");
    assert_eq!(single.cores, 1);

    let multi = em::best_placement(demand, &online, None, 0.0, 2).expect("有候选");
    assert_eq!(multi.cores, 2, "核数要如实回传");
    assert!(
        multi.supply >= demand,
        "2 核口径下必须能供上：supply={} demand={demand}",
        multi.supply
    );
    let c = em::curve(multi.cluster).expect("有曲线");
    assert!(
        (multi.cost - c.cost(multi.gear, 2)).abs() <= f32::EPSILON,
        "成本必须按 2 核计（不能拿 1 核成本冒充）"
    );
    // 在线核数不足的簇不得参与比价（prime 只有 2 核，要 3 核时必须排除它）
    let three = em::best_placement(demand, &online, None, 0.0, 3).expect("有候选");
    assert_eq!(three.cluster, CoreGroup::Big, "3 核只能落 big");
}

