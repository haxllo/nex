//! WebView2 overlay for Nex.
//!
//! A borderless, transparent, always-on-top tao window hosts a wry
//! WebView that renders the premium cmdk-style UI from embedded
//! HTML/CSS/JS assets. The Rust side pushes state to JS via
//! `evaluate_script("window.nex.apply(..)")` and receives input via
//! the wry IPC handler, translating it into the existing
//! [`OverlayEvent`] channel the runtime worker drains.
//!
//! Architecture:
//!   * [`host`]    — tao event loop + wry WebView + warm-then-release.
//!   * [`model`]   — `OverlayEvent`, `OverlayRow`, `OverlayRowRole`,
//!                    `ShimState`, `Theme`.
//!   * [`icons`]   — LRU PNG-byte cache for the `nexasset://icon/…`
//!                    custom protocol.
//!   * [`shim`]    — `NativeOverlayShell` imperative API the runtime
//!                    speaks; forwards setters to the host via
//!                    `UiCommand` events.
//!   * [`platform`] — system-theme detect, instance signaling
//!                    (`FindWindowW` + registered window messages).
//!   * [`hotkey`]  — `RegisterHotKey` + `GetMessageW` listener thread.
//!   * [`tray`]    — system tray icon with context menu.
//!   * [`indexing_progress`] — progress window for first-time indexing.

#[cfg(target_os = "windows")]
pub(crate) mod host;
#[cfg(target_os = "windows")]
pub(crate) mod hotkey;
#[cfg(target_os = "windows")]
pub(crate) mod icons;
#[cfg(target_os = "windows")]
pub(crate) mod model;
#[cfg(target_os = "windows")]
pub(crate) mod platform;
#[cfg(target_os = "windows")]
pub(crate) mod shim;
#[cfg(target_os = "windows")]
pub(crate) mod tray;
#[cfg(target_os = "windows")]
pub(crate) mod indexing_progress;

#[cfg(target_os = "windows")]
pub use model::{OverlayEvent, OverlayRow, OverlayRowRole};

#[cfg(target_os = "windows")]
pub use shim::NativeOverlayShell;

#[cfg(target_os = "windows")]
pub use platform::{
    is_instance_window_present, signal_existing_instance_quit, signal_existing_instance_show,
};

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
