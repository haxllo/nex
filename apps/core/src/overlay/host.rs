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

/// Tracks whether the overlay window is currently visible.
/// Used by the WM_INPUT handler to decide whether to toggle the overlay
/// on Win key-down (only toggle when visible; hook handles first press).
pub(crate) static OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);

use crossbeam_channel::Sender;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
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

/// Embedded web UI assets (premium Raycast-dark cmdk UI).
const INDEX_HTML: &str = include_str!("../../assets/index.html");
const STYLE_CSS: &str = include_str!("../../assets/style.css");
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
    /// Show + focus the overlay (builds the WebView if not yet created).
    Show,
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
    Resize(f64),
    /// Exit the event loop (clean shutdown).
    Quit,
    /// Debounce timer fired — apply the coalesced resize height.
    ApplyResize,
    /// Delayed keyboard state check (posted ~200ms after hide).
    CheckKeyboardState(Instant),
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
    apply_window_chrome(&window, &state);
    unsafe { install_instance_signal_subclass(hwnd, &event_tx); }

    // Register raw input sink permanently at startup so the overlay
    // receives WM_INPUT for keyboard events regardless of which window
    // is foreground.  This ensures hotkey detection works even when
    // Task Manager, advanced settings, or other native Windows windows
    // have focus (the WH_KEYBOARD_LL hook thread may not receive events
    // for elevated/UWP windows).
    register_raw_input_sink(hwnd, false);

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
    let mut ready = false;
    let mut warm_gen: u64 = 0;
    let mut was_focused = false;
    let mut last_show = Instant::now();
    let mut show_pending = false;

    // Resize debounce state. Growth requests go through a debounce
    // timer (UiCommand::Resize stores the target height and arms the
    // timer; UiCommand::ApplyResize fires after the quiet period and
    // calls set_inner_size). Shrink requests bypass debounce entirely
    // and apply immediately to prevent acrylic exposure.
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
                        let _ = resize_debounce_proxy.send_event(UiCommand::ApplyResize);
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
                            let _ = warm_release_proxy.send_event(UiCommand::Teardown(generation));
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok();
    let warm_release_arm = warm_release_tx.clone();

    let _ = event_loop.run_return(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(cmd) => match cmd {
                UiCommand::WebviewReady => {
                    crate::runtime::log_info(&format!("[nex] host UiCommand::WebviewReady received"));
                    ready = true;
                    if state.lock().map(|s| s.visible).unwrap_or(false) {
                        position_window(&window, hwnd);
                        window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, INITIAL_HEIGHT));
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
                            Ok(wv) => webview = Some(wv),
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
                        // WebView exists but page hasn't loaded yet
                        // (e.g. show raced with a prior cold start).
                        // Reset and rebuild the WebView.
                        crate::runtime::log_info("[nex] host WebView not ready, resetting");
                        webview = None;
                        ready = false;
                        show_pending = true;
                        match build_webview(&window, &state, &proxy, &event_tx) {
                            Ok(wv) => webview = Some(wv),
                            Err(e) => {
                                crate::runtime::log_warn(&format!("[nex] webview rebuild failed: {e}"));
                                return;
                            }
                        }
                        return;
                    }
                    // Set show_pending FIRST so spurious Focused(false) during
                    // position_window / set_inner_size / push_state cannot
                    // trigger Escape (which would desync OverlayState from
                    // the actual window state).
                    show_pending = true;
                    position_window(&window, hwnd);
                    // Start at search-bar height — JS sends resize when content appears.
                    window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, INITIAL_HEIGHT));
                    last_applied_height = INITIAL_HEIGHT;
                    pending_resize = None;
                    first_resize_after_show = true;
                    // Push state with show_pending so the JS side sends
                    // post("painted") to trigger the deferred show.
                    push_state(&webview, &state, &icon_cache, true);
                }
                UiCommand::Hide => {
                    // Re-inject the menu-mask key (0xE8) and spin-wait for the
                    // RIT to register it, so that when RIDEV_NOHOTKEYS is
                    // removed and the window is hidden, the newly-foreground
                    // window never sees a bare Win key — it sees Win+0xE8.
                    // This prevents Start from opening during the hide
                    // transition when the Win key is still physically held.
                    crate::overlay::hotkey::hold_mask_before_hide();
                    // Remove RIDEV_NOHOTKEYS but keep the sink registered so
                    // the overlay always receives WM_INPUT for all keyboard
                    // events regardless of foreground window.  This ensures
                    // the hotkey is detected even when Task Manager, advanced
                    // settings, or other elevated/UWP windows have focus.
                    register_raw_input_sink(hwnd, false);
                    RAW_WIN_DOWN.store(0, Ordering::SeqCst);
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
                        let _ = proxy_delay.send_event(UiCommand::CheckKeyboardState(Instant::now()));
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
                    register_raw_input_sink(hwnd, false);
                    RAW_WIN_DOWN.store(0, Ordering::SeqCst);
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

                UiCommand::Resize(h) => {
                    let h = h.clamp(INITIAL_HEIGHT, MAX_HEIGHT);
                    if h < last_applied_height {
                        // Shrink request: apply immediately to avoid
                        // exposing acrylic while the debounce delays
                        // the native window size reduction.
                        pending_resize = None;
                        if (h - last_applied_height).abs() > 0.5 {
                            last_applied_height = h;
                            window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, h));
                        }
                    } else if first_resize_after_show {
                        // First resize after show: apply immediately to
                        // prevent the "search bar first, items pop in
                        // 100ms later" visual flash.
                        first_resize_after_show = false;
                        pending_resize = None;
                        if (h - last_applied_height).abs() > 0.5 {
                            last_applied_height = h;
                            window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, h));
                        }
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
                            window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, h));
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
            },
            Event::WindowEvent {
                event: WindowEvent::Focused(focused),
                ..
            } => {
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
                        crate::runtime::log_info(&format!(
                            "[nex::debug] Focused(false): sending Escape (was_focused={} show_pending={} grace={}ms state_vis={})",
                            was_focused_val, show_pending_val, grace_ms, state_vis,
                        ));
                        let _ = event_tx.send(OverlayEvent::Escape);
                    } else {
                        crate::runtime::log_info(&format!(
                            "[nex::debug] Focused(false): BLOCKED Escape (was_focused={} show_pending={} bare_win={} grace={}ms state_vis={})",
                            was_focused_val, show_pending_val, bare_win, grace_ms, state_vis,
                        ));
                    }
                }
            }
            _ => {}
        }
    });

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
            let _ = proxy.send_event(UiCommand::WebviewReady);
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
        "powerPopup" => {
            let _ = event_tx.send(OverlayEvent::TogglePowerPopup);
        }
        "resize" => {
            if let Some(h) = value.get("v").and_then(|v| v.as_f64()) {
                let _ = proxy.send_event(UiCommand::Resize(h));
            }
        }
        "painted" => {
            // First paint after push_state — safe to show the window.
            // Deferred from WebviewReady / Show to avoid a flash of
            // uncomposited content before the WebView2 paints.
            let _ = proxy.send_event(UiCommand::Painted);
        }
        "openConfig" => {
            let path = state
                .lock()
                .map(|s| s.help_config_path.clone())
                .unwrap_or_default();
            if !path.is_empty() {
                open_path(&path);
            }
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
        _ => {}
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
                OverlayRowRole::TopHit | OverlayRowRole::Item => "item",
            };
            let selectable = matches!(
                r.role,
                OverlayRowRole::Item | OverlayRowRole::TopHit | OverlayRowRole::Calculator | OverlayRowRole::QuickLaunch
            );
            let icon = if r.icon_path.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(r.icon_path.clone())
            };
            // Include the actual file path for addToQuickLaunch
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
    let y = top + (work_h as f32 * 0.20) as i32;
    window.set_outer_position(PhysicalPosition::new(x.max(left), y.max(top)));
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
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        ShowWindow, SW_SHOW,
    };
    unsafe {
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
        SetFocus(hwnd);
        if attached {
            // Only detach if attach succeeded (both must be attached).
            // We can't know reliably, so just try — it's harmless if
            // they were never attached.
            AttachThreadInput(cur_tid, fg_tid, 0);
        }
    }
}

fn open_path(path: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
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
                    if is_win {
                        if crate::overlay::hotkey::is_win_key_hotkey() {
                            if (flags & RI_KEY_BREAK) != 0 {
                                RAW_WIN_DOWN.store(0, Ordering::SeqCst);
                            } else if RAW_WIN_DOWN
                                .compare_exchange(
                                    0,
                                    vk as u32,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                // Ignore Win key in WM_INPUT — hook/helper fires
                                // on key-up for both show and hide.
                                crate::runtime::log_info(&format!(
                                    "[nex::debug] WM_INPUT Win key={:?} ignoring (hook/helper fires on key-up)",
                                    vk,
                                ));
                            }
                        }
                        // Always track Win key-up for mask key cleanup
                        // even when the hotkey is not a Win key hotkey.
                        if (flags & RI_KEY_BREAK) != 0 {
                            RAW_WIN_DOWN.store(0, Ordering::SeqCst);
                        }
                    } else if (flags & RI_KEY_BREAK) == 0 {
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

/// Tracks raw-input VK of held Win key so we only send one toggle per press.
static RAW_WIN_DOWN: AtomicU32 = AtomicU32::new(0);

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
