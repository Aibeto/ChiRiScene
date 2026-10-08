//! affinity_retry.rs - 区块索引: [retry] [missing] [tests]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// [retry]
#[derive(Default)]
pub(crate) struct InvalidMaskRetry {
    failure: Option<InvalidMaskFailure>,
}

struct InvalidMaskFailure {
    starttime: u64,
    mask: Vec<usize>,
    failures: u32,
    retry_at: Instant,
}

impl InvalidMaskRetry {
    pub(crate) fn is_active(&self) -> bool {
        self.failure.is_some()
    }

    pub(crate) fn should_skip(&mut self, starttime: u64, mask: &[usize], now: Instant) -> bool {
        match &self.failure {
            Some(failure) if failure.starttime == starttime && failure.mask == mask => {
                now < failure.retry_at
            }
            _ => {
                self.clear();
                false
            }
        }
    }

    pub(crate) fn record(
        &mut self,
        errno: Option<i32>,
        starttime: u64,
        mask: &[usize],
        now: Instant,
    ) {
        // 仅 Linux EINVAL 建立冷却；成功与其他错误清除连续失败状态。
        if errno != Some(22) {
            self.clear();
            return;
        }
        let failures = self.failure.as_ref().map_or(1, |failure| {
            if failure.starttime == starttime && failure.mask == mask {
                failure.failures.saturating_add(1).min(4)
            } else {
                1
            }
        });
        self.failure = Some(InvalidMaskFailure {
            starttime,
            mask: mask.to_vec(),
            failures,
            retry_at: now + Duration::from_secs(4 << (failures - 1)),
        });
    }

    pub(crate) fn clear(&mut self) {
        self.failure = None;
    }
}

// [missing]
pub(crate) struct MissingNodeCache {
    retry_at_ms: AtomicU64,
}

impl Default for MissingNodeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MissingNodeCache {
    pub(crate) const fn new() -> Self {
        Self {
            retry_at_ms: AtomicU64::new(0),
        }
    }

    pub(crate) fn should_skip(&self, now_ms: u64) -> bool {
        let retry_at_ms = self.retry_at_ms.load(Ordering::Relaxed);
        retry_at_ms != 0 && now_ms < retry_at_ms
    }

    pub(crate) fn probe_due(&self, now_ms: u64) -> bool {
        let retry_at_ms = self.retry_at_ms.load(Ordering::Relaxed);
        retry_at_ms != 0 && now_ms >= retry_at_ms
    }

    pub(crate) fn allows_write(&self, normal_write_due: bool, now_ms: u64) -> bool {
        !self.should_skip(now_ms) && (normal_write_due || self.probe_due(now_ms))
    }

    pub(crate) fn record(&self, errno: Option<i32>, now_ms: u64) {
        let retry_at_ms = if errno == Some(2) {
            now_ms.saturating_add(600_000)
        } else {
            0
        };
        self.retry_at_ms.store(retry_at_ms, Ordering::Relaxed);
    }

    pub(crate) fn clear(&self) {
        self.retry_at_ms.store(0, Ordering::Relaxed);
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn invalid_mask_backoff_is_bounded_and_identity_scoped() {
        let origin = Instant::now();
        let mut retry = InvalidMaskRetry::default();
        let mask = [4, 5, 7];
        let mut now = origin;
        for seconds in [4, 8, 16, 32, 32] {
            retry.record(Some(22), 100, &mask, now);
            assert!(retry.should_skip(100, &mask, now));
            assert!(retry.should_skip(100, &mask, now + Duration::from_secs(seconds - 1)));
            now += Duration::from_secs(seconds);
            assert!(!retry.should_skip(100, &mask, now));
        }
        assert!(!retry.should_skip(101, &mask, now));
        retry.record(Some(22), 100, &mask, now);
        assert!(!retry.should_skip(100, &[4, 5], now));
        retry.record(Some(22), 100, &[4, 5], now);
        assert!(!retry.should_skip(100, &[4, 5], now + Duration::from_secs(4)));
        retry.record(None, 100, &[4, 5], now);
        assert!(!retry.should_skip(100, &[4, 5], now));
        retry.record(Some(1), 100, &mask, now);
        assert!(!retry.should_skip(100, &mask, now));
        retry.record(Some(22), 100, &mask, now);
        assert!(retry.is_active());
        retry.clear();
        assert!(!retry.should_skip(100, &mask, now));
    }

    #[test]
    fn reused_identity_and_changed_mask_cancel_active_cooldown() {
        let now = Instant::now();
        let mask = [4, 5, 7];
        let mut retry = InvalidMaskRetry::default();
        retry.record(Some(22), 100, &mask, now);
        assert!(!retry.should_skip(101, &mask, now));
        retry.record(Some(22), 101, &mask, now);
        assert!(!retry.should_skip(101, &[4], now));
        retry.record(Some(22), 101, &[4], now);
        assert!(!retry.should_skip(101, &[4], now + Duration::from_secs(4)));
    }

    #[test]
    fn skipped_requests_do_not_extend_missing_deadline_and_probe_bypasses_same_value_guard() {
        let cache = MissingNodeCache::default();
        cache.record(Some(2), 0);
        // 值变化与恢复请求均只改变正常写入守卫，不清除节点缺失状态。
        assert!(!cache.allows_write(true, 10_000));
        assert!(!cache.allows_write(true, 590_000));
        assert!(!cache.allows_write(false, 599_999));
        assert!(cache.allows_write(false, 600_000));
        cache.record(None, 600_000);
        assert!(!cache.allows_write(false, 600_001));
        assert!(cache.allows_write(true, 660_000));
    }

    #[test]
    fn missing_node_cache_only_tracks_enoent_and_expires() {
        let cache = MissingNodeCache::default();
        assert!(!cache.should_skip(0));
        cache.record(Some(2), 0);
        assert!(cache.should_skip(599_999));
        assert!(!cache.should_skip(600_000));
        assert!(cache.probe_due(600_000));
        cache.record(None, 600_000);
        assert!(!cache.should_skip(600_001));
        assert!(!cache.probe_due(600_001));
        cache.record(Some(13), 600_001);
        assert!(!cache.should_skip(600_002));
        cache.record(Some(2), 600_002);
        cache.clear();
        assert!(!cache.should_skip(600_003));
    }
}
