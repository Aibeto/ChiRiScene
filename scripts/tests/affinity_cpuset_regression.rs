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

// [affinity_failure_contract]
/// 单核钉定失败必须**记账**：不记账的话退避闸永远读不到失败，退化成每轮硬撞
/// （2026-10-26 真机取证：sm8550 侧 `pin fg_pin` 失败 63,856 次，单 pid 单核 1 分钟 390~417 次）。
#[test]
fn single_core_pin_failure_is_bookkept_before_return() {
    let source = include_str!("../../src/chiri/affinity.rs");
    let pin = source
        .split("    fn pin_core(")
        .nth(1)
        .unwrap()
        .split("    fn unpin_core(")
        .next()
        .unwrap();
    assert!(
        pin.contains("self.pin_fail_wait(tid, now)"),
        "pin_core 缺失败退避闸"
    );
    let fail = pin.split("if res.is_err() {").nth(1).expect("缺失败分支");
    assert!(
        fail.find("pin_fail_streak").unwrap() < fail.find("return").unwrap(),
        "失败分支必须先记账再 return"
    );
    // 成功路径必须清零，否则退避会跟着线程一辈子
    assert!(source.contains("st.pin_fail_streak = 0;"));
}

/// EEM 放置写必须**落回执/回滚**：成功记回读掩码、失败退回锚点，否则引擎记账与
/// 内核掩码分叉，「先扩后缩」的每拍都落地前提被打破（sm8550 实测 `place` 失败 62,531 次）。
#[test]
fn eem_place_acknowledges_write_outcome() {
    let source = include_str!("../../src/chiri/affinity.rs");
    let round = source.split("    fn eem_place_round(").nth(1).unwrap();
    assert!(round.contains("self.eem.note_applied("), "写成功必须落回执");
    assert!(round.contains("self.eem.abort("), "写失败/升组失败必须回滚");
}

