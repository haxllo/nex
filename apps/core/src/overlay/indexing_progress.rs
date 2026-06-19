//! First-time indexing progress window (tao + wry).
//!
//! Replaces the legacy Win32 modal progress window and the Iced stub.
//! A tiny borderless tao window hosts a wry WebView that renders an
//! animated progress bar; the indexing closure writes to an
//! `Arc<AtomicU32>` and a polling thread pushes `requestAnimationFrame`
//! updates via `evaluate_script`. The window lives only while the
//! closure runs.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
};

const WINDOW_WIDTH: f64 = 360.0;
const WINDOW_HEIGHT: f64 = 120.0;

const PROGRESS_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en" data-theme="dark">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Nex Indexing</title>
<style>
:root{--bg:rgba(22,22,25,0.92);--text:#f4f4f6;--bar-bg:rgba(255,255,255,0.08);--bar-fg:#6ea8fe;--radius:10px}
*{box-sizing:border-box;margin:0;padding:0}
html,body{background:transparent;font-family:"Segoe UI Variable","Segoe UI",sans-serif;color:var(--text);-webkit-font-smoothing:antialiased}
body{display:flex;align-items:center;justify-content:center;height:100vh}
#panel{width:100%;height:100%;background:var(--bg);border-radius:var(--radius);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:12px;padding:20px}
#label{font-size:14px;color:var(--text);text-align:center}
#track{width:260px;height:6px;background:var(--bar-bg);border-radius:3px;overflow:hidden}
#bar{width:0%;height:100%;background:var(--bar-fg);border-radius:3px;transition:width 200ms ease}
#pct{font-size:12px;color:var(--bar-fg)}
</style></head>
<body>
<main id="panel">
  <div id="label">Indexing your files…</div>
  <div id="track"><div id="bar"></div></div>
  <div id="pct">0%</div>
</main>
<script>
window.updateProgress=function(v){var p=Math.max(0,Math.min(100,v));document.getElementById("bar").style.width=p+"%";document.getElementById("pct").textContent=p+"%"};
</script>
</body>
</html>"#;

enum Cmd {
    Update(u32),
    WorkDone,
    Close,
}

pub(crate) fn run_with_progress_window<F, T>(work: F) -> T
where
    F: FnOnce(Arc<AtomicU32>) -> T + Send + 'static,
    T: Send + 'static,
{
    let progress = Arc::new(AtomicU32::new(0));
    let result_slot: Arc<std::sync::Mutex<Option<T>>> = Arc::new(std::sync::Mutex::new(None));

    let progress_for_work = progress.clone();
    let result_slot_for_work = result_slot.clone();

    // Spawn the indexing work on its own thread.
    let (work_done_tx, work_done_rx) = std::sync::mpsc::channel::<()>();
    let work_thread = thread::Builder::new()
        .name("nex-indexer".into())
        .spawn(move || {
            let result = work(progress_for_work);
            *result_slot_for_work.lock().unwrap() = Some(result);
            let _ = work_done_tx.send(());
        })
        .expect("failed to spawn indexer thread");

    // Run the progress window on the current (main) thread.
    let mut event_loop = EventLoopBuilder::<Cmd>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Nex Indexing")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top(true)
        .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
        .with_skip_taskbar(true)
        .with_no_redirection_bitmap(true)
        .with_window_classname("NexProgressWindowClass")
        .build(&event_loop)
        .expect("failed to create progress window");

    // Position on the primary monitor, centered, upper third.
    let (x, y) = progress_window_position();
    window.set_outer_position(PhysicalPosition::new(x, y));

    // Apply DWM rounded corners.
    unsafe {
        let pref: i32 = 2; // DWMWCP_ROUND
        DwmSetWindowAttribute(
            window.hwnd() as windows_sys::Win32::Foundation::HWND,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }

    // Apply acrylic if available; falls back to CSS opaque panel.
    let _ = window_vibrancy::apply_acrylic(&window, Some((18, 18, 20, 130)));

    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_html(PROGRESS_PAGE)
        .build(&window)
        .expect("failed to build progress webview");

    window.set_visible(true);

    // Spawn a polling thread that reads the AtomicU32 and posts Cmd::Update.
    let progress_for_poll = progress.clone();
    let proxy_for_poll = proxy.clone();
    let _poll_thread = thread::Builder::new()
        .name("nex-progress-poll".into())
        .spawn(move || {
            let mut last = u32::MAX;
            loop {
                thread::sleep(Duration::from_millis(120));
                let current = progress_for_poll.load(Ordering::Relaxed);
                if current >= 100 {
                    let _ = proxy_for_poll.send_event(Cmd::Update(100));
                    break;
                }
                if current != last {
                    let _ = proxy_for_poll.send_event(Cmd::Update(current));
                    last = current;
                }
            }
        })
        .ok();

    // Watch for work-thread completion so the window always closes,
    // even if the indexer errors before writing progress=100.
    let proxy_for_done = proxy.clone();
    let _done_watcher = thread::Builder::new()
        .name("nex-progress-done-watcher".into())
        .spawn(move || {
            let _ = work_done_rx.recv();
            let _ = proxy_for_done.send_event(Cmd::WorkDone);
        })
        .expect("failed to spawn done-watcher thread");

    let mut closed = false;

    let _ = event_loop.run_return(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(Cmd::Update(v)) => {
                let _ = webview.evaluate_script(&format!(
                    "window.updateProgress&&window.updateProgress({v})"
                ));
                if v >= 100 {
                    // Give the user a moment to see 100%, then close.
                    let p = proxy.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(600));
                        let _ = p.send_event(Cmd::Close);
                    });
                }
            }
            Event::UserEvent(Cmd::WorkDone) => {
                // Work thread finished (success or error). If progress
                // never reached 100 the Update(100)->Close path never
                // fires, so close now to unblock the main thread.
                if !closed {
                    closed = true;
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(Cmd::Close) => {
                if !closed {
                    closed = true;
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });

    // Wait for the work thread to finish, then return the result.
    // Join propagates any panics. The done-watcher ensured the
    // event loop already exited, so this is non-blocking.
    let _ = work_thread.join();
    let result = result_slot
        .lock()
        .unwrap()
        .take()
        .expect("indexer thread finished without storing result");
    result
}

fn progress_window_position() -> (i32, i32) {
    use windows_sys::Win32::Foundation::{RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics;
    use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSCREEN;

    let primary_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    // Get primary monitor work area.
    let monitor = unsafe { MonitorFromPoint(std::mem::zeroed(), MONITOR_DEFAULTTOPRIMARY) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let work: RECT = info.rcWork;
        let x = work.left + ((work.right - work.left - WINDOW_WIDTH as i32) / 2);
        let y = work.top + ((work.bottom - work.top) as f32 * 0.25) as i32;
        (x.max(0), y.max(0))
    } else {
        (((primary_w - WINDOW_WIDTH as i32) / 2).max(0), 100)
    }
}
