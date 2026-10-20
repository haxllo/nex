//! WebView2 overlay host (tao window + wry WebView).
//!
//! The current overlay implementation: a single borderless, transparent,
//! always-on-top tao window hosts a wry WebView that renders the
//! premium cmdk-style UI from embedded HTML/CSS/JS assets. The Rust
//! side pushes state to JS via `ICoreWebView2::PostWebMessageAsString`
//! (fire-and-forget, never blocks the host event loop) and receives
//! input via the wry IPC handler, translating it into the existing
//! [`OverlayEvent`] channel the runtime worker already drains.
//!
//! The window is positioned on the monitor under the cursor, grabs
//! foreground focus on show (the `AttachThreadInput` trick — winit/tao
//! cannot steal focus reliably on its own), and resizes to hug the
//! web content so the DWM acrylic backdrop wraps the panel exactly.
//!
//! Memory: the WebView stays warm for the process lifetime so open
//! timing is consistent. After hide, a warm-release timer clears the
//! decoded icon cache (the main reclaimable overlay heap) while
//! leaving the page loaded.
//!
//! [`run`] MUST be called on the main thread (tao, like winit, panics
//! if the event loop is created off the main thread).

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Debounce window for live resize requests. Only applied to growth
/// requests — shrink requests bypass debounce to avoid exposing
/// acrylic while the native window waits for the timer to fire.
const RESIZE_DEBOUNCE_MS: u64 = 100;

/// Growth jumps larger than this bypass the debounce and apply at once —
/// a big expansion (e.g. Show-all-apps) painting before the debounced
/// grow lands reads as laggy. Small per-keystroke growth stays debounced.
const RESIZE_IMMEDIATE_GROWTH: f64 = 80.0;

/// Tracks whether the overlay window is currently visible.
/// Used by the WM_INPUT handler to decide whether to toggle the overlay
/// on Win key-down (only toggle when visible; hook handles first press).
pub(crate) static OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);

/// The overlay window's HWND, cached at startup so other modules (e.g.
/// live hotkey updates) can re-register the raw-input sink.
static OVERLAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Re-arm the raw-input sink after a live hotkey update. The registration
/// itself no longer depends on the hotkey (see `register_raw_input_sink`);
/// re-registering stays cheap and keeps the sink armed across updates.
pub(crate) fn rearm_raw_input_sink() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    register_raw_input_sink(hwnd as HWND);
}

/// Set to `true` before `run_return` enters and `false` after it
/// returns.  Guarded proxy sends ( [`try_send_ui`] ) check this so
/// straggler messages cannot land on a destroyed tao runner.
static LOOP_ALIVE: AtomicBool = AtomicBool::new(false);

/// Send a [`UiCommand`] through the event-loop proxy only if the
/// event loop is still alive. Silently discards the send error
/// (mirrors the old `let _ = proxy.send_event(…)` pattern).
fn try_send_ui(proxy: &EventLoopProxy<UiCommand>, cmd: UiCommand) {
    if LOOP_ALIVE.load(Ordering::SeqCst) {
        let _ = proxy.send_event(cmd);
    }
}

fn start_chat_picker_listing(
    proxy: &EventLoopProxy<UiCommand>,
    generation: u64,
    directory: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let task_proxy = proxy.clone();
    std::thread::Builder::new()
        .name("nex-chat-picker-browse".into())
        .spawn(move || {
            let result = match directory {
                Some(path) => super::file_picker::chat_picker_directory(path),
                None => Ok(super::file_picker::chat_picker_locations()),
            };
            try_send_ui(
                &task_proxy,
                UiCommand::ChatPickerListed { generation, result },
            );
        })
        .map(|_| ())
        .map_err(|e| format!("Nex couldn't open this folder: {e}"))
}

fn start_chat_picker_file_read(
    proxy: &EventLoopProxy<UiCommand>,
    generation: u64,
    paths: Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let task_proxy = proxy.clone();
    std::thread::Builder::new()
        .name("nex-chat-picker-read".into())
        .spawn(move || {
            let result = super::file_picker::read_chat_files(paths);
            try_send_ui(
                &task_proxy,
                UiCommand::ChatPickerFilesRead { generation, result },
            );
        })
        .map(|_| ())
        .map_err(|e| format!("Nex couldn't read the selected files: {e}"))
}

use crossbeam_channel::Sender;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use wry::http::{header::CONTENT_TYPE, Request, Response};
use wry::WebViewExtWindows;
use wry::{WebView, WebViewBuilder};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DEFAULT,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClassNameW, GetForegroundWindow, GetShellWindow, IsWindow,
    RegisterWindowMessageW, SetForegroundWindow, SetWindowPos, HWND_BOTTOM, HWND_TOPMOST,
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WM_INPUT,
};

use crate::overlay::icons::IconCache;
use crate::overlay::model::Theme;
use crate::overlay::model::{OverlayEvent, OverlayRowRole, ShimState, TileSize};

const WINDOW_WIDTH: f64 = 700.0;
const INITIAL_HEIGHT: f64 = 60.0;
const MAX_HEIGHT: f64 = 680.0;
const FOCUS_GRACE_MS: u64 = 400;

/// How long after a Show completes focus-loss is treated as a flap-pair
/// rather than a genuine dismissal. During this window we always defer
/// to the 150ms deferred-hide path, which checks `has_focus` after
/// WebView2 settles and only fires Escape if focus did not come back.
///
/// This is the fix for the "first press after long idle flashes then
/// hides" bug: after `ui_warm_release_ms` (5s) of idle the user can be
/// well outside the old 1500ms re-assert window, so a single focus-flap
/// from WebView2 acquiring its input element fires Escape immediately.
/// 2000ms covers typical Explorer/Task Manager re-assertion cycles while
/// still feeling snappy on a real outside click.
const POST_SHOW_QUIESCENCE_MS: u64 = 2000;

///Embedded UI assets for settings window
const SETTINGS_HTML: &str = include_str!("../../assets/settings.html");
const SETTINGS_JS: &str = include_str!("../../assets/settings.js");

/// Embedded web UI assets (premium dark cmdk UI).
const INDEX_HTML: &str = include_str!("../../assets/index.html");
pub(crate) const STYLE_CSS: &str = include_str!("../../assets/style.css");
const APP_JS: &str = include_str!("../../assets/app.js");
const MEDIA_CLOCK_JS: &str = include_str!("../../assets/media_clock.js");
const DISABLE_NATIVE_CONTEXT_MENU: &str = r#"
document.addEventListener('contextmenu', function (event) {
  event.preventDefault();
}, true);
"#;
/// Commands the shim posts to the UI thread via the event-loop proxy.
#[derive(Debug, Clone)]
pub(crate) enum UiCommand {
    /// The web page finished loading and registered `window.nex`.
    WebviewReady,
    /// Re-push the current [`ShimState`] snapshot to the page.
    Apply,
    /// Icons decoded in the background are now cached — re-send the
    /// icon data JSON so the page can patch placeholder <img> elements.
    /// The payload contains only newly decoded icon paths, avoiding a
    /// full snapshot re-encode on every icon batch.
    ApplyIcons(Vec<String>),
    /// Only the selected index changed — send a lightweight update.
    SelectChanged(usize),
    /// Ready now-playing media JSON from the background media worker.
    /// Posted, never computed, on the event-loop thread.
    ApplyMediaData(String),
    /// Fetched What's New content JSON from the background fetch thread.
    ApplyWhatsNew(String),
    /// Chat state and streamed response updates.
    ChatData(String),
    /// Open the in-WebView attachment picker.
    OpenChatFilePicker,
    ChatPickerListed {
        generation: u64,
        result: Result<super::file_picker::ChatPickerDirectory, String>,
    },
    ChatPickerFilesRead {
        generation: u64,
        result: Result<Vec<super::file_picker::SelectedChatFile>, String>,
    },
    ChatPickerNavigate(u64),
    ChatPickerUp,
    ChatPickerAdd(Vec<u64>),
    ChatPickerCancel {
        dismiss_overlay: bool,
    },
    /// Only the status text changed — send a lightweight update.
    ApplyStatus,
    /// Show + focus the overlay (builds the WebView if not yet created).
    Show,
    /// Focus the search input without showing/reshowing the overlay.
    FocusInput,
    /// Hide the overlay and arm the warm-release timer.
    Hide,
    /// Hide and signal completion (used for synchronous hide-before-launch).
    HideSync(std::sync::mpsc::Sender<()>),
    /// The glass worker declared its D2D engine persistently failed. The
    /// panel switches to the opaque fallback instead of a transparent page
    /// over a surface that will not present.
    GlassFailed,
    /// Fired by the warm-release timer; if still hidden and the
    /// generation matches, clears the icon cache while keeping the
    /// WebView warm for consistent re-open timing.
    Teardown(u64),
    /// The page painted after a push_state — trigger deferred show.
    Painted,
    /// The native glass renderer presented a frame at this panel size. The
    /// layer is uncloaked only once such an ack matches the size the page
    /// has, so the first pixels seen belong to the current backdrop.
    GlassPresented {
        w: u32,
        h: u32,
    },
    /// The glass material changed appearance (light surface + dark text,
    /// or the reverse). The page swaps its palette to follow.
    GlassPalette(bool),
    /// The page measured its content height (CSS px); resize to hug it.
    Resize {
        h: f64,
        immediate: bool,
    },
    /// Exit the event loop (clean shutdown).
    Quit,
    /// Debounce timer fired — apply the coalesced resize height.
    ApplyResize,
    /// Delayed keyboard state check (posted ~200ms after hide).
    CheckKeyboardState(Instant),
    /// Open/show the settings window, preload with a config snapshot.
    OpenSettings {
        snapshot: String,
    },
    /// Result of a settings save attempt, pushed into the settings page.
    SettingsSaveResult {
        json: String,
    },
    /// Push a captured hotkey combo into the settings page.
    SettingsHotkeyRecorded {
        combo: String,
    },
    /// Delayed focus re-assertion after show (fights Explorer focus theft
    /// on Win key hotkeys).  Spawned ~250ms after Painted.
    FocusReassert,
    /// Close (hide) the settings window via tao API.
    CloseSettings,
}

/// Everything [`run`] needs. Built by the runtime before it hands the
/// main thread to the event loop.
pub(crate) struct Host {
    pub(crate) state: Arc<Mutex<ShimState>>,
    pub(crate) proxy_slot: Arc<Mutex<Option<EventLoopProxy<UiCommand>>>>,
    pub(crate) icon_cache: Arc<IconCache>,
    pub(crate) event_tx: Sender<OverlayEvent>,
    pub(crate) is_running: Arc<AtomicBool>,
}

