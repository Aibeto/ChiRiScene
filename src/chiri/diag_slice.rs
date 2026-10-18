//! diag_slice.rs - [policy] [reuse]
//! 同帧协作工作切片：候选只在本帧内处理完，不丢弃也不推迟到下一 tick
use std::time::Duration;

// [policy]
pub fn slow_anchor(elapsed: Duration) -> bool {
    elapsed > Duration::from_millis(200)
}

pub fn slice_due(completed: usize) -> bool {
    completed != 0 && completed % 16 == 0
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
