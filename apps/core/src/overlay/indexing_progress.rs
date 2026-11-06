//! Indexing progress window shown during large full discovery passes.
//!
//! Lives in the overlay's tao event loop as a second, on-demand window. It
//! never takes focus (`WS_EX_NOACTIVATE`, shown with `SW_SHOWNOACTIVATE`) and
//! sits in the bottom-right corner of the primary work area, away from the
//! overlay, so search stays usable while it is open.

#![cfg(target_os = "windows")]

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event_loop::EventLoopWindowTarget;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use wry::{WebView, WebViewBuilder};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_HIDE, SW_SHOWNOACTIVATE,
    WS_EX_NOACTIVATE,
};

use crate::indexing_progress::FullPassSnapshot;

const WINDOW_WIDTH: f64 = 340.0;
const WINDOW_HEIGHT: f64 = 112.0;
const EDGE_MARGIN: i32 = 16;

const PROGRESS_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Nex Indexing</title>
<style>
:root{--bg:rgba(22,22,25,0.62);--text:#f4f4f6;--muted:rgba(244,244,246,0.62);--bar-bg:rgba(255,255,255,0.08);--bar-fg:#6ea8fe;--border:rgba(255,255,255,0.09)}
*{box-sizing:border-box;margin:0;padding:0}
html,body{background:transparent;font-family:"Segoe UI Variable Display","Segoe UI Variable","Segoe UI",system-ui,sans-serif;color:var(--text);-webkit-font-smoothing:antialiased;user-select:none;cursor:default}
body{height:100vh}
#panel{width:100%;height:100%;background:var(--bg);border:1px solid var(--border);display:flex;flex-direction:column;justify-content:center;gap:10px;padding:16px 20px}
#label{font-size:14px}
#track{height:6px;background:var(--bar-bg);border-radius:3px;overflow:hidden;position:relative}
#bar{position:absolute;top:0;bottom:0;width:35%;background:var(--bar-fg);border-radius:3px;animation:slide 1.4s ease-in-out infinite}
@keyframes slide{0%{left:-35%}100%{left:100%}}
.done #bar{animation:none;left:0;width:100%}
#detail{font-size:12px;color:var(--muted);font-variant-numeric:tabular-nums}
</style></head>
<body>
<main id="panel">
  <div id="label">Indexing your files…</div>
  <div id="track"><div id="bar"></div></div>
  <div id="detail">Starting…</div>
</main>
<script>
function fmtCount(n){return Number(n).toLocaleString()}
function fmtTime(ms){var s=Math.floor(ms/1000);var m=Math.floor(s/60);s=s%60;return m+":"+(s<10?"0":"")+s}
window.updateProgress=function(p){
  document.body.classList.remove("done");
  document.getElementById("label").textContent="Indexing your files…";
  document.getElementById("detail").textContent=fmtCount(p.indexed)+" of "+fmtCount(p.discovered)+" found indexed · "+fmtTime(p.elapsed_ms);
};
window.finishProgress=function(p){
  document.body.classList.add("done");
  document.getElementById("label").textContent="Indexing complete";
  document.getElementById("detail").textContent=fmtCount(p.discovered)+" items · "+fmtTime(p.elapsed_ms);
};
</script>
</body>
</html>"#;

pub(crate) struct ProgressWindow {
    window: Window,
    webview: WebView,
}

impl ProgressWindow {
    pub(crate) fn build<T: 'static>(target: &EventLoopWindowTarget<T>) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title("Nex Indexing")
            .with_decorations(false)
            .with_transparent(true)
            .with_visible(false)
            .with_focused(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_no_redirection_bitmap(true)
            .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
            .with_window_classname("NexProgressWindowClass")
            .build(target)
            .map_err(|e| format!("progress window: {e}"))?;
        let hwnd = window.hwnd() as HWND;
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE as isize);
            let round: i32 = 2; // DWMWCP_ROUND
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &round as *const i32 as *const std::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
        let _ = window_vibrancy::apply_acrylic(&window, Some((18, 18, 20, 130)));

        let webview = WebViewBuilder::new()
            .with_transparent(true)
            .with_html(PROGRESS_PAGE)
            .build(&window)
            .map_err(|e| format!("progress webview: {e}"))?;
        Ok(Self { window, webview })
    }

    pub(crate) fn show(&self, progress: FullPassSnapshot) {
        self.update(progress);
        let size = self.window.outer_size();
        if let Some((x, y)) = bottom_right_position(size.width as i32, size.height as i32) {
            self.window.set_outer_position(PhysicalPosition::new(x, y));
        }
        unsafe {
            ShowWindow(self.window.hwnd() as HWND, SW_SHOWNOACTIVATE);
        }
    }

    pub(crate) fn update(&self, progress: FullPassSnapshot) {
        let _ = self.webview.evaluate_script(&format!(
            "window.updateProgress && window.updateProgress({})",
            progress.to_json()
        ));
    }

    pub(crate) fn finish(&self, progress: FullPassSnapshot) {
        let _ = self.webview.evaluate_script(&format!(
            "window.finishProgress && window.finishProgress({})",
            progress.to_json()
        ));
    }

    pub(crate) fn hide(&self) {
        unsafe {
            ShowWindow(self.window.hwnd() as HWND, SW_HIDE);
        }
    }
}

/// Bottom-right of the primary monitor's work area (above the taskbar), in
/// physical pixels to match `outer_size`.
fn bottom_right_position(width: i32, height: i32) -> Option<(i32, i32)> {
    let monitor = unsafe { MonitorFromPoint(std::mem::zeroed(), MONITOR_DEFAULTTOPRIMARY) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let work: RECT = info.rcWork;
    Some((
        (work.right - width - EDGE_MARGIN).max(work.left),
        (work.bottom - height - EDGE_MARGIN).max(work.top),
    ))
}