pub(crate) fn run(host: Host) -> Result<(), String> {
    let Host {
        state,
        proxy_slot,
        icon_cache,
        event_tx,
        is_running,
    } = host;

    let mut event_loop = EventLoopBuilder::<UiCommand>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    if let Ok(mut slot) = proxy_slot.lock() {
        *slot = Some(proxy.clone());
    }

    let window = WindowBuilder::new()
        .with_title("Nex")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top(true)
        .with_visible(false)
        .with_inner_size(LogicalSize::new(WINDOW_WIDTH, INITIAL_HEIGHT))
        .with_skip_taskbar(true)
        .with_window_classname("NexOverlayWindowClass")
        .with_no_redirection_bitmap(true)
        .build(&event_loop)
        .map_err(|e| format!("failed to create overlay window: {e}"))?;

    let hwnd = window.hwnd() as HWND;
    crate::overlay::hotkey::set_overlay_hwnd(hwnd as isize);
    if let Ok(mut s) = state.lock() {
        s.hwnd = hwnd as isize;
    }
    let acrylic_available = apply_window_chrome(&window, &state);
    // Native refraction layer (enabled by default; NEX_REFRACT_LAB=0 opts
    // out) sits below the WebView. Create it before build_webview so it
    // stays lower in z-order without needing the WebView HWND. Any
    // initialization failure keeps the Acrylic fallback available.
    let mut glass_layer = crate::overlay::refraction::GlassLayer::create_for_window(
        hwnd as isize,
        crate::overlay::refraction::physical_px(WINDOW_WIDTH, window.scale_factor()),
        crate::overlay::refraction::physical_px(MAX_HEIGHT, window.scale_factor()),
        proxy.clone(),
    );
    // The page drops its painted backgrounds when native glass is live
    // so refracted pixels show through (pushed with every snapshot).
    // Latch layer existence only — NOT the current panel mode. The
    // snapshot gates glassNative on panel_background per push, so a
    // session started opaque can still switch to liquid mid-run.
    // (Latching the mode here froze glass off for the whole session
    // whenever the saved config was opaque.)
    if let Ok(mut s) = state.lock() {
        s.glass_native = glass_layer.is_some();
        // Material/palette seed from the theme; the backdrop overrides
        // both per frame (the worker reports back via GlassPalette).
        s.glass_light = s.theme == Theme::Light;
    }
    // The worker seeds its material from the same theme before the
    // first frame, so page palette and native tint start in step.
    if let Some(layer) = glass_layer.as_ref() {
        let dark = state
            .lock()
            .map(|s| s.theme == Theme::Dark)
            .unwrap_or(true);
        layer.set_theme(dark);
    }
    unsafe {
        install_instance_signal_subclass(hwnd, &event_tx);
    }

    // Register raw input sink permanently at startup so the overlay
    // receives WM_INPUT for keyboard events regardless of which window
    // is foreground.  This ensures hotkey detection works even when
    // Task Manager, advanced settings, or other native Windows windows
    // have focus (the WH_KEYBOARD_LL hook thread may not receive events
    // for elevated/UWP windows).
    // Win hotkey detection lives in raw input — registered with
    // RIDEV_INPUTSINK only: the Start menu is kept shut by the injected
    // menu-mask key, never by RIDEV_NOHOTKEYS (which also kills the
    // user's own registered hotkeys, e.g. Win+Shift+S, while the overlay
    // is focused). nex toggles on Win key-up.
    register_raw_input_sink(hwnd);
    OVERLAY_HWND.store(hwnd as isize, Ordering::SeqCst);
    // apply_window_chrome ran before the layer existed and before this HWND
    // was cached, so settle the capture flag now that both are known.
    sync_capture_exclusion(&state);

    // Start suppression comes from the injected menu-mask key (see
    // overlay::hotkey); raw input is registered with RIDEV_INPUTSINK
    // only so registered system hotkeys keep working while nex is
    // focused.

    // Build the WebView eagerly at startup so the page is fully
    // rendered in the background before the first show.  The WebView
    // stays resident; only the icon cache is released on idle.
    let mut webview = match build_webview(&window, &state, &proxy, &event_tx, acrylic_available) {
        Ok(wv) => Some(wv),
        Err(e) => {
            crate::logging::warn(&format!("[nex] webview build failed: {e}"));
            None
        }
    };
    let mut settings_ui: Option<(tao::window::Window, wry::WebView)> = None;
    let mut settings_window_id: Option<tao::window::WindowId> = None;
    let last_settings_snapshot: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));

    if let Some(ref wv) = webview {
        subscribe_webview2_diagnostics(wv);
        // Oversize the viewport up front so the first show/grow has
        // pre-rasterized content beyond the window edge.
        keep_webview_viewport_max(wv);
    }
    let mut ready = false;
    let mut warm_gen: u64 = 0;
    let mut was_focused = false;
    // One-shot: a single focus re-assert after hotkey-show (fights focus
    // theft from Task Manager / elevated windows). Reset on every Show.
    let mut focus_reassert_used = false;
    let mut last_show = Instant::now();
    let mut show_pending = false;
    // Show-path latency telemetry: stamped on UiCommand::Show receipt,
    // reported when Painted reveals the window.
    let mut show_cmd_at: Option<Instant> = None;
    let mut show_arm_at: Option<Instant> = None;
    // Previously-foregrounded windows, newest last (capped). Pushed on
    // Show and on every genuine focus loss; drained (dead pruned) on
    // hide. A single slot restores stale windows after multi-window
    // flows (settings open/close, transient popups).
    let mut fg_stack: Vec<HWND> = Vec::new();
    let deferred_hide_armed = Arc::new(AtomicBool::new(false));
    // Epoch counter: incremented on every Show so stale deferred-hide
    // threads from a previous cycle can detect they are outdated and
    // skip firing Escape.
    let deferred_hide_epoch = Arc::new(AtomicU64::new(0));

    // Resize debounce state. Growth requests go through a debounce
    // timer (UiCommand::Resize stores the target height and arms the
    // timer; UiCommand::ApplyResize fires after the quiet period and
    // calls set_inner_size). Shrink requests and immediate-flagged
    // resizes bypass debounce entirely and apply immediately.
    let mut pending_resize: Option<f64> = None;
    let mut last_applied_height: f64 = INITIAL_HEIGHT;
    // First resize after show bypasses debounce so content appears
    // immediately instead of showing search bar → 100ms wait → items.
    let mut first_resize_after_show = false;
    let (resize_debounce_tx, resize_debounce_rx) =
        crossbeam_channel::unbounded::<Option<Duration>>();
    let resize_debounce_proxy = proxy.clone();
    std::thread::Builder::new()
        .name("nex-ui-resize-debounce".into())
        .spawn(move || {
            let mut armed: Option<Instant> = None;
            loop {
                let timeout = armed.map(|when| when.saturating_duration_since(Instant::now()));
                let result = match timeout {
                    Some(d) => resize_debounce_rx.recv_timeout(d),
                    None => resize_debounce_rx
                        .recv()
                        .map_err(|_| crossbeam_channel::RecvTimeoutError::Disconnected),
                };
                match result {
                    Ok(Some(delay)) => {
                        armed = Some(Instant::now() + delay);
                    }
                    Ok(None) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        armed = None;
                        try_send_ui(&resize_debounce_proxy, UiCommand::ApplyResize);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok();
    let resize_debounce_arm = resize_debounce_tx.clone();

    // Single warm-release timer thread. Hide arms it with (gen, delay);
    // it sends Teardown(gen) when the deadline passes. Teardown clears
    // the icon cache only — the WebView stays warm. Re-arming replaces
    // the previous deadline, so rapid hide/show cycles don't stack
    // sleeping threads.
    let (warm_release_tx, warm_release_rx) =
        crossbeam_channel::unbounded::<Option<(u64, Duration)>>();
    let warm_release_proxy = proxy.clone();
    std::thread::Builder::new()
        .name("nex-ui-warm-release".into())
        .spawn(move || {
            let mut armed: Option<(Instant, u64)> = None;
            loop {
                let timeout = armed.map(|(when, _)| when.saturating_duration_since(Instant::now()));
                let result = match timeout {
                    Some(d) => warm_release_rx.recv_timeout(d),
                    None => warm_release_rx
                        .recv()
                        .map_err(|_| crossbeam_channel::RecvTimeoutError::Disconnected),
                };
                match result {
                    Ok(Some((generation, delay))) => {
                        armed = Some((Instant::now() + delay, generation));
                    }
                    Ok(None) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        if let Some((_, generation)) = armed.take() {
                            try_send_ui(&warm_release_proxy, UiCommand::Teardown(generation));
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok();
    let warm_release_arm = warm_release_tx.clone();

    LOOP_ALIVE.store(true, Ordering::SeqCst);
    let mut chat_picker_generation = 0_u64;
    let mut chat_picker_listing: Option<super::file_picker::ChatPickerDirectory> = None;
    let run_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        event_loop.run_return(move |event, target, control_flow| {
            *control_flow = ControlFlow::Wait;

            match event {
                Event::UserEvent(cmd) => match cmd {
                    UiCommand::WebviewReady => {
                        crate::runtime::log_info(&format!("[nex] host UiCommand::WebviewReady received"));
                        ready = true;
                        if state.lock().map(|s| s.visible).unwrap_or(false) {
                            position_window(&window, hwnd);
                            apply_window_height(&window, webview.as_ref(), &mut glass_layer, INITIAL_HEIGHT);
                            last_applied_height = INITIAL_HEIGHT;
                            pending_resize = None;
                            first_resize_after_show = true;
                            refresh_pointer_over_panel(hwnd, &state);
                            prepare_glass_show(&glass_layer, &state);
                            push_state(&webview, &state, &icon_cache, true);
                            show_pending = true;

                        }
                    }
                    UiCommand::GlassPresented { w, h } => {
                        // The renderer has a frame. Re-presenting one for the
                        // size the page is on lifts the cloak — never the
                        // last session's held buffer.
                        if let Some(layer) = glass_layer.as_ref() {
                            layer.note_frame(w, h);
                        }
                        let live = OVERLAY_VISIBLE.load(Ordering::SeqCst)
                            && state.lock().map(|s| s.visible).unwrap_or(false)
                            && glass_active(&state);
                        let revealed = live
                            && glass_layer.as_ref().is_some_and(|layer| layer.reveal());
                        // The page drops its painted surface exactly when the
                        // layer shows through, so it is never see-through
                        // over a cloaked glass.
                        if revealed {
                            if let Ok(mut s) = state.lock() {
                                s.glass_frame_ready = true;
                            }
                            if ready {
                                push_state(&webview, &state, &icon_cache, false);
                            }
                        }
                    }
                    UiCommand::GlassFailed => {
                        crate::logging::warn(
                            "[nex] glass engine failed persistently — switching to opaque panel",
                        );
                        if let Some(layer) = glass_layer.as_ref() {
                            layer.set_visible(false);
                        }
                        sync_webview_background(webview.as_ref(), &state);
                        if ready {
                            push_state(&webview, &state, &icon_cache, false);
                        }
                    }
                    UiCommand::GlassPalette(light) => {
                        // Recorded even while hidden: the state is the
                        // page's source of truth, so the next push (the
                        // one at show) already carries the right palette.
                        let visible = match state.lock() {
                            Ok(mut s) => {
                                s.glass_light = light;
                                s.visible
                            }
                            Err(_) => false,
                        };
                        if ready && visible {
                            push_state(&webview, &state, &icon_cache, false);
                        }
                    }
                UiCommand::Apply => {
                    // Push while visible — or while a show is pending: stale
                    // in-flight results (or a slow quick-launch load) are the
                    // awaited content, and gating them on visibility deadlocks
                    // the reveal on the 500ms cap. The page's pendingShow
                    // bridge consumes the first non-empty rows and reveals
                    // fast; without a pending show, hidden pushes stay
                    // suppressed as before.
                    if ready
                        && (state.lock().map(|s| s.visible).unwrap_or(false) || show_pending)
                    {
                        push_state(&webview, &state, &icon_cache, false);
                    }
                    // Opaque panel modes park the glass layer immediately
                    // (no waiting for the next show/hide cycle).
                    if !glass_active(&state) {
                        if let Some(layer) = glass_layer.as_ref() {
                            layer.set_visible(false);
                        }
                    }
                    sync_webview_background(webview.as_ref(), &state);
                }
                UiCommand::ApplyStatus => {
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        push_status(&webview, &state);
                    }
                }
                UiCommand::ApplyIcons(decoded_paths) => {
                    // Progressive icon delivery: the background prefetch
                    // thread decoded icons and posted this command. Send only
                    // the newly decoded icon data so the page can patch
                    // placeholder <img> elements that painted with no src
                    // (cold cache) without re-encoding the whole snapshot.
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        let snapshot = {
                            let Ok(s) = state.lock() else { return };
                            s.clone()
                        };
                        let icons_json =
                            snapshot_icon_subset_json(&snapshot, &icon_cache, &decoded_paths);
                        if !icons_json.is_empty() {
                            if let Some(wv) = webview.as_ref() {
                                post_json(wv, &icons_json);
                            }
                        }
                    }
                }
                UiCommand::ApplyMediaData(json) => {
                    // Ready media JSON from the background worker — just
                    // post it. Never snapshot WinRT here: blocking calls
                    // wedge the event loop (frozen overlay, no hide/input).
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        if let Some(wv) = webview.as_ref() {
                            post_json(wv, &json);
                        }
                    }
                }
                UiCommand::ApplyWhatsNew(json) => {
                    // Fetched notes JSON — same fire-and-forget push; the
                    // network fetch ran on the runtime worker thread.
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        if let Some(wv) = webview.as_ref() {
                            post_json(wv, &json);
                        }
                    }
                }
                UiCommand::ChatData(json) => {
                    // Chat can complete a provider sign-in while focus is in
                    // the Codex window. Keep its status update queued into
                    // the warm WebView even if Nex is temporarily hidden.
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            post_json(wv, &json);
                        }
                    }
                }
                UiCommand::OpenChatFilePicker => {
                    let should_open = state
                        .lock()
                        .map(|mut s| {
                            if s.visible && !s.chat_file_picker_open && !s.chat_file_picker_pending {
                                s.chat_file_picker_open = true;
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !should_open {
                        return;
                    }

                    chat_picker_generation = chat_picker_generation.wrapping_add(1);
                    chat_picker_listing = None;
                    let generation = chat_picker_generation;
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            post_json(
                                wv,
                                &serde_json::json!({
                                    "chatPickerOpen": true,
                                    "chatPickerLoading": true,
                                    "chatPickerListing": null,
                                    "chatPickerError": null
                                }),
                            );
                        }
                    }
                    if let Err(error) = start_chat_picker_listing(&proxy, generation, None) {
                        try_send_ui(
                            &proxy,
                            UiCommand::ChatPickerListed {
                                generation,
                                result: Err(error),
                            },
                        );
                    }
                }
                UiCommand::ChatPickerListed { generation, result } => {
                    let active = state
                        .lock()
                        .map(|s| s.chat_file_picker_open || s.chat_file_picker_pending)
                        .unwrap_or(false);
                    if generation != chat_picker_generation || !active {
                        return;
                    }
                    match result {
                        Ok(listing) => {
                            chat_picker_listing = Some(listing.clone());
                            if ready {
                                if let Some(wv) = webview.as_ref() {
                                    post_json(
                                        wv,
                                        &serde_json::json!({
                                            "chatPickerLoading": false,
                                            "chatPickerListing": listing
                                        }),
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            if ready {
                                if let Some(wv) = webview.as_ref() {
                                    post_json(
                                        wv,
                                        &serde_json::json!({
                                            "chatPickerLoading": false,
                                            "chatPickerError": error
                                        }),
                                    );
                                }
                            }
                        }
                    }
                }
                UiCommand::ChatPickerNavigate(id) => {
                    let active = state
                        .lock()
                        .map(|s| s.chat_file_picker_open || s.chat_file_picker_pending)
                        .unwrap_or(false);
                    if !active {
                        return;
                    }
                    let path = chat_picker_listing
                        .as_ref()
                        .and_then(|listing| {
                            usize::try_from(id)
                                .ok()
                                .and_then(|index| listing.entries.get(index))
                                .filter(|entry| entry.id == id && entry.is_directory)
                                .map(|entry| entry.path.clone())
                        });
                    let Some(path) = path else {
                        return;
                    };
                    chat_picker_generation = chat_picker_generation.wrapping_add(1);
                    let generation = chat_picker_generation;
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            post_json(wv, &serde_json::json!({ "chatPickerLoading": true }));
                        }
                    }
                    if let Err(error) = start_chat_picker_listing(&proxy, generation, Some(path)) {
                        try_send_ui(
                            &proxy,
                            UiCommand::ChatPickerListed {
                                generation,
                                result: Err(error),
                            },
                        );
                    }
                }
                UiCommand::ChatPickerUp => {
                    let active = state
                        .lock()
                        .map(|s| s.chat_file_picker_open || s.chat_file_picker_pending)
                        .unwrap_or(false);
                    // At "This PC" (no current dir) there is nowhere to go back to.
                    // At a drive root there is no parent dir: go back to locations.
                    let current = chat_picker_listing
                        .as_ref()
                        .and_then(|listing| listing.current_dir.clone());
                    let Some(current) = current else {
                        return;
                    };
                    let parent = current
                        .parent()
                        .filter(|path| !path.as_os_str().is_empty())
                        .map(|path| path.to_path_buf());
                    if !active {
                        return;
                    }
                    chat_picker_generation = chat_picker_generation.wrapping_add(1);
                    let generation = chat_picker_generation;
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            post_json(wv, &serde_json::json!({ "chatPickerLoading": true }));
                        }
                    }
                    if let Err(error) = start_chat_picker_listing(&proxy, generation, parent) {
                        try_send_ui(
                            &proxy,
                            UiCommand::ChatPickerListed {
                                generation,
                                result: Err(error),
                            },
                        );
                    }
                }
                UiCommand::ChatPickerAdd(ids) => {
                    let active = state
                        .lock()
                        .map(|s| s.chat_file_picker_open || s.chat_file_picker_pending)
                        .unwrap_or(false);
                    if !active {
                        return;
                    }
                    let mut paths = Vec::with_capacity(ids.len());
                    for id in &ids {
                        let entry = chat_picker_listing
                            .as_ref()
                            .and_then(|listing| {
                                usize::try_from(*id)
                                    .ok()
                                    .and_then(|index| listing.entries.get(index))
                            })
                            .filter(|entry| entry.id == *id && !entry.is_directory);
                        let Some(entry) = entry else {
                            paths.clear();
                            break;
                        };
                        paths.push(entry.path.clone());
                    }
                    if paths.is_empty() || paths.len() != ids.len() || paths.len() > 3 {
                        if ready {
                            if let Some(wv) = webview.as_ref() {
                                post_json(
                                    wv,
                                    &serde_json::json!({
                                        "chatPickerLoading": false,
                                        "chatPickerError": "Choose up to 3 supported text files."
                                    }),
                                );
                            }
                        }
                        return;
                    }
                    chat_picker_generation = chat_picker_generation.wrapping_add(1);
                    chat_picker_listing = None;
                    let generation = chat_picker_generation;
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            post_json(wv, &serde_json::json!({ "chatPickerLoading": true }));
                        }
                    }
                    if let Err(error) = start_chat_picker_file_read(&proxy, generation, paths) {
                        try_send_ui(
                            &proxy,
                            UiCommand::ChatPickerFilesRead {
                                generation,
                                result: Err(error),
                            },
                        );
                    }
                }
                UiCommand::ChatPickerCancel { dismiss_overlay } => {
                    chat_picker_generation = chat_picker_generation.wrapping_add(1);
                    chat_picker_listing = None;
                    if let Ok(mut s) = state.lock() {
                        s.chat_file_picker_open = false;
                        s.chat_file_picker_pending = false;
                    }
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            post_json(
                                wv,
                                &serde_json::json!({
                                    "chatPickerOpen": false,
                                    "chatPickerClosed": true,
                                    "chatPickerLoading": false
                                }),
                            );
                        }
                    }
                    if dismiss_overlay {
                        let _ = event_tx.send(OverlayEvent::Escape);
                    }
                }
                UiCommand::ChatPickerFilesRead { generation, result } => {
                    let active = state
                        .lock()
                        .map(|s| s.chat_file_picker_open || s.chat_file_picker_pending)
                        .unwrap_or(false);
                    if generation != chat_picker_generation || !active {
                        return;
                    }
                    chat_picker_listing = None;
                    if let Ok(mut s) = state.lock() {
                        s.chat_file_picker_open = false;
                        s.chat_file_picker_pending = false;
                    }
                    if ready {
                        if let Some(wv) = webview.as_ref() {
                            match result {
                                Ok(files) => post_json(
                                    wv,
                                    &serde_json::json!({
                                        "chatPickerOpen": false,
                                        "chatPickerClosed": true,
                                        "chatPickerLoading": false,
                                        "chatFilesSelected": files
                                    }),
                                ),
                                Err(error) => post_json(
                                    wv,
                                    &serde_json::json!({
                                        "chatPickerOpen": false,
                                        "chatPickerClosed": true,
                                        "chatPickerLoading": false,
                                        "chatPickerError": error
                                    }),
                                ),
                            }
                        }
                    }
                }
                UiCommand::SelectChanged(idx) => {
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        push_selected(&webview, idx);
                    }
                }
                UiCommand::Show => {
                    crate::runtime::log_info(&format!("[nex] host UiCommand::Show received webview_exists={} ready={} show_pending={}", webview.is_some(), ready, show_pending));
                    show_cmd_at = Some(Instant::now());
                    push_fg_stack(&mut fg_stack, hwnd);
                    if webview.is_none() {
                        ready = false;
                        // Mark the show as pending before building the
                        // WebView so that spurious Focused(false) events
                        // (sent by Tao/Windows during WebView creation)
                        // do not trigger Escape and hide the overlay
                        // before WebviewReady can display it.
                        show_pending = true;

                        match build_webview(
                            &window,
                            &state,
                            &proxy,
                            &event_tx,
                            acrylic_available,
                        ) {
                            Ok(wv) => {
                                subscribe_webview2_diagnostics(&wv);
                                webview = Some(wv);
                            }
                            Err(e) => {
                                crate::logging::warn(&format!("[nex] webview build failed: {e}"));
                                return;
                            }
                        }
                        // Defer show until WebviewReady — the WebView
                        // loads async and we don't want a blank window.
                        return;
                    }
                    if !ready {
                        // WebView exists but the page hasn't loaded yet
                        // (e.g. Show raced the eager build's WebviewReady
                        // across threads). Do NOT destroy and rebuild —
                        // WebviewReady is already queued and completes the
                        // show via show_pending.
                        show_pending = true;

                        return;
                    }
                    // Already up: don't replay the full show pipeline
                    // (position, push, Painted roundtrip can flash). The
                    // page is current (Apply pushes while visible), so
                    // just re-assert foreground + input and disarm any
                    // stale deferred-hide.
                    if !show_pending
                        && OVERLAY_VISIBLE.load(Ordering::SeqCst)
                        && state.lock().map(|s| s.visible).unwrap_or(false)
                    {
                        deferred_hide_armed.store(false, Ordering::SeqCst);
                        deferred_hide_epoch.fetch_add(1, Ordering::SeqCst);
                        request_foreground(hwnd);
                        window.set_focus();
                        focus_input(&webview);
                        return;
                    }
                    // Set show_pending FIRST so spurious Focused(false) during
                    // position_window / set_inner_size / push_state cannot
                    // trigger Escape (which would desync OverlayState from
                    // the actual window state).
                    show_pending = true;

                    // Cancel any pending deferred-hide from a previous
                    // focus-loss — the user意图 is to show, not hide.
                    // Bump the epoch so any in-flight deferred-hide thread
                    // from the previous cycle detects it is stale.
                    deferred_hide_armed.store(false, Ordering::SeqCst);
                    deferred_hide_epoch.fetch_add(1, Ordering::SeqCst);
                    position_window(&window, hwnd);
                    // Spike glass follows the main window: same rect,
                    // ordered directly below it (never activating).
                    if let Some(layer) = glass_layer.as_mut() {
                        let scale = window.scale_factor();
                        layer.sync_to_main(
                            hwnd as isize,
                            scale,
                            WINDOW_WIDTH,
                            INITIAL_HEIGHT,
                            crate::overlay::refraction::physical_px(WINDOW_WIDTH, scale),
                            crate::overlay::refraction::physical_px(MAX_HEIGHT, scale),
                        );
                    }
                    // Start at search-bar height — JS sends resize when content appears.
                    apply_window_height(&window, webview.as_ref(), &mut glass_layer, INITIAL_HEIGHT);
                    last_applied_height = INITIAL_HEIGHT;
                    pending_resize = None;
                    first_resize_after_show = true;
                    // Re-detect platform look on every show — accent color and
                    // theme may have changed in Windows settings while nex was
                    // hidden. The window is still invisible here, so re-applying
                    // the acrylic tint cannot flicker.
                    if let Ok(mut s) = state.lock() {
                        s.accent_color = crate::overlay::platform::detect_accent_color();
                        s.theme = crate::overlay::platform::detect_system_theme();
                        // Deliberately not re-seeding the glass palette
                        // from the theme here. While hidden the worker
                        // keeps the material current from the desktop and
                        // reports it, so the show push already carries
                        // the right one. Seeding it from the theme handed
                        // the page white text on a dark Windows theme and
                        // only flipped it a frame later - invisible over a
                        // dark backdrop, a flash over a bright one.
                    }
                    // Theme may have flipped while hidden: the tint
                    // direction has to follow it.
                    if let Some(layer) = glass_layer.as_ref() {
                        let dark = state
                            .lock()
                            .map(|s| s.theme == Theme::Dark)
                            .unwrap_or(true);
                        layer.set_theme(dark);
                    }
                    apply_window_chrome(&window, &state);
                    // Wake the renderer before the page paints: its present
                    // lands while the glass window is still cloaked, so the
                    // reveal carries this desktop, not the last session's.
                    refresh_pointer_over_panel(hwnd, &state);
                    prepare_glass_show(&glass_layer, &state);
                    // Push state with show_pending so the JS side sends
                    // post("painted") to trigger the deferred show.
                    push_state(&webview, &state, &icon_cache, true);
                    // Mode may have changed while hidden (Apply skips
                    // pushes then), so re-sync the surface every show.
                    sync_webview_background(webview.as_ref(), &state);
                    show_arm_at = Some(Instant::now());
                    if let Some(t0) = show_cmd_at {
                        crate::runtime::log_info(&format!(
                            "[nex] show_latency show_arm_ms={}",
                            t0.elapsed().as_millis()
                        ));
                    }
                }
                UiCommand::FocusInput => {
                    // Start/Explorer can reassert foreground after the
                    // initial show. Treat refocus as a complete native focus
                    // handoff, not only a WebView DOM focus request.
                    // Do not optimistically mark focus: the delayed reassert
                    // below checks actual foreground ownership.
                    let visible = state.lock().map(|s| s.visible).unwrap_or(false);
                    if visible {
                        register_raw_input_sink(hwnd);
                        request_foreground(hwnd);
                    }
                    window.set_focus();
                    focus_input(&webview);
                    if visible {
                        let proxy_reassert = proxy.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(250));
                            try_send_ui(&proxy_reassert, UiCommand::FocusReassert);
                        });
                    }
                }
                UiCommand::Hide => {
                    // Re-inject the menu-mask key (0xE8) and spin-wait for the
                    // RIT to register it, so that when the window is hidden the
                    // newly-foreground window never sees a bare Win key — it
                    // sees Win+0xE8. This prevents Start from opening during
                    // the hide transition when the Win key is still physically
                    // held. No-op unless the hotkey is Win.
                    crate::overlay::hotkey::hold_mask_before_hide();
                    register_raw_input_sink(hwnd);
                    reset_raw_win_state();
                    hide_overlay_window(hwnd, &window, &glass_layer, &state);
                    OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
                    restore_fg_stack(&mut fg_stack, hwnd);
                    // Cloak fallback: if focus is still ours the invisible
                    // window owns the foreground and typing would land in
                    // the void — real-hide instead so the OS moves on.
                    if unsafe { GetForegroundWindow() } == hwnd {
                        let _ = window.set_visible(false);
                    }
                    crate::overlay::hotkey::release_mask_after_hide();
                    let fg_after = unsafe { GetForegroundWindow() };
                    // OS visibility distinguishes cloak (still "visible")
                    // from fallback real-hide; logical flag is always false
                    // here by construction.
                    let os_visible = unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd)
                    };
                    let win_down = unsafe {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x5B) as u16 & 0x8000 != 0
                    };
                    crate::runtime::log_info(&format!(
                        "[nex::debug] Hide: fg={:?} os_visible={} win_down={}",
                        fg_after, os_visible, win_down,
                    ));
                    // Delayed keyboard-state check: poll Win key after user
                    // likely released it.  If still pressed, something is stuck.
                    let proxy_delay = proxy.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(200));
                        try_send_ui(&proxy_delay, UiCommand::CheckKeyboardState(Instant::now()));
                    });
                    // Clear any pending resize so stale height doesn't
                    // apply after next Show.
                    pending_resize = None;
                    // Push cleared state while hidden so next Show has
                    // a fresh page ready to render.
                    if ready {
                        push_state(&webview, &state, &icon_cache, false);
                    }
                    if let Ok(mut s) = state.lock() {
                        s.has_focus = false;
                    }
                    was_focused = false;
                    show_pending = false;

                    // Clear deferred-hide flag — the overlay is already
                    // hidden, no need for the thread to fire Escape.
                    deferred_hide_armed.store(false, Ordering::SeqCst);
                    deferred_hide_epoch.fetch_add(1, Ordering::SeqCst);
                    warm_gen = warm_gen.wrapping_add(1);
                    let generation = warm_gen;
                    let delay = state
                        .lock()
                        .map(|s| s.ui_warm_release_ms)
                        .unwrap_or(5_000)
                        .max(500) as u64;
                    // Re-arm the single warm-release timer thread.
                    let _ = warm_release_arm.send(Some((generation, Duration::from_millis(delay))));
                }
                UiCommand::HideSync(ack) => {
                    crate::overlay::hotkey::hold_mask_before_hide();
                    register_raw_input_sink(hwnd);
                    reset_raw_win_state();
                    hide_overlay_window(hwnd, &window, &glass_layer, &state);
                    OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
                    restore_fg_stack(&mut fg_stack, hwnd);
                    // Same cloak fallback as Hide (see above).
                    if unsafe { GetForegroundWindow() } == hwnd {
                        let _ = window.set_visible(false);
                    }
                    crate::overlay::hotkey::release_mask_after_hide();
                    pending_resize = None;
                    if ready {
                        push_state(&webview, &state, &icon_cache, false);
                    }
                    if let Ok(mut s) = state.lock() {
                        s.has_focus = false;
                    }
                    was_focused = false;
                    focus_reassert_used = false;
                    show_pending = false;

                    warm_gen = warm_gen.wrapping_add(1);
                    let generation = warm_gen;
                    let delay = state
                        .lock()
                        .map(|s| s.ui_warm_release_ms)
                        .unwrap_or(5_000)
                        .max(500) as u64;
                    let _ = warm_release_arm.send(Some((generation, Duration::from_millis(delay))));
                    let _ = ack.send(());
                }
                UiCommand::Teardown(generation) => {
                    let still_hidden = !state.lock().map(|s| s.visible).unwrap_or(false);
                    if still_hidden && generation == warm_gen {
                        // Keep WebView + ready so re-open is always the
                        // warm path (consistent timing). Drop decoded
                        // PNG icons — the bulk of reclaimable overlay
                        // heap outside Chromium — but keep the 48 most
                        // recent so top hits repaint without re-decode.
                        let entries = icon_cache.len();
                        icon_cache.retain_recent(48);
                        let kept = icon_cache.len();
                        crate::logging::info(&format!(
                            "[nex] ui warm-release: icon cache trimmed entries={entries} kept={kept} (webview kept warm)"
                        ));
                    }
                }

                UiCommand::Resize { h, immediate } => {
                    let h = h.clamp(INITIAL_HEIGHT, MAX_HEIGHT);
                    if immediate || h < last_applied_height {
                        // Immediate flag, shrink request, or first resize
                        // after show: apply right away. Immediate skips the
                        // growth debounce so QL→results transitions don't
                        // leak into the quick-launch area.
                        first_resize_after_show = false;
                        pending_resize = None;
                        if (h - last_applied_height).abs() > 0.5 {
                            last_applied_height = h;
                            apply_window_height(&window, webview.as_ref(), &mut glass_layer, h);
                        }
                    } else if first_resize_after_show {
                        // First resize after show: apply immediately to
                        // prevent the "search bar first, items pop in
                        // 100ms later" visual flash.
                        first_resize_after_show = false;
                        pending_resize = None;
                        if (h - last_applied_height).abs() > 0.5 {
                            last_applied_height = h;
                            apply_window_height(&window, webview.as_ref(), &mut glass_layer, h);
                        }
                    } else if h - last_applied_height >= RESIZE_IMMEDIATE_GROWTH {
                        // Large growth: apply immediately so the window is
                        // already full-size when the new rows paint.
                        first_resize_after_show = false;
                        pending_resize = None;
                        last_applied_height = h;
                        apply_window_height(&window, webview.as_ref(), &mut glass_layer, h);
                    } else {
                        // Growth request: debounce to coalesce rapid
                        // resize requests and prevent DWM acrylic flash
                        // when the user is typing quickly.
                        pending_resize = Some(h);
                        let _ = resize_debounce_arm
                            .send(Some(Duration::from_millis(RESIZE_DEBOUNCE_MS)));
                    }
                }
                UiCommand::ApplyResize => {
                    // Debounce timer fired — apply the pending resize if the
                    // height actually changed. Skip redundant set_inner_size
                    // calls (same height as last applied) to avoid unnecessary
                    // DWM recomposition and potential flash.
                    if let Some(h) = pending_resize.take() {
                        if (h - last_applied_height).abs() > 0.5 {
                            last_applied_height = h;
                            apply_window_height(&window, webview.as_ref(), &mut glass_layer, h);
                        }
                    }
                }
                UiCommand::CheckKeyboardState(triggered) => {
                    let elapsed = triggered.elapsed().as_millis();
                    let win_down_now = unsafe {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x5B) as u16 & 0x8000 != 0
                    };
                    let fg_now = unsafe { GetForegroundWindow() };
                    let win_down_raw_now = unsafe {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x5C) as u16 & 0x8000 != 0
                    };
                    crate::runtime::log_info(&format!(
                        "[nex::debug] CheckKeyboardState(+{}ms): fg={:?} win_down={} rwin_down={}",
                        elapsed, fg_now, win_down_now, win_down_raw_now,
                    ));
                }
                UiCommand::Painted => {
                    crate::runtime::log_info(&format!("[nex] host UiCommand::Painted received show_pending={}", show_pending));
                    if show_pending {
                        if let Some(t0) = show_arm_at.take() {
                            crate::runtime::log_info(&format!(
                                "[nex] show_latency painted_wait_ms={}",
                                t0.elapsed().as_millis()
                            ));
                        }
                        // Gate on live state: an outside-click Escape during
                        // the show window (between Show and first paint)
                        // already hid the overlay — do NOT resurrect it.
                        // Empty rows are valid for a fresh config with no
                        // Quick Launch pins. Visibility is the stale-paint
                        // guard; row count must not block the empty state.
                        let should_show = state.lock().map(|s| s.visible).unwrap_or(false);
                        if !should_show {
                            crate::runtime::log_info("[nex] host Painted: stale (visible=false), skipping show");
                            show_pending = false;

                            return;
                        }
                        if glass_active(&state)
                            && glass_layer.as_ref().is_some_and(|layer| !layer.is_ready())
                        {
                            // Glass frame not ready yet: show the panel now
                            // anyway (its opaque surface covers the gap, the
                            // page only goes transparent once the renderer
                            // acks a frame) and let that ack reveal the layer
                            // - blocking the whole panel on first capture
                            // reads as a hung show.
                            crate::runtime::log_info(
                                "[nex] host Painted: glass frame pending, showing panel opaque",
                            );
                        }
                        show_pending = false;

                        last_show = Instant::now();
                        was_focused = false;
                        focus_reassert_used = false;
                        // Always register raw input sink so the overlay
                        // receives WM_INPUT for all keyboard events while
                        // foreground.  This works around Chromium/WebView2
                        // installing its own WH_KEYBOARD_LL hook when focused
                        // (installed after ours, so Chromium eats the events
                        // before our hook can process them).  RIDEV_INPUTSINK
                        // only — system hotkeys stay functional.
                        register_raw_input_sink(hwnd);
                        // Prime DOM focus before revealing the window. Once
                        // visible, there must not be a gap where the overlay
                        // can receive WM_CHAR while no editable element owns
                        // WebView focus (Windows beeps for those unhandled
                        // characters).
                        focus_input(&webview);
                        // Transparent-hide reversal (order mirrors hide):
                        // uncloak + child visible before first frame.
                        set_overlay_cloaked(hwnd, false);
                        set_webview_child_visible(&window, true);
                        // Single activation path (helper owns foreground;
                        // local force only without helper). Request before
                        // first visible frame so DWM doesn't show Acrylic's
                        // inactive fallback.
                        request_foreground(hwnd);
                        window.set_visible(true);
                        // Set focus on the native host after activation, then
                        // restore DOM focus to the WebView's search control.
                        // Foreground activation alone can leave focus on the
                        // previous window or the host HWND during first show.
                        let _ = window.set_focus();
                        OVERLAY_VISIBLE.store(true, Ordering::SeqCst);
                        // Start glass capture with the panel. Its HWND is
                        // revealed by the layer's own ack — a frame at the
                        // size the page just settled on — or right here if
                        // one for this size already landed.
                        // Parked in opaque panel modes (solid CSS covers it).
                        if glass_active(&state) {
                            if let Some(layer) = glass_layer.as_ref() {
                                layer.set_visible(true);
                                if layer.reveal() {
                                    if let Ok(mut s) = state.lock() {
                                        s.glass_frame_ready = true;
                                    }
                                    push_state(&webview, &state, &icon_cache, false);
                                }
                            }
                        }
                        // Re-assert topmost Z-position. When the overlay
                        // Explorer from exposing the taskbar. Depending on
                        // WebView2/tao focus notifications, the next show can
                        // otherwise remain marked unfocused even though the
                        // overlay has been foregrounded and its input focused.
                        crate::overlay::hotkey::set_overlay_focus(true);
                        if let Ok(mut s) = state.lock() {
                            s.has_focus = true;
                        }
                        // Helper was already signaled by request_foreground
                        // above; no second signal needed.
                        // Re-assert topmost Z-position. When the overlay
                        // hides while focused, the focus-sink window
                        // (activated before hide to prevent Explorer
                        // activation) may cause the overlay to appear
                        // below other topmost windows (e.g. Start menu)
                        // on the next show.  HWND_TOPMOST ensures we
                        // rise above any competing topmost window.
                        unsafe {
                            SetWindowPos(
                                hwnd,
                                HWND_TOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                            );
                        }
                        focus_input(&webview);
                        if let Some(t0) = show_cmd_at.take() {
                            let fg_owned = unsafe { GetForegroundWindow() == hwnd };
                            crate::runtime::log_info(&format!(
                                "[nex] show_latency host_ms={} fg_owned_at_reveal={}",
                                t0.elapsed().as_millis(),
                                fg_owned
                            ));
                        }
                        // Delayed re-assertion: Explorer re-asserts foreground
                        // ~100-200ms after Win key-down reaches the OS.  The
                        // initial force_foreground above often loses that race.
                        // A second attempt after 250ms catches Explorer once
                        // it has settled.
                        let proxy_reassert = proxy.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(250));
                            try_send_ui(&proxy_reassert, UiCommand::FocusReassert);
                        });
                    }
                }
                UiCommand::FocusReassert => {
                    // Post-show delayed re-assertion — fights Explorer
                    // focus theft on Win key hotkeys.  Same as the initial
                    // force_foreground + focus_input, but runs after
                    // Explorer has finished its re-assertion cycle.
                    // Only fire when overlay IS visible but another window
                    // actually owns the foreground — force_foreground calls
                    // ShowWindow(SW_SHOW) which would re-show a hidden
                    // window. Use native ownership rather than logical focus
                    // state, which can lag WebView2 focus transitions.
                    let (visible, logical_focused) = state
                        .lock()
                        .map(|s| (s.visible, s.has_focus))
                        .unwrap_or((false, false));
                    let foreground_owned = unsafe { GetForegroundWindow() == hwnd };
                    // The container can own the OS foreground while the
                    // WebView input remains unfocused after Start/Explorer
                    // takes and returns focus. Recover DOM focus in that
                    // state too; only retake the OS foreground when needed.
                    if visible && (!foreground_owned || !logical_focused) {
                        if !foreground_owned {
                            crate::runtime::log_info(&format!(
                                "[nex] FocusReassert: retaking foreground (was {:?})",
                                unsafe { GetForegroundWindow() }
                            ));
                            request_foreground(hwnd);
                            window.set_focus();
                        }
                        focus_input(&webview);
                    }
                }
                UiCommand::CloseSettings => {
                    if let Some((w, _)) = &settings_ui {
                        animate_window_close(w.hwnd() as HWND);
                        let _ = w.set_visible(false);
                        crate::runtime::log_info("[nex] settings window hidden via custom close");
                        let _ = event_tx.send(OverlayEvent::SettingsClosed);
                    }
                }
                UiCommand::Quit => {
                    *control_flow = ControlFlow::Exit;
                    // Post WM_QUIT to force GetMessageW in run_return to
                    // return 0. Without this, if the tao state machine is
                    // stuck in HandlingMainEvents (no pending WM_PAINT to
                    // transition it to Idle), the exit check
                    // (!runner.handling_events()) fails and the loop hangs
                    // on GetMessageW forever.
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(
                            0,
                        );
                    }
                }
                UiCommand::SettingsHotkeyRecorded { combo } => {
                    if let Some((_, wv)) = &settings_ui {
                        // Combo originates from the recordHotkey native path,
                        // not free-form JS — but still JSON-escape it before
                        // interpolating into script so a quote can never
                        // break out of the string literal.
                        let arg = serde_json::to_string(&combo).unwrap_or_default();
                        let script =
                            format!("window.hotkeyRecorded && window.hotkeyRecorded({arg})");
                        let _ = wv.evaluate_script(&script);
                    }
                }
                UiCommand::OpenSettings { snapshot}=> {
                    *last_settings_snapshot.lock().unwrap() = Some(snapshot.clone());
                    if let Some((w, _)) = &settings_ui {
                        //Already open just raise it
                        let _ = w.set_visible(true);
                        let _ = w.set_focus();
                    } else {
                        let sw = tao::window::WindowBuilder::new()
                            .with_title("Nex Settings")
                            .with_inner_size(tao::dpi::LogicalSize::new(790.0, 560.0))
                            .with_decorations(false)
                            .with_visible(false)
                            .build(target)
                            .expect("settings window");
                        settings_window_id = Some(sw.id());
                        position_window_centered(&sw);
                        let settings_hwnd_for_ipc: Arc<Mutex<Option<HWND>>> = Arc::new(Mutex::new(Some(sw.hwnd() as HWND)));
                        let save_tx = event_tx.clone();
                        let record_hwnd = hwnd;
                        let snapshot_for_ipc = last_settings_snapshot.clone();
                        let proxy_for_ipc = proxy.clone();
                        let webview = wry::WebViewBuilder::new()
                            .with_initialization_script(DISABLE_NATIVE_CONTEXT_MENU)
                            .with_background_color((0, 0, 0, 0))
                            .with_url("nexasset://localhost/settings.html")
                            .with_custom_protocol("nexasset".into(),move |_id, request| {
                                serve_asset(request, false)
                            })
                            .with_ipc_handler(move |req| {
                                use crate::overlay::ipc::{parse_settings, SettingsMessage};
                                let body = req.body().clone();
                                match parse_settings(&body) {
                                    Ok(SettingsMessage::Save(payload)) => {
                                        if payload.cfg.is_object() {
                                            let _ = save_tx.send(OverlayEvent::SaveSettings(body));
                                        } else {
                                            crate::runtime::log_info(
                                                "[nex] settings ipc rejected: save cfg is not an object",
                                            );
                                        }
                                    }
                                    Ok(SettingsMessage::Ready(_)) => {
                                        let snap = snapshot_for_ipc.lock().unwrap().clone();
                                        if let Some(snap) = snap {
                                            let _ = proxy_for_ipc.send_event(UiCommand::OpenSettings {
                                                snapshot: snap,
                                            });
                                        };
                                    }
                                    Ok(SettingsMessage::RecordHotkey(_)) => {
                                        RECORDING_HOTKEY.store(true, Ordering::SeqCst);
                                        // Raw input already swallows the keys;
                                        // keep Start shut by holding the mask
                                        // while Win is down (see handle_recording_key).
                                        register_raw_input_sink(record_hwnd);
                                    }
                                    Ok(SettingsMessage::CancelRecord(_)) => {
                                        RECORDING_HOTKEY.store(false, Ordering::SeqCst);
                                        register_raw_input_sink(record_hwnd);
                                    }
                                    Ok(SettingsMessage::Minimize(_)) => {
                                        use windows_sys::Win32::UI::WindowsAndMessaging::{
                                            ShowWindow, SW_MINIMIZE,
                                        };
                                        if let Ok(slot) = settings_hwnd_for_ipc.lock() {
                                            if let Some(h) = *slot {
                                                unsafe {
                                                    ShowWindow(h, SW_MINIMIZE);
                                                }
                                            }
                                        }
                                    }
                                    Ok(SettingsMessage::Close(_)) => {
                                        let _ = proxy_for_ipc.send_event(UiCommand::CloseSettings);
                                    }
                                    Err(reject) => {
                                        crate::runtime::log_info(&format!(
                                            "[nex] settings ipc rejected: {reject}"
                                        ));
                                    }
                                }
                            })
                            .build(&sw)
                            .expect("settings webview");

                        let _ = sw.set_visible(true);
                        animate_window_open(sw.hwnd() as HWND);
                        settings_ui = Some((sw, webview));
                    }
                    // push config into the page (works for both fresh and warm opens)
                    if let Some((_, wv)) = &settings_ui {
                        let _ = wv.evaluate_script(&format!(
                            "window.applySettings && window.applySettings({snapshot})"
                        ));
                    }
                }
                UiCommand::SettingsSaveResult { json } => {
                    if let Some((_, wv)) = &settings_ui {
                        let _ = wv.evaluate_script(&format!(
                            "window.saveResult && window.saveResult({json})"
                        ));
                    }
                }
            },
            Event::WindowEvent {
                window_id,
                event: WindowEvent::Focused(focused),
                ..
            } => {
                // Only the launcher overlay drives focus state — the settings
                // window must not toggle Escape-gating or hotkey-focus tracking.
                if window_id != window.id() {
                    return
                };
                crate::overlay::hotkey::set_overlay_focus(focused);
                if let Ok(mut s) = state.lock() {
                    s.has_focus = focused;
                }
                if focused {
                    was_focused = true;
                }
                if !focused {
                    // Track the window that took foreground for the restore
                    // stack (newest-live-first foreground history). Filtered to
                    // foreign live non-shell windows inside.
                    push_fg_stack(&mut fg_stack, hwnd);
                    let picker_active = state
                        .lock()
                        .map(|s| s.chat_file_picker_open || s.chat_file_picker_pending)
                        .unwrap_or(false);
                    let was_focused_val = was_focused;
                    let show_pending_val = show_pending;
                    let bare_win = crate::overlay::hotkey::is_bare_win_press_active();
                    let grace_ms = last_show.elapsed().as_millis() as u64;
                    let state_vis = state.lock().map(|s| s.visible).unwrap_or(false);
                    // Single deferred-hide arm shared by every path below.
                    // The 150ms thread re-checks live focus before sending
                    // Escape, so late WebView2 flap-pairs and transient
                    // toasts survive while genuine outside clicks still
                    // hide (150ms later instead of instantly).
                    let arm_deferred_hide = || {
                        if deferred_hide_armed.swap(true, Ordering::SeqCst) {
                            return;
                        }
                        let state_clone = state.clone();
                        let tx_clone = event_tx.clone();
                        let proxy_clone = proxy.clone();
                        let armed = deferred_hide_armed.clone();
                        let epoch_clone = deferred_hide_epoch.clone();
                        let my_epoch = deferred_hide_epoch.load(Ordering::SeqCst);
                        let hwnd_raw = hwnd as isize;
                        std::thread::Builder::new()
                            .name("nex-deferred-hide".into())
                            .spawn(move || {
                                std::thread::sleep(Duration::from_millis(150));
                                // If a new Show happened during the
                                // sleep, the epoch will have advanced —
                                // skip firing Escape.
                                if epoch_clone.load(Ordering::SeqCst) != my_epoch {
                                    return;
                                }
                                let dismiss_picker = state_clone.lock().ok().and_then(|s| {
                                    // Native ownership beats the logical flag:
                                    // a missed Focused(true) (focus bouncing
                                    // off a just-hidden window) leaves
                                    // has_focus false while the overlay still
                                    // owns the foreground. Escaping then kills
                                    // a live overlay (flash-then-hide).
                                    let foreground_owned = unsafe {
                                        GetForegroundWindow() as isize == hwnd_raw
                                    };
                                    if s.visible
                                        && !s.has_focus
                                        && !foreground_owned
                                        && !crate::overlay::hotkey::is_bare_win_press_active()
                                    {
                                        Some(
                                            s.chat_file_picker_open || s.chat_file_picker_pending,
                                        )
                                    } else {
                                        None
                                    }
                                });
                                if let Some(picker_active) = dismiss_picker {
                                    if picker_active {
                                        try_send_ui(
                                            &proxy_clone,
                                            UiCommand::ChatPickerCancel {
                                                dismiss_overlay: true,
                                            },
                                        );
                                    } else {
                                        let _ = tx_clone.send(OverlayEvent::Escape);
                                    }
                                }
                                armed.store(false, Ordering::SeqCst);
                            })
                            .ok();
                    };
                    if was_focused_val
                        && !show_pending_val
                        && !bare_win
                        && !picker_active
                        && grace_ms >= FOCUS_GRACE_MS
                        && state_vis
                    {
                        // Focus bounced back to the previous foreground window
                        // (Task Manager repaints, elevated windows reassert).
                        // One re-assert via force_foreground — if focus is
                        // lost AGAIN after that, the deferred-hide path
                        // below handles it as a genuine dismissal.
                        if grace_ms < 1500 && !focus_reassert_used {
                            focus_reassert_used = true;
                            crate::runtime::log_info(&format!(
                                "[nex::debug] Focused(false): re-asserting foreground (grace={}ms)",
                                grace_ms
                            ));
                            request_foreground(hwnd);
                            // Put keyboard focus back INTO the webview —
                            // SetForeground alone leaves the page unfocusable
                            // until clicked.
                            focus_input(&webview);
                            return;
                        }
                        // Within the post-show quiescence window a focus loss
                        // is almost always a WebView2 flap-pair (Chromium
                        // moving focus from container HWND into its input
                        // element) rather than a real outside click. Defer
                        // to the 150ms thread below which checks has_focus
                        // after settling. Without this, the first show after
                        // a long idle (grace_ms > 1500) fires Escape on the
                        // first flap and the overlay flashes then hides.
                        if grace_ms < POST_SHOW_QUIESCENCE_MS {
                            crate::runtime::log_info(&format!(
                                "[nex::debug] Focused(false): deferring to quiescence check (grace={}ms)",
                                grace_ms
                            ));
                            // Reset the re-assert slot so a second flap-pair
                            // inside the quiescence window gets another
                            // re-assert attempt.
                            focus_reassert_used = false;
                            arm_deferred_hide();
                            return;
                        } else if picker_active && state_vis && !bare_win {
                            // File picker open long past show: no flap-pair
                            // risk this late, so an outside click dismisses
                            // on this event instead of after the 150ms
                            // settle (the perceptible picker-hide lag).
                            let foreground_owned = unsafe { GetForegroundWindow() } == hwnd;
                            let lost = state
                                .lock()
                                .map(|s| s.visible && !s.has_focus)
                                .unwrap_or(false);
                            if lost && !foreground_owned {
                                try_send_ui(
                                    &proxy,
                                    UiCommand::ChatPickerCancel { dismiss_overlay: true },
                                );
                                return;
                            }
                            arm_deferred_hide();
                        } else {
                            crate::runtime::log_info(&format!(
                                "[nex::debug] Focused(false): deferring post-quiescence hide (was_focused={} show_pending={} grace={}ms state_vis={})",
                                was_focused_val, show_pending_val, grace_ms, state_vis,
                            ));
                            // PROBE: name the window that stole focus.
                            unsafe {
                                use windows_sys::Win32::UI::WindowsAndMessaging::{
                                    GetForegroundWindow, GetWindowTextW,
                                };
                                let fg = GetForegroundWindow();
                                let mut buf = [0u16; 128];
                                let len = GetWindowTextW(fg, buf.as_mut_ptr(), 128);
                                let title: String = String::from_utf16_lossy(&buf[..len as usize]);
                                crate::runtime::log_info(&format!(
                                    "[nex::probe] focus thief: fg=0x{:x} title='{}'",
                                    fg as isize, title
                                ));
                            }
                            // No instant Escape here: past quiescence a late
                            // WebView2 flap still looks identical to a real
                            // dismissal at event time. The deferred check
                            // below tells them apart.
                            arm_deferred_hide();
                        }
                    } else {
                        crate::runtime::log_info(&format!(
                            "[nex::debug] Focused(false): BLOCKED Escape (was_focused={} show_pending={} picker_active={} bare_win={} grace={}ms state_vis={})",
                            was_focused_val, show_pending_val, picker_active, bare_win, grace_ms, state_vis,
                        ));
                        // Deferred hide: if overlay still visible and no
                        // retry thread is armed, spawn one.  The double
                        // Focused(false) per blur is deduplicated by the
                        // swap — only the first event arms a thread.
                        // 150ms: imperceptible delay for genuine outside
                        // clicks; focus-flap at show (same-ms pairs in
                        // logs) re-focuses well within this window and
                        // cancels the hide.
                        if state_vis {
                            arm_deferred_hide();
                        }
                    }
                }
            }
            Event::WindowEvent {
                window_id,
                event: tao::event::WindowEvent::CloseRequested{ .. },
                ..
            } => {
                //only the settings window may be "closed"; hide it instead of
                //destroying it so repopening from the tray is instant
                if Some(window_id) == settings_window_id {
                    if let Some((_, wv)) = &settings_ui {
                        let _ = wv.evaluate_script("window.requestCloseSettings && window.requestCloseSettings()");
                    }
                }
            }
            _ => {}
        }
    });
    }));

    if let Err(payload) = run_result {
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "(unknown panic payload)".to_string()
        };
        crate::logging::warn(&format!(
            "[nex] event loop panicked during teardown; continuing shutdown: {msg}"
        ));
    }

    LOOP_ALIVE.store(false, Ordering::SeqCst);
    is_running.store(false, Ordering::SeqCst);

    Ok(())
}

