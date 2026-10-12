//! clg_tuning_regression.rs: [config_contract]
//! Run: rustc --test scripts/tests/clg_tuning_regression.rs -o devimpbin/p23_config_test
const FEATURE: &str = include_str!("../../module/config/8550/feature.yaml");
fn default_value(key: &str) -> f32 {
    let block = FEATURE.split("# [default]").nth(1).unwrap().split("# [boost]").next().unwrap();
    block.lines().find_map(|line| line.strip_prefix(&format!("    {key}: "))).unwrap().parse().unwrap()
}
// [config_contract]
#[test]
fn conservative_p2_three_knobs() {
    assert_eq!(default_value("up_jump_threshold"), 0.55);
    assert_eq!(default_value("spike_jump_threshold"), 0.50);
    assert_eq!(default_value("up_rate_limit_ticks"), 4.0);
}
#[test]
// ponytail: regression-guard scope is the template text only — keeps authors from silently weakening the self-check assertion text back to a bound; the behavioral invariant is enforced by replay_clg.py's `selfcheck` invocation, which is where the strictness actually lives. This test does not freeze config values; it will keep passing when default knobs are later retuned.
fn saturation_selfcheck_must_not_be_weakened() {
    let template = include_str!("clg_replay.rs");
    assert!(template.contains("saturated_perf"), "self-check saturation assertion must exist");
    assert!(template.contains("assert_eq!(saturated_freq, hw_max"), "self-check must assert reaching hardware max");
    assert!(!template.contains("saturated_perf > 0.9"), "weak lower-bound assertion is not acceptable for saturation");
}

#[test]
fn p3_coupled_down_knobs() {
    assert_eq!(default_value("down_rate_limit_ticks"), 1.0);
    assert_eq!(default_value("down_fast_threshold"), 0.40);
}
#[test]
fn preserve_target_ceiling_and_previous_delay() {
    assert_eq!(default_value("up_threshold"), 0.88);
    assert_eq!(default_value("steady_decay_target_ratio"), 0.90);
    assert_eq!(default_value("perf_ceil"), 1.0);
    assert_eq!(default_value("spike_decay"), 0.10);
    assert!(FEATURE.lines().any(|line| line == "scene_mode_delay_secs: 3000"));
}
