//! WebView2 overlay host (tao window + wry WebView).
//!
//! Replaces the Iced boot/view. A single borderless, transparent,
//! always-on-top tao window hosts a wry WebView that renders the
//! premium cmdk-style UI from embedded HTML/CSS/JS assets. The Rust
//! side pushes state to JS via `evaluate_script("window.nex.apply(..)")`
//! and receives input via the wry IPC handler, translating it into the
//! existing [`OverlayEvent`] channel the runtime worker already drains.
//!
//! The window is positioned on the monitor under the cursor, grabs
//! foreground focus on show (the `AttachThreadInput` trick — winit/tao
//! cannot steal focus reliably on its own), and resizes to hug the
//! web content so the DWM acrylic backdrop wraps the panel exactly.
//!
//! Memory: the WebView is created lazily on first show and dropped a
//! few seconds after the overlay is hidden (warm-then-release), so the
//! heavy Chromium processes are not resident while idle.
//!
//! [`run`] MUST be called on the main thread (tao, like winit, panics
//! if the event loop is created off the main thread).

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use wry::http::{header::CONTENT_TYPE, Request, Response};
use wry::{WebView, WebViewBuilder};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;

use crate::overlay::icons::IconCache;
use crate::overlay::model::{OverlayEvent, OverlayRowRole, ShimState};
use crate::overlay::model::Theme;

const WINDOW_WIDTH: f64 = 720.0;
const INITIAL_HEIGHT: f64 = 60.0;
const MIN_HEIGHT: f64 = 56.0;
const MAX_HEIGHT: f64 = 560.0;
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
    /// Show + focus the overlay (builds the WebView if released).
    Show,
    /// Hide the overlay and arm the warm-release timer.
    Hide,
    /// Fired by the warm-release timer; drops the WebView if still
    /// hidden and the generation still matches.
    Teardown(u64),
    /// The page painted after a push_state — trigger deferred show.
    Painted,
    /// The page measured its content height (CSS px); resize to hug it.
    Resize(f64),
    /// Exit the event loop (clean shutdown).
    Quit,
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
    apply_window_chrome(&window, hwnd, &state);
    unsafe { install_instance_signal_subclass(hwnd, &event_tx); }

    // Build the WebView eagerly at startup so the page is fully
    // rendered in the background before the first show.  Subsequent
    // re-shows after warm-release rebuild lazily (same Show path).
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

    // Single warm-release timer thread. Hide arms it with (gen, delay);
    // it sends Teardown(gen) when the deadline passes. Re-arming replaces
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
                    Ok(Some((gen, delay))) => {
                        armed = Some((Instant::now() + delay, gen));
                    }
                    Ok(None) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        if let Some((_, gen)) = armed.take() {
                            let _ = warm_release_proxy.send_event(UiCommand::Teardown(gen));
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
                    ready = true;
                    if state.lock().map(|s| s.visible).unwrap_or(false) {
                        position_window(&window, hwnd);
                        push_state(&webview, &state, &icon_cache);
                        focus_input(&webview);
                        show_pending = true;
                    }
                }
                UiCommand::Apply => {
                    if ready && state.lock().map(|s| s.visible).unwrap_or(false) {
                        push_state(&webview, &state, &icon_cache);
                    }
                }
                UiCommand::Show => {
                    if webview.is_none() {
                        ready = false;
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
                        return;
                    }
                    position_window(&window, hwnd);
                    // Page already has the idle state from the Hide
                    // flush — skip push_state to avoid a DOM-rebuild
                    // flash.
                    focus_input(&webview);
                    show_pending = true;
                }
                UiCommand::Hide => {
                    // Push current state (which the shim has already
                    // cleared to idle) before hiding, so the page is
                    // ready-to-show on the next open.
                    if ready {
                        push_state(&webview, &state, &icon_cache);
                    }
                    window.set_visible(false);
                    if let Ok(mut s) = state.lock() {
                        s.has_focus = false;
                    }
                    was_focused = false;
                    show_pending = false;
                    warm_gen = warm_gen.wrapping_add(1);
                    let gen = warm_gen;
                    let delay = state
                        .lock()
                        .map(|s| s.ui_warm_release_ms)
                        .unwrap_or(5_000)
                        .max(500) as u64;
                    // Re-arm the single warm-release timer thread.
                    let _ = warm_release_arm.send(Some((gen, Duration::from_millis(delay))));
                }
                UiCommand::Teardown(gen) => {
                    let still_hidden = !state.lock().map(|s| s.visible).unwrap_or(false);
                    if still_hidden && gen == warm_gen {
                        webview = None;
                        ready = false;
                        icon_cache.clear();
                        crate::logging::info("[nex] ui warm-release: webview torn down");
                    }
                }
                UiCommand::Resize(h) => {
                    let height = h.clamp(MIN_HEIGHT, MAX_HEIGHT);
                    window.set_inner_size(LogicalSize::new(WINDOW_WIDTH, height));
                }
                UiCommand::Painted => {
                    if show_pending {
                        show_pending = false;
                        last_show = Instant::now();
                        window.set_visible(true);
                        force_foreground(hwnd);
                    }
                }
                UiCommand::Quit => {
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::WindowEvent {
                event: WindowEvent::Focused(focused),
                ..
            } => {
                if let Ok(mut s) = state.lock() {
                    s.has_focus = focused;
                }
                if focused {
                    was_focused = true;
                }
                // Click-outside-to-dismiss: only after the window has
                // been focused and the show-grace period has elapsed
                // (the initial Resize / WM_ACTIVATE dance can cause
                // transient unfocused events that we must ignore).
                if !focused
                    && was_focused
                    && last_show.elapsed().as_millis() as u64 >= FOCUS_GRACE_MS
                    && state.lock().map(|s| s.visible).unwrap_or(false)
                {
                    let _ = event_tx.send(OverlayEvent::Escape);
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
        _ => {}
    }
}

/// Push the current state snapshot to the page.
fn push_state(webview: &Option<WebView>, state: &Arc<Mutex<ShimState>>, icons: &Arc<IconCache>) {
    let Some(wv) = webview else { return };
    let Ok(s) = state.lock() else { return };
    let json = snapshot_json(&s, icons);
    drop(s);
    let _ = wv.evaluate_script(&format!("window.nex&&window.nex.apply({json})"));
}

fn focus_input(webview: &Option<WebView>) {
    if let Some(wv) = webview {
        let _ = wv.evaluate_script("window.nex&&window.nex.focus()");
    }
}

/// Serialize the overlay state into the JSON the page consumes.
fn snapshot_json(s: &ShimState, icons: &Arc<IconCache>) -> String {
    let rows: Vec<serde_json::Value> = s
        .rows
        .iter()
        .map(|r| {
            let role = match r.role {
                OverlayRowRole::Header => "header",
                OverlayRowRole::Status => "status",
                OverlayRowRole::Calculator => "calculator",
                OverlayRowRole::TopHit | OverlayRowRole::Item => "item",
            };
            let selectable = matches!(
                r.role,
                OverlayRowRole::Item | OverlayRowRole::TopHit | OverlayRowRole::Calculator
            );
            let icon = if r.icon_path.is_empty() {
                serde_json::Value::Null
            } else {
                // Embed the icon as a data: URI so the WebView doesn't
                // need to fetch it — custom protocols don't work for
                // subresource requests in WebView2.
                let b64 = icons
                    .png_bytes(&r.icon_path)
                    .map(|arc| base64_png(arc.as_ref()))
                    .unwrap_or_default();
                if b64.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(b64)
                }
            };
            serde_json::json!({
                "role": role,
                "title": r.title,
                "subtitle": r.path,
                "kind": r.kind,
                "icon": icon,
                "selectable": selectable,
                "resultIndex": r.result_index,
            })
        })
        .collect();

    let theme = match s.theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
    };

    serde_json::json!({
        "query": s.query,
        "rows": rows,
        "selected": s.selected,
        "status": s.status_text,
        "placeholder": s.placeholder_hint,
        "hotkeyHint": s.hotkey_hint,
        "hotkeyIssue": s.hotkey_issue_active,
        "theme": theme,
    })
    .to_string()
}

