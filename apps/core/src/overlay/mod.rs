//! Iced 0.14 overlay for Nex.
//!
//! Replaces the legacy `windows_overlay` module that hand-rolled Win32
//! GDI / GDI+ / DWM rendering. The shell (`NativeOverlayShell`) is
//! kept as a thin shim so the runtime layer (`runtime_loop`,
//! `runtime_overlay_rows`, etc.) can keep calling the same imperative
//! setters as before. Internally the shim forwards every call into a
//! single shared `Model` and posts the corresponding `Message` to a
//! dedicated Iced runtime thread.
//!
//! Architecture:
//!   * [`model`]    — the Elm-style `Model`, `Message`, and `update()`.
//!   * [`view`]     — the pure widget tree built from the `Model`.
//!   * [`theme`]    — light + dark palettes and theme detection.
//!   * [`geometry`] — layout tokens that map 1:1 to the legacy
//!                     `windows_overlay::types` constants.
//!   * [`icons`]    — LRU image cache for .ico / .png file paths.
//!   * [`platform`] — `RegisterHotKey`, `Shell_NotifyIcon`, instance
//!                     signal, system-theme registry read.
//!   * [`shim`]     — the `NativeOverlayShell` imperative API the
//!                     runtime speaks.

#[cfg(target_os = "windows")]
pub(crate) mod boot;
#[cfg(target_os = "windows")]
pub(crate) mod geometry;
#[cfg(target_os = "windows")]
pub(crate) mod icons;
#[cfg(target_os = "windows")]
pub(crate) mod model;
#[cfg(target_os = "windows")]
pub(crate) mod platform;
#[cfg(target_os = "windows")]
pub(crate) mod shim;
#[cfg(target_os = "windows")]
pub(crate) mod theme;
#[cfg(target_os = "windows")]
pub(crate) mod view;

#[cfg(target_os = "windows")]
pub use model::{OverlayEvent, OverlayRow, OverlayRowRole};

#[cfg(target_os = "windows")]
pub use shim::NativeOverlayShell;

// Single-instance signalling helpers re-exported the same way the
// legacy `windows_overlay` module exported them. Used by
// `runtime_loop.rs` and `runtime_process.rs`.
#[cfg(target_os = "windows")]
pub(crate) use platform::is_instance_window_present;
#[cfg(target_os = "windows")]
pub(crate) use platform::signal_existing_instance_quit;
#[cfg(target_os = "windows")]
pub(crate) use platform::signal_existing_instance_show;

#[cfg(not(target_os = "windows"))]
pub fn is_instance_window_present() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn signal_existing_instance_show() -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
pub fn signal_existing_instance_quit() -> Result<bool, String> {
    Ok(false)
}
