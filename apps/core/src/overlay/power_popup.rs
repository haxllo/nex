//! Standalone power menu popup (tao + wry).
//!
//! A separate borderless, transparent, always-on-top tao window hosts a
//! wry WebView that renders the power menu (Lock / Sleep / Shutdown /
//! Restart / Sign Out) with an in-popup confirm step for the destructive
//! actions. The popup is built once at startup (hidden) and repositioned /
//! shown instantly on each click. It runs on its own named thread so
//! [`toggle`] never blocks the caller.
//!
//! All actions are forwarded over the shared `crossbeam_channel`
//! [`Sender<OverlayEvent>`] exactly like the tray / hotkey paths.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use crate::overlay::model::OverlayEvent;
use crate::runtime::log_warn;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{EventLoopBuilderExtWindows, WindowBuilderExtWindows, WindowExtWindows};
use tao::window::WindowBuilder;
use wry::http::Request;
use wry::WebViewBuilder;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
    DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetSystemMetrics, GetWindowRect, SM_CXSCREEN,
    SM_CYSCREEN,
};

const POPUP_WIDTH: f64 = 130.0; // matches footer #power-menu min-width
const POPUP_HEIGHT: f64 = 172.0;

// ── Module state ──────────────────────────────────────────────────────

/// Control channel to the persistent popup thread. `None` until
/// [`init_power_popup`] succeeds; cleared on thread exit or panic.
static CTL: Mutex<Option<tao::event_loop::EventLoopProxy<PopupCmd>>> = Mutex::new(None);

/// One-shot flag so [`init_power_popup`] is idempotent.
static INIT_GUARD: AtomicBool = AtomicBool::new(false);

enum PopupCmd {
    Show(isize), // anchor hwnd
    Resize(f64, f64), // (w, h) measured content size from JS, logical px
    Hide,        // internal: IPC handler / focus-loss asks loop to hide
    Quit,
}