// ─────────────────────────────────────────────────────────────────
// Win32 glue: window chrome, positioning, focus
// ─────────────────────────────────────────────────────────────────

/// Apply DWM rounded corners + acrylic backdrop + native shadow.
fn apply_window_chrome(window: &Window, hwnd: HWND, state: &Arc<Mutex<ShimState>>) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        DWMWA_WINDOW_CORNER_PREFERENCE,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    // DWMWCP_ROUND = 2
    let pref: i32 = 2;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
    // Enable native DWM drop shadow (negative margins = extend frame
    // into entire client area, which adds the shadow even on layered
    // windows).
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
    let dark = state.lock().map(|s| s.theme == Theme::Dark).unwrap_or(true);
    // Acrylic blur behind the (transparent) WebView. Falls back to a
    // CSS-painted panel if the OS refuses (window-vibrancy returns Err).
    let tint = if dark {
        Some((18, 18, 20, 130))
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
    let y = top + (work_h as f32 * 0.18) as i32;
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
            AttachThreadInput(cur_tid, fg_tid, 1);
        }
        ShowWindow(hwnd, SW_SHOW);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        if attached {
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
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }
    let ctx = &*(dwrefdata as *const InstanceSignalCtx);
    if msg != 0 {
        if msg == ctx.msg_show {
            let _ = ctx.event_tx.send(OverlayEvent::ExternalShow);
            return 0;
        }
        if msg == ctx.msg_quit {
            let _ = ctx.event_tx.send(OverlayEvent::ExternalQuit);
            return 0;
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe fn install_instance_signal_subclass(
    hwnd: HWND,
    event_tx: &Sender<OverlayEvent>,
) {
    let show_name: Vec<u16> = "Nex.ExternalShow.v1".encode_utf16().chain(std::iter::once(0)).collect();
    let quit_name: Vec<u16> = "Nex.ExternalQuit.v1".encode_utf16().chain(std::iter::once(0)).collect();
    let msg_show = RegisterWindowMessageW(show_name.as_ptr());
    let msg_quit = RegisterWindowMessageW(quit_name.as_ptr());
    if msg_show == 0 || msg_quit == 0 {
        return;
    }
    let ctx = Box::new(InstanceSignalCtx {
        msg_show,
        msg_quit,
        event_tx: event_tx.clone(),
    });
    let ptr = Box::into_raw(ctx) as usize;
    SetWindowSubclass(hwnd, Some(instance_signal_subclass), 1, ptr);
}

// ─────────────────────────────────────────────────────────────────
// base64-encode PNG bytes into a data: URI (embed in JSON payload)
// ─────────────────────────────────────────────────────────────────

fn base64_png(bytes: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("data:image/png;base64,");
    out.reserve(bytes.len() * 4 / 3 + 4);
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
