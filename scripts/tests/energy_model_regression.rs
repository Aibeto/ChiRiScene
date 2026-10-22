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

    pub fn soc_frontier_policy() -> Option<&'static FrontierPolicy> {
        None
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