/// Popup page. Visual tokens mirror `assets/style.css` `:root` (dark) and
/// `html[data-theme="light"]` blocks; the layout mirrors the footer
/// `#power-menu` / `#power-confirm` structure. `{theme}`
/// placeholder is substituted at runtime.
const POWER_POPUP_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en" data-theme="{theme}">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Nex Power</title>
<style>
:root{--radius:8px;--row-radius:6px;--font:"InterVariable","Inter",system-ui,-apple-system,sans-serif;--bg-opaque:rgba(20,20,22,1);--border:rgba(255,255,255,0.09);--text:#f4f4f6;--text-faint:#76767f;--sel:rgba(255,255,255,0.09);--accent:#6ea8fe;--divider:rgba(255,255,255,0.06)}
html[data-theme="light"]{--bg-opaque:rgba(255,255,255,1);--border:rgba(0,0,0,0.1);--text:#16161a;--text-faint:#8a8a93;--sel:rgba(0,0,0,0.06);--accent:#2f6bff;--divider:rgba(0,0,0,0.07)}
*{box-sizing:border-box;margin:0;padding:0;-webkit-user-select:none;user-select:none}
html,body{background:transparent;font-family:var(--font);font-weight:400;letter-spacing:0.2px;color:var(--text);-webkit-font-smoothing:antialiased;overflow:hidden}
#menu{width:100%;padding:4px;border-radius:var(--radius);background:var(--bg-opaque);border:1px solid var(--border);box-shadow:0 4px 16px rgba(0,0,0,0.4);overflow:hidden}
#confirm{width:100%;padding:4px;border-radius:var(--radius);background:var(--bg-opaque);border:1px solid var(--border);box-shadow:0 4px 16px rgba(0,0,0,0.4);overflow:hidden}
#menu button,#confirm button{display:block;width:100%;padding:6px 10px;border:none;border-radius:var(--row-radius);background:transparent;color:var(--text);font-family:inherit;font-size:12px;cursor:pointer;text-align:left}
#menu button:hover,#confirm button:hover{background:var(--sel)}
#menu hr,#confirm hr{height:1px;margin:4px 6px;border:none;background:var(--divider)}
#confirm{display:none;min-width:150px}
#confirm .confirm-title{padding:6px 10px 2px;font-size:11px;color:var(--text-faint);white-space:nowrap}
#confirm button.confirm-yes{color:var(--accent)}
</style></head>
<body>
<div id="menu">
  <button type="button" data-power="lock">Lock</button>
  <button type="button" data-power="sleep">Sleep</button>
  <hr/>
  <button type="button" data-power="shutdown">Shutdown</button>
  <button type="button" data-power="restart">Restart</button>
  <hr/>
  <button type="button" data-power="signout">Sign Out</button>
</div>
<div id="confirm">
  <div class="confirm-title" id="confirm-title">Shut down now?</div>
  <hr/>
  <button type="button" class="confirm-yes" id="confirm-yes">Shut down</button>
  <button type="button" id="confirm-cancel">Cancel</button>
</div>
<script>
function post(t,v){window.ipc.postMessage(JSON.stringify(v===undefined?{t}:{t,v}))}
var menu=document.getElementById("menu");
var confirmPanel=document.getElementById("confirm");
var confirmTitle=document.getElementById("confirm-title");
var confirmYes=document.getElementById("confirm-yes");
var pending=null;
menu.addEventListener("click",function(e){
  var b=e.target.closest("button");if(!b)return;
  var a=b.dataset.power;if(!a)return;
  if(a==="shutdown"||a==="restart"){
    var isShutdown=a==="shutdown";
    confirmTitle.textContent=isShutdown?"Shut down now?":"Restart now?";
    confirmYes.textContent=isShutdown?"Shut down":"Restart";
    pending=a;
    menu.style.display="none";
    confirmPanel.style.display="block";
    confirmYes.focus();
    reportSize();
    return;
  }
  post("action",a);
});
confirmPanel.addEventListener("click",function(e){
  var b=e.target.closest("button");if(!b)return;
  if(b.id==="confirm-cancel"){
    pending=null;
    confirmPanel.style.display="none";
    menu.style.display="block";
    reportSize();
    return;
  }
  if(b.id==="confirm-yes"&&pending){
    var a=pending;
    pending=null;
    post("action",a);
  }
});
document.addEventListener("keydown",function(e){if(e.key==="Escape")post("close")});
function reportSize(){
  var el=(confirmPanel.style.display==="block")?confirmPanel:menu;
  post("size",{w:Math.ceil(el.offsetWidth),h:Math.ceil(el.offsetHeight)});
}
window.resetView=function(){
  pending=null;
  menu.style.display="block";
  confirmPanel.style.display="none";
  reportSize();
};
reportSize();
setTimeout(reportSize,150);
</script>
</body>
</html>"#;

// ── Public API ───────────────────────────────────────────────────────

/// Build the persistent power popup once at startup. Non-blocking:
/// spawns a background thread that creates the tao EventLoop + WebView2
/// (hidden). The page paints while hidden so it is ready on first click.
/// Idempotent — safe to call multiple times.
pub(crate) fn init_power_popup(event_tx: Sender<OverlayEvent>) {
    if INIT_GUARD.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawn_result = thread::Builder::new()
        .name("nex-power-popup".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_popup(event_tx)
            }));
            // Thread going down — ensure suppress flag is clear.
            crate::overlay::hotkey::set_suppress_focus_escape(false);
            // Clear CTL so dead channel never lingers.
            if let Ok(mut guard) = CTL.lock() {
                *guard = None;
            }
            match result {
                Ok(Err(e)) => log_warn(&format!("[nex] power popup error: {e}")),
                Err(_) => log_warn("[nex] power popup thread panicked"),
                Ok(Ok(())) => {}
            }
        });
    if let Err(e) = spawn_result {
        log_warn(&format!("[nex] failed to spawn power popup thread: {e}"));
    }
}

/// Toggle the power popup at the given anchor window.
/// If visible → hide. If hidden → show at anchor.
pub(crate) fn toggle(anchor_hwnd: isize) {
    if let Ok(guard) = CTL.lock() {
        if let Some(ref proxy) = *guard {
            let _ = proxy.send_event(PopupCmd::Show(anchor_hwnd));
        } else {
            log_warn("[nex] power popup not ready, ignoring toggle");
        }
    }
}

/// Best-effort quit: hide popup and exit its event loop.
pub(crate) fn quit() {
    if let Ok(mut guard) = CTL.lock() {
        if let Some(proxy) = guard.take() {
            let _ = proxy.send_event(PopupCmd::Quit);
        }
    }
}

// ── Internal loop ────────────────────────────────────────────────────

