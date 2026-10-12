//! affinity_cpuset_regression.rs - 区块索引: [integration]

#[path = "../../src/chiri/affinity_cpuset.rs"]
mod affinity_cpuset;

// [integration]
#[test]
fn identity_guard_precedes_retry_refresh_and_invalidates_stale_candidate() {
    let source = include_str!("../../src/chiri/affinity.rs");
    let precheck = source
        .split("    fn group_bind_precheck(")
        .nth(1)
        .unwrap()
        .split("    fn pin_core(")
        .next()
        .unwrap();
    assert!(
        precheck.find("check_group_cpuset").unwrap()
            < precheck.find("state.starttime = starttime").unwrap()
    );
    assert!(
        precheck.contains("self.threads.remove(&tid)"),
        "reused identity must be reseeded next scan"
    );
}

#[test]
fn every_group_write_uses_cpuset_guard_without_touching_single_core_writer() {
    let source = include_str!("../../src/chiri/affinity.rs");
    let precheck = source
        .split("    fn group_bind_precheck(")
        .nth(1)
        .unwrap()
        .split("    fn pin_core(")
        .next()
        .unwrap();
    assert!(
        precheck.contains("check_group_cpuset"),
        "group writes lack thread cpuset guard"
    );
    let writer = source
        .split("pub(crate) fn set_tid_affinity(")
        .nth(1)
        .unwrap()
        .split("pub fn read_tid_mask(")
        .next()
        .unwrap();
    assert!(
        !writer.contains("check_group_cpuset"),
        "single-core path must remain unchanged"
    );
    assert_eq!(source.matches("&& self.group_bind_precheck(").count(), 3);
}
