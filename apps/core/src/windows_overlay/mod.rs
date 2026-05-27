//! Windows native overlay shell for Nex launcher.
//!
//! This module implements a DirectComposition-aware overlay window using raw Win32 APIs.
//! It provides the UI layer for Nex: search input, results list, status bar,
//! system tray icon, animations, theming, and icon caching.

#[cfg(target_os = "windows")]
pub(crate) mod animation;
#[cfg(target_os = "windows")]
pub(crate) mod d2d_renderer;
#[cfg(target_os = "windows")]
pub(crate) mod gdiplus_rendering;
#[cfg(target_os = "windows")]
pub(crate) mod icon_cache;
#[cfg(target_os = "windows")]
pub(crate) mod icon_loader;
#[cfg(target_os = "windows")]
pub(crate) mod input;
#[cfg(target_os = "windows")]
pub(crate) mod layout;
#[cfg(target_os = "windows")]
pub(crate) mod painting;
#[cfg(target_os = "windows")]
pub(crate) mod state;
#[cfg(target_os = "windows")]
pub(crate) mod tray;
#[cfg(target_os = "windows")]
pub(crate) mod types;
#[cfg(target_os = "windows")]
pub(crate) mod window;

// Re-export the public API
#[cfg(target_os = "windows")]
pub use types::{NativeOverlayShell, OverlayEvent, OverlayRow, OverlayRowRole};

// Forward instance-notification helpers from the window module
#[cfg(target_os = "windows")]
pub(crate) use window::is_instance_window_present;
#[cfg(target_os = "windows")]
pub(crate) use window::signal_existing_instance_quit;
#[cfg(target_os = "windows")]
pub(crate) use window::signal_existing_instance_show;

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
