//! fas_focus.rs: [decision] [tests]

use std::time::Duration;

pub(crate) const WINDOW_EVIDENCE_MAX_AGE: Duration =
    crate::monitor::window_visibility::OBSERVATION_TTL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusDecision {
    Focused,
    LoadOnly,
    Grace,
}

// [decision]
/// 焦点决策。`frame_feedback_active` = 「现在真的能取到窗口帧率」（`FasManager::frame_feedback_active`）。
///
/// **帧率优先规则**：窗口证据判为 `LoadOnly`（即 `WindowVisibility::VisibleUnfocused`）时，若帧源
/// 仍在正常产出帧，则维持帧驱动控制、不降级为仅调频。动机：游戏上加一层帧率悬浮窗/输入法时游戏
/// 窗口不再 `Focused`，但帧数据依旧可取；此时降 `LoadOnly` 会关掉帧驱动，表现为「FAS 异常退出」。
/// **边界**：本规则只作用于 `LoadOnly`。`Hidden`/`Unknown`（含真正隐藏）仍按原样走 `Grace`，
/// 「有叠加窗口时退出 FAS 是预期行为」这条语义不变。
pub(crate) fn decide_focus(
    same_foreground_owner: bool,
    same_raw_owner: bool,
    evidence: Option<(FocusDecision, Duration)>,
    frame_feedback_active: bool,
) -> FocusDecision {
    if same_foreground_owner && same_raw_owner {
        return FocusDecision::Focused;
    }
    evidence
        .filter(|(_, age)| *age < WINDOW_EVIDENCE_MAX_AGE)
        .map_or(FocusDecision::Grace, |(decision, _)| match decision {
            // 帧率可取 → 不因叠加窗口降级为仅调频，维持帧驱动
            FocusDecision::LoadOnly if frame_feedback_active => FocusDecision::Focused,
            other => other,
        })
}

pub(crate) fn event_matches_snapshot(
    event_package: &str,
    event_pid: i32,
    event_generation: u64,
    snapshot_package: &str,
    snapshot_pid: i32,
    snapshot_generation: u64,
    event_starttime: Option<u64>,
    snapshot_starttime: Option<u64>,
) -> bool {
    event_pid > 0
        && event_pid == snapshot_pid
        && event_package == snapshot_package
        && event_generation == snapshot_generation
        && starttime_proves_same_process(event_starttime, snapshot_starttime)
}