/// Build a WebView on `window` with the custom protocol + IPC handler.
fn build_webview(
    window: &Window,
    state: &Arc<Mutex<ShimState>>,
    proxy: &EventLoopProxy<UiCommand>,
    event_tx: &Sender<OverlayEvent>,
    acrylic_available: bool,
) -> Result<WebView, String> {
    let ipc_state = state.clone();
    let ipc_proxy = proxy.clone();
    let ipc_tx = event_tx.clone();

    // Opaque panel modes get an opaque surface from birth: Chromium
    // picks subpixel (ClearType) vs grayscale at creation and ignores
    // later switches, so this must match the startup mode. Glass stays
    // transparent (refraction needs it).
    let opaque_bg: Option<(u8, u8, u8, u8)> = state
        .lock()
        .ok()
        .and_then(|s| match s.panel_background {
            crate::config::PanelBackground::Black => Some((0, 0, 0, 255)),
            crate::config::PanelBackground::White => Some((255, 255, 255, 255)),
            crate::config::PanelBackground::Glass => None,
        });
    // The acrylic CSS override (transparent panel-surface) must also stay
    // glass-only: injected in an opaque mode it puts all text over a
    // transparent panel and grayscales everything, voiding the opaque
    // surface above. Both are creation-time: switching modes needs restart.
    let glass_css = acrylic_available && opaque_bg.is_none();
    let builder = WebViewBuilder::new()
        .with_initialization_script(DISABLE_NATIVE_CONTEXT_MENU)
        .with_transparent(opaque_bg.is_none());
    let builder = match opaque_bg {
        Some(bg) => builder.with_background_color(bg),
        None => builder.with_background_color((0, 0, 0, 0)),
    };
    let webview = builder
        .with_url("nexasset://localhost/")
        .with_custom_protocol("nexasset".into(), move |_id, request| {
            serve_asset(request, glass_css)
        })
        .with_ipc_handler(move |req: Request<String>| {
            handle_ipc(req.body(), &ipc_state, &ipc_proxy, &ipc_tx);
        })
        .build(window)
        .map_err(|e| format!("{e}"))?;

    Ok(webview)
}

