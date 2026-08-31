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

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// Re-arm the raw-input sink with the current hotkey's suppression
/// requirement. Called after a live hotkey update — the sink's
/// RIDEV_NOHOTKEYS flag is chosen at registration time, so it must be
/// refreshed whenever is_win_key_hotkey() changes.
pub(crate) fn rearm_raw_input_sink() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    register_raw_input_sink(hwnd as HWND, crate::overlay::hotkey::is_win_key_hotkey());
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
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RegisterRawInputDevices, RIDEV_INPUTSINK, RIDEV_NOHOTKEYS, RIDEV_REMOVE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, RegisterWindowMessageW, SetWindowPos,
    WM_INPUT,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

use crate::overlay::icons::IconCache;
use crate::overlay::model::{OverlayEvent, OverlayRowRole, ShimState};
use crate::overlay::model::Theme;

const WINDOW_WIDTH: f64 = 720.0;
const INITIAL_HEIGHT: f64 = 60.0;
const MAX_HEIGHT: f64 = 530.0;
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
const SETTINGS_HTML: &str= include_str!("../../assets/settings.html");
const SETTINGS_JS: &str= include_str!("../../assets/settings.js");

/// Embedded web UI assets (premium Raycast-dark cmdk UI).
const INDEX_HTML: &str = include_str!("../../assets/index.html");
pub(crate) const STYLE_CSS: &str = include_str!("../../assets/style.css");
const APP_JS: &str = include_str!("../../assets/app.js");


/// Commands the shim posts to the UI thread via the event-loop proxy.
#[derive(Debug, Clone)]
pub(crate) enum UiCommand {
    /// The web page finished loading and registered `window.nex`.
    WebviewReady,
    /// Re-push the current [`ShimState`] snapshot to the page.
    Apply,
    /// Icons decoded in the background are now cached — re-send the
    /// icon data JSON so the page can patch placeholder <img> elements.
    ApplyIcons,
    /// Only the selected index changed — send a lightweight update.
    SelectChanged(usize),
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
    /// Fired by the warm-release timer; if still hidden and the
    /// generation matches, clears the icon cache while keeping the
    /// WebView warm for consistent re-open timing.
    Teardown(u64),
    /// The page painted after a push_state — trigger deferred show.
    Painted,
    /// The page measured its content height (CSS px); resize to hug it.
    Resize { h: f64, immediate: bool },
    /// Exit the event loop (clean shutdown).
    Quit,
    /// Debounce timer fired — apply the coalesced resize height.
    ApplyResize,
    /// Delayed keyboard state check (posted ~200ms after hide).
    CheckKeyboardState(Instant),
    /// Open/show the settings window, preload with a config snapshot.
    OpenSettings { snapshot: String },
    /// Result of a settings save attempt, pushed into the settings page.
    SettingsSaveResult { json: String },
    /// Push a captured hotkey combo into the settings page.
    SettingsHotkeyRecorded { combo: String },
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
    apply_window_chrome(&window, &state);
    unsafe { install_instance_signal_subclass(hwnd, &event_tx); }

    // Register raw input sink permanently at startup so the overlay
    // receives WM_INPUT for keyboard events regardless of which window
    // is foreground.  This ensures hotkey detection works even when
    // Task Manager, advanced settings, or other native Windows windows
    // have focus (the WH_KEYBOARD_LL hook thread may not receive events
    // for elevated/UWP windows).
    // Win hotkey detection lives in raw input — arm NOHOTKEYS from the
    // start so bare-Win never opens Start; nex toggles on key-up.
    register_raw_input_sink(hwnd, crate::overlay::hotkey::is_win_key_hotkey());
    OVERLAY_HWND.store(hwnd as isize, Ordering::SeqCst);

    // Start suppression is handled by RIDEV_NOHOTKEYS via
    // RegisterRawInputDevices instead of a focus-sink window.

