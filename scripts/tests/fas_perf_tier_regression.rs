//! fas_perf_tier_regression.rs: [fas_perf_tier]
//! Run: rustc --test scripts/tests/fas_perf_tier_regression.rs -o devimpbin/_xlog/fas_perf_tier_regression
//! 契约：FAS 性能优先档（perf_lock）是「按应用显式开启」的锁频档，缺省必须关闭；
//! 锁频必须在热降频之前生效，否则内核热限频会被锁频档击穿（安全边界）。
const ENDFIELD: &str = include_str!("../../module/config/normal/fas/endfield.yaml");
const TYPES: &str = include_str!("../../src/fas_types.rs");
const POLICY: &str = include_str!("../../src/scheduler/fas/policy_mgmt.rs");

// [config_contract] 终末地 = 性能优先档：锁频 + 单档 60 + 温度护栏关闭
#[test]
fn endfield_runs_performance_first_tier() {
    assert!(ENDFIELD.contains("perf_lock: true"), "终末地必须开启 perf_lock");
    assert!(ENDFIELD.contains("fps_gears: [60.0]"), "单档 60 才能整段禁用齿轮机（升/降档两条掉频来源）");
    assert!(ENDFIELD.contains("target_fps: [60]"));
    assert!(ENDFIELD.contains("core_temp_threshold: 0.0"), "FAS 侧温度护栏关闭，热量交给内核 thermal");
}

// [default_contract] 锁频档是逐应用显式开关：缺省 false，避免误开把整机功耗抬起来
#[test]
fn perf_lock_defaults_off() {
    assert!(TYPES.contains("perf_lock: false"), "FasRulesConfig 缺省 perf_lock 必须为 false");
    assert!(TYPES.contains("pub perf_lock: bool"));
}

// [order_contract] 锁频赋值必须早于 thermal_cap 压顶：锁频不能压过内核热限频
#[test]
fn lock_applies_before_thermal_cap() {
    let lock_idx = POLICY.find("if locked {").expect("apply_freqs 必须保留 locked 分支");
    let cap_idx = POLICY
        .find("if let Some(cap) = thermal_cap {")
        .expect("apply_freqs 必须保留 thermal_cap 压顶");
    assert!(lock_idx < cap_idx, "锁频分支必须写在 thermal_cap 之前");
}

// [guard_contract] 开环（load_only）分支必须被锁频旁路，否则帧源静默时仍按下探频率
#[test]
fn load_only_is_bypassed_under_lock() {
    assert!(
        POLICY.contains("let load_only = !locked && self.uses_load_control();"),
        "load_only 必须与 locked 互斥"
    );
}