/// Serve embedded UI assets.
fn serve_asset(
    request: Request<Vec<u8>>,
    acrylic_available: bool,
) -> Response<std::borrow::Cow<'static, [u8]>> {
    let path = request.uri().path().to_string();

    let (content_type, body): (&str, std::borrow::Cow<'static, [u8]>) = match path.as_str() {
        "/" | "/index.html" => {
            if acrylic_available {
                // Set the glass theme before the linked stylesheet can paint.
                let html = INDEX_HTML.replacen(
                    "<html lang=\"en\" data-theme=\"dark\">",
                    "<html lang=\"en\" data-theme=\"dark\" data-native-material=\"acrylic\">",
                    1,
                );
                ("text/html", html.into_bytes().into())
            } else {
                ("text/html", INDEX_HTML.as_bytes().into())
            }
        }
        "/style.css" => ("text/css", STYLE_CSS.as_bytes().into()),
        "/app.js" => ("text/javascript", APP_JS.as_bytes().into()),
        "/media_clock.js" => ("text/javascript", MEDIA_CLOCK_JS.as_bytes().into()),
        "/settings.html" => ("text/html", SETTINGS_HTML.as_bytes().into()),
        "/settings.js" => ("text/javascript", SETTINGS_JS.as_bytes().into()),
        _ => return not_found(),
    };
    Response::builder()
        .header(CONTENT_TYPE, content_type)
        .header("Access-Control-Allow-Origin", "*")
        .body(body)
        .unwrap_or_else(|_| empty_response())
}