fn run_popup(event_tx: Sender<OverlayEvent>) -> Result<(), String> {
    let mut builder = EventLoopBuilder::<PopupCmd>::with_user_event();
    builder.with_any_thread(true);
    let mut event_loop = builder.build();
    let proxy = event_loop.create_proxy();

    // Store control channel in module state.
    if let Ok(mut guard) = CTL.lock() {
        *guard = Some(proxy.clone());
    }

    let window = WindowBuilder::new()
        .with_title("Nex Power")
        .with_decorations(false)
        .with_transparent(true)
        .with_visible(false) // hidden until first Show command
        .with_no_redirection_bitmap(true)
        .with_resizable(false)
        .with_always_on_top(true)
        .with_skip_taskbar(true)
        .with_inner_size(LogicalSize::new(POPUP_WIDTH, POPUP_HEIGHT))
        .with_window_classname("NexPowerPopupClass")
        .build(&event_loop)
        .map_err(|e| format!("failed to create power popup window: {e}"))?;

    // Rounded corners — mirrors indexing_progress::apply_chrome.
    // No acrylic needed: the popup page is fully opaque.
    let hwnd = window.hwnd() as HWND;
    unsafe {
        let pref: i32 = 2; // DWMWCP_ROUND
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }

    // Remove any DWM frame edge around the frameless popup.
    let mut margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        DwmExtendFrameIntoClientArea(hwnd, &mut margins);
    }

    let html = build_html();
    let ipc_tx = event_tx.clone();
    let ipc_proxy = proxy.clone();
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_html(html)
        .with_ipc_handler(move |req: Request<String>| {
            handle_ipc(req.body(), &ipc_tx, &ipc_proxy);
        })
        .build(&window)
        .map_err(|e| format!("failed to build power popup webview: {e}"))?;

    // Loop state — page paints while hidden so size is set before first Show.
    let mut visible = false;
    let mut show_time = Instant::now();
    let mut wants_focus = false;
    let mut focus_deadline = Instant::now();
    let mut size: (f64, f64) = (POPUP_WIDTH, POPUP_HEIGHT);
    let mut last_anchor: isize = 0;

    let _ = event_loop.run_return(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Poll;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                hide_popup(&window, &webview, &mut visible, &event_tx, last_anchor, true);
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                // Ignore spurious WM_KILLFOCUS within 150ms of show.
                if visible && show_time.elapsed() > Duration::from_millis(150) {
                    hide_popup(&window, &webview, &mut visible, &event_tx, last_anchor, true);
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => {
                wants_focus = false;
            }
            Event::UserEvent(PopupCmd::Show(anchor)) => {
                if visible {
                    // Toggle: hide
                    hide_popup(&window, &webview, &mut visible, &event_tx, last_anchor, true);
                } else {
                    // Show at anchor — set suppress BEFORE any focus work
                    // so the synchronous WM_KILLFOCUS from SetFocus sees it.
                    crate::overlay::hotkey::set_suppress_focus_escape(true);
                    window.set_inner_size(LogicalSize::new(size.0, size.1));
                    log_warn(&format!("[nex::popup] Show size set inner={:?} outer={:?}", window.inner_size(), window.outer_size()));
                    let hwnd = anchor as HWND;
                    let scale = dpi_scale(hwnd);
                    let (x, y) = popup_position(hwnd, scale, size.0, size.1);
                    window.set_outer_position(PhysicalPosition::new(x, y));
                    window.set_visible(true);
                    window.set_focus();
                    wants_focus = true;
                    focus_deadline = Instant::now() + Duration::from_millis(300);
                    show_time = Instant::now();
                    visible = true;
                    last_anchor = anchor;
                }
            }
            Event::UserEvent(PopupCmd::Resize(w, h)) => {
                size = (w.clamp(100.0, 300.0), h.clamp(80.0, 400.0));
                log_warn(&format!("[nex::popup] Resize -> {:.0}x{:.0} (visible={})", w, h, visible));
                window.set_inner_size(LogicalSize::new(size.0, size.1));
                log_warn(&format!("[nex::popup] Resize done, outer={:?} inner={:?}", window.outer_size(), window.inner_size()));
                // Keep the right edge anchored: recompute x from the anchor.
                if visible {
                    let hwnd = last_anchor as HWND;
                    let scale = dpi_scale(hwnd);
                    let (x, y) = popup_position(hwnd, scale, size.0, size.1);
                    window.set_outer_position(PhysicalPosition::new(x, y));
                }
            }
            Event::UserEvent(PopupCmd::Hide) => {
                hide_popup(&window, &webview, &mut visible, &event_tx, last_anchor, true);
            }
            Event::UserEvent(PopupCmd::Quit) => {
                hide_popup(&window, &webview, &mut visible, &event_tx, last_anchor, false);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
        // Focus retry: re-attempt set_focus each iteration until deadline.
        if wants_focus && Instant::now() < focus_deadline {
            let _ = window.set_focus();
        } else {
            wants_focus = false;
        }
    });

    // `webview` is moved into the run_return closure and dropped when the
    // loop exits — WebView2 teardown happens after the window closes.
    Ok(())
}

