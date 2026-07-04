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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayRowRole {
    Item,
    Header,
    TopHit,
    Status,
    Calculator,
    QuickLaunch,
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
    SearchResultsReady,
    /// Pin an app to Quick Launch by title.
    PinApp(String),
    /// Unpin an app from Quick Launch by title.
    UnpinApp(String),
    /// Add an app to Quick Launch by path.
    AddToQuickLaunch(String),
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
    pub help_config_path: String,
    pub hotkey_hint: String,
    pub hotkey_issue_active: bool,
    pub game_mode_enabled: bool,
    pub theme: Theme,
    pub rows: Vec<OverlayRow>,
    pub selected: usize,
    pub visible: bool,
    pub has_focus: bool,
    pub idle_cache_trim_ms: u32,
    pub active_memory_target_mb: u16,
    pub ui_warm_release_ms: u32,
    /// Quick Launch items for idle state (empty query).
    pub quick_launch_items: Vec<QuickLaunchItem>,
    /// Whether Quick Launch is visible (query is empty).
    pub quick_launch_visible: bool,
}

impl Default for ShimState {
    fn default() -> Self {
        Self {
            query: String::new(),
            status_text: String::new(),
            placeholder_hint: None,
            help_config_path: String::new(),
            hotkey_hint: "Ctrl+Space".into(),
            hotkey_issue_active: false,
            game_mode_enabled: false,
            theme: Theme::Dark,
            rows: Vec::new(),
            selected: 0,
            visible: false,
            has_focus: false,
            idle_cache_trim_ms: 90_000,
            active_memory_target_mb: 72,
            ui_warm_release_ms: 5_000,
            quick_launch_items: Vec::new(),
            quick_launch_visible: false,
        }
    }
}