fn not_found() -> Response<std::borrow::Cow<'static, [u8]>> {
    Response::builder()
        .status(404)
        .body(std::borrow::Cow::Borrowed(&b""[..]))
        .unwrap_or_else(|_| empty_response())
}

fn empty_response() -> Response<std::borrow::Cow<'static, [u8]>> {
    Response::new(std::borrow::Cow::Borrowed(&b""[..]))
}

/// Encode PNG bytes as a `data:image/png;base64,...` URI for inline
/// embedding in JSON. Used because WebView2 custom protocols don't
/// support sub-resource loading for `<img>` tags — the browser
/// silently ignores `nexasset://localhost/icon/...` URLs. See
/// `docs/plans/robustness-audit.md` "Investigation Log" for details.
fn base64_data_uri(bytes: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(22 + bytes.len() * 4 / 3 + 4);
    out.push_str("data:image/png;base64,");
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 63) as usize] as char);
        out.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() >= 2 {
            out.push(CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() >= 3 {
            out.push(CHARS[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Parse one IPC message from the page and act on it. The body is
/// strictly deserialized into [`crate::overlay::ipc::OverlayMessage`]:
/// malformed JSON, unknown message types, and unknown fields are
/// rejected with a log line instead of being silently coerced.
fn handle_ipc(
    body: &str,
    state: &Arc<Mutex<ShimState>>,
    proxy: &EventLoopProxy<UiCommand>,
    event_tx: &Sender<OverlayEvent>,
) {
    use crate::overlay::ipc::{parse_overlay, OverlayMessage};
    let msg = match parse_overlay(body) {
        Ok(msg) => msg,
        Err(reject) => {
            crate::runtime::log_info(&format!("[nex] overlay ipc rejected: {reject}"));
            return;
        }
    };
    match msg {
        OverlayMessage::Ready(_) => {
            try_send_ui(proxy, UiCommand::WebviewReady);
        }
        OverlayMessage::Query(p) => {
            // Ignore queries that fire after hide (debounced input
            // races with Escape).  The shim clears query/rows on
            // hide; a stale query would prevent idle-state setup.
            if !state.lock().map(|s| s.visible).unwrap_or(false) {
                return;
            }
            let q = p.v;
            if let Ok(mut s) = state.lock() {
                s.query = q.clone();
            }
            let _ = event_tx.send(OverlayEvent::QueryChanged(q));
        }
        OverlayMessage::Submit(p) => {
            let idx = p.v as usize;
            if let Ok(mut s) = state.lock() {
                s.selected = idx;
            }
            let _ = event_tx.send(OverlayEvent::Submit);
        }
        OverlayMessage::Select(p) => {
            let idx = p.v as usize;
            if let Ok(mut s) = state.lock() {
                s.selected = idx;
            }
        }
        OverlayMessage::Escape(_) => {
            let _ = event_tx.send(OverlayEvent::Escape);
        }
        OverlayMessage::Resize(p) => {
            // JS sends {t:"resize", v:{v:h, immediate:bool}} (new) or
            // {t:"resize", v:h} (legacy number-only).
            let (h, immediate) = p.v.height_and_immediate();
            try_send_ui(proxy, UiCommand::Resize { h, immediate });
        }
        OverlayMessage::Painted(_) => {
            // First paint after push_state — safe to show the window.
            // Deferred from WebviewReady / Show to avoid a flash of
            // uncomposited content before the WebView2 paints.
            try_send_ui(proxy, UiCommand::Painted);
        }
        OverlayMessage::Pin(p) => {
            let _ = event_tx.send(OverlayEvent::PinApp(p.v));
        }
        OverlayMessage::Unpin(p) => {
            let _ = event_tx.send(OverlayEvent::UnpinApp(p.v));
        }
        OverlayMessage::Bookmark(p) => {
            let _ = event_tx.send(OverlayEvent::Bookmark(p.v.title, p.v.url, p.v.remove));
        }
        OverlayMessage::AddToQuickLaunch(p) => {
            let _ = event_tx.send(OverlayEvent::AddToQuickLaunch(p.v));
        }
        OverlayMessage::PowerAction(p) => {
            let event = match p.v.as_str() {
                "lock" => OverlayEvent::TrayLock,
                "sleep" => OverlayEvent::TraySleep,
                "shutdown" => OverlayEvent::PowerMenuShutdown,
                "restart" => OverlayEvent::PowerMenuRestart,
                "signout" => OverlayEvent::TraySignOut,
                // Unreachable: the parser allow-lists power actions, but
                // log rather than silently drop if schemas ever diverge.
                _ => {
                    crate::runtime::log_info(&format!(
                        "[nex] overlay ipc rejected: unknown power action {:?}",
                        p.v
                    ));
                    return;
                }
            };
            let _ = event_tx.send(event);
        }
        OverlayMessage::ContextAction(p) => {
            let _ = event_tx.send(OverlayEvent::ContextAction(p.v.action, p.v.title, p.v.path));
        }
        OverlayMessage::Settings(_) => {
            let _ = event_tx.send(OverlayEvent::OpenSettings);
        }
        OverlayMessage::CheckUpdates(_) => {
            let _ = event_tx.send(OverlayEvent::CheckUpdates);
        }
        OverlayMessage::MediaToggle(_) => {
            let _ = event_tx.send(OverlayEvent::MediaToggle);
        }
        OverlayMessage::MediaNext(_) => {
            let _ = event_tx.send(OverlayEvent::MediaNext);
        }
        OverlayMessage::MediaPrev(_) => {
            let _ = event_tx.send(OverlayEvent::MediaPrev);
        }
        OverlayMessage::MediaRefresh(_) => {
            let _ = event_tx.send(OverlayEvent::MediaRefresh);
        }
        OverlayMessage::MediaSeek(p) => {
            let _ = event_tx.send(OverlayEvent::MediaSeek(p.v));
        }
        OverlayMessage::MediaVolume(p) => {
            let _ = event_tx.send(OverlayEvent::MediaVolume(p.v));
        }
        OverlayMessage::MediaMute(_) => {
            let _ = event_tx.send(OverlayEvent::MediaMute);
        }
        OverlayMessage::MediaSession(p) => {
            let _ = event_tx.send(OverlayEvent::MediaSession(p.v));
        }
        OverlayMessage::WhatsNew(_) => {
            let _ = event_tx.send(OverlayEvent::WhatsNew);
        }
        OverlayMessage::ChatState(_) => {
            let _ = event_tx.send(OverlayEvent::ChatState);
        }
        OverlayMessage::ChatConfigure(p) => {
            if let Ok(raw) = serde_json::to_string(&p) {
                let _ = event_tx.send(OverlayEvent::ChatConfigure(raw));
            }
        }
        OverlayMessage::ChatFetchModels(p) => {
            if let Ok(raw) = serde_json::to_string(&p) {
                let _ = event_tx.send(OverlayEvent::ChatFetchModels(raw));
            }
        }
        OverlayMessage::ChatSend(p) => {
            if let Ok(raw) = serde_json::to_string(&p) {
                let _ = event_tx.send(OverlayEvent::ChatSend(raw));
            }
        }
        OverlayMessage::ChatConnect(p) => {
            let _ = event_tx.send(OverlayEvent::ChatConnect(p.v));
        }
        OverlayMessage::ChatDisconnect(_) => {
            let _ = event_tx.send(OverlayEvent::ChatDisconnect);
        }
        OverlayMessage::ChatCancel(p) => {
            let _ = event_tx.send(OverlayEvent::ChatCancel(p.request_id));
        }
        OverlayMessage::ChatPickFiles(_) => {
            try_send_ui(proxy, UiCommand::OpenChatFilePicker);
        }
        OverlayMessage::ChatPickerNavigate(p) => {
            try_send_ui(proxy, UiCommand::ChatPickerNavigate(p.v));
        }
        OverlayMessage::ChatPickerUp(_) => {
            try_send_ui(proxy, UiCommand::ChatPickerUp);
        }
        OverlayMessage::ChatPickerAdd(p) => {
            try_send_ui(proxy, UiCommand::ChatPickerAdd(p.entries));
        }
        OverlayMessage::ChatPickerCancel(_) => {
            try_send_ui(
                proxy,
                UiCommand::ChatPickerCancel {
                    dismiss_overlay: false,
                },
            );
        }
        OverlayMessage::OpenExternal(p) => {
            let _ = event_tx.send(OverlayEvent::OpenExternal(p.v));
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// WebView2 diagnostics — subscribe to process-death events so the
// overlay doesn't silently go dead without any log output.
// ─────────────────────────────────────────────────────────────────

/// Best-effort subscription to `ICoreWebView2::add_ProcessFailed`.
///
/// Logs `[nex:webview2] process_failed kind=<N>` when the WebView2
/// renderer, GPU, or utility process crashes or is killed. Never
/// propagates — subscription failure is logged and forgotten.
fn subscribe_webview2_diagnostics(webview: &WebView) {
    use std::sync::atomic::{AtomicU32, Ordering};
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2ProcessFailedEventArgs, ICoreWebView2ProcessFailedEventHandler,
        ICoreWebView2ProcessFailedEventHandler_Vtbl, COREWEBVIEW2_PROCESS_FAILED_KIND,
    };
    use windows_core::Interface;

    // ── minimal COM object implementing ICoreWebView2ProcessFailedEventHandler ──

    #[repr(C)]
    struct ProcessFailedHandler {
        vtbl: *const ICoreWebView2ProcessFailedEventHandler_Vtbl,
        ref_count: AtomicU32,
    }

    unsafe impl Send for ProcessFailedHandler {}
    unsafe impl Sync for ProcessFailedHandler {}

    unsafe extern "system" fn invoke(
        _this: *mut core::ffi::c_void,
        _sender: *mut core::ffi::c_void,
        args: *mut core::ffi::c_void,
    ) -> windows_core::HRESULT {
        if !args.is_null() {
            // SAFETY: WebView2 passes a valid ICoreWebView2ProcessFailedEventArgs pointer
            let args = unsafe { &*(args as *const ICoreWebView2ProcessFailedEventArgs) };
            let mut kind: COREWEBVIEW2_PROCESS_FAILED_KIND = unsafe { std::mem::zeroed() };
            if unsafe { args.ProcessFailedKind(&mut kind) }.is_ok() {
                crate::logging::error(&format!("[nex:webview2] process_failed kind={}", kind.0));
            } else {
                crate::logging::error("[nex:webview2] process_failed kind=<unreadable>");
            }
        }
        windows_core::HRESULT(0)
    }

    unsafe extern "system" fn add_ref(this: *mut core::ffi::c_void) -> u32 {
        // SAFETY: `this` points to a valid ProcessFailedHandler from Box::into_raw
        let h = unsafe { &*(this as *const ProcessFailedHandler) };
        h.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn release(this: *mut core::ffi::c_void) -> u32 {
        // SAFETY: `this` points to a valid ProcessFailedHandler from Box::into_raw
        let h = unsafe { &*(this as *const ProcessFailedHandler) };
        let prev = h.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            // SAFETY: ref_count reached 0; we own the sole reference.
            unsafe {
                drop(Box::from_raw(this as *mut ProcessFailedHandler));
            }
        }
        prev - 1
    }

    unsafe extern "system" fn query_interface(
        _this: *mut core::ffi::c_void,
        iid: *const windows_core::GUID,
        out: *mut *mut core::ffi::c_void,
    ) -> windows_core::HRESULT {
        if out.is_null() {
            return windows_core::imp::E_POINTER;
        }
        // SAFETY: out is non-null per check above
        unsafe {
            *out = core::ptr::null_mut();
        }
        if iid.is_null() {
            return windows_core::imp::E_INVALIDARG;
        }
        // Support IUnknown and our own interface.
        // SAFETY: iid is non-null per check above
        let iid = unsafe { &*iid };
        if *iid == <windows_core::IUnknown as Interface>::IID
            || *iid == <ICoreWebView2ProcessFailedEventHandler as Interface>::IID
        {
            // SAFETY: out is non-null per check above
            unsafe {
                *out = _this;
            }
            // SAFETY: _this points to a valid ProcessFailedHandler
            unsafe {
                add_ref(_this);
            }
            windows_core::imp::S_OK
        } else {
            windows_core::imp::E_NOINTERFACE
        }
    }

    static VTBL: ICoreWebView2ProcessFailedEventHandler_Vtbl =
        ICoreWebView2ProcessFailedEventHandler_Vtbl {
            base__: windows_core::IUnknown_Vtbl {
                QueryInterface: query_interface,
                AddRef: add_ref,
                Release: release,
            },
            Invoke: invoke,
        };

    // ── construct handler + subscribe ──

    let wv2 = webview.webview();
    let handler = Box::new(ProcessFailedHandler {
        vtbl: &VTBL as *const _,
        ref_count: AtomicU32::new(1),
    });
    // SAFETY: ProcessFailedHandler is #[repr(C)] with vtbl pointer as the first
    // field, matching the COM object layout expected by ICoreWebView2ProcessFailedEventHandler
    // (which is repr(transparent) over IUnknown over *mut c_void).
    let handler_ptr = Box::into_raw(handler) as *mut core::ffi::c_void;
    let handler_com: ICoreWebView2ProcessFailedEventHandler =
        unsafe { core::mem::transmute(handler_ptr) };

    let mut token: i64 = 0;
    let result = unsafe { wv2.add_ProcessFailed(&handler_com, &mut token) };
    match result {
        Ok(()) => {
            crate::logging::info("[nex] webview2 ProcessFailed subscription OK");
            // Intentionally leak handler_com — WebView2 holds a ref and
            // will call remove_ProcessFailed (via token) implicitly when
            // the webview is destroyed. Our AddRef=1 keeps it alive.
            // The handler just logs; no cleanup needed.
            std::mem::forget(handler_com);
        }
        Err(e) => {
            crate::logging::warn(&format!("[nex] webview2 subscriptions skipped: {e}"));
            // Reclaim our allocation since WebView2 didn't take it.
            // SAFETY: handler_ptr was created from Box::into_raw above and has
            // not been freed — we are the sole owner on the error path.
            drop(unsafe { Box::from_raw(handler_ptr as *mut ProcessFailedHandler) });
        }
    }
}

/// Fire-and-forget: send a JSON string to the WebView page via
/// `ICoreWebView2::PostWebMessageAsJson`.
fn post_json(webview: &WebView, json: impl std::fmt::Display) {
    let json = json.to_string();
    let wv2 = webview.webview();
    let wide: Vec<u16> = json.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = wv2.PostWebMessageAsJson(windows_core::PCWSTR::from_raw(wide.as_ptr()));
    }
}

/// Push the current state snapshot to the page.
///
/// Uses a two-message protocol:
/// 1. Lightweight state JSON (~2KB) — rows, theme, query, selected.
///    Icon fields contain only the file path (cache key for JS).
/// 2. Icon data JSON (~134KB for 20 rows) — `{"icons": {path: dataUri}}`.
///
/// Both use `PostWebMessageAsJson` (fire-and-forget). The state lock is
/// released before any icon encoding occurs — only the ShimState clone
/// runs under the lock (~microseconds).
/// Native glass renders only in glass panel mode: opaque black/white
/// panels paint solid and the layer stays parked (no capture, no wait).
fn glass_active(state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>) -> bool {
    crate::overlay::platform::transparency_enabled()
        && !crate::overlay::refraction::glass_failed()
        && state.lock().map(|s| !s.panel_background.is_opaque()).unwrap_or(false)
}

/// Wake the renderer a beat before the panel is revealed so its next
/// present lands while the glass window is still cloaked. Revealing
/// first would uncloak whatever the last session left presented.
fn prepare_glass_show(
    glass_layer: &Option<crate::overlay::refraction::GlassLayer>,
    state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>,
) {
    if glass_active(state) {
        if let Some(layer) = glass_layer.as_ref() {
            layer.prepare_show();
        }
    }
}

/// Record whether the cursor is over the panel right now. The page holds
/// the pointer position from the last session so a re-render keeps the
/// selection under a stationary mouse; that memory is only the truth if
/// the pointer never left the panel, and the page cannot ask the OS.
fn refresh_pointer_over_panel(
    hwnd: HWND,
    state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>,
) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut pt = POINT { x: 0, y: 0 };
    let inside = unsafe { GetWindowRect(hwnd, &mut rect) != 0 && GetCursorPos(&mut pt) != 0 }
        && pt.x >= rect.left
        && pt.x < rect.right
        && pt.y >= rect.top
        && pt.y < rect.bottom;
    if let Ok(mut s) = state.lock() {
        s.pointer_over_panel = inside;
    }
}

/// Whether a window showing this panel material has to stay out of screen
/// capture. Only the native glass does: it samples the desktop through
/// duplication, so its own composited frame would feed back into the blur.
/// Opaque Black/White panels draw nothing that can feed back, so they must
/// stay capturable — screenshots, OBS and Discord all read the same flag.
fn capture_exclusion_needed(mode: crate::config::PanelBackground, glass_native: bool) -> bool {
    glass_native && !mode.is_opaque()
}

/// The background the panel will actually paint with, after the transparency
/// and glass-failure fallbacks.
fn effective_panel_mode(
    state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>,
) -> crate::config::PanelBackground {
    let mut mode = state
        .lock()
        .map(|s| s.panel_background)
        .unwrap_or(crate::config::PanelBackground::Glass);
    if (!crate::overlay::platform::transparency_enabled() && !mode.is_opaque())
        || crate::overlay::refraction::glass_failed()
    {
        mode = if state.lock().map(|s| s.theme == Theme::Light).unwrap_or(false) {
            crate::config::PanelBackground::White
        } else {
            crate::config::PanelBackground::Black
        };
    }
    mode
}

/// Keep the main window's capture affinity in step with the material. Cheap
/// and idempotent: the flag is only touched when the answer changes, so this
/// runs on every Show/Apply.
fn sync_capture_exclusion(
    state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>,
) {
    static APPLIED: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    let exclude = capture_exclusion_needed(
        effective_panel_mode(state),
        state.lock().map(|s| s.glass_native).unwrap_or(false),
    );
    let want = i8::from(exclude);
    if APPLIED.load(Ordering::SeqCst) == want {
        return;
    }
    if crate::overlay::refraction::set_capture_exclusion(hwnd, exclude) {
        APPLIED.store(want, Ordering::SeqCst);
    }
}

/// Match the WebView surface to the panel mode. Opaque modes get an
/// opaque surface, which re-enables ClearType subpixel text (Chromium
/// only subpixels over opaque — transparent always falls back to
/// grayscale, and no CSS can change that). Glass keeps transparency.
///
/// Skips when the mode already matches: runtime SetDefaultBackgroundColor
/// cannot re-enable ClearType anyway (creation-time only), and this runs
/// on every Show and Apply — including per keystroke.
fn sync_webview_background(
    webview: Option<&wry::WebView>,
    state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>,
) {
    // Capture visibility tracks the material, not the window (see
    // sync_capture_exclusion). Before the early return so a live mode switch
    // also updates the flag.
    sync_capture_exclusion(state);
    let Some(wv) = webview else { return };
    let mode = effective_panel_mode(state);
    static LAST_MODE: std::sync::Mutex<Option<crate::config::PanelBackground>> =
        std::sync::Mutex::new(None);
    if LAST_MODE.lock().map(|g| *g == Some(mode)).unwrap_or(false) {
        return;
    }
    let color: wry::RGBA = match mode {
        crate::config::PanelBackground::Black => (0, 0, 0, 255),
        crate::config::PanelBackground::White => (255, 255, 255, 255),
        crate::config::PanelBackground::Glass => (0, 0, 0, 0),
    };
    match wv.set_background_color(color) {
        Ok(()) => {
            if let Ok(mut last) = LAST_MODE.lock() {
                *last = Some(mode);
            }
            crate::logging::info(&format!(
                "[nex] webview surface set for {:?} background (ClearType {})",
                mode,
                if mode.is_opaque() { "on" } else { "off" }
            ))
        }
        Err(error) => crate::logging::warn(&format!("[nex] webview background update failed: {error:?}")),
    }
}

fn push_state(
    webview: &Option<WebView>,
    state: &Arc<Mutex<ShimState>>,
    icons: &Arc<IconCache>,
    show_pending: bool,
) {
    let Some(wv) = webview else { return };

    // Phase 1: Clone state under lock (microseconds).
    let snapshot = {
        let Ok(s) = state.lock() else { return };
        s.clone()
    };

    // Phase 2: Build lightweight JSON without icons (~2KB).
    let state_json = snapshot_state_json(&snapshot, show_pending);

    // Phase 3: Encode icons outside lock (~2-5ms for 20 rows).
    // Note: png_bytes() may block on first decode per icon (cold cache),
    // but the state lock is not held during this work.
    let icons_json = snapshot_icons_json(&snapshot, icons);

    // Phase 4: Send both messages back-to-back (same frame).
    post_json(&wv, &state_json);
    if !icons_json.is_empty() {
        post_json(&wv, &icons_json);
    }
}

/// Push only a selection change to the page (lightweight, no full
/// re-render). The JS side detects the missing `rows` field and
/// applies the selection incrementally.
fn push_selected(webview: &Option<WebView>, selected: usize) {
    let Some(wv) = webview else { return };
    let json = serde_json::json!({ "selected": selected }).to_string();
    post_json(&wv, &json);
}

/// Push only a status text change (lightweight, no full re-render).
/// Mirrors push_selected: the JS side detects the missing `rows` field
/// and applies the status without rebuilding the result list.
fn push_status(webview: &Option<WebView>, state: &Arc<Mutex<ShimState>>) {
    let Some(wv) = webview else { return };
    let status = state
        .lock()
        .ok()
        .map(|s| s.status_text.clone())
        .unwrap_or_default();
    let json = serde_json::json!({ "status": status }).to_string();
    post_json(&wv, &json);
}

fn focus_input(webview: &Option<WebView>) {
    if let Some(wv) = webview {
        // DOM focus alone (`input.focus()`) does not move WebView2's native
        // controller focus. On a fresh show, Windows can therefore deliver
        // the first WM_CHAR to the host/previous focus target and play the
        // system error sound. Move native keyboard focus into WebView2 first,
        // then focus the editable search element inside the page.
        if let Err(error) = wv.focus() {
            crate::runtime::log_info(&format!("[nex] WebView native focus failed: {error}"));
        }
        let _ = wv.evaluate_script("window.nex&&window.nex.focus()");
    }
}

/// Serialize the overlay state into lightweight JSON without icon data.
/// Icon fields contain only the file path (used as a JS cache key).
fn snapshot_state_json(s: &ShimState, show_pending: bool) -> String {
    snapshot_state_with_transparency(
        s,
        show_pending,
        crate::overlay::platform::transparency_enabled()
            && !crate::overlay::refraction::glass_failed(),
    )
}

fn snapshot_state_with_transparency(s: &ShimState, show_pending: bool, transparent: bool) -> String {
    let background = if !transparent && !s.panel_background.is_opaque() {
        if s.theme == Theme::Light { "white" } else { "black" }
    } else { s.panel_background.as_str() };
    static LENS_CLIP: std::sync::LazyLock<String> = std::sync::LazyLock::new(crate::overlay::refraction::lens_clip_css);
    let rows: Vec<serde_json::Value> = s
        .rows
        .iter()
        .map(|r| {
            let role = match r.role {
                OverlayRowRole::Header => "header",
                OverlayRowRole::Status => "status",
                OverlayRowRole::Calculator => "calculator",
                OverlayRowRole::QuickLaunch => "quick_launch",
                OverlayRowRole::ShowAllApps => "show_all_apps",
                OverlayRowRole::ClipboardHistory => "clipboard_history",
                OverlayRowRole::TopHit | OverlayRowRole::Item => "item",
            };
            let selectable = matches!(
                r.role,
                OverlayRowRole::Item
                    | OverlayRowRole::TopHit
                    | OverlayRowRole::Calculator
                    | OverlayRowRole::QuickLaunch
                    | OverlayRowRole::ShowAllApps
                    | OverlayRowRole::ClipboardHistory
            );
            let icon = if r.icon_path.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(r.icon_path.clone())
            };
            // r.icon_path carries the real executable/filesystem path.
            // r.path is the display subtitle (publisher name for UWP apps).
            let file_path = if r.icon_path.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(r.icon_path.clone())
            };
            let tile_size = r.tile_size.as_ref().map(|t| match t {
                TileSize::Small => "small",
                TileSize::Medium => "medium",
                TileSize::Large => "large",
            });
            serde_json::json!({
                "role": role,
                "title": r.title,
                "subtitle": r.path,
                "kind": r.kind,
                "url": r.url.clone(),
                "icon": icon,
                "filePath": file_path,
                "selectable": selectable,
                "resultIndex": r.result_index,
                "clipboardThumbnail": r.clipboard_thumbnail,
                "clipboardFullImage": r.clipboard_full_image,
                "tileSize": tile_size,
            })
        })
        .collect();

    let theme = match s.theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
    };

    // Include Quick Launch items for idle state
    let quick_launch: Vec<serde_json::Value> = s
        .quick_launch_items
        .iter()
        .map(|item| {
            serde_json::json!({
                "title": item.title,
                "path": item.path,
                "url": if item.kind.eq_ignore_ascii_case("bookmark") { item.path.clone() } else { String::new() },
                "kind": item.kind,
                "icon": item.icon_path,
                "pinned": item.is_pinned,
            })
        })
        .collect();

    serde_json::json!({
        "query": s.query,
        "rows": rows,
        "selected": s.selected,
        "status": s.status_text,
        "placeholder": s.placeholder_hint,
        "hotkeyHint": s.hotkey_hint,
        "hotkeyIssue": s.hotkey_issue_active,
        "theme": theme,
        "accent": s.accent_color,
        "gridView": s.grid_view,
        "bentoView": s.bento_view,
        "completion": s.completion,
        "showPending": show_pending,
        "contentPending": s.content_pending,
        "quickLaunch": quick_launch,
        "quickLaunchVisible": s.quick_launch_visible,
        "updateAvailable": s.update_available,
        "whatsNewPending": s.whats_new_pending,
        "overlayVisible": s.visible,
        "pointerOverPanel": s.pointer_over_panel,
        "panelBackground": background,
        "glassNative": transparent && s.glass_native && s.glass_frame_ready && !s.panel_background.is_opaque(),
        "glassLight": transparent && s.glass_native && s.glass_light,
        "glassClip": &*LENS_CLIP,
    })
    .to_string()
}

fn snapshot_icon_subset_json(
    s: &ShimState,
    icons: &Arc<IconCache>,
    decoded_paths: &[String],
) -> String {
    let wanted: std::collections::HashSet<&str> =
        decoded_paths.iter().map(String::as_str).collect();
    let mut seen = std::collections::HashSet::new();
    let icon_map: serde_json::Map<String, serde_json::Value> = s
        .rows
        .iter()
        .filter(|r| !r.icon_path.is_empty())
        .filter(|r| wanted.contains(r.icon_path.as_str()))
        .filter(|r| seen.insert(r.icon_path.clone()))
        .filter_map(|r| {
            let b64 = icons
                .png_bytes_cached(&r.icon_path)
                .map(|arc| base64_data_uri(arc.as_ref()))
                .unwrap_or_default();
            if b64.is_empty() {
                None
            } else {
                Some((r.icon_path.clone(), serde_json::Value::String(b64)))
            }
        })
        .collect();

    if icon_map.is_empty() {
        return String::new();
    }

    serde_json::json!({ "icons": icon_map }).to_string()
}

/// Serialize icon data as `{"icons": {path: dataUri, ...}}`.
/// Deduplicates by path to avoid encoding the same icon twice when
/// multiple rows share a path (e.g. two shortcuts to the same .exe).
/// Returns an empty string if no icons.
///
/// Non-blocking: only already-decoded (warm) icons are included. The
/// background prefetch thread fills the cache and posts `ApplyIcons`
/// to re-invoke this on the host thread, delivering newly-decoded
/// icons as a separate `{"icons": ...}` message the page patches in.
fn snapshot_icons_json(s: &ShimState, icons: &Arc<IconCache>) -> String {
    let mut seen = std::collections::HashSet::new();
    let icon_map: serde_json::Map<String, serde_json::Value> = s
        .rows
        .iter()
        .filter(|r| !r.icon_path.is_empty())
        .filter(|r| seen.insert(r.icon_path.clone()))
        .filter_map(|r| {
            let b64 = icons
                .png_bytes_cached(&r.icon_path)
                .map(|arc| base64_data_uri(arc.as_ref()))
                .unwrap_or_default();
            if b64.is_empty() {
                None
            } else {
                Some((r.icon_path.clone(), serde_json::Value::String(b64)))
            }
        })
        .collect();

    if icon_map.is_empty() {
        return String::new();
    }

    serde_json::json!({ "icons": icon_map }).to_string()
}

// ─────────────────────────────────────────────────────────────────
// Win32 glue: window chrome, positioning, focus
// ─────────────────────────────────────────────────────────────────

/// Apply DWM's real desktop acrylic, with legacy acrylic as a Windows 10 fallback.
fn apply_window_chrome(window: &Window, state: &Arc<Mutex<ShimState>>) -> bool {
    let (dark, glass_native, opaque_panel) = state.lock().map(|s| {
        (
            s.theme == Theme::Dark,
            s.glass_native,
            s.panel_background.is_opaque() || !crate::overlay::platform::transparency_enabled(),
        )
    }).unwrap_or((true, false, false));
    let hwnd = window.hwnd() as HWND;
    let transitions_disabled = 1_i32;
    let immersive_dark = i32::from(dark);
    // Native glass uses the same continuous profile for both HWND regions
    // and CSS. Opaque/acrylic modes retain the system's default corners.
    let rounded_corners = if glass_native && !opaque_panel {
        windows_sys::Win32::Graphics::Dwm::DWMWCP_DONOTROUND
    } else { DWMWCP_DEFAULT };
    let _ = set_dwm_attribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &transitions_disabled);
    let _ = set_dwm_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &immersive_dark);
    let _ = set_dwm_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &rounded_corners);
    // Native glass already shades its own edge. A second system frame
    // reads as an unrelated hard outline (especially on bright backdrops).
    let border_color: u32 = if glass_native && !opaque_panel { 0xFFFFFFFE } else { 0xFFFFFFFF };
    let _ = set_dwm_attribute(hwnd, windows_sys::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR, &border_color);

    // Keep DWM acrylic for the fallback path. With the native refraction
    // layer active, even reapplying acrylic on a later show paints over the
    // captured image, so keep the main window backdrop disabled.
    // Opaque panels cover the window fully, so the backdrop contributes
    // nothing but square corners poking past the rounded panel — off too.
    let backdrop = if glass_native || opaque_panel {
        DWMSBT_NONE
    } else {
        DWMSBT_TRANSIENTWINDOW
    };
    let native_backdrop = set_dwm_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop);
    // Frame rendering stays on; glass supplies its own region at reveal.
    // BORDER_COLOR above uses DWM's NONE/DEFAULT sentinels, not a tint.
    let available = if opaque_panel {
        // Opaque panels paint solid over an opaque WebView surface
        // (ClearType): no backdrop, and no acrylic fallback — any
        // transparency would void subpixel text.
        false
    } else if native_backdrop {
        true
    } else if glass_native {
        crate::logging::warn(
            "[nex][refract] could not keep the main DWM backdrop disabled on show",
        );
        false
    } else {
        let tint = if dark {
            Some((20, 22, 28, 112))
        } else {
            Some((245, 247, 250, 104))
        };
        window_vibrancy::apply_acrylic(window, tint).is_ok()
    };
    if !available && !opaque_panel {
        crate::logging::info("[nex] native acrylic unavailable; using opaque CSS fallback");
    }
    available
}

