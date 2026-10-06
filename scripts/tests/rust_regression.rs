//! rust_regression.rs: [modules]
//! Portable checks of the real window parser and foreground decision modules.

#![allow(dead_code)]

// [modules]
#[path = "../../src/monitor/window_visibility.rs"]
pub(crate) mod window_visibility_source;

mod monitor {
    pub(crate) use crate::window_visibility_source as window_visibility;
}

#[path = "../../src/chiri/fas_focus.rs"]
mod fas_focus;

#[path = "../../src/chiri/fas_process.rs"]
mod fas_process;