fn hide_popup(
    window: &tao::window::Window,
    webview: &wry::WebView,
    visible: &mut bool,
    event_tx: &Sender<OverlayEvent>,
    anchor: isize,
    refocus: bool,
) {
    if *visible {
        let _ = webview.evaluate_script("window.resetView&&window.resetView()");
        window.set_visible(false);
        *visible = false;
        crate::overlay::hotkey::set_suppress_focus_escape(false);
        if refocus {
            // The popup was dismissed by a click elsewhere: after the
            // foreground settles, refocus the overlay search input —
            // unless the click landed on a real app window.
            let tx = event_tx.clone();
            let anchor = anchor;
            let popup_hwnd = window.hwnd() as isize;
            std::thread::Builder::new()
                .name("nex-popup-refocus".into())
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(50)); // let foreground settle
                    if should_refocus_search(anchor, popup_hwnd) {
                        let _ = tx.send(OverlayEvent::FocusSearchInput);
                    }
                })
                .ok();
        }
    }
}

/// Decide whether the overlay search input should be refocused after the
/// power popup hides: true when the foreground window is null, the popup
/// itself, the anchor (overlay), or the desktop / taskbar — false when a
/// real app window now owns focus (don't steal it back).
fn should_refocus_search(anchor: isize, popup_hwnd: isize) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() {
        return true;
    }
    if fg as isize == anchor || fg as isize == popup_hwnd {
        return true;
    }
    let mut class_buf = [0u16; 64];
    let len = unsafe { GetClassNameW(fg, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    if len == 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&class_buf[..len as usize]);
    matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd")
}

/// Parse one IPC message `{"t": ..., "v": ...}` and act on it.
fn handle_ipc(body: &str, event_tx: &Sender<OverlayEvent>, proxy: &tao::event_loop::EventLoopProxy<PopupCmd>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    let t = value.get("t").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "action" => {
            let action = value.get("v").and_then(|v| v.as_str()).unwrap_or("");
            let event = match action {
                "lock" => Some(OverlayEvent::TrayLock),
                "sleep" => Some(OverlayEvent::TraySleep),
                "shutdown" => Some(OverlayEvent::PowerMenuShutdown),
                "restart" => Some(OverlayEvent::PowerMenuRestart),
                "signout" => Some(OverlayEvent::TraySignOut),
                _ => None,
            };
            if let Some(e) = event {
                let _ = event_tx.send(e);
            }
            let _ = proxy.send_event(PopupCmd::Hide);
        }
        "close" => {
            let _ = proxy.send_event(PopupCmd::Hide);
        }
        "size" => {
            let w = value.get("w").and_then(|v| v.as_f64()).unwrap_or(POPUP_WIDTH);
            let h = value.get("h").and_then(|v| v.as_f64()).unwrap_or(POPUP_HEIGHT);
            log_warn(&format!("[nex::popup] IPC size w={} h={}", w, h));
            let _ = proxy.send_event(PopupCmd::Resize(w, h));
        }
        _ => {}
    }
}

fn build_html() -> String {
    let theme = match crate::overlay::platform::detect_system_theme() {
        crate::overlay::model::Theme::Light => "light",
        crate::overlay::model::Theme::Dark => "dark",
    };
    POWER_POPUP_PAGE
        .replace("{theme}", theme)
}

/// Physical-pixel DPI scale of the anchor window (1.0 = 96 DPI).
fn dpi_scale(anchor_hwnd: HWND) -> f64 {
    if anchor_hwnd.is_null() {
        return 1.0;
    }
    let dpi = unsafe { GetDpiForWindow(anchor_hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f64 / 96.0
    }
}

/// Position the popup next to the anchor window (physical px):
/// x = anchor right - popup width - 8px, y = anchor top + search row
/// (60px) + 6px gap. Clamped to the work area of the monitor containing
/// the anchor (primary monitor as fallback).
fn popup_position(anchor_hwnd: HWND, scale: f64, width: f64, height: f64) -> (i32, i32) {
    let width = (width * scale) as i32;
    let height = (height * scale) as i32;

    let mut rect: RECT = unsafe { std::mem::zeroed() };
    let anchored = !anchor_hwnd.is_null() && unsafe { GetWindowRect(anchor_hwnd, &mut rect) } != 0;
    if !anchored {
        let x = (unsafe { GetSystemMetrics(SM_CXSCREEN) } - width).max(0);
        let y = (unsafe { GetSystemMetrics(SM_CYSCREEN) } - height).max(0);
        return (x, y);
    }

    let (x, y) = (
        rect.right - width - (8.0 * scale) as i32,
        rect.top + (66.0 * scale) as i32,
    );

    let center = POINT {
        x: (rect.left + rect.right) / 2,
        y: (rect.top + rect.bottom) / 2,
    };
    let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTOPRIMARY) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let work = if !monitor.is_null() && unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        info.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        }
    };

    let x = x.clamp(work.left, (work.right - width).max(work.left));
    let y = y.clamp(work.top, (work.bottom - height).max(work.top));
    (x, y)
}
