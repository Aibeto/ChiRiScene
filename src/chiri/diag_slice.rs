//! diag_slice.rs - [policy] [reuse]
//! Same-frame cooperative work slices; never discard a candidate or defer it to another tick.
use std::time::Duration;

// [policy]
pub fn slow_anchor(elapsed: Duration) -> bool {
    elapsed > Duration::from_millis(200)
}

pub fn slice_due(completed: usize) -> bool {
    completed != 0 && completed % 16 == 0
}

pub fn anchor_rows<T>(candidates: impl IntoIterator<Item = T>, mut render: impl FnMut(T) -> Option<String>) -> Vec<String> {
    candidates.into_iter().filter_map(|candidate| render(candidate)).collect()
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
