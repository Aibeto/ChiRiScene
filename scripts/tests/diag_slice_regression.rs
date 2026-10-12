//! diag_slice_regression.rs - [source_contract] [modules]
//! Run: rustc --edition=2024 --test scripts/tests/diag_slice_regression.rs -o devimpbin/p4_scratch.rs

// [modules]
#[path = "../../src/chiri/diag_slice.rs"]
mod diag_slice;

#[test]
fn slow_threshold_and_slices_are_exact() {
    use std::time::Duration;
    assert!(!diag_slice::slow_anchor(Duration::from_millis(200)));
    assert!(diag_slice::slow_anchor(Duration::from_nanos(200_000_001)));
    for n in 1..100 { assert_eq!(diag_slice::slice_due(n), n % 16 == 0); }
}

#[test]
fn serialized_full_slots_reuse_and_invalidate_on_identity() {
    let mut cache = diag_slice::FullRowCache::default();
    let original = "t 4 7 NaN u=0 core=-1 home=-1 pin=0 uclamp=-1";
    assert_eq!(cache.get_or_render(|| original.to_owned()), original);
    let ptr = cache.get_or_render(|| panic!("must reuse")).as_ptr();
    assert_eq!(cache.get_or_render(|| panic!("must reuse")).as_ptr(), ptr);
    cache.invalidate();
    assert_eq!(cache.get_or_render(|| "t 8 7 new u=9 core=2 home=2 pin=1 uclamp=-1".to_owned()), "t 8 7 new u=9 core=2 home=2 pin=1 uclamp=-1");
}

#[test]
fn anchor_preserves_candidate_order_success_set_and_current_identity() {
    let rows = diag_slice::anchor_rows([7, 8, 9, 10], |tid| match tid {
        7 => Some("t 1 7 new u=11 core=2 home=1 pin=1 uclamp=-1".to_owned()),
        8 => None, // failed stat: never revive a cached dead thread
        9 => Some("t 1 9 cold u=3 core=0 home=-1 pin=0 uclamp=-1".to_owned()),
        10 => Some("t 2 10 NaN u=0 core=-1 home=-1 pin=0 uclamp=-1".to_owned()),
        _ => unreachable!(),
    });
    assert_eq!(rows.len(), 3);
    assert!(rows[0].starts_with("t 1 7 new "));
    assert!(rows[1].starts_with("t 1 9 cold "));
    assert!(rows[2].starts_with("t 2 10 NaN "));
    assert!(rows.iter().all(|row| row.split_whitespace().count() == 9));
}

// [source_contract]
#[test]
fn source_has_no_dead_refresh_paths_and_anchor_finalize_unsampled() { // ponytail: real-device integration remains TODO; this guard freezes no-dead-path wiring.
    let source = include_str!("../../src/chiri/mod.rs");
    assert!(!source.contains("if refresh {
                        // 刷新帧：全量重锚（下游据此重建存活集合），不做槽级省略；comm 必构造
                        // Full rows are assembled once at the final anchor boundary.
                        None"));
    assert!(!source.contains("if refresh {
                        None"));
    assert!(source.contains("Some(e.full_row(tid))"), "existing refresh entries must render immediately");
    assert!(source.contains("refresh |= diag_slice::slow_anchor(build_started.elapsed());"));
}

#[test]
fn refresh_frames_do_not_double_append_thread_rows() { // ponytail: structural guard only; real /proc builder integration remains device-side TODO.
    let source = include_str!("../../src/chiri/mod.rs");
    assert!(!source.contains("let regular_rows = th_rows"), "refresh branch must consume regular rows, not duplicate them");
}

#[test]
fn builder_uses_reuse_and_late_full_anchor() {
    let source = include_str!("../../src/chiri/mod.rs");
    assert!(source.contains("diag_slice::slow_anchor"), "missing >200ms full anchor policy");
    assert!(source.contains("diag_slice::slice_due"), "missing bounded work slice checkpoints");
    assert!(source.contains("full_row: diag_slice::FullRowCache"), "missing same-identity serialized row reuse");
    assert!(source.contains("// [slow_anchor]"), "missing late-build cold candidate completion");
    assert!(source.contains("const DIAG_WORKER_ENABLED: bool = false"));
}
