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
    activity_lookup(app_modes, activity, activity_aware)
        .or_else(|| app_modes.get(package))
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
    assert_eq!(select_mode(&m, PKG, ACT, true).map(String::as_str), Some("boost"));
}

#[test]
fn unconfigured_activity_falls_back_to_package_rule() {
    // 只配了包名级 → activity 命中不了，必须回退包名（默认回退，零行为变化）
    let m = modes(&[(PKG, "boost")]);
    assert_eq!(select_mode(&m, PKG, ACT, true).map(String::as_str), Some("boost"));
}

#[test]
fn empty_activity_falls_back_to_package_rule() {
    // activity 未知（非 FAS 前台恒为空）：即使用户配了可能匹配的 activity 键也不能命中
    let m = modes(&[(PKG, "default"), (ACT, "boost")]);
    assert_eq!(select_mode(&m, PKG, "", true).map(String::as_str), Some("default"));
}

#[test]
fn activity_aware_off_ignores_activity_keys() {
    let m = modes(&[(PKG, "default"), (ACT, "boost")]);
    assert_eq!(select_mode(&m, PKG, ACT, false).map(String::as_str), Some("default"));
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
    assert_eq!(select_mode(&m, PKG, ACT, true).map(String::as_str), Some("boost"));
}