fn set_dwm_attribute<T>(hwnd: HWND, attribute: i32, value: &T) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute as u32,
            value as *const T as *const std::ffi::c_void,
            std::mem::size_of::<T>() as u32,
        ) >= 0
    }
}

/// Keep the WebView viewport pinned to the maximum panel size so content
/// beyond the window edge is always rasterized. Window growth then just
/// reveals already-painted pixels — no blank webview frame, no acrylic
/// flash; shrink requires no WebView work at all (the window clips it).
fn keep_webview_viewport_max(webview: &WebView) {
    use wry::dpi::{LogicalPosition, LogicalSize};
    let _ = webview.set_bounds(wry::Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(WINDOW_WIDTH, MAX_HEIGHT).into(),
    });
}

/// Resize the window and immediately re-assert the oversized WebView
/// viewport (wry snaps it to the window size on WM_SIZE). The spike
/// glass re-aligns to the main window rect (size follows for free).
fn apply_window_height(
    window: &Window,
    webview: Option<&WebView>,
    glass: &mut Option<crate::overlay::refraction::GlassLayer>,
    h: f64,
) {
    window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, h));
    if let Some(wv) = webview {
        keep_webview_viewport_max(wv);
    }
    if let Some(layer) = glass {
        let scale = window.scale_factor();
        layer.sync_to_main(
            window.hwnd() as isize,
            scale,
            WINDOW_WIDTH,
            h,
            crate::overlay::refraction::physical_px(WINDOW_WIDTH, scale),
            crate::overlay::refraction::physical_px(MAX_HEIGHT, scale),
        );
    }
}

