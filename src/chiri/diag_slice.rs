//! diag_slice.rs - [policy] [reuse]
//! 同帧协作工作切片：候选只在本帧内处理完，不丢弃也不推迟到下一 tick
use std::time::Duration;

// [policy]
pub fn slow_anchor(elapsed: Duration) -> bool {
    elapsed > Duration::from_millis(200)
}

const SLICE_YIELD_INTERVAL: usize = 256;

pub fn slice_due(completed: usize) -> bool {
    completed != 0 && completed % SLICE_YIELD_INTERVAL == 0
}

pub fn anchor_rows<T>(
    candidates: impl IntoIterator<Item = T>,
    mut render: impl FnMut(T) -> Option<String>,
) -> Vec<String> {
    candidates
        .into_iter()
        .filter_map(|candidate| render(candidate))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{anchor_rows, slice_due};

    #[test]
    fn large_snapshot_slices_keep_all_rows_and_yield_every_256_candidates() {
        let candidates: Vec<usize> = (0..=512).collect();
        let rows = anchor_rows(candidates.clone(), |candidate| Some(candidate.to_string()));
        let yields: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|&completed| slice_due(completed))
            .collect();

        assert_eq!(rows.len(), candidates.len());
        assert_eq!(rows.first().map(String::as_str), Some("0"));
        assert_eq!(rows.last().map(String::as_str), Some("512"));
        assert_eq!(yields, [256, 512]);
    }
}

// [reuse]
#[derive(Default)]
pub struct FullRowCache {
    row: String,
}

impl FullRowCache {
    pub fn invalidate(&mut self) {
        self.row.clear();
    }

    pub fn get_or_render(&mut self, render: impl FnOnce() -> String) -> &str {
        if self.row.is_empty() {
            self.row = render();
        }
        &self.row
    }
}
