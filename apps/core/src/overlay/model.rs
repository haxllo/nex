//! Overlay data types shared by host, shim, and the runtime.
//!
//! `ShimState` is the framework-agnostic snapshot of overlay state
//! that the WebView host reads to build the JSON snapshot pushed to
//! the web UI. The IPC handler writes back the live `query`/`selected`
//! values so the runtime's getters stay correct.

/// One row in the visible result list. Mirrors the legacy
/// `OverlayRow` so `runtime_overlay_rows` can build it without
/// changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayRow {
    pub role: OverlayRowRole,
    pub result_index: Option<usize>,
    pub kind: String,
    pub title: String,
    pub path: String,
    pub icon_path: String,
    /// Base64 data URI for clipboard image thumbnails.
    pub clipboard_thumbnail: Option<String>,
    /// Base64 data URI for full clipboard image (sent on expand).
    pub clipboard_full_image: Option<String>,
    /// Tile size for bento grid layout.
    pub tile_size: Option<TileSize>,
}

/// Tile sizes for the clipboard history bento grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileSize {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayRowRole {
    Item,
    Header,
    TopHit,
    Status,
    Calculator,
    QuickLaunch,
    ShowAllApps,
    /// Clipboard history bento grid item.
    ClipboardHistory,
}

/// Events the runtime callback receives on the worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayEvent {
    Hotkey(i32),
    QueryChanged(String),
    MoveSelection(i32),
    Submit,
    Escape,
    ExternalShow,
    ExternalQuit,
    TrayToggleGameMode,
    TrayCheckForUpdates,
    /// Status text result from a background updater run (shown in overlay).
    UpdateStatus(String),
    TrayLock,
    TraySleep,
    TrayShutdown,
    TrayRestart,
    TraySignOut,
    /// Shutdown requested from the power menu after in-overlay confirmation.
    PowerMenuShutdown,
    /// Restart requested from the power menu after in-overlay confirmation.
    PowerMenuRestart,
    /// Refocus the overlay search input without showing/reshowing the overlay.
    FocusSearchInput,
    SearchResultsReady,
    /// Deliver the remainder of the Show-all-apps index after the first
    /// page has painted (self-scheduled by the runtime worker).
    ShowAllAppsFillRest,
    /// Pin an app to Quick Launch by title.
    PinApp(String),
    /// Unpin an app from Quick Launch by title.
    UnpinApp(String),
    /// Add an app to Quick Launch by path.
    AddToQuickLaunch(String),
    /// Context menu action: { action, title, path }.
    ContextAction(String, String, String),
    /// Periodic 250ms heartbeat to drive config reload and dead-listener
    /// recovery even when no user events are arriving.
    Tick,
    /// Open the settings window.
    OpenSettings,
    /// Settings page requested a save; payload is the raw cfg JSON.
    SaveSettings(String),
    /// The settings window was closed (hidden).
    SettingsClosed,
    /// A hotkey combo captured natively for the settings page.
    /// Empty string = recording cancelled.
    HotkeyRecorded(String),

}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Theme {
    Dark,
    Light,
}

/// A single item in the Quick Launch section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickLaunchItem {
    pub title: String,
    pub path: String,
    pub subtitle: String,
    pub icon_path: String,
    pub is_pinned: bool,
}

/// The shared, framework-agnostic snapshot of overlay state. The
/// [`crate::overlay::shim::NativeOverlayShell`] owns this behind an
/// `Arc<Mutex<>>`; the WebView host (`crate::overlay::host`) reads it
/// to build the JSON snapshot pushed to the web UI, and the IPC
/// handler writes back the live `query`/`selected` values so the
/// runtime's getters stay correct.
#[derive(Debug, Clone)]
pub struct ShimState {
    pub query: String,
    pub status_text: String,
    pub placeholder_hint: Option<String>,
    pub hotkey_hint: String,
    pub hotkey_issue_active: bool,
    pub game_mode_enabled: bool,
    pub grid_view: bool,
    pub theme: Theme,
    /// System accent color as `#RRGGBB` (from DWM AccentColor), if readable.
    pub accent_color: Option<String>,
    pub rows: Vec<OverlayRow>,
    pub selected: usize,
    pub visible: bool,
    pub has_focus: bool,
    pub hwnd: isize,
    pub idle_cache_trim_ms: u32,
    pub active_memory_target_mb: u16,
    pub ui_warm_release_ms: u32,
    /// Quick Launch items for idle state (empty query).
    pub quick_launch_items: Vec<QuickLaunchItem>,
    /// Whether Quick Launch is visible (query is empty).
    pub quick_launch_visible: bool,
    /// Whether the bento grid view is active (clipboard history).
    pub bento_view: bool,
}

impl Default for ShimState {
    fn default() -> Self {
        Self {
            query: String::new(),
            status_text: String::new(),
            placeholder_hint: None,
            hotkey_hint: "Ctrl+Space".into(),
            hotkey_issue_active: false,
            game_mode_enabled: false,
            grid_view: false,
            theme: Theme::Dark,
            accent_color: None,
            rows: Vec::new(),
            selected: 0,
            visible: false,
            has_focus: false,
            hwnd: 0,
            idle_cache_trim_ms: 90_000,
            active_memory_target_mb: 72,
            ui_warm_release_ms: 5_000,
            quick_launch_items: Vec::new(),
            quick_launch_visible: false,
            bento_view: false,
        }
    }
}