/// Center the window horizontally on the monitor under the cursor and
/// anchor it in the upper third (launcher placement).
fn position_window(window: &Window, _hwnd: HWND) {
    let Some((left, top, right, bottom)) = cursor_monitor_work_area() else {
        return;
    };
    let scale = window.scale_factor();
    let width_phys = (WINDOW_WIDTH * scale) as i32;
    let work_w = right - left;
    let work_h = bottom - top;
    let x = left + (work_w - width_phys) / 2;
    let y = top + (work_h as f32 * 0.25) as i32;
    window.set_outer_position(PhysicalPosition::new(x.max(left), y.max(top)));
}
/// Center a window on the cursor's monitor work area (both axes).
fn position_window_centered(window: &Window) {
    let Some((left, top, right, bottom)) = cursor_monitor_work_area() else {
        return;
    };
    let size = window.inner_size();
    let x = left + (right - left - size.width as i32) / 2;
    let y = top + (bottom - top - size.height as i32) / 2;
    window.set_outer_position(PhysicalPosition::new(x.max(left), y.max(top)));
}

fn animate_window_open(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AnimateWindow;
    const AW_BLEND: u32 = 0x00080000;
    const AW_ACTIVATE: u32 = 0x00020000;
    unsafe {
        AnimateWindow(hwnd, 200, AW_BLEND | AW_ACTIVATE);
    }
}

fn animate_window_close(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AnimateWindow;
    const AW_BLEND: u32 = 0x00080000;
    const AW_HIDE: u32 = 0x00100000;
    unsafe {
        AnimateWindow(hwnd, 200, AW_BLEND | AW_HIDE);
    }
}

fn cursor_monitor_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return None;
    }
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let r: RECT = info.rcWork;
    Some((r.left, r.top, r.right, r.bottom))
}

/// Push the current foreground window for later restore. Skips null,
/// self, shell, dead, and consecutive-duplicate windows. Called on Show
/// and on genuine focus loss so the restore target tracks reality
/// instead of a stale single slot.
fn push_fg_stack(stack: &mut Vec<HWND>, overlay_hwnd: HWND) {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() || foreground == overlay_hwnd {
        return;
    }
    if unsafe { IsWindow(foreground) } == 0 {
        return;
    }
    if is_shell_surface(foreground) {
        return;
    }
    if stack.last().is_some_and(|&top| top == foreground) {
        return;
    }
    stack.push(foreground);
    while stack.len() > 8 {
        stack.remove(0);
    }
}

/// Identify Explorer's desktop and taskbar surfaces by their window class.
/// The shell identity check also covers the shell window if its class changes.
fn is_shell_surface(hwnd: HWND) -> bool {
    if shell_window().is_some_and(|shell| shell == hwnd) {
        return true;
    }

    let mut class_name = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if len <= 0 || len as usize > class_name.len() {
        return false;
    }
    ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"]
        .iter()
        .any(|name| class_name[..len as usize].iter().copied().eq(name.encode_utf16()))
}

fn shell_window() -> Option<HWND> {
    let shell = unsafe { GetShellWindow() };
    if !shell.is_null() && unsafe { IsWindow(shell) } != 0 {
        return Some(shell);
    }
    let class = [
        b'P' as u16,
        b'r' as u16,
        b'o' as u16,
        b'g' as u16,
        b'm' as u16,
        b'a' as u16,
        b'n' as u16,
        0,
    ];
    let progman = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if !progman.is_null() && unsafe { IsWindow(progman) } != 0 {
        Some(progman)
    } else {
        None
    }
}

/// Restore the newest live window from the stack, pruning dead ones.
/// Clears the rest: each show cycle starts tracking fresh.
fn restore_fg_stack(stack: &mut Vec<HWND>, overlay_hwnd: HWND) {
    let mut restored = false;
    while let Some(target) = stack.pop() {
        if unsafe { IsWindow(target) } != 0 && !is_shell_surface(target) {
            if unsafe { SetForegroundWindow(target) } != 0 {
                restored = true;
                break;
            }
        }
    }
    stack.clear();
    let foreground = unsafe { GetForegroundWindow() };
    if !restored
        && (foreground == overlay_hwnd
            || (!foreground.is_null() && is_shell_surface(foreground)))
    {
        if let Some(shell) = shell_window() {
            unsafe {
                SetForegroundWindow(shell);
            }
        }
    }
}

fn hide_overlay_window(
    hwnd: HWND,
    window: &Window,
    glass: &Option<crate::overlay::refraction::GlassLayer>,
    state: &std::sync::Arc<std::sync::Mutex<crate::overlay::model::ShimState>>,
) {
    // Spike glass vanishes with the panel.
    if let Some(layer) = glass {
        layer.set_visible(false);
    }
    // The page goes back to its painted panel while the glass is cloaked,
    // so the next show never paints transparent over a stale layer.
    if let Ok(mut s) = state.lock() {
        s.glass_frame_ready = false;
    }
    // Transparent-hide first: cloak + click-through + child hidden.
    // WS_EX_TRANSPARENT on the parent does not stop hit-testing on the
    // child HWND, so the page is hidden separately — an invisible page
    // must never eat clicks (the click-outside hook keys off the panel
    // rect and would go blind).
    if set_overlay_cloaked(hwnd, true) {
        set_webview_child_visible(window, false);
        return;
    }
    // Fallback: classic hide when cloaking is unavailable.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            0,
            0,
            0,
            0,
            SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
    let _ = window.set_visible(false);
}

/// Find the WRY_WEBVIEW child HWND (null when the WebView isn't built).
fn find_webview_child(window: &Window) -> HWND {
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW};
    struct Search {
        found: HWND,
    }
    unsafe extern "system" fn find_webview(hwnd: HWND, param: isize) -> i32 {
        const WRY_WEBVIEW: &[u16] = &[
            b'W' as u16, b'R' as u16, b'Y' as u16, b'_' as u16, b'W' as u16, b'E' as u16,
            b'B' as u16, b'V' as u16, b'I' as u16, b'E' as u16, b'W' as u16, 0,
        ];
        let search = &mut *(param as *mut Search);
        let mut name = [0u16; 32];
        let len = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
        if len > 0 && name[..len as usize] == WRY_WEBVIEW[..len as usize] {
            search.found = hwnd;
            return 0;
        }
        1
    }
    let mut search = Search {
        found: std::ptr::null_mut(),
    };
    unsafe {
        EnumChildWindows(
            window.hwnd() as HWND,
            Some(find_webview),
            &mut search as *mut Search as isize,
        );
    }
    search.found
}

/// Show/hide only the WebView child. Used by transparent-hide: the page
/// must not eat clicks while invisible, but hiding the child alone keeps
/// regions, z-order, and renderer state (unlike hiding the parent).
fn set_webview_child_visible(window: &Window, visible: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};
    let child = find_webview_child(window);
    if child.is_null() {
        return;
    }
    unsafe {
        // SW_SHOWNA: reveal without activating — activation belongs to
        // the foreground flow, not the visibility toggle.
        ShowWindow(child, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

/// Cloak-based visibility (transparent-hide): DWM stops composing the
/// window and clicks pass through, but nothing is destroyed — regions,
/// z-order, and the warm WebView survive, so the next show is a reveal,
/// not a rebuild. Returns false when cloaking is unavailable (caller
/// falls back to SW_HIDEWINDOW).
fn set_overlay_cloaked(hwnd: HWND, cloak: bool) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };
    const TRANSPARENT_BIT: isize = 0x20; // WS_EX_TRANSPARENT
    let cloak_int: i32 = if cloak { 1 } else { 0 };
    if unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK as u32,
            &cloak_int as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        )
    } < 0
    {
        return false;
    }
    let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let want = if cloak {
        exstyle | TRANSPARENT_BIT
    } else {
        exstyle & !TRANSPARENT_BIT
    };
    if want != exstyle {
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE,
            );
        }
    }
    true
}

/// Activate the overlay without injecting keyboard input. `AttachThreadInput`
/// lets the foreground transfer share the active input queue when Windows
/// permits it; the elevated helper handles the UIPI case.
///
/// When an elevated window (Task Manager, High IL) has foreground,
/// `AttachThreadInput` fails silently because of UIPI — that's fine.
/// The elevated helper process already called `AllowSetForegroundWindow`
/// with nex.exe's PID, so `SetForegroundWindow` will succeed without it.
fn force_foreground(hwnd: HWND) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
        SetForegroundWindow, ShowWindow, SW_SHOW, SW_SHOWNA,
    };
    unsafe {
        // Already foreground — skip the synthetic Alt tap which would
        // blur the WebView2 child's focused input element.
        let fg = GetForegroundWindow();
        if fg == hwnd && IsWindowVisible(hwnd) != 0 {
            return;
        }
        let fg = GetForegroundWindow();
        let cur_tid = GetCurrentThreadId();
        let fg_tid = if fg.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, std::ptr::null_mut())
        };
        let attached = fg_tid != 0 && fg_tid != cur_tid;
        if attached {
            // May fail (return 0) when fg_tid belongs to a higher-IL
            // process (e.g. Task Manager). This is acceptable —
            // AllowSetForegroundWindow (called by helper) handles it.
            let _ = AttachThreadInput(cur_tid, fg_tid, 1);
        }
        let foreground_before_show = fg == hwnd || SetForegroundWindow(hwnd) != 0;
        ShowWindow(
            hwnd,
            if foreground_before_show {
                SW_SHOWNA
            } else {
                SW_SHOW
            },
        );
        BringWindowToTop(hwnd);
        if !foreground_before_show {
            SetForegroundWindow(hwnd);
        }
        // NOTE: deliberately NO SetFocus(hwnd) here — keyboard focus must
        // land on the WebView2 CHILD window, not the container. Forcing it
        // onto the container leaves the page unable to receive typing.
        if attached {
            // Only detach if attach succeeded (both must be attached).
            // We can't know reliably, so just try — it's harmless if
            // they were never attached.
            AttachThreadInput(cur_tid, fg_tid, 0);
        }
    }
}

