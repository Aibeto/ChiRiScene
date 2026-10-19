//! clg_tuning_regression.rs: [config_contract]
//! Run: rustc --test scripts/tests/clg_tuning_regression.rs -o devimpbin/p23_config_test
const FEATURE: &str = include_str!("../../module/config/sm8550/feature.yaml");
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
// [high_zone] 高位升频门槛契约：gate/util 阈值与三段启用值；hold 兜底必须在（防确认条件错配永久卡高位）
#[test]
fn high_zone_gate_knobs() {
    assert_eq!(default_value("high_zone_gate"), 0.85);
    assert_eq!(default_value("high_zone_count"), 2.0);
    assert_eq!(default_value("high_zone_util"), 0.90);
    // [boost] 是 feature.yaml 的最后一个段头：它之后没有同级段头，整段延伸到文件末尾
    for (section, next) in [("# [reduce]", Some("# [default]")), ("# [boost]", None)] {
        let block = FEATURE.split(section).nth(1).unwrap();
        let body = match next {
            Some(n) => block.split(n).next().unwrap(),
            None => block,
        };
        assert!(body.contains("high_zone_gate: 0.85"), "{section} must enable high-zone gate");
    }
}
#[test]
fn preserve_target_ceiling_and_previous_delay() {
    assert_eq!(default_value("up_threshold"), 0.88);
    assert_eq!(default_value("steady_decay_target_ratio"), 0.90);
    assert_eq!(default_value("perf_ceil"), 1.0);
    assert_eq!(default_value("spike_decay"), 0.10);
    assert!(FEATURE.lines().any(|line| line == "scene_mode_delay_secs: 3000"));
}
