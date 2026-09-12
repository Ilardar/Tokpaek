//! Tokpaek — a movable tray strip showing AI-tool quota windows.
//!
//! Library target: the binary is a thin entry point over these modules, so
//! integration tests can reach the same code the app runs.

pub mod active;
pub mod app;
pub mod config;
pub mod diaglog;
pub mod frame_policy;
pub mod gauge;
pub mod i18n;
pub mod icon;
pub mod menu;
pub mod providers;
pub mod scheduler;
pub mod settings_ui;
pub mod shortcuts;
pub mod tray;
pub mod update;
pub mod windowing;
pub mod winproc;
