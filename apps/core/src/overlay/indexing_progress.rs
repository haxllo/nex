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
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_HIDE, SW_SHOWNOACTIVATE,
    WS_EX_NOACTIVATE,
};

use crate::indexing_progress::FullPassSnapshot;

const WINDOW_WIDTH: f64 = 340.0;
const WINDOW_HEIGHT: f64 = 104.0;
const EDGE_MARGIN: i32 = 16;

const PROGRESS_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en" data-theme="dark">
<head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Nex Indexing</title>
<style>
:root{--bg:rgba(20,20,22,0.78);--text:#f4f4f5;--muted:#bcbcc4;--track:rgba(255,255,255,0.16);--fill:#f4f4f5}
:root[data-theme="light"]{--bg:rgba(250,250,251,0.82);--text:#151518;--muted:#55555c;--track:rgba(0,0,0,0.14);--fill:#151518}
*{box-sizing:border-box;margin:0;padding:0}
html,body{background:transparent;font-family:"Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;color:var(--text);-webkit-font-smoothing:antialiased;user-select:none;cursor:default}
body{height:100vh}
#panel{width:100%;height:100%;background:var(--bg);display:flex;flex-direction:column;justify-content:center;gap:12px;padding:18px 20px}
#label{font-size:13px;font-weight:600;letter-spacing:-.01em}
#track{height:3px;background:var(--track);border-radius:2px;overflow:hidden;position:relative}
#bar{position:absolute;top:0;bottom:0;width:38%;background:var(--fill);border-radius:2px;animation:slide 1.4s cubic-bezier(.4,0,.2,1) infinite}
@keyframes slide{0%{left:-38%}100%{left:100%}}
.done #bar{animation:none;left:0;width:100%}
#detail{font-size:12px;color:var(--muted);font-variant-numeric:tabular-nums}
</style></head>
<body>
<main id="panel">
  <div id="label">Indexing your files</div>
  <div id="track"><div id="bar"></div></div>
  <div id="detail">Starting…</div>
</main>
<script>
function fmtCount(n){return Number(n).toLocaleString()}
function fmtTime(ms){var s=Math.floor(ms/1000);var m=Math.floor(s/60);s=s%60;return m+":"+(s<10?"0":"")+s}
window.updateProgress=function(p){
  document.body.classList.remove("done");
  document.getElementById("label").textContent="Indexing your files";
  document.getElementById("detail").textContent=fmtCount(p.discovered)+" files · "+fmtTime(p.elapsed_ms);
};
window.finishProgress=function(p){
  document.body.classList.add("done");
  document.getElementById("label").textContent="Indexing complete";
  document.getElementById("detail").textContent=fmtCount(p.discovered)+" files · "+fmtTime(p.elapsed_ms);
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
            // tao reserves the DWM shadow inset (SM_CXSIZEFRAME + SM_CXPADDEDBORDER,
            // 8 px left/right/bottom and 1 px top on Windows 11) outside the client
            // area of an undecorated window. There is no shadow here - the window is
            // acrylic over its whole surface - so the inset only painted an 8 px
            // translucent frame around the panel.
            .with_undecorated_shadow(false)
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
        let _ = window_vibrancy::apply_acrylic(&window, Some((20, 20, 22, 140)));

        let webview = WebViewBuilder::new()
            .with_transparent(true)
            // Before the page's own scripts: the first `show` runs its
            // `evaluate_script` while the document is still loading, so a theme
            // set there can be discarded with the navigation.
            .with_initialization_script(format!(
                "document.documentElement.dataset.theme='{}'",
                if crate::overlay::platform::detect_look().1 == crate::overlay::model::Theme::Light {
                    "light"
                } else {
                    "dark"
                }
            ))
            .with_html(PROGRESS_PAGE)
            .build(&window)
            .map_err(|e| format!("progress webview: {e}"))?;
        Ok(Self { window, webview })
    }

    pub(crate) fn show(&self, progress: FullPassSnapshot) {
        self.apply_theme();
        self.update(progress);
        let size = self.window.outer_size();
        if let Some((x, y)) = bottom_right_position(size.width as i32, size.height as i32) {
            self.window.set_outer_position(PhysicalPosition::new(x, y));
        }
        // After the position sync, because tao re-derives the window styles
        // there and puts WS_CAPTION back. Without this DWM composes a ~2px
        // frame around the panel, thin at the rounded corners.
        crate::overlay::host::strip_caption_frame(self.window.hwnd() as HWND);
        unsafe {
            ShowWindow(self.window.hwnd() as HWND, SW_SHOWNOACTIVATE);
        }
    }

    /// Theme the panel and the acrylic tint from the same answer the launcher
    /// uses. The page cannot ask `prefers-color-scheme`: WebView2 answers that
    /// from its own default, not from what Nex resolved for the overlay, so a
    /// light desktop could still get the dark panel.
    fn apply_theme(&self) {
        let (_, theme) = crate::overlay::platform::detect_look();
        let light = theme == crate::overlay::model::Theme::Light;
        let tint = if light {
            (248, 248, 250, 160)
        } else {
            (20, 20, 22, 140)
        };
        let _ = window_vibrancy::apply_acrylic(&self.window, Some(tint));
        apply_frame(self.window.hwnd() as HWND, !light);
        let name = if light { "light" } else { "dark" };
        let _ = self
            .webview
            .evaluate_script(&format!("document.documentElement.dataset.theme='{name}'"));
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

    /// Native handle, so the overlay's focus handler can tell this window
    /// apart from a real dismissal.
    pub(crate) fn hwnd(&self) -> HWND {
        self.window.hwnd() as HWND
    }
}

/// Turn off the native window frame. tao forces `WS_CAPTION` on every window,
/// and DWM composes a rounded ring around it even with acrylic — the ring is
/// thin at the corners and ~2px on the straight edges, which reads as a border
/// the panel does not draw. The caption strip in `show` removes the frame
/// itself; this removes the accent border DWM draws over the glass.
fn apply_frame(hwnd: HWND, dark: bool) {
    let no_border: u32 = 0xFFFFFFFE; // DWMWA_COLOR_NONE
    let dark_mode = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            &no_border as *const u32 as *const std::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark_mode as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
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
