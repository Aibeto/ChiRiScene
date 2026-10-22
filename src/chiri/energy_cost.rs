//! energy_cost.rs: [source] [power] [accounting] [probe] [tests]
//! 功耗遥测模型（**第二代帕累托前沿 CLG-EEM 的共用底座**，非 FDP 专属）：数据链
//! mdocs/sm8550/sm8550-freq-power.md §3 → scripts/frontier_policy.py → soc.yaml 受控块
//! `per_cluster.<簇>.power_w` → `soc_frontier_policy()`，本模块不手抄表。
//!
//! 现役消费方：① `src/chiri/mod.rs` 的 CPU 动态功耗遥测分解（`cpu_dynamic_power_w*`）；
//! ② `src/chiri/affinity.rs` 的 EEM 放置内核（`read_cluster_freqs` 取三簇实际频率）。
//! 上一代 FDP 的「边际能耗成本 → 迁入目的簇」决策链（`fdp_destination` 等）已于 2026-10-23 随
//! 帕累托前沿概念模型一并移除。

use crate::common::{self, CoreGroup};
use std::sync::OnceLock;

// [source]
/// 取该簇的 (频率档 kHz, 逐档簇功耗 W)，数据源 = `soc_frontier_policy().per_cluster`
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
/// 在 (freqs, powers) 上线性插值：越界取端点（低于首档→首档功耗，高于末档→末档功耗）；表空 / 长度不一致 → `None`
/// 纯函数（不读全局），便于测试
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

/// 该簇在 `freq_khz` 处的簇功耗（W）：查 `soc_frontier_policy()` 逐档表并线性插值，越界取端点
/// 无 frontier_policy 段 / 表不齐 → `None`（8550 与 zumapro 都有该段）
pub fn cluster_power_w(cluster: CoreGroup, freq_khz: u32) -> Option<f32> {
    let (freqs, powers) = cluster_energy_table(cluster)?;
    interp(freqs, powers, freq_khz)
}

// [accounting]
/// CPU 动态功率估计（W）：逐簇 `簇功耗表(实测频率) × 簇忙碌比` 求和，忙碌比 = 簇内各核 util 之和 ÷ 簇核数
/// 无功耗表的 SoC 返回 None 对应列写 -，同秒已有频率快照时可改用 cpu_dynamic_power_w_with_frequencies
pub fn cpu_dynamic_power_w(core_utils: &[f32]) -> Option<f32> {
    if !has_cpu_energy_table() {
        return None;
    }
    let frequencies = read_cluster_freqs();
    let ranges = common::chiri_core_ranges();
    let spans = [ranges.little, ranges.big, ranges.prime];
    sum_cpu_dynamic_power_w(core_utils, &spans, &frequencies, cluster_power_w)
}

/// CPU 动态功率估计（W）：使用同秒 `(policy ID, scaling_cur_freq kHz)` 快照，顺序不限、policy ID 不得重复
/// 缺失、读取失败及 0 频率均跳过，不回退模型、不额外读频率；policy 到簇的拓扑映射沿用 `read_cluster_freqs` 的一次性发现结果
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
        // 缺失 util 不当成零，有样本时仍按整簇核数归一化
        total_power += cluster_power * (busy_utilization / span.len() as f32).clamp(0.0, 1.0);
        has_observed_cluster = true;
    }
    has_observed_cluster.then_some(total_power)
}

// [probe]
/// 簇 → 数组槽位（little 0 / big 1 / prime 2），`read_cluster_freqs` 内部索引
fn cluster_slot(cluster: CoreGroup) -> usize {
    match cluster {
        CoreGroup::Little => 0,
        CoreGroup::Big => 1,
        CoreGroup::Prime => 2,
    }
}

/// 簇 → cpufreq policy id 的映射只发现一次：policy 目录开机后固定（热插拔只动核的 online，不增删 policy），此后每轮只读 3 个 `scaling_cur_freq`
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

/// 三簇当前实际运行频率（kHz，返回 `[little, big, prime]`）：按缓存好的 policy id 各读一个 `scaling_cur_freq`，共 3 次 open
/// 口径 = FREQ_QOS 聚合后的实际频率，不是 CLG 的决策原值/上限；读不到的簇为 `None`
/// （EEM 放置内核据此取三簇工作点：`src/chiri/affinity.rs` 的 `eem_place_round` 整轮只读一次）
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

// [tests]
// 本仓库无测试基建（无 CI 执行 `cargo test`），以下测试只保证编译通过、不会被执行
// 依赖 SoC 运行期数据的断言在开发机恒为 None/兜底值，故只用「None 或满足不变量」的弱断言
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
            sum_cpu_dynamic_power_w(
                &[1.0, 0.0, 1.0, 0.5],
                &spans,
                &frequencies,
                power_at_frequency
            ),
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
}