    // Build the WebView eagerly at startup so the page is fully
    // rendered in the background before the first show.  The WebView
    // stays resident; only the icon cache is released on idle.
    let mut webview = match build_webview(&window, &state, &proxy, &event_tx) {
        Ok(wv) => Some(wv),
        Err(e) => {
            crate::logging::warn(&format!("[nex] webview build failed: {e}"));
            None
        }
    };
    let mut settings_ui: Option<(tao::window::Window, wry::WebView)> = None;
    let mut settings_window_id: Option<tao::window::WindowId>= None;
    let last_settings_snapshot: std::sync::Arc<std::sync::Mutex<Option<String>>> = std::sync::Arc::new(std::sync::Mutex::new(None));

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
    let deferred_hide_armed = Arc::new(AtomicBool::new(false));

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
                let timeout = armed
                    .map(|when| when.saturating_duration_since(Instant::now()));
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
                let timeout = armed
                    .map(|(when, _)| when.saturating_duration_since(Instant::now()));
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
                        apply_window_height(&window, webview.as_ref(), INITIAL_HEIGHT);
                        last_applied_height = INITIAL_HEIGHT;
                        pending_resize = None;
                        first_resize_after_show = true;
                        push_state(&webview, &state, &icon_cache, true);
                        show_pending = true;
                    }
                }
                UiCommand::Apply => {
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        push_state(&webview, &state, &icon_cache, false);
                    }
                }
                UiCommand::ApplyStatus => {
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        push_status(&webview, &state);
                    }
                }
                UiCommand::ApplyIcons => {
                    // Progressive icon delivery: the background prefetch
                    // thread decoded icons and posted this command. Re-send
                    // the icon data JSON so the page can patch placeholder
                    // <img> elements that painted with no src (cold cache).
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        let snapshot = {
                            let Ok(s) = state.lock() else { return };
                            s.clone()
                        };
                        let icons_json = snapshot_icons_json(&snapshot, &icon_cache);
                        if !icons_json.is_empty() {
                            if let Some(wv) = webview.as_ref() {
                                post_json(wv, &icons_json);
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
                    if webview.is_none() {
                        ready = false;
                        // Mark the show as pending before building the
                        // WebView so that spurious Focused(false) events
                        // (sent by Tao/Windows during WebView creation)
                        // do not trigger Escape and hide the overlay
                        // before WebviewReady can display it.
                        show_pending = true;
                        match build_webview(&window, &state, &proxy, &event_tx) {
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
                    // Set show_pending FIRST so spurious Focused(false) during
                    // position_window / set_inner_size / push_state cannot
                    // trigger Escape (which would desync OverlayState from
                    // the actual window state).
                    show_pending = true;
                    // Cancel any pending deferred-hide from a previous
                    // focus-loss — the user意图 is to show, not hide.
                    deferred_hide_armed.store(false, Ordering::SeqCst);
                    position_window(&window, hwnd);
                    // Start at search-bar height — JS sends resize when content appears.
                    apply_window_height(&window, webview.as_ref(), INITIAL_HEIGHT);
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
                    }
                    apply_window_chrome(&window, &state);
                    // Push state with show_pending so the JS side sends
                    // post("painted") to trigger the deferred show.
                    push_state(&webview, &state, &icon_cache, true);
                }
                UiCommand::FocusInput => {
                    window.set_focus();
                    focus_input(&webview);
                }
                UiCommand::Hide => {
                    // Re-inject the menu-mask key (0xE8) and spin-wait for the
                    // RIT to register it, so that when RIDEV_NOHOTKEYS is
                    // removed and the window is hidden, the newly-foreground
                    // window never sees a bare Win key — it sees Win+0xE8.
                    // This prevents Start from opening during the hide
                    // transition when the Win key is still physically held.
                    crate::overlay::hotkey::hold_mask_before_hide();
                    // Keep NOHOTKEYS armed for Win hotkeys (bare Win = toggle,
                    // Start must stay suppressed even while hidden); drop it
                    // for other hotkeys so their combos reach apps normally.
                    register_raw_input_sink(hwnd, crate::overlay::hotkey::is_win_key_hotkey());
                    RAW_WIN_DOWN.store(0, Ordering::SeqCst);
                    RAW_WIN_CHORD.store(false, Ordering::SeqCst);
                    window.set_visible(false);
                    OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
                    crate::overlay::hotkey::release_mask_after_hide();
                    let fg_after = unsafe { GetForegroundWindow() };
                    let is_visible = unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd)
                    };
                    let win_down = unsafe {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x5B) as u16 & 0x8000 != 0
                    };
                    crate::runtime::log_info(&format!(
                        "[nex::debug] Hide: fg={:?} visible={} win_down={}",
                        fg_after, is_visible, win_down,
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
                    register_raw_input_sink(hwnd, crate::overlay::hotkey::is_win_key_hotkey());
                    RAW_WIN_DOWN.store(0, Ordering::SeqCst);
                    RAW_WIN_CHORD.store(false, Ordering::SeqCst);
                    window.set_visible(false);
                    OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
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
                        // heap outside Chromium.
                        let entries = icon_cache.len();
                        icon_cache.clear();
                        crate::logging::info(&format!(
                            "[nex] ui warm-release: icon cache cleared entries={entries} (webview kept warm)"
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
                            apply_window_height(&window, webview.as_ref(), h);
                        }
                    } else if first_resize_after_show {
                        // First resize after show: apply immediately to
                        // prevent the "search bar first, items pop in
                        // 100ms later" visual flash.
                        first_resize_after_show = false;
                        pending_resize = None;
                        if (h - last_applied_height).abs() > 0.5 {
                            last_applied_height = h;
                            apply_window_height(&window, webview.as_ref(), h);
                        }
                    } else if h - last_applied_height >= RESIZE_IMMEDIATE_GROWTH {
                        // Large growth: apply immediately so the window is
                        // already full-size when the new rows paint.
                        first_resize_after_show = false;
                        pending_resize = None;
                        last_applied_height = h;
                        apply_window_height(&window, webview.as_ref(), h);
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
                            apply_window_height(&window, webview.as_ref(), h);
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
                        show_pending = false;
                        last_show = Instant::now();
                        was_focused = false;
                        focus_reassert_used = false;
                        // Gate on live state: an outside-click Escape during
                        // the show window (between Show and first paint)
                        // already hid the overlay — do NOT resurrect it.
                        // Also reject stale Painted when Hide cleared rows
                        // (search bar would show with no content).
                        let should_show = state.lock().map(|s| s.visible && !s.rows.is_empty()).unwrap_or(false);
                        if !should_show {
                            crate::runtime::log_info("[nex] host Painted: stale (visible=false or rows empty), skipping show");
                            return;
                        }
                        window.set_visible(true);
                        OVERLAY_VISIBLE.store(true, Ordering::SeqCst);
                        // Always register raw input sink so the overlay
                        // receives WM_INPUT for all keyboard events while
                        // foreground.  This works around Chromium/WebView2
                        // installing its own WH_KEYBOARD_LL hook when focused
                        // (installed after ours, so Chromium eats the events
                        // before our hook can process them).
                        // For Win-key hotkeys, also enable RIDEV_NOHOTKEYS
                        // to suppress Start at the RIT level.
                        register_raw_input_sink(hwnd, crate::overlay::hotkey::is_win_key_hotkey());
                        force_foreground(hwnd);
                        // Focus the page's input — without this the first
                        // show after launch is visible but unfocused.
                        focus_input(&webview);
                        // Signal the elevated helper to call SetForegroundWindow
                        // from High IL (bypasses UIPI for Task Manager scenario).
                        crate::overlay::hotkey::signal_overlay_ready();
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
                    // Only fire when overlay IS visible but lost focus —
                    // force_foreground calls ShowWindow(SW_SHOW) which
                    // would re-show a hidden window.
                    let should_reassert = state.lock().map(|s| s.visible && !s.has_focus).unwrap_or(false);
                    if should_reassert {
                        force_foreground(hwnd);
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
                        let _ = wv.evaluate_script(&format!(
                            "window.hotkeyRecorded && window.hotkeyRecorded('{combo}')"
                        ));
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
                            .with_background_color((0, 0, 0, 0))
                            .with_url("nexasset://localhost/settings.html")
                            .with_custom_protocol("nexasset".into(),move |_id, request| {
                                serve_asset(request)
                            })
                            .with_ipc_handler(move |req| {
                                let body = req.body().clone();
                                if body.contains("\"t\":\"save\"") {
                                    let _ = save_tx.send(OverlayEvent::SaveSettings(body));
                                } else if body.contains("\"t\":\"ready\"") {
                                    let snap = snapshot_for_ipc.lock().unwrap().clone();
                                    if let Some(snap) = snap {
                                        let _ = proxy_for_ipc.send_event(UiCommand::OpenSettings { snapshot: snap });
                                    };
                                } else if body.contains("\"t\":\"recordHotkey\"") {
                                    RECORDING_HOTKEY.store(true, Ordering::SeqCst);
                                    // Keep Start menu shut while capturing
                                    register_raw_input_sink(record_hwnd, true);
                                } else if body.contains("\"t\":\"cancelRecord\"") {
                                    RECORDING_HOTKEY.store(false, Ordering::SeqCst);
                                    // Restore normal Win routing.
                                    register_raw_input_sink(
                                        record_hwnd,
                                        crate::overlay::hotkey::is_win_key_hotkey(),
                                    );
                                } else if body.contains("\"t\":\"minimize\"") {
                                    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE};
                                    if let Ok(mut slot) = settings_hwnd_for_ipc.lock() {
                                        if let Some(h) = *slot {
                                            unsafe { ShowWindow(h, SW_MINIMIZE); }
                                        }
                                    }
                                } else if body.contains("\"t\":\"close\"") {
                                    let _ = proxy_for_ipc.send_event(UiCommand::CloseSettings);
                                }
                                else {
                                    crate::runtime::log_info(&format!("[nex] settings ipc: {body}"));
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
                    let was_focused_val = was_focused;
                    let show_pending_val = show_pending;
                    let bare_win = crate::overlay::hotkey::is_bare_win_press_active();
                    let grace_ms = last_show.elapsed().as_millis() as u64;
                    let state_vis = state.lock().map(|s| s.visible).unwrap_or(false);
                    if was_focused_val && !show_pending_val && !bare_win && grace_ms >= FOCUS_GRACE_MS && state_vis
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
                            force_foreground(hwnd);
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
                            // Arm the deferred-hide thread inline. The 150ms
                            // window is enough for WebView2 to finish moving
                            // focus into its input element and post a
                            // Focused(true) back; if focus does NOT return
                            // we treat it as a genuine dismissal.
                            if !deferred_hide_armed.swap(true, Ordering::SeqCst) {
                                let state_clone = state.clone();
                                let tx_clone = event_tx.clone();
                                let armed = deferred_hide_armed.clone();
                                std::thread::Builder::new()
                                    .name("nex-deferred-hide".into())
                                    .spawn(move || {
                                        std::thread::sleep(Duration::from_millis(150));
                                        if let Ok(s) = state_clone.lock() {
                                            if s.visible
                                                && !s.has_focus
                                                && !crate::overlay::hotkey::is_bare_win_press_active()
                                            {
                                                let _ = tx_clone.send(OverlayEvent::Escape);
                                            }
                                        }
                                        armed.store(false, Ordering::SeqCst);
                                    })
                                    .ok();
                            }
                            return;
                        } else {
                            crate::runtime::log_info(&format!(
                                "[nex::debug] Focused(false): sending Escape (was_focused={} show_pending={} grace={}ms state_vis={})",
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
                            let _ = event_tx.send(OverlayEvent::Escape);
                        }
                    } else {
                        crate::runtime::log_info(&format!(
                            "[nex::debug] Focused(false): BLOCKED Escape (was_focused={} show_pending={} bare_win={} grace={}ms state_vis={})",
                            was_focused_val, show_pending_val, bare_win, grace_ms, state_vis,
                        ));
                        // Deferred hide: if overlay still visible and no
                        // retry thread is armed, spawn one.  The double
                        // Focused(false) per blur is deduplicated by the
                        // swap — only the first event arms a thread.
                        // 150ms: imperceptible delay for genuine outside
                        // clicks; focus-flap at show (same-ms pairs in
                        // logs) re-focuses well within this window and
                        // cancels the hide.
                        if state_vis && !deferred_hide_armed.swap(true, Ordering::SeqCst) {
                            let state_clone = state.clone();
                            let tx_clone = event_tx.clone();
                            let armed = deferred_hide_armed.clone();
                            std::thread::Builder::new()
                                .name("nex-deferred-hide".into())
                                .spawn(move || {
                                    std::thread::sleep(Duration::from_millis(150));
                                    if let Ok(s) = state_clone.lock() {
                                        if s.visible
                                            && !s.has_focus
                                            && !crate::overlay::hotkey::is_bare_win_press_active()
                                        {
                                            let _ = tx_clone.send(OverlayEvent::Escape);
                                        }
                                    }
                                    armed.store(false, Ordering::SeqCst);
                                })
                                .ok();
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
                    if let Some((w, _)) = &settings_ui {
                        animate_window_close(w.hwnd() as HWND);
                        let _ = w.set_visible(false);
                        crate::runtime::log_info("[nex] settings window hidden via X");
                        let _ = event_tx.send(OverlayEvent::SettingsClosed);
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
        crate::logging::warn(&format!("[nex] event loop panicked during teardown; continuing shutdown: {msg}"));
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
) -> Result<WebView, String> {
    let ipc_state = state.clone();
    let ipc_proxy = proxy.clone();
    let ipc_tx = event_tx.clone();

    WebViewBuilder::new()
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_url("nexasset://localhost/")
        .with_custom_protocol("nexasset".into(), move |_id, request| {
            serve_asset(request)
        })
        .with_ipc_handler(move |req: Request<String>| {
            handle_ipc(req.body(), &ipc_state, &ipc_proxy, &ipc_tx);
        })
        .build(window)
        .map_err(|e| format!("{e}"))
}

/// Serve embedded UI assets.
fn serve_asset(
    request: Request<Vec<u8>>,
) -> Response<std::borrow::Cow<'static, [u8]>> {
    let path = request.uri().path().to_string();

    let (content_type, body): (&str, std::borrow::Cow<'static, [u8]>) = match path.as_str() {
        "/" | "/index.html" => ("text/html", INDEX_HTML.as_bytes().into()),
        "/style.css" => ("text/css", STYLE_CSS.as_bytes().into()),
        "/app.js" => ("text/javascript", APP_JS.as_bytes().into()),
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

/// Parse one IPC message from the page and act on it.
fn handle_ipc(
    body: &str,
    state: &Arc<Mutex<ShimState>>,
    proxy: &EventLoopProxy<UiCommand>,
    event_tx: &Sender<OverlayEvent>,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    let t = value.get("t").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "ready" => {
            try_send_ui(proxy, UiCommand::WebviewReady);
        }
        "query" => {
            // Ignore queries that fire after hide (debounced input
            // races with Escape).  The shim clears query/rows on
            // hide; a stale query would prevent idle-state setup.
            if !state.lock().map(|s| s.visible).unwrap_or(false) {
                return;
            }
            let q = value
                .get("v")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Ok(mut s) = state.lock() {
                s.query = q.clone();
            }
            let _ = event_tx.send(OverlayEvent::QueryChanged(q));
        }
        "submit" => {
            let idx = value.get("v").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            if let Ok(mut s) = state.lock() {
                s.selected = idx;
            }
            let _ = event_tx.send(OverlayEvent::Submit);
        }
        "select" => {
            let idx = value.get("v").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            if let Ok(mut s) = state.lock() {
                s.selected = idx;
            }
        }
        "escape" => {
            let _ = event_tx.send(OverlayEvent::Escape);
        }
        "resize" => {
            // JS sends {t:"resize", v:{v:h, immediate:bool}} (new) or
            // {t:"resize", v:h} (legacy number-only).
            let (h, immediate) = match value.get("v") {
                Some(serde_json::Value::Object(obj)) => {
                    let h = obj.get("v").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let imm = obj.get("immediate").and_then(|v| v.as_bool()).unwrap_or(false);
                    (h, imm)
                }
                Some(serde_json::Value::Number(n)) => {
                    (n.as_f64().unwrap_or(0.0), false)
                }
                _ => return,
            };
            try_send_ui(proxy, UiCommand::Resize { h, immediate });
        }
        "painted" => {
            // First paint after push_state — safe to show the window.
            // Deferred from WebviewReady / Show to avoid a flash of
            // uncomposited content before the WebView2 paints.
            try_send_ui(proxy, UiCommand::Painted);
        }
        "pin" => {
            if let Some(title) = value.get("v").and_then(|v| v.as_str()) {
                let _ = event_tx.send(OverlayEvent::PinApp(title.to_string()));
            }
        }
        "unpin" => {
            if let Some(title) = value.get("v").and_then(|v| v.as_str()) {
                let _ = event_tx.send(OverlayEvent::UnpinApp(title.to_string()));
            }
        }
        "addToQuickLaunch" => {
            if let Some(path) = value.get("v").and_then(|v| v.as_str()) {
                let _ = event_tx.send(OverlayEvent::AddToQuickLaunch(path.to_string()));
            }
        }
        "powerAction" => {
            if let Some(action) = value.get("v").and_then(|v| v.as_str()) {
                let event = match action {
                    "lock" => OverlayEvent::TrayLock,
                    "sleep" => OverlayEvent::TraySleep,
                    "shutdown" => OverlayEvent::PowerMenuShutdown,
                    "restart" => OverlayEvent::PowerMenuRestart,
                    "signout" => OverlayEvent::TraySignOut,
                    _ => return,
                };
                let _ = event_tx.send(event);
            }
        }
        "contextAction" => {
            let action = value.get("v").and_then(|v| v.get("action")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let title = value.get("v").and_then(|v| v.get("title")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let path = value.get("v").and_then(|v| v.get("path")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let _ = event_tx.send(OverlayEvent::ContextAction(action, title, path));
        }
        "settings" => {
            let _ = event_tx.send(OverlayEvent::OpenSettings);
        }
        _ => {}
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
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND,
        ICoreWebView2ProcessFailedEventArgs,
        ICoreWebView2ProcessFailedEventHandler,
        ICoreWebView2ProcessFailedEventHandler_Vtbl,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
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
            let mut kind: COREWEBVIEW2_PROCESS_FAILED_KIND =
                unsafe { std::mem::zeroed() };
            if unsafe { args.ProcessFailedKind(&mut kind) }.is_ok() {
                crate::logging::error(&format!(
                    "[nex:webview2] process_failed kind={}",
                    kind.0
                ));
            } else {
                crate::logging::error("[nex:webview2] process_failed kind=<unreadable>");
            }
        }
        windows_core::HRESULT(0)
    }

    unsafe extern "system" fn add_ref(
        this: *mut core::ffi::c_void,
    ) -> u32 {
        // SAFETY: `this` points to a valid ProcessFailedHandler from Box::into_raw
        let h = unsafe { &*(this as *const ProcessFailedHandler) };
        h.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn release(
        this: *mut core::ffi::c_void,
    ) -> u32 {
        // SAFETY: `this` points to a valid ProcessFailedHandler from Box::into_raw
        let h = unsafe { &*(this as *const ProcessFailedHandler) };
        let prev = h.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            // SAFETY: ref_count reached 0; we own the sole reference.
            unsafe { drop(Box::from_raw(this as *mut ProcessFailedHandler)); }
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
        unsafe { *out = core::ptr::null_mut(); }
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
            unsafe { *out = _this; }
            // SAFETY: _this points to a valid ProcessFailedHandler
            unsafe { add_ref(_this); }
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
            crate::logging::warn(&format!(
                "[nex] webview2 subscriptions skipped: {e}"
            ));
            // Reclaim our allocation since WebView2 didn't take it.
            // SAFETY: handler_ptr was created from Box::into_raw above and has
            // not been freed — we are the sole owner on the error path.
            drop(unsafe { Box::from_raw(handler_ptr as *mut ProcessFailedHandler) });
        }
    }
}

/// Fire-and-forget: send a JSON string to the WebView page via
/// `ICoreWebView2::PostWebMessageAsJson`.
fn post_json(webview: &WebView, json: &str) {
    let wv2 = webview.webview();
    let wide: Vec<u16> = json
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = wv2.PostWebMessageAsJson(
            windows_core::PCWSTR::from_raw(wide.as_ptr()),
        );
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
fn push_state(webview: &Option<WebView>, state: &Arc<Mutex<ShimState>>, icons: &Arc<IconCache>, show_pending: bool) {
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
        let _ = wv.evaluate_script("window.nex&&window.nex.focus()");
    }
}

/// Serialize the overlay state into lightweight JSON without icon data.
/// Icon fields contain only the file path (used as a JS cache key).
fn snapshot_state_json(s: &ShimState, show_pending: bool) -> String {
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
                OverlayRowRole::TopHit | OverlayRowRole::Item => "item",
            };
            let selectable = matches!(
                r.role,
                OverlayRowRole::Item
                    | OverlayRowRole::TopHit
                    | OverlayRowRole::Calculator
                    | OverlayRowRole::QuickLaunch
                    | OverlayRowRole::ShowAllApps
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
            serde_json::json!({
                "role": role,
                "title": r.title,
                "subtitle": r.path,
                "kind": r.kind,
                "icon": icon,
                "filePath": file_path,
                "selectable": selectable,
                "resultIndex": r.result_index,
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
        "showPending": show_pending,
        "quickLaunch": quick_launch,
        "quickLaunchVisible": s.quick_launch_visible,
    })
    .to_string()
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

/// Apply acrylic backdrop. CSS handles border-radius + box-shadow on #panel.
fn apply_window_chrome(window: &Window, state: &Arc<Mutex<ShimState>>) {
    let dark = state.lock().map(|s| s.theme == Theme::Dark).unwrap_or(true);
    // Disable DWM transition animation (zoom-out+fade) so hide is instant.
    let hwnd = window.hwnd() as HWND;
    unsafe {
        let disabled: i32 = 1;
        DwmSetWindowAttribute(
            hwnd,
            3, // DWMWA_TRANSITIONS_FORCEDISABLED
            &disabled as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
    // Acrylic blur behind the (transparent) WebView. Falls back to a
    // CSS-painted panel if the OS refuses (window-vibrancy returns Err).
    let tint = if dark {
        Some((0, 0, 0, 230))
    } else {
        Some((245, 245, 247, 140))
    };
    if let Err(_e) = window_vibrancy::apply_acrylic(window, tint) {
        crate::logging::info("[nex] acrylic unavailable; using opaque panel");
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
/// viewport (wry snaps it to the window size on WM_SIZE).
fn apply_window_height(window: &Window, webview: Option<&WebView>, h: f64) {
    window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, h));
    if let Some(wv) = webview {
        keep_webview_viewport_max(wv);
    }
}

/// Center the window horizontally on the monitor under the cursor and
/// anchor it in the upper third (Raycast/Spotlight placement).
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
    let scale = window.scale_factor();
    let size = window.inner_size().to_logical::<f64>(scale);
    let x = left as f64 + ((right - left) as f64 - size.width) / 2.0;
    let y = top as f64 + ((bottom - top) as f64 - size.height) / 2.0;
    window.set_outer_position(PhysicalPosition::new(
        x.max(left as f64) as i32,
        y.max(top as f64) as i32,
    ));
}

fn animate_window_open(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AnimateWindow;
    const AW_BLEND: u32 = 0x00080000;
    const AW_ACTIVATE: u32 = 0x00020000;
    unsafe { AnimateWindow(hwnd, 200, AW_BLEND | AW_ACTIVATE); }
}

fn animate_window_close(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AnimateWindow;
    const AW_BLEND: u32 = 0x00080000;
    const AW_HIDE: u32 = 0x00100000;
    unsafe { AnimateWindow(hwnd, 200, AW_BLEND | AW_HIDE); }
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

/// Steal foreground focus reliably. winit/tao cannot do this on its own
/// because Windows blocks `SetForegroundWindow` from background apps;
/// the `AttachThreadInput` trick is the standard workaround.
///
/// When an elevated window (Task Manager, High IL) has foreground,
/// `AttachThreadInput` fails silently because of UIPI — that's fine.
/// The elevated helper process already called `AllowSetForegroundWindow`
/// with nex.exe's PID, so `SetForegroundWindow` will succeed without it.
fn force_foreground(hwnd: HWND) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    #[allow(unused_imports)]
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        ShowWindow, SW_SHOW,
    };
    unsafe {
        // Already foreground — skip the synthetic Alt tap which would
        // blur the WebView2 child's focused input element.
        let fg = GetForegroundWindow();
        if fg == hwnd {
            return;
        }
        // Classic foreground-lock unlock: a synthetic key event marks
        // input as recent, satisfying the OS check that otherwise makes
        // SetForegroundWindow silently no-op against stubborn windows.
        let tap = |down: bool| {
            let mut input: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT =
                std::mem::zeroed();
            input.r#type = windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD;
            input.Anonymous.ki.wVk = 0x12; // VK_MENU (Alt) — harmless tap
            input.Anonymous.ki.dwFlags = if down {
                0
            } else {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP
            };
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SendInput(
                1,
                &input,
                std::mem::size_of::<windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT>()
                    as i32,
            );
        };
        let _ = tap(true);

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
        ShowWindow(hwnd, SW_SHOW);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        // NOTE: deliberately NO SetFocus(hwnd) here — keyboard focus must
        // land on the WebView2 CHILD window, not the container. Forcing it
        // onto the container leaves the page unable to receive typing.
        if attached {
            // Only detach if attach succeeded (both must be attached).
            // We can't know reliably, so just try — it's harmless if
            // they were never attached.
            AttachThreadInput(cur_tid, fg_tid, 0);
        }
        let _ = tap(false); // release the Alt tap
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
            // Detect Win key via raw input.  RIDEV_NOHOTKEYS blocks
            // WH_KEYBOARD_LL for the keyboard, so we rely on raw input
            // to catch toggle presses while the overlay is visible.
            let h_raw_input = lparam as windows_sys::Win32::Foundation::HANDLE;
            let header_sz = std::mem::size_of::<RAWINPUTHEADER>() as u32;
            let mut raw_input: std::mem::MaybeUninit<RAWINPUT> =
                std::mem::MaybeUninit::uninit();
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
                    // keys globally, even over elevated windows), not in
                    // any hook — so it works identically whether the
                    // helper is connected or not.
                    if is_win && crate::overlay::hotkey::is_win_key_hotkey() {
                        if (flags & RI_KEY_BREAK) != 0 {
                            let was_down = RAW_WIN_DOWN.swap(0, Ordering::SeqCst);
                            let chord = RAW_WIN_CHORD.swap(false, Ordering::SeqCst);
                            // NOHOTKEYS is armed persistently for Win hotkeys,
                            // so Start never appears; bare Win key-up toggles
                            // regardless of overlay visibility.
                            if was_down != 0 && !chord {
                                crate::runtime::log_info(
                                    "[nex::debug] WM_INPUT bare Win key-up, toggling",
                                );
                                let _ = ctx.event_tx.send(OverlayEvent::Hotkey(1));
                            }
                        } else if RAW_WIN_DOWN
                            .compare_exchange(
                                0,
                                vk as u32,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            )
                            .is_ok()
                        {
                            RAW_WIN_CHORD.store(false, Ordering::SeqCst);
                        }
                        if (flags & RI_KEY_BREAK) != 0 {
                            RAW_WIN_DOWN.store(0, Ordering::SeqCst);
                        }
                    } else if (flags & RI_KEY_BREAK) == 0 {
                        // Any other key pressed while Win is held makes this a
                        // chord (Win+E, Win+D…) — never a toggle.
                        if RAW_WIN_DOWN.load(Ordering::SeqCst) != 0 {
                            RAW_WIN_CHORD.store(true, Ordering::SeqCst);
                        }
                        // Non-Win hotkey detection via raw input.
                        // WH_KEYBOARD_LL may not fire when the overlay
                        // is foreground (Chromium installs its own LL
                        // hook which runs first), so we check the
                        // configured hotkey here.
                        if crate::overlay::hotkey::check_raw_input_hotkey(vk) {
                            crate::runtime::log_info(&format!(
                                "[nex::debug] WM_INPUT hotkey vk={} sending toggle",
                                vk,
                            ));
                            let _ = ctx.event_tx.send(OverlayEvent::Hotkey(1));
                        }
                    }
                }
            }
        }
    }
    // SAFETY: hwnd is valid window handle from subclass registration
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

unsafe fn install_instance_signal_subclass(
    hwnd: HWND,
    event_tx: &Sender<OverlayEvent>,
) {
    let show_name: Vec<u16> = "Nex.ExternalShow.v1".encode_utf16().chain(std::iter::once(0)).collect();
    let quit_name: Vec<u16> = "Nex.ExternalQuit.v1".encode_utf16().chain(std::iter::once(0)).collect();
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
unsafe fn handle_recording_key(
    vk: u16,
    flags: u16,
    ctx: &InstanceSignalCtx,
) -> LRESULT {
    let is_break = (flags & RI_KEY_BREAK) != 0;
    //ESC cancels recording, any other key is a candidate.
    let is_win = vk == VK_LWIN || vk == VK_RWIN;
    if vk == 0x1B && !is_break {
        RECORDING_HOTKEY.store(false, Ordering::SeqCst);
        let _ = ctx.event_tx.send(OverlayEvent::HotkeyRecorded(String::new()));
        return 0;
    }
    let is_modifier =matches!(vk, 0xA0..=0xA5 | 0x10 | 0x11 |0x12 |0x5B | 0x5C);
    if is_win {
        //Commit bare "Win" on keyup only if no other jey is joined it.
        if is_break && !RECORD_WIN_CHORD.load(Ordering::SeqCst) {
            RECORDING_HOTKEY.store(false, Ordering::SeqCst);
            let _ = ctx.event_tx.send(OverlayEvent::HotkeyRecorded("Win".into()));
        }
        return 0;
    }
    if !is_break && !is_modifier {
        let mut parts: Vec<String> = Vec::new();
        unsafe  {
            if GetAsyncKeyState(0x11) as u16 & 0x8000 != 0 { parts.push("Ctrl".into()); }
            if GetAsyncKeyState(0x12) as u16 & 0x8000 != 0 { parts.push("Alt".into()); }
            if GetAsyncKeyState(0x10) as u16 & 0x8000 != 0 { parts.push("Shift".into()); }
            if GetAsyncKeyState(VK_LWIN as i32) as u16 & 0x8000 != 0 || GetAsyncKeyState(VK_RWIN as i32) as u16 & 0x8000 != 0 {
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

/// Tracks raw-input VK of held Win key so we only send one toggle per press.
static RAW_WIN_DOWN: AtomicU32 = AtomicU32::new(0);
/// Set when another key is pressed while Win is held (Win+E, Win+D…) so the
/// Win key-up is treated as a chord, not a bare toggle.
static RAW_WIN_CHORD: AtomicBool = AtomicBool::new(false);

/// Clear the raw-input Win tracking. Called after a live hotkey change so a
/// half-finished press of the old combo can't swallow the new one.
pub(crate) fn reset_raw_win_state() {
    RAW_WIN_DOWN.store(0, Ordering::SeqCst);
    RAW_WIN_CHORD.store(false, Ordering::SeqCst);
}
pub(crate) static RECORDING_HOTKEY: AtomicBool = AtomicBool::new(false);
static RECORD_WIN_CHORD: AtomicBool = AtomicBool::new(false); //win+other seen
const RID_INPUT: u32 = 0x10000003;
const RI_KEY_BREAK: u16 = 0x0001;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;

/// Register keyboard raw input sink so the overlay receives `WM_INPUT`
/// for all keyboard events while foreground.  When `suppress_win` is
/// true, also adds `RIDEV_NOHOTKEYS` to suppress Win-key Start at the
/// RIT level (required for Win-key hotkey).  Returns true on success.
fn register_raw_input_sink(hwnd: HWND, suppress_win: bool) -> bool {
    let flags = if suppress_win {
        RIDEV_NOHOTKEYS | RIDEV_INPUTSINK
    } else {
        RIDEV_INPUTSINK
    };
    let mut rid = RAWINPUTDEVICE {
        usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
        usUsage: 0x06,     // HID_USAGE_GENERIC_KEYBOARD
        dwFlags: flags,
        hwndTarget: hwnd,
    };
    let ok = unsafe {
        RegisterRawInputDevices(
            &mut rid,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        )
    } != 0;
    let err = if !ok { unsafe { windows_sys::Win32::Foundation::GetLastError() } } else { 0 };
    crate::runtime::log_info(&format!(
        "[nex::debug] register_raw_input_sink: ok={} last_err={} suppress_win={}",
        ok, err, suppress_win,
    ));
    ok
}

fn vk_to_hotkey(vk: u16) -> Option<String> {
    match vk {
        0x41..=0x5A => Some (((vk as u8) as char).to_string()), // A-Z
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
        RegisterRawInputDevices(
            &mut rid,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        )
    } != 0;
    let err = if !ok { unsafe { windows_sys::Win32::Foundation::GetLastError() } } else { 0 };
    crate::runtime::log_info(&format!(
        "[nex::debug] unregister_raw_input_sink: ok={} last_err={}",
        ok, err,
    ));
}
// ─────────────────────────────────────────────────────────────────