/// starttime 只在两侧都能读到时才作决定性判据；一侧缺失时不凭缺失否定命中
pub(crate) fn starttime_proves_same_process(
    event_starttime: Option<u64>,
    snapshot_starttime: Option<u64>,
) -> bool {
    match (event_starttime, snapshot_starttime) {
        (Some(event), Some(snapshot)) => event == snapshot,
        _ => true,
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_at_deadline_renews_before_expiry() {
        assert_eq!(
            decide_focus(true, true, None, false),
            FocusDecision::Focused
        );
    }

    #[test]
    fn ignored_system_overlay_does_not_renew_from_filtered_game() {
        assert_eq!(decide_focus(true, false, None, false), FocusDecision::Grace);
    }

    #[test]
    fn visible_overlay_keeps_load_only_without_extending_unknown_grace() {
        assert_eq!(
            decide_focus(
                true,
                false,
                Some((FocusDecision::LoadOnly, Duration::ZERO)),
                false
            ),
            FocusDecision::LoadOnly
        );
        assert_eq!(decide_focus(true, false, None, false), FocusDecision::Grace);
    }

    // [frame_priority] 帧率可取 → 不因叠加窗口降级为仅调频（开发者需求：帧率悬浮窗不该导致 FAS 异常退出）
    #[test]
    fn obtainable_frame_rate_prevents_load_only_downgrade() {
        // VisibleUnfocused（叠加窗口盖住游戏）但帧源仍在产出 → 维持帧驱动
        assert_eq!(
            decide_focus(
                true,
                false,
                Some((FocusDecision::LoadOnly, Duration::ZERO)),
                true
            ),
            FocusDecision::Focused
        );
    }

    #[test]
    fn stale_frame_source_still_downgrades_to_load_only() {
        // 取不到帧 → 仍按原样降 LoadOnly（保持现状，不改变叠加窗口的处理）
        assert_eq!(
            decide_focus(
                true,
                false,
                Some((FocusDecision::LoadOnly, Duration::ZERO)),
                false
            ),
            FocusDecision::LoadOnly
        );
    }

    #[test]
    fn hidden_overlay_still_exits_even_when_frame_rate_is_obtainable() {
        // 真正隐藏（Grace）不受帧率规则影响 —— 「有叠加窗口时退出 FAS 是预期行为」语义不变
        assert_eq!(
            decide_focus(
                false,
                false,
                Some((FocusDecision::Grace, Duration::ZERO)),
                true
            ),
            FocusDecision::Grace
        );
        // 无窗口证据（None）也不因帧率可用而被拉回 Focused，仍走 Grace
        assert_eq!(decide_focus(true, false, None, true), FocusDecision::Grace);
    }

    #[test]
    fn focused_evidence_unaffected_by_frame_rate_flag() {
        // 已确证 Focused 的路径与帧率可用性无关
        assert_eq!(
            decide_focus(
                false,
                false,
                Some((FocusDecision::Focused, Duration::ZERO)),
                false
            ),
            FocusDecision::Focused
        );
        assert_eq!(
            decide_focus(
                false,
                false,
                Some((FocusDecision::Focused, Duration::ZERO)),
                true
            ),
            FocusDecision::Focused
        );
    }

    #[test]
    fn expired_load_only_never_revives_from_frame_rate() {
        // 证据过期（>= TTL）→ 先落 Grace，帧率规则不参与（过期证据不得续命）
        assert_eq!(
            decide_focus(
                true,
                false,
                Some((FocusDecision::LoadOnly, WINDOW_EVIDENCE_MAX_AGE)),
                true
            ),
            FocusDecision::Grace
        );
    }

    #[test]
    fn same_owner_pair_short_circuits_regardless_of_frame_rate() {
        // 两 owner 相同 → 直接 Focused（短路不看证据），与帧率无关
        assert_eq!(
            decide_focus(true, true, None, false),
            FocusDecision::Focused
        );
        assert_eq!(decide_focus(true, true, None, true), FocusDecision::Focused);
    }

    #[test]
    fn focused_window_can_prove_focus_despite_raw_candidate() {
        assert_eq!(
            decide_focus(
                false,
                false,
                Some((FocusDecision::Focused, Duration::ZERO)),
                false
            ),
            FocusDecision::Focused
        );
    }

    #[test]
    fn expired_window_cannot_keep_owner_alive() {
        assert_eq!(
            decide_focus(
                true,
                false,
                Some((FocusDecision::LoadOnly, WINDOW_EVIDENCE_MAX_AGE)),
                false
            ),
            FocusDecision::Grace
        );
    }

    #[test]
    fn event_must_agree_on_generation() {
        assert!(!event_matches_snapshot(
            "game", 22, 7, "game", 22, 8, None, None
        ));
        assert!(event_matches_snapshot(
            "game", 22, 7, "game", 22, 7, None, None
        ));
    }

    #[test]
    fn old_generation_event_cannot_renew_current_session() {
        assert!(!event_matches_snapshot(
            "game", 11, 3, "game", 22, 4, None, None
        ));
        assert!(!event_matches_snapshot(
            "old.game", 11, 3, "new.game", 11, 3, None, None
        ));
    }

    #[test]
    fn same_pid_reused_process_is_rejected_by_starttime() {
        assert!(!event_matches_snapshot(
            "game",
            22,
            7,
            "game",
            22,
            7,
            Some(1000),
            Some(2000)
        ));
        assert!(event_matches_snapshot(
            "game",
            22,
            7,
            "game",
            22,
            7,
            Some(2000),
            Some(2000)
        ));
    }

    #[test]
    fn missing_starttime_on_either_side_does_not_break_match() {
        assert!(starttime_proves_same_process(None, Some(2000)));
        assert!(starttime_proves_same_process(Some(1000), None));
        assert!(starttime_proves_same_process(None, None));
    }
}
