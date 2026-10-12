//! mod.rs: [mods]

mod controller;
mod fps_window;
mod frame_pipeline;
mod gear_state;
mod pid;
mod pid_jank;
mod policy_controller;
mod policy_mgmt;

pub use controller::FasController;

#[cfg(test)]
pub(crate) use controller::tests::controller_with_policy;