/// Single activation entry point (mature privilege split): the elevated
/// helper owns SetForegroundWindow — High IL, UIPI-proof, with its own
/// verify-and-retry loop. The local AttachThreadInput path runs only
/// while no helper is active (first run, elevation declined); the
/// FocusReassert watchdog re-requests if foreground still isn't ours,
/// and a dead helper clears HELPER_ACTIVE via pipe-EOF so later
/// requests fall back to local automatically.
fn request_foreground(hwnd: HWND) {
    crate::overlay::hotkey::signal_overlay_ready();
    if !crate::overlay::hotkey::helper_active() {
        force_foreground(hwnd);
    }
}

// ─────────────────────────────────────────────────────────────────
// Instance-signal subclass — relays ExternalShow/ExternalQuit
// registered window messages (posted by a second `nex.exe` process)
// into the `event_tx` channel that the runtime worker drains.
// ─────────────────────────────────────────────────────────────────

struct InstanceSignalCtx {
    msg_show: u32,
    msg_quit: u32,
    event_tx: Sender<OverlayEvent>,
}

unsafe extern "system" fn instance_signal_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    dwrefdata: usize,
) -> LRESULT {
    if dwrefdata == 0 {
        // SAFETY: hwnd is valid window handle from subclass registration
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: dwrefdata is a valid pointer stored by SetWindowSubclass
    let ctx = unsafe { &*(dwrefdata as *const InstanceSignalCtx) };
    if msg != 0 {
        if msg == ctx.msg_show {
            let _ = ctx.event_tx.send(OverlayEvent::ExternalShow);
            return 0;
        }
        if msg == ctx.msg_quit {
            let _ = ctx.event_tx.send(OverlayEvent::ExternalQuit);
            return 0;
        }
        if msg == WM_INPUT {
            // Detect Win key via raw input. Raw input catches the toggle
            // press even where the LL hook is bypassed (Chromium's own
            // hook, elevated foreground); the Start menu is suppressed by
            // the menu-mask key, never by RIDEV_NOHOTKEYS.
            let h_raw_input = lparam as windows_sys::Win32::Foundation::HANDLE;
            let header_sz = std::mem::size_of::<RAWINPUTHEADER>() as u32;
            let mut raw_input: std::mem::MaybeUninit<RAWINPUT> = std::mem::MaybeUninit::uninit();
            let mut sz = std::mem::size_of::<RAWINPUT>() as u32;
            let written = unsafe {
                GetRawInputData(
                    h_raw_input,
                    RID_INPUT,
                    raw_input.as_mut_ptr() as *mut core::ffi::c_void,
                    &mut sz,
                    header_sz,
                )
            };
            if written > 0 {
                // SAFETY: GetRawInputData filled the struct when written > 0.
                let raw = unsafe { raw_input.assume_init_ref() };
                if raw.header.dwType == 1 {
                    // RIM_TYPEKEYBOARD
                    let vk = unsafe { raw.data.keyboard.VKey };
                    let flags = unsafe { raw.data.keyboard.Flags };
                    let is_win = vk == VK_LWIN || vk == VK_RWIN;
                    // Native hotkey recording for the settings page:
                    // swallow keys and translate them while armed.
                    if RECORDING_HOTKEY.load(Ordering::SeqCst) {
                        return unsafe { handle_recording_key(vk, flags, ctx) };
                    }
                    // Win hotkey detection lives HERE (raw input receives
                    // keys globally; it does NOT reach elevated foreground
                    // windows, which is what the helper's pipe is for), not
                    // in any hook — so it works identically whether the
                    // helper is connected or not.
                    // Both this and the helper can report one Win press:
                    // toggle via send_hotkey_event, never directly.
                    if is_win && crate::overlay::hotkey::is_win_key_hotkey() {
                        if (flags & RI_KEY_BREAK) != 0 {
                            // Whichever detector sees the key-up first
                            // consumes the press; the other gets false, so a
                            // bare tap toggles exactly once. The decision
                            // uses the shared chord flag, so a dropped raw
                            // make event (Win+E) can no longer make this
                            // press look bare.
                            if crate::overlay::hotkey::finish_win_press() {
                                crate::runtime::log_info(
                                    "[nex::debug] WM_INPUT Win key-up, bare press, toggling",
                                );
                                crate::overlay::hotkey::invalidate_start_gesture();
                                let _ = crate::overlay::hotkey::send_hotkey_event(&ctx.event_tx, 1);
                            }
                            crate::overlay::hotkey::release_mask_for_press();
                        } else {
                            crate::overlay::hotkey::note_win_press_start();
                            // Covers the case where the hook did not see this
                            // key-down (Chromium's hook runs first while the
                            // overlay is focused).
                            crate::overlay::hotkey::hold_mask_for_press();
                        }
                    } else if (flags & RI_KEY_BREAK) == 0 {
                        // Any other key pressed while Win is held makes this a
                        // chord (Win+E, Win+D…) — never a toggle.
                        if vk != crate::overlay::hotkey::VK_MENU_MASK {
                            crate::overlay::hotkey::note_chord_key();
                        }
                        // Non-Win hotkey detection via raw input.
                        // WH_KEYBOARD_LL may not fire when the overlay
                        // is foreground (Chromium installs its own LL
                        // hook which runs first), so we check the
                        // configured hotkey here. Overlaps the hook and
                        // the RegisterHotKeyW fallback: send through
                        // send_hotkey_event, never directly.
                        if crate::overlay::hotkey::check_raw_input_hotkey(vk) {
                            crate::runtime::log_info(&format!(
                                "[nex::debug] WM_INPUT hotkey vk={} sending toggle",
                                vk,
                            ));
                            let _ = crate::overlay::hotkey::send_hotkey_event(&ctx.event_tx, 1);
                        }
                    }
                }
            }
        }
    }
    // SAFETY: hwnd is valid window handle from subclass registration
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

unsafe fn install_instance_signal_subclass(hwnd: HWND, event_tx: &Sender<OverlayEvent>) {
    let show_name: Vec<u16> = "Nex.ExternalShow.v1"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let quit_name: Vec<u16> = "Nex.ExternalQuit.v1"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: string pointers are NUL-terminated wide strings
    let msg_show = unsafe { RegisterWindowMessageW(show_name.as_ptr()) };
    let msg_quit = unsafe { RegisterWindowMessageW(quit_name.as_ptr()) };
    if msg_show == 0 || msg_quit == 0 {
        return;
    }
    let ctx = Box::new(InstanceSignalCtx {
        msg_show,
        msg_quit,
        event_tx: event_tx.clone(),
    });
    let ptr = Box::into_raw(ctx) as usize;
    // SAFETY: hwnd is valid window handle, subclass proc is valid
    unsafe { SetWindowSubclass(hwnd, Some(instance_signal_subclass), 1, ptr) };
}

// ─────────────────────────────────────────────────────────────────
// Raw input helpers — suppress Win-key Start while overlay is foreground.
// ─────────────────────────────────────────────────────────────────

/// While RECORDING_HOTKEY is armed, translate raw keyboard events into a
/// canonical hotkey string and report via OverlayEvent::HotkeyRecorded.
/// Every event is swallowed (returns 0) — nothing leaks to the OS.
unsafe fn handle_recording_key(vk: u16, flags: u16, ctx: &InstanceSignalCtx) -> LRESULT {
    let is_break = (flags & RI_KEY_BREAK) != 0;
    //ESC cancels recording, any other key is a candidate.
    let is_win = vk == VK_LWIN || vk == VK_RWIN;
    if vk == 0x1B && !is_break {
        RECORDING_HOTKEY.store(false, Ordering::SeqCst);
        let _ = ctx
            .event_tx
            .send(OverlayEvent::HotkeyRecorded(String::new()));
        return 0;
    }
    let is_modifier = matches!(vk, 0xA0..=0xA5 | 0x10 | 0x11 | 0x12 | 0x5B | 0x5C);
    if is_win {
        //Commit bare "Win" on keyup only if no other jey is joined it.
        if is_break && !RECORD_WIN_CHORD.load(Ordering::SeqCst) {
            RECORDING_HOTKEY.store(false, Ordering::SeqCst);
            let _ = ctx
                .event_tx
                .send(OverlayEvent::HotkeyRecorded("Win".into()));
        }
        return 0;
    }
    if !is_break && !is_modifier {
        let mut parts: Vec<String> = Vec::new();
        unsafe {
            if GetAsyncKeyState(0x11) as u16 & 0x8000 != 0 {
                parts.push("Ctrl".into());
            }
            if GetAsyncKeyState(0x12) as u16 & 0x8000 != 0 {
                parts.push("Alt".into());
            }
            if GetAsyncKeyState(0x10) as u16 & 0x8000 != 0 {
                parts.push("Shift".into());
            }
            if GetAsyncKeyState(VK_LWIN as i32) as u16 & 0x8000 != 0
                || GetAsyncKeyState(VK_RWIN as i32) as u16 & 0x8000 != 0
            {
                parts.push("Win".into());
                RECORD_WIN_CHORD.store(true, Ordering::SeqCst);
            }
        }
        if let Some(name) = vk_to_hotkey(vk) {
            parts.push(name);
            let combo = parts.join("+");
            RECORDING_HOTKEY.store(false, Ordering::SeqCst);
            let _ = ctx.event_tx.send(OverlayEvent::HotkeyRecorded(combo));
        }
        //unrecordable vks are swallowed silently (no beep, no OS hotkey).  The settings page
    }
    0
}

/// Clear the Win press tracking. Called after a live hotkey change and on
/// hide so a half-finished press of the old binding can't decide the next one.
pub(crate) fn reset_raw_win_state() {
    crate::overlay::hotkey::reset_win_press_state();
}
pub(crate) static RECORDING_HOTKEY: AtomicBool = AtomicBool::new(false);
static RECORD_WIN_CHORD: AtomicBool = AtomicBool::new(false); //win+other seen
const RID_INPUT: u32 = 0x10000003;
const RI_KEY_BREAK: u16 = 0x0001;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;

/// Register the keyboard raw-input sink so the overlay receives `WM_INPUT`
/// for all keyboard events, foreground or not. `RIDEV_INPUTSINK` only:
/// `RIDEV_NOHOTKEYS` would also suppress every registered system hotkey
/// (Snipping Tool, shell chords) while nex has focus, and Start is kept
/// shut by the injected menu-mask key instead. Returns true on success.
fn register_raw_input_sink(hwnd: HWND) -> bool {
    let mut rid = RAWINPUTDEVICE {
        usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
        usUsage: 0x06,     // HID_USAGE_GENERIC_KEYBOARD
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    let ok = unsafe {
        RegisterRawInputDevices(&mut rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
    } != 0;
    let err = if !ok {
        unsafe { windows_sys::Win32::Foundation::GetLastError() }
    } else {
        0
    };
    crate::runtime::log_info(&format!(
        "[nex::debug] register_raw_input_sink: ok={} last_err={}",
        ok, err,
    ));
    ok
}

fn vk_to_hotkey(vk: u16) -> Option<String> {
    match vk {
        0x41..=0x5A => Some(((vk as u8) as char).to_string()), // A-Z
        0x30..=0x39 => Some(((vk as u8) as char).to_string()), // 0-9
        0x20 => Some("Space".into()),
        0x5B | 0x5C => Some("Win".into()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)), //F1 -F24
        _ => None,
    }
}

/// Remove the raw input sink registration, restoring normal keyboard
/// input routing.  Call before hiding the overlay.

fn unregister_raw_input_sink() {
    let mut rid = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: std::ptr::null_mut(),
    };
    let ok = unsafe {
        RegisterRawInputDevices(&mut rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
    } != 0;
    let err = if !ok {
        unsafe { windows_sys::Win32::Foundation::GetLastError() }
    } else {
        0
    };
    crate::runtime::log_info(&format!(
        "[nex::debug] unregister_raw_input_sink: ok={} last_err={}",
        ok, err,
    ));
}
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod snapshot_tests {
    use super::snapshot_state_with_transparency;
    use crate::config::PanelBackground;
    use crate::overlay::model::ShimState;
    use serde_json::{json, Value};

    fn state(
        glass_native: bool,
        frame_ready: bool,
        light: bool,
        background: PanelBackground,
    ) -> ShimState {
        ShimState {
            glass_native,
            glass_frame_ready: frame_ready,
            glass_light: light,
            panel_background: background,
            ..ShimState::default()
        }
    }

    fn field(snapshot: &Value, key: &str) -> Value {
        snapshot[key].clone()
    }

    fn push(s: &ShimState) -> Value {
        serde_json::from_str(&snapshot_state_with_transparency(s, false, true)).expect("snapshot is JSON")
    }

    #[test]
    fn capture_exclusion_follows_the_material_not_the_window() {
        use super::capture_exclusion_needed;
        // Glass feeds off the desktop duplication: it must stay out of
        // captures or it samples its own last frame.
        assert!(capture_exclusion_needed(PanelBackground::Glass, true));
        // Opaque panels never feed anything, so screenshots/OBS/Discord must
        // see the app (regression: the flag used to be set for every mode).
        assert!(!capture_exclusion_needed(PanelBackground::Black, true));
        assert!(!capture_exclusion_needed(PanelBackground::White, true));
        // No glass layer (creation failed / NEX_REFRACT_LAB=0): nothing to hide.
        assert!(!capture_exclusion_needed(PanelBackground::Glass, false));
    }

    #[test]
    fn reduced_transparency_keeps_page_opaque_even_with_ready_glass() {
        let s = state(true, true, true, PanelBackground::Glass);
        let push: Value = serde_json::from_str(&snapshot_state_with_transparency(&s, false, false)).unwrap();
        assert_eq!(push["glassNative"], json!(false));
        assert_eq!(push["glassLight"], json!(false));
        assert!(push["panelBackground"] == "black" || push["panelBackground"] == "white");
    }

    #[test]
    fn glass_native_waits_for_a_presented_frame() {
        // The page goes transparent on this flag, so it must stay false
        // until a frame has actually been presented: that gate is what
        // fixed the fully-transparent window before the effects showed.
        assert_eq!(
            field(&push(&state(true, false, false, PanelBackground::Glass)), "glassNative"),
            json!(false)
        );
        assert_eq!(
            field(&push(&state(true, true, false, PanelBackground::Glass)), "glassNative"),
            json!(true)
        );
        // Opaque panel modes never ask the page to go transparent.
        assert_eq!(
            field(&push(&state(true, true, false, PanelBackground::Black)), "glassNative"),
            json!(false)
        );
    }

    #[test]
    fn a_non_glass_push_never_asks_for_the_light_palette() {
        // glassLight alone must not flip the page palette: an Acrylic or
        // opaque push with a stale light flag would repaint the panel.
        assert_eq!(
            field(&push(&state(false, true, true, PanelBackground::Black)), "glassLight"),
            json!(false)
        );
        assert_eq!(
            field(&push(&state(true, true, true, PanelBackground::Glass)), "glassLight"),
            json!(true)
        );
    }

    #[test]
    fn pointer_over_panel_is_pushed_every_snapshot() {
        // The page needs this on every push: it decides whether a fresh
        // show may keep its last hover coordinates.
        let mut s = state(true, true, false, PanelBackground::Glass);
        s.pointer_over_panel = true;
        assert_eq!(field(&push(&s), "pointerOverPanel"), json!(true));
        s.pointer_over_panel = false;
        assert_eq!(field(&push(&s), "pointerOverPanel"), json!(false));
    }
}
