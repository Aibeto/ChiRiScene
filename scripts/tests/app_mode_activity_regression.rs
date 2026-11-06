//! app_mode_activity_regression.rs: [rules]
//! Activity 级前台识别的**选择逻辑**回归：默认回退 + 选择性优先级覆盖。
//!
//! `determine_mode` 本体依赖 common/i18n/tokio，不适合独立引入；这里按相同的判定顺序
//! 复刻「activity 键优先、未命中回退包名键」这一步选择逻辑，锁住两条契约：
//!   1) 开关开 + activity 有配置 → 用 activity 的模式（选择性覆盖）
//!   2) 开关关 / activity 为空 / activity 无配置 → 用包名的模式（默认回退）
//! 生产实现的对应函数见 src/monitor/app_detect.rs 的 `activity_lookup`（同名同语义），
//! 两侧关键分支必须同步修改。

#![allow(dead_code)]

use std::collections::HashMap;

/// 与 src/monitor/app_detect.rs::activity_lookup 同语义的选择函数。
fn activity_lookup<'a>(
    app_modes: &'a HashMap<String, String>,
    activity: &str,
    activity_aware: bool,
) -> Option<&'a String> {
    if !activity_aware || activity.is_empty() {
        return None;
    }
    app_modes.get(activity)
}

/// 完整选择：activity 优先，未命中回退包名。
fn select_mode<'a>(
    app_modes: &'a HashMap<String, String>,
    package: &str,
    activity: &str,
    activity_aware: bool,
) -> Option<&'a String> {
    activity_lookup(app_modes, activity, activity_aware).or_else(|| app_modes.get(package))
}

fn modes(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

const PKG: &str = "com.tencent.tmgp.sgame";
const ACT: &str = "com.tencent.tmgp.sgame/.SGameActivity";

#[test]
fn configured_activity_overrides_package_rule() {
    let m = modes(&[(PKG, "default"), (ACT, "boost")]);
    assert_eq!(
        select_mode(&m, PKG, ACT, true).map(String::as_str),
        Some("boost")
    );
}

#[test]
fn unconfigured_activity_falls_back_to_package_rule() {
    // 只配了包名级 → activity 命中不了，必须回退包名（默认回退，零行为变化）
    let m = modes(&[(PKG, "boost")]);
    assert_eq!(
        select_mode(&m, PKG, ACT, true).map(String::as_str),
        Some("boost")
    );
}

#[test]
fn empty_activity_falls_back_to_package_rule() {
    // activity 未知（非 FAS 前台恒为空）：即使用户配了可能匹配的 activity 键也不能命中
    let m = modes(&[(PKG, "default"), (ACT, "boost")]);
    assert_eq!(
        select_mode(&m, PKG, "", true).map(String::as_str),
        Some("default")
    );
}

#[test]
fn activity_aware_off_ignores_activity_keys() {
    let m = modes(&[(PKG, "default"), (ACT, "boost")]);
    assert_eq!(
        select_mode(&m, PKG, ACT, false).map(String::as_str),
        Some("default")
    );
}

#[test]
fn no_rule_at_all_returns_none_for_global_fallback() {
    // 两侧都没有 → None，由 determine_mode 继续走特调映射/全局模式
    let m = modes(&[(PKG, "default")]);
    assert_eq!(select_mode(&m, "com.other.app", ACT, true), None);
}

#[test]
fn activity_key_is_exact_not_prefix_match() {
    // 精确查表，不做前缀/模糊匹配：只写包名键不会意外命中 activity
    let m = modes(&[(PKG, "boost"), ("com.tencent.tmgp.sgame/.Other", "reduce")]);
    assert_eq!(
        select_mode(&m, PKG, ACT, true).map(String::as_str),
        Some("boost")
    );
}

// ---------------------------------------------------------------------------
// 特调白名单层（2026-11-07 新增，activity-first）
// ---------------------------------------------------------------------------
// `determine_mode` 里特调白名单是 activity 查表之外的**包名级早退**，历史上先于 activity；
// 带 activity-first 的条目改为「查到 activity 规则就让位」。这里镜像新的判定顺序，锁住三条契约：
//   1) 条目未声明标志位 → 包名回退优先（bili 现状，逐位不变）
//   2) 声明了标志位但本轮没有 activity 规则 → 仍用包名回退（播放页靠它兜底）
//   3) 声明了标志位且有 activity 规则 → 用 activity 规则（详情页拉回 default）

/// 与 src/monitor/app_detect.rs::special_yields_to_activity 同语义
fn special_yields_to_activity(activity_first: bool, has_activity_rule: bool) -> bool {
    activity_first && has_activity_rule
}

/// 镜像 determine_mode 的前台选择（FAS 段不建模）：activity 先查 → 特调白名单 → app_modes 包名级。
/// `special` = Some((回退模式, activity_first))。
fn select_with_special<'a>(
    app_modes: &'a HashMap<String, String>,
    package: &str,
    activity: &str,
    activity_aware: bool,
    special: Option<(&'a str, bool)>,
) -> Option<String> {
    let activity_mode = activity_lookup(app_modes, activity, activity_aware);
    if let Some((fallback, activity_first)) = special {
        if !special_yields_to_activity(activity_first, activity_mode.is_some()) {
            return Some(fallback.to_string());
        }
    }
    activity_mode
        .or_else(|| app_modes.get(package))
        .cloned()
}

#[test]
fn special_entry_without_flag_keeps_package_fallback_priority() {
    // 历史行为：白名单包整包进特调，rules.yaml 里的 activity 规则写而不生效（bili 现状，勿改）
    let m = modes(&[(ACT, "default")]);
    assert_eq!(
        select_with_special(&m, PKG, ACT, true, Some(("playback", false))).as_deref(),
        Some("playback")
    );
}

#[test]
fn activity_first_entry_yields_to_explicit_activity_rule() {
    // 腾讯视频详情页：包名回退是 playback，但这一页被显式拉回 default（触摸密度 28.1%）
    let m = modes(&[(ACT, "default")]);
    assert_eq!(
        select_with_special(&m, PKG, ACT, true, Some(("playback", true))).as_deref(),
        Some("default")
    );
}

#[test]
fn activity_first_entry_keeps_fallback_when_activity_unknown() {
    // 播放器 activity 名尚未观测到（activity 为空或没配键）→ 仍走包名回退的 playback 兜底
    let m = modes(&[(ACT, "default")]);
    assert_eq!(
        select_with_special(&m, PKG, "", true, Some(("playback", true))).as_deref(),
        Some("playback")
    );
    assert_eq!(
        select_with_special(&m, PKG, "com.tencent.qqlive/.PlayerActivity", true, Some(("playback", true))).as_deref(),
        Some("playback")
    );
}

#[test]
fn activity_first_does_not_affect_activity_aware_off() {
    // 采样关闭（旧配置/无 activity 键）时 activity 恒不可用 → 标志位无从生效，退回包名回退
    let m = modes(&[(ACT, "default")]);
    assert_eq!(
        select_with_special(&m, PKG, ACT, false, Some(("playback", true))).as_deref(),
        Some("playback")
    );
}
