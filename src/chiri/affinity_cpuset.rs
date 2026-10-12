//! affinity_cpuset.rs - 区块索引: [guard] [tests]

// [guard]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CpusetCheck {
    Allowed,
    EmptyIntersection,
    Unknown,
    IdentityChanged,
    GroupChanged,
}

pub(super) fn check_group_cpuset(
    tid: i32,
    cpus: &[usize],
    expected_starttime: Option<u64>,
    mut read: impl FnMut(&str) -> Option<String>,
    mut identity: impl FnMut() -> Option<u64>,
) -> CpusetCheck {
    let Some(before) = identity() else {
        return CpusetCheck::Unknown;
    };
    if expected_starttime.is_some_and(|expected| expected != before) {
        return CpusetCheck::IdentityChanged;
    }
    let proc_path = format!("/proc/{tid}/cpuset");
    let Some(group) = read(&proc_path) else {
        return CpusetCheck::Unknown;
    };
    let group = group.strip_suffix('\n').unwrap_or(&group);
    if !group.starts_with('/') || group.split('/').any(|part| part == "." || part == "..") {
        return CpusetCheck::Unknown;
    }
    let group_path = group.trim_matches('/');
    let cpus_path = if group_path.is_empty() {
        "/dev/cpuset/cpus".to_owned()
    } else {
        format!("/dev/cpuset/{group_path}/cpus")
    };
    let Some(mask) = read(&cpus_path) else {
        return CpusetCheck::Unknown;
    };
    let (Some(after), Some(after_group)) = (identity(), read(&proc_path)) else {
        return CpusetCheck::Unknown;
    };
    if before != after {
        return CpusetCheck::IdentityChanged;
    }
    if group != after_group.strip_suffix('\n').unwrap_or(&after_group) {
        return CpusetCheck::GroupChanged;
    }
    // Reject the whole malformed list: dropping bad tokens would fabricate an empty set.
    if mask.trim().is_empty() {
        // Linux cpuset v1: an empty cpus entry inherits the nearest ancestor mask.
        // We only know the child entry here, so the effective set is unknown and
        // the caller must keep its original best-effort write fallback.
        // Also documents the cpuset-tree-missing case: when /dev/cpuset is absent
        // (read failed above), we already returned Unknown — the guard is a no-op.
        return CpusetCheck::Unknown;
    }
    let mut intersects = false;
    {
        for part in mask.trim().split(',') {
            if part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-')
            {
                return CpusetCheck::Unknown;
            }
            let range = if let Some((start, end)) = part.split_once('-') {
                start
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .zip(end.trim().parse::<usize>().ok())
            } else {
                part.parse::<usize>().ok().map(|cpu| (cpu, cpu))
            };
            let Some((start, end)) = range.filter(|&(start, end)| start <= end && end < 1024)
            else {
                return CpusetCheck::Unknown;
            };
            intersects |= cpus.iter().any(|&cpu| (start..=end).contains(&cpu));
        }
    }
    if intersects {
        CpusetCheck::Allowed
    } else {
        CpusetCheck::EmptyIntersection
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;

    fn check(cpus: &[usize], mask: Option<&str>) -> CpusetCheck {
        check_group_cpuset(
            42,
            cpus,
            Some(100),
            |path| match path {
                "/proc/42/cpuset" => Some("/background\n".into()),
                "/dev/cpuset/background/cpus" => mask.map(str::to_owned),
                _ => panic!("unexpected path {path}"),
            },
            || Some(100),
        )
    }

    #[test]
    fn empty_intersection_skips_group_write() {
        assert_eq!(
            check(&[4, 5, 6], Some("0-3")),
            CpusetCheck::EmptyIntersection
        );
    }

    #[test]
    fn empty_group_cpus_is_inherited_uncertainty_not_empty_intersection() {
        // Linux cpuset: empty cpus inherits the nearest ancestor set; treating it as the
        // empty set would permanently disable every bind on such a group.
        assert_eq!(check(&[4, 5], Some("")), CpusetCheck::Unknown);
    }

    #[test]
    fn partial_intersection_keeps_original_group_target_eligible() {
        assert_eq!(check(&[3, 4, 5], Some("0-3")), CpusetCheck::Allowed);
        assert_eq!(check(&[4, 5], Some("4-5")), CpusetCheck::Allowed);
    }

    #[test]
    fn missing_or_malformed_read_is_unknown_not_empty() {
        for mask in [
            None,
            Some("bad"),
            Some("0-3,bad"),
            Some("3-0"),
            Some("1024"),
            Some("0,,3"),
            Some("+0"),
            Some("+0-+3"),
        ] {
            assert_eq!(check(&[4, 5], mask), CpusetCheck::Unknown);
        }
        assert_eq!(
            check_group_cpuset(42, &[4], Some(100), |_| None, || Some(100)),
            CpusetCheck::Unknown
        );
    }

    #[test]
    fn identity_change_does_not_apply_old_cpuset() {
        let mut identities = vec![Some(100u64), Some(200)].into_iter();
        assert_eq!(
            check_group_cpuset(
                42,
                &[4],
                Some(100),
                |path| {
                    Some(
                        if path.starts_with("/proc") {
                            "/background"
                        } else {
                            "0-3"
                        }
                        .into(),
                    )
                },
                || identities.next().unwrap()
            ),
            CpusetCheck::IdentityChanged
        );
    }

    #[test]
    fn reused_tid_is_not_checked_against_previous_identity() {
        assert_eq!(
            check_group_cpuset(
                42,
                &[4],
                Some(100),
                |_| Some("/background".into()),
                || Some(200)
            ),
            CpusetCheck::IdentityChanged
        );
    }

    #[test]
    fn group_change_during_read_invalidates_snapshot() {
        let mut groups = ["/background", "/top-app"].into_iter();
        assert_eq!(
            check_group_cpuset(
                42,
                &[4],
                Some(100),
                |path| {
                    Some(
                        if path.starts_with("/proc") {
                            groups.next().unwrap()
                        } else {
                            "0-3"
                        }
                        .into(),
                    )
                },
                || Some(100)
            ),
            CpusetCheck::GroupChanged
        );
    }

    #[test]
    fn unknown_identity_or_group_after_read_never_becomes_empty() {
        let mut identities = vec![Some(100u64), None].into_iter();
        assert_eq!(
            check_group_cpuset(
                42,
                &[4],
                Some(100),
                |path| {
                    Some(
                        if path.starts_with("/proc") {
                            "/background"
                        } else {
                            "0-3"
                        }
                        .into(),
                    )
                },
                || identities.next().unwrap()
            ),
            CpusetCheck::Unknown
        );
        let mut groups = [Some("/background"), None].into_iter();
        assert_eq!(
            check_group_cpuset(
                42,
                &[4],
                Some(100),
                |path| {
                    if path.starts_with("/proc") {
                        groups.next().unwrap().map(str::to_owned)
                    } else {
                        Some("0-3".into())
                    }
                },
                || Some(100)
            ),
            CpusetCheck::Unknown
        );
    }

    #[test]
    fn invalid_group_paths_cannot_read_outside_cpuset_root() {
        for group in ["", "background", "/../other", "/top-app/./child"] {
            assert_eq!(
                check_group_cpuset(
                    42,
                    &[4],
                    Some(100),
                    |path| {
                        assert_eq!(path, "/proc/42/cpuset");
                        Some(group.into())
                    },
                    || Some(100)
                ),
                CpusetCheck::Unknown
            );
        }
    }

    #[test]
    fn group_name_whitespace_is_preserved() {
        assert_eq!(
            check_group_cpuset(
                42,
                &[4],
                Some(100),
                |path| match path {
                    "/proc/42/cpuset" => Some("/background \n".into()),
                    "/dev/cpuset/background /cpus" => Some("4".into()),
                    _ => None,
                },
                || Some(100)
            ),
            CpusetCheck::Allowed
        );
    }

    #[test]
    fn absent_identity_is_unknown_not_empty() {
        assert_eq!(
            check_group_cpuset(42, &[4], None, |_| Some("0-3".into()), || None),
            CpusetCheck::Unknown
        );
    }

    // [regression s12]
    #[test]
    fn large_range_end_1024_beyond_is_unknown() {
        // Some("1000-2000") → Unknown: end>=1024 range filter rejects it
        assert_eq!(
            check(&[500], Some("1000-2000")),
            CpusetCheck::Unknown
        );
    }

    #[test]
    fn upper_bound_1023_is_allowed() {
        // Some("1023") → Allowed: positive upper bound prevents < to <= regression
        assert_eq!(
            check(&[1023], Some("1023")),
            CpusetCheck::Allowed
        );
    }

    #[test]
    fn empty_cpus_is_empty_intersection() {
        // empty cpus &[] → EmptyIntersection
        assert_eq!(
            check(&[], Some("0-1023")),
            CpusetCheck::EmptyIntersection
        );
    }

    #[test]
    fn root_cpuset_and_fresh_group_membership_are_read_each_attempt() {
        let mut group = "/";
        for expected in [CpusetCheck::EmptyIntersection, CpusetCheck::Allowed] {
            assert_eq!(
                check_group_cpuset(
                    42,
                    &[4],
                    Some(100),
                    |path| {
                        match path {
                            "/proc/42/cpuset" => Some(group.into()),
                            "/dev/cpuset/cpus" => Some("0-3".into()),
                            "/dev/cpuset/top-app/cpus" => Some("4-7".into()),
                            _ => panic!("unexpected path {path}"),
                        }
                    },
                    || Some(100)
                ),
                expected
            );
            group = "/top-app";
        }
    }
}
