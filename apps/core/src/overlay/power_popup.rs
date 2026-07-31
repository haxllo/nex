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
use tao::platform::windows::{EventLoopBuilderExtWindows, WindowBuilderExtWindows};
use tao::window::WindowBuilder;
use wry::http::Request;
use wry::WebViewBuilder;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
};

const POPUP_WIDTH: f64 = 152.0;
const POPUP_HEIGHT: f64 = 172.0;

// ── Module state ──────────────────────────────────────────────────────

/// Control channel to the persistent popup thread. `None` until
/// [`init_power_popup`] succeeds; cleared on thread exit or panic.
static CTL: Mutex<Option<tao::event_loop::EventLoopProxy<PopupCmd>>> = Mutex::new(None);

/// One-shot flag so [`init_power_popup`] is idempotent.
static INIT_GUARD: AtomicBool = AtomicBool::new(false);

enum PopupCmd {
    Show(isize), // anchor hwnd
    Hide,        // internal: IPC handler / focus-loss asks loop to hide
    Quit,
}

/// Feather-style stroke icons matching the overlay's `#power-btn` SVG
/// (`viewBox="0 0 24 24"`, `stroke="currentColor"`, `stroke-width="2"`).
/// The power icon paths are copied verbatim from `assets/index.html`.
const ICON_POWER: &str = "<svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M12 2v10\"/><path d=\"M18.4 6.6a9 9 0 1 1-12.8 0\"/></svg>";
const ICON_LOCK: &str = "<svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><rect x=\"3\" y=\"11\" width=\"18\" height=\"11\" rx=\"2\" ry=\"2\"/><path d=\"M7 11V7a5 5 0 0 1 10 0v4\"/></svg>";
const ICON_MOON: &str = "<svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z\"/></svg>";
const ICON_RESTART: &str = "<svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><polyline points=\"23 4 23 10 17 10\"/><path d=\"M20.49 15a9 9 0 1 1-2.12-9.36L23 10\"/></svg>";
const ICON_SIGNOUT: &str = "<svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4\"/><polyline points=\"16 17 21 12 16 7\"/><line x1=\"21\" y1=\"12\" x2=\"9\" y2=\"12\"/></svg>";

/// Popup page. Visual tokens mirror `assets/style.css` `:root` (dark) and
/// `html[data-theme="light"]` blocks; the layout mirrors the footer
/// `#power-menu` / `#power-confirm` structure. `{theme}` and `{icon-*}`
/// placeholders are substituted at runtime.
const POWER_POPUP_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en" data-theme="{theme}">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Nex Power</title>
<style>
:root{--radius:8px;--row-radius:6px;--font:"InterVariable","Inter",system-ui,-apple-system,sans-serif;--bg-opaque:rgba(20,20,22,1);--border:rgba(255,255,255,0.09);--text:#f4f4f6;--text-faint:#76767f;--sel:rgba(255,255,255,0.09);--accent:#6ea8fe;--divider:rgba(255,255,255,0.06)}
html[data-theme="light"]{--bg-opaque:rgba(255,255,255,1);--border:rgba(0,0,0,0.1);--text:#16161a;--text-faint:#8a8a93;--sel:rgba(0,0,0,0.06);--accent:#2f6bff;--divider:rgba(0,0,0,0.07)}
*{box-sizing:border-box;margin:0;padding:0;-webkit-user-select:none;user-select:none}
html,body{background:transparent;font-family:var(--font);color:var(--text);-webkit-font-smoothing:antialiased;overflow:hidden}
#menu,#confirm{width:100%;padding:4px;border-radius:var(--radius);background:var(--bg-opaque);border:1px solid var(--border);box-shadow:0 4px 16px rgba(0,0,0,0.4);overflow:hidden}
#menu button,#confirm button{display:block;width:100%;padding:6px 10px;border:none;border-radius:var(--row-radius);background:transparent;color:var(--text);font-family:inherit;font-size:12px;cursor:pointer;text-align:left}
#menu button{display:flex;align-items:center;gap:8px}
#menu button svg,#confirm button svg{flex:none}
#menu button:hover,#confirm button:hover{background:var(--sel)}
#menu hr,#confirm hr{height:1px;margin:4px 6px;border:none;background:var(--divider)}
#confirm{display:none}
#confirm .confirm-title{padding:6px 10px 2px;font-size:11px;color:var(--text-faint);white-space:nowrap}
#confirm button.confirm-yes{color:var(--accent)}
</style></head>
<body>
<div id="menu">
  <button type="button" data-power="lock">{icon-lock}Lock</button>
  <button type="button" data-power="sleep">{icon-moon}Sleep</button>
  <hr/>
  <button type="button" data-power="shutdown">{icon-power}Shutdown</button>
  <button type="button" data-power="restart">{icon-restart}Restart</button>
  <hr/>
  <button type="button" data-power="signout">{icon-signout}Sign Out</button>
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
    return;
  }
  if(b.id==="confirm-yes"&&pending){
    var a=pending;
    pending=null;
    post("action",a);
  }
});
document.addEventListener("keydown",function(e){if(e.key==="Escape")post("close")});
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

    // Loop state
    let mut visible = false;
    let mut show_time = Instant::now();
    let mut wants_focus = false;
    let mut focus_deadline = Instant::now();

    let _ = event_loop.run_return(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Poll;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                hide_popup(&window, &mut visible);
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                // Ignore spurious WM_KILLFOCUS within 150ms of show.
                if visible && show_time.elapsed() > Duration::from_millis(150) {
                    hide_popup(&window, &mut visible);
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
                    hide_popup(&window, &mut visible);
                } else {
                    // Show at anchor
                    let hwnd = anchor as HWND;
                    let scale = dpi_scale(hwnd);
                    let (x, y) = popup_position(hwnd, scale);
                    window.set_outer_position(PhysicalPosition::new(x, y));
                    window.set_visible(true);
                    window.set_focus();
                    wants_focus = true;
                    focus_deadline = Instant::now() + Duration::from_millis(300);
                    show_time = Instant::now();
                    visible = true;
                    crate::overlay::hotkey::set_suppress_focus_escape(true);
                }
            }
            Event::UserEvent(PopupCmd::Hide) => {
                hide_popup(&window, &mut visible);
            }
            Event::UserEvent(PopupCmd::Quit) => {
                hide_popup(&window, &mut visible);
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

    // `webview` is intentionally kept alive (outer binding) until the
    // loop exits so WebView2 teardown happens after the window closes.
    drop(webview);
    Ok(())
}

fn hide_popup(window: &tao::window::Window, visible: &mut bool) {
    if *visible {
        window.set_visible(false);
        *visible = false;
        crate::overlay::hotkey::set_suppress_focus_escape(false);
    }
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
        .replace("{icon-lock}", ICON_LOCK)
        .replace("{icon-moon}", ICON_MOON)
        .replace("{icon-power}", ICON_POWER)
        .replace("{icon-restart}", ICON_RESTART)
        .replace("{icon-signout}", ICON_SIGNOUT)
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
fn popup_position(anchor_hwnd: HWND, scale: f64) -> (i32, i32) {
    let width = (POPUP_WIDTH * scale) as i32;
    let height = (POPUP_HEIGHT * scale) as i32;

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
