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
pub(crate) fn decide_focus(
    same_foreground_owner: bool,
    same_raw_owner: bool,
    evidence: Option<(FocusDecision, Duration)>,
) -> FocusDecision {
    if same_foreground_owner && same_raw_owner {
        return FocusDecision::Focused;
    }
    evidence
        .filter(|(_, age)| *age < WINDOW_EVIDENCE_MAX_AGE)
        .map_or(FocusDecision::Grace, |(decision, _)| decision)
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
        assert_eq!(decide_focus(true, true, None), FocusDecision::Focused);
    }

    #[test]
    fn ignored_system_overlay_does_not_renew_from_filtered_game() {
        assert_eq!(decide_focus(true, false, None), FocusDecision::Grace);
    }

    #[test]
    fn visible_overlay_keeps_load_only_without_extending_unknown_grace() {
        assert_eq!(
            decide_focus(true, false, Some((FocusDecision::LoadOnly, Duration::ZERO))),
            FocusDecision::LoadOnly
        );
        assert_eq!(decide_focus(true, false, None), FocusDecision::Grace);
    }

    #[test]
    fn focused_window_can_prove_focus_despite_raw_candidate() {
        assert_eq!(
            decide_focus(false, false, Some((FocusDecision::Focused, Duration::ZERO))),
            FocusDecision::Focused
        );
    }

    #[test]
    fn expired_window_cannot_keep_owner_alive() {
        assert_eq!(
            decide_focus(
                true,
                false,
                Some((FocusDecision::LoadOnly, WINDOW_EVIDENCE_MAX_AGE))
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
