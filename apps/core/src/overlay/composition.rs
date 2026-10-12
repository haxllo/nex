//! WebView2 visual-hosting foundation (composition controller).
//!
//! NOT YET WIRED — [`crate::overlay::host`] still uses wry HWND hosting.
//! This module stages the pieces the ClearType-with-rounded-corners
//! migration needs, against the pinned stack only (`webview2-com-sys`
//! 0.38.2, `windows` 0.61, Evergreen 155). No new crates.
//!
//! Two verified corrections to the original sketch live here:
//!
//! 1. Classic DirectComposition (`IDCompositionVisual::SetClip`) accepts
//!    only *rectangle* clips — it cannot express our 20px radius token.
//!    The rounded silhouette needs the Windows.UI.Composition path:
//!    `Compositor` → `ContainerVisual` → `CompositionGeometricClip` over
//!    a `CompositionRoundedRectangleGeometry`, bound to the tao `WS_POPUP`
//!    via `ICompositorDesktopInterop::CreateDesktopWindowTarget`.
//!    `with_no_redirection_bitmap(true)` (already set) is the correct
//!    pairing: no redirection surface, visuals only. See [`CompositorHost`].
//!
//! 2. There is no `SendKeyboardInput` in this SDK (zero hits in
//!    `webview2-com-sys` 0.38.2) — keyboard needs no manual forwarding:
//!    the composition controller still parents to our HWND, so
//!    `WM_KEY*` keeps its normal routing (same as the WPF
//!    `WebView2CompositionControl`, where focus sits on the controller's
//!    HWND). Only *spatial* input (mouse/touch/pen) must be forwarded via
//!    [`send_mouse_input`]. The LL hooks / raw-input sink / 0xE8 mask in
//!    [`crate::overlay::hotkey`] are global and unaffected.
//!
//! Opaque-surface parity with wry (which this must preserve on cutover):
//! wry sets `ICoreWebView2ControllerOptions3::DefaultBackgroundColor` at
//! creation (alpha clamped: any non-zero → 255) and
//! `ICoreWebView2Controller2::SetDefaultBackgroundColor` at runtime — see
//! [`creation_background`] / [`sync_background`]. The composition
//! controller implements `ICoreWebView2Controller`, so bounds, visibility
//! and `MoveFocus` keep working through the same interface.
//!
//! Deferred to the wiring commit (needs on-box runtime verification):
//! `WebMessageReceived` → existing `handle_ipc`, `WebResourceRequested`
//! → existing `serve_asset` (navigate to [`NAV_URL`] with the
//! [`PROTOCOL_FILTER`]; wry rewrites `nexasset://…` to http workaround
//! URIs — replicated in [`apply_protocol_workaround`]), `NavigationCompleted`
//! → Ready/Painted, `CursorChanged` → `SetCursor`.

#![cfg(target_os = "windows")]

// ── Panel token ──────────────────────────────────────────────────────────

/// Panel corner-radius token, logical px. Single source for the
/// composition clip; replaces the `20.0` literals in the GDI region
/// helpers this migration deletes.
pub const PANEL_RADIUS_LP: f64 = 20.0;

/// Custom-protocol scheme served today by `serve_asset` in host.rs.
pub const NEXASSET_SCHEME: &str = "nexasset";

/// wry navigates custom protocols through an http(s) workaround URI
/// (`wry-0.55.1/src/custom_protocol_workaround.rs`; default on Windows
/// is `http`, `use_https=false`). Replicate exactly or the page never
/// loads: navigate here and filter here.
pub const NAV_URL: &str = "http://nexasset.localhost/";
/// `AddWebResourceRequestedFilter` pattern for the workaround URIs.
pub const PROTOCOL_FILTER: &str = "http://nexasset.localhost/*";

/// `window.ipc.postMessage` shim, verbatim from wry's `attach_ipc_handler`.
/// The page's Rust-bound chatter (`app.js`) goes through `window.ipc`;
/// Rust→page already uses standard `PostWebMessageAsJson`, which is
/// controller-agnostic. Inject via
/// `ICoreWebView2::AddScriptToExecuteOnDocumentCreated` on cutover.
pub const IPC_SHIM_SCRIPT: &str = r#"Object.defineProperty(window, 'ipc', { value: Object.freeze({ postMessage: s=> window.chrome.webview.postMessage(s) }) });"#;

/// Corner radius in physical px for the current monitor scale.
/// (regions used diameters: `20*scale*2`; the geometry wants a radius.)
pub fn corner_radius_px(scale_factor: f64) -> f32 {
    (PANEL_RADIUS_LP * scale_factor) as f32
}

/// `windows_numerics::Vector2` mirror. The type lives in the
/// `windows-numerics` crate (not re-exported by `windows`), and this module
/// adds no new dependencies — so the two-f32 ABI is spelled locally and
/// converted at the call site. Layout matches `Vector2 { X: f32, Y: f32 }`
/// (frozen DirectXMath ABI); the size assert fails the build on drift.
#[repr(C)]
#[derive(Clone, Copy)]
struct Vec2 {
    x: f32,
    y: f32,
}
const _: () = assert!(std::mem::size_of::<Vec2>() == 8);

/// Physical panel size in px for the current height + monitor scale.
/// Mirrors the `w_px`/`h_px` math in the region helpers it replaces.
pub fn panel_size_px(window_width_lp: f64, height_lp: f64, scale_factor: f64) -> (f32, f32) {
    (
        (window_width_lp * scale_factor) as f32,
        (height_lp * scale_factor) as f32,
    )
}

// ── Opaque surface (ClearType gate) ──────────────────────────────────────

/// Creation/runtime surface color. `Some((r,g,b))` = opaque panel
/// (ClearType on); `None` = glass (transparent, grayscale).
/// Alpha clamp mirrors wry's `create_controller`: any non-zero alpha is
/// forced to 255 — a half-alpha surface still disables subpixel text, so
/// partial transparency is never what you want here.
pub fn surface_color(
    opaque_rgb: Option<(u8, u8, u8)>,
) -> webview2_com_sys::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_COLOR {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_COLOR;
    match opaque_rgb {
        Some((r, g, b)) => COREWEBVIEW2_COLOR { A: 255, R: r, G: g, B: b },
        None => COREWEBVIEW2_COLOR { A: 0, R: 0, G: 0, B: 0 },
    }
}

// ── Protocol workaround URIs (wry parity) ────────────────────────────────

/// `nexasset://localhost/<path>` → `http://nexasset.localhost/<path>`.
/// Mirrors `wry::custom_protocol_workaround::apply_uri_work_around`.
pub fn apply_protocol_workaround(uri: &str) -> String {
    uri.replace(&format!("{NEXASSET_SCHEME}://"), "http://nexasset.")
}

/// `http://nexasset.localhost/<path>` → path routed to `serve_asset`
/// (i.e. `/<path>`). Returns `None` for foreign URIs.
pub fn workaround_path(uri: &str) -> Option<&str> {
    uri.strip_prefix("http://nexasset.localhost")
}

// ── Mouse forwarding ─────────────────────────────────────────────────────

/// Minimal mouse action set the host forwards from tao `WindowEvent`s to
/// `ICoreWebView2CompositionController::SendMouseInput`. Coordinates are
/// physical px relative to the panel origin (same space as `SetBounds`).
pub enum MouseForward {
    Move { x_px: i32, y_px: i32 },
    Leave,
    LeftDown { x_px: i32, y_px: i32 },
    LeftUp { x_px: i32, y_px: i32 },
    RightDown { x_px: i32, y_px: i32 },
    RightUp { x_px: i32, y_px: i32 },
    MiddleDown { x_px: i32, y_px: i32 },
    MiddleUp { x_px: i32, y_px: i32 },
    Wheel { x_px: i32, y_px: i32, delta: i32 },
}

/// Map a [`MouseForward`] to `(event_kind, virtual_keys, mouse_data)`.
/// Virtual-keys bitmask: bit0 = left button held, bit1 = right, bit2 =
/// middle (matches `COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_*`). Pure for test.
pub fn mouse_forward_args(action: &MouseForward) -> (i32, i32, u32) {
    use MouseForward as M;
    match *action {
        M::Move { .. } => (512, 0, 0),        // MOUSE_MOVE
        M::Leave => (675, 0, 0),              // MOUSE_LEAVE
        M::LeftDown { .. } => (513, 1, 0),    // LEFT_BUTTON_DOWN
        M::LeftUp { .. } => (514, 0, 0),      // LEFT_BUTTON_UP
        M::RightDown { .. } => (516, 2, 0),   // RIGHT_BUTTON_DOWN
        M::RightUp { .. } => (517, 0, 0),     // RIGHT_BUTTON_UP
        M::MiddleDown { .. } => (519, 16, 0), // MIDDLE_BUTTON_DOWN
        M::MiddleUp { .. } => (520, 0, 0),    // MIDDLE_BUTTON_UP
        M::Wheel { delta, .. } => (522, 0, delta as u32), // WHEEL (WHEEL_DELTA=120/click)
    }
}

/// Position carried by a [`MouseForward`]; `None` for `Leave`.
pub fn mouse_forward_point(action: &MouseForward) -> Option<(i32, i32)> {
    use MouseForward as M;
    match *action {
        M::Move { x_px, y_px }
        | M::LeftDown { x_px, y_px }
        | M::LeftUp { x_px, y_px }
        | M::RightDown { x_px, y_px }
        | M::RightUp { x_px, y_px }
        | M::MiddleDown { x_px, y_px }
        | M::MiddleUp { x_px, y_px }
        | M::Wheel { x_px, y_px, .. } => Some((x_px, y_px)),
        M::Leave => None,
    }
}

/// Forward one mouse action to a visually-hosted WebView.
pub fn send_mouse_input(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    action: &MouseForward,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MOUSE_EVENT_KIND, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS,
    };
    let (kind, keys, data) = mouse_forward_args(action);
    let (x, y) = mouse_forward_point(action).unwrap_or((0, 0));
    unsafe {
        controller.SendMouseInput(
            COREWEBVIEW2_MOUSE_EVENT_KIND(kind),
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(keys),
            data,
            windows::Win32::Foundation::POINT { x, y },
        )
    }
}

// ── WinComp tree (the rounded silhouette) ────────────────────────────────

/// Windows.UI.Composition tree hosting the WebView visual.
///
/// Layout: `DesktopWindowTarget(SetRoot(root))` → `root: ContainerVisual`
/// → `content: ContainerVisual` (the `RootVisualTarget` WebView2 draws
/// into) with a `CompositionGeometricClip(RoundedRectangleGeometry)` at
/// the panel token. `content` is deliberately a child, not the root, so
/// later overlay visuals (toasts, drag-ghost) can join `root` as siblings
/// without new HWNDs — one of the free perks, no extra code today.
///
/// All sizes are physical px (desktop interop targets are not DPI-virtualized).
/// Call [`CompositorHost::resize`] on every `apply_window_height`, and set
/// [`set_rasterization_scale`] with the same `scale_factor()` or the page
/// renders blurry above 100%.
pub struct CompositorHost {
    // Kept alive for sibling visuals later (toasts, drag-ghost join
    // `root` without new HWNDs); the target holds its own COM ref today.
    _root: windows::UI::Composition::ContainerVisual,
    content: windows::UI::Composition::ContainerVisual,
    geometry: windows::UI::Composition::CompositionRoundedRectangleGeometry,
    _target: windows::UI::Composition::Desktop::DesktopWindowTarget,
    // DispatcherQueueController for this thread: WinComp requires one
    // before Compositor activation on classic Win32 threads. Held while
    // the tree lives (None when the queue already existed, e.g. rebuild).
    _dq_controller: Option<windows::System::DispatcherQueueController>,
}

impl CompositorHost {
    /// Build the tree on `hwnd` (tao main-window HWND as isize).
    /// `topmost=true` matches the always-on-top overlay.
    pub fn new(hwnd_sys: isize) -> Result<Self, String> {
        use windows::UI::Composition::Compositor;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::WinRT::{
            CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_STA,
            DQTYPE_THREAD_CURRENT,
        };
        use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
        // One queue per thread; a repeat call (page rebuild on the same
        // thread) fails but the existing queue still serves — so this is
        // best-effort and Compositor::new below is the real gate.
        let dq_controller = unsafe {
            CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_STA,
            })
        }
        .ok();
        let compositor = Compositor::new().map_err(|e| format!("Compositor::new: {e}"))?;
        let interop: ICompositorDesktopInterop =
            windows_core::Interface::cast(&compositor).map_err(|e| format!("cast interop: {e}"))?;
        let target = unsafe { interop.CreateDesktopWindowTarget(HWND(hwnd_sys as *mut _), true) }
            .map_err(|e| format!("CreateDesktopWindowTarget: {e}"))?;
        let root = compositor
            .CreateContainerVisual()
            .map_err(|e| format!("CreateContainerVisual(root): {e}"))?;
        let content = compositor
            .CreateContainerVisual()
            .map_err(|e| format!("CreateContainerVisual(content): {e}"))?;
        let geometry = compositor
            .CreateRoundedRectangleGeometry()
            .map_err(|e| format!("CreateRoundedRectangleGeometry: {e}"))?;
        let clip = compositor
            .CreateGeometricClipWithGeometry(&geometry)
            .map_err(|e| format!("CreateGeometricClip: {e}"))?;
        content
            .SetClip(&clip)
            .map_err(|e| format!("SetClip: {e}"))?;
        root.Children()
            .and_then(|kids| kids.InsertAtTop(&content))
            .map_err(|e| format!("InsertAtTop: {e}"))?;
        target
            .SetRoot(&root)
            .map_err(|e| format!("SetRoot: {e}"))?;
        Ok(Self {
            _root: root,
            content,
            geometry,
            _target: target,
            _dq_controller: dq_controller,
        })
    }

    /// Resize the content visual + rounded clip. `radius_px` comes from
    /// [`corner_radius_px`]; size from [`panel_size_px`].
    pub fn resize(&self, w_px: f32, h_px: f32, radius_px: f32) -> Result<(), String> {
        // SAFETY: Vec2 is layout-identical to windows_numerics::Vector2
        // (size-asserted above); the target type is inferred from the callee.
        let size = Vec2 { x: w_px, y: h_px };
        let radius = Vec2 {
            x: radius_px,
            y: radius_px,
        };
        self.content
            .SetSize(unsafe { std::mem::transmute_copy::<Vec2, _>(&size) })
            .map_err(|e| format!("visual SetSize: {e}"))?;
        self.geometry
            .SetSize(unsafe { std::mem::transmute_copy::<Vec2, _>(&size) })
            .map_err(|e| format!("geometry SetSize: {e}"))?;
        self.geometry
            .SetCornerRadius(unsafe { std::mem::transmute_copy::<Vec2, _>(&radius) })
            .map_err(|e| format!("SetCornerRadius: {e}"))?;
        Ok(())
    }

    /// The visual WebView2 draws into — pass to [`set_root_visual`].
    pub fn content_visual(&self) -> &windows::UI::Composition::ContainerVisual {
        &self.content
    }
}

// ── Controller conveniences (composition controller IS a controller) ─────

/// Connect the WebView's visual tree under our content visual.
pub fn set_root_visual(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    content: &windows::UI::Composition::ContainerVisual,
) -> windows_core::Result<()> {
    let unk: windows_core::IUnknown = windows_core::Interface::cast(content)?;
    unsafe { controller.SetRootVisualTarget(&unk) }
}

/// Creation-time opaque surface. Call on the `ControllerOptions` before
/// `CreateCoreWebView2CompositionControllerWithOptions` — this is what
/// gates ClearType; later switches are ignored by Chromium. Mirrors wry.
pub fn creation_background(
    options: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2ControllerOptions,
    opaque_rgb: Option<(u8, u8, u8)>,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2ControllerOptions3;
    let color = surface_color(opaque_rgb);
    let options3: ICoreWebView2ControllerOptions3 =
        windows_core::Interface::cast(options)?;
    unsafe { options3.SetDefaultBackgroundColor(color) }
}

/// Runtime surface switch (panel background changes without restart).
/// Takes effect on newly-painted content; a creation-time mismatch still
/// needs a controller rebuild for ClearType. Mirrors wry's
/// `set_background_color`.
pub fn sync_background(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    opaque_rgb: Option<(u8, u8, u8)>,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller, ICoreWebView2Controller2,
    };
    let base: ICoreWebView2Controller = windows_core::Interface::cast(controller)?;
    let c2: ICoreWebView2Controller2 = windows_core::Interface::cast(&base)?;
    unsafe { c2.SetDefaultBackgroundColor(surface_color(opaque_rgb)) }
}

/// Size the page in physical px. Replaces `keep_webview_viewport_max`
/// (the oversized-viewport trick): with no child HWND there is nothing
/// for wry to snap on `WM_SIZE`, so size exactly and drop the trick —
/// growth re-rasters instead of revealing stale pixels (verify on-box).
pub fn set_bounds_px(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    w_px: i32,
    h_px: i32,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
    let base: ICoreWebView2Controller = windows_core::Interface::cast(controller)?;
    unsafe {
        base.SetBounds(windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: w_px,
            bottom: h_px,
        })
    }
}

/// Per-monitor DPI crispness for visual hosting. Call with
/// `window.scale_factor()` on show + `WM_DPICHANGED`; without it the
/// page renders at 96dpi and upscales (blurry above 100%).
pub fn set_rasterization_scale(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    scale_factor: f64,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller, ICoreWebView2Controller3,
    };
    let base: ICoreWebView2Controller = windows_core::Interface::cast(controller)?;
    let c3: ICoreWebView2Controller3 = windows_core::Interface::cast(&base)?;
    unsafe { c3.SetRasterizationScale(scale_factor) }
}

/// Show/hide the page (replaces tao `set_visible` for content; the HWND
/// visibility semantics in host.rs stay untouched).
pub fn set_visible(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    visible: bool,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
    let base: ICoreWebView2Controller = windows_core::Interface::cast(controller)?;
    unsafe { base.SetIsVisible(visible) }
}

/// Give the page keyboard focus (replaces `wv.focus()`; the DOM half —
/// `window.nex.focus()` via `ExecuteScript` — is unchanged).
pub fn move_focus_programmatic(
    controller: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
) -> windows_core::Result<()> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, ICoreWebView2Controller,
    };
    let base: ICoreWebView2Controller = windows_core::Interface::cast(controller)?;
    unsafe { base.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) }
}

/// In-memory `IStream` for `CreateWebResourceResponse` bodies
/// (replaces wry's `SHCreateMemStream` call — same API, same semantics).
pub fn mem_stream(bytes: &[u8]) -> Option<windows::Win32::System::Com::IStream> {
    unsafe { windows::Win32::UI::Shell::SHCreateMemStream(Some(bytes)) }
}

// ── Async creation (hand-rolled completed handlers + pump) ───────────────

use std::sync::mpsc;

// ── Async creation via webview2_com factories ────────────────────────────
// Hand-rolled COM completed-handlers died here (S_OK + dangling result;
// implement!/webview2_com-built handlers live in every tested combo), so
// every async completion and event subscription below goes through
// webview2_com's proven factories. No raw vtbls remain in this module.

/// Create the shared WebView2 environment (wry-parity options object,
/// default user-data dir). Must run on the tao main thread — completions
/// arrive through its message loop.
pub fn create_environment() -> Result<
    webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    String,
> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2Environment,
        ICoreWebView2EnvironmentOptions,
    };
    let (tx, rx) = mpsc::channel();
    let handler = webview2_com::CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |error_code, environment| {
            let result: Result<ICoreWebView2Environment, String> = (|| {
                error_code.map_err(|e| format!("{e}"))?;
                environment.ok_or_else(|| "null environment".to_string())
            })();
            let _ = tx.send(result);
            Ok(())
        },
    ));
    let options = webview2_com::CoreWebView2EnvironmentOptions::default();
    let user_data = windows_core::HSTRING::from("");
    let create = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            windows_core::PCWSTR::null(),
            &user_data,
            &ICoreWebView2EnvironmentOptions::from(options),
            &handler,
        )
    };
    std::mem::forget(handler);
    if let Err(e) = create {
        return Err(format!("CreateCoreWebView2Environment: {e}"));
    }
    webview2_com::wait_with_pump(rx)
        .map_err(|e| format!("CreateCoreWebView2Environment: {e}"))?
}

/// Create a composition controller on `hwnd` with the opaque surface from
/// birth (`opaque_rgb`: `Some` = black/white ClearType modes, `None` =
/// glass). Prefers `...WithOptions` (sets `DefaultBackgroundColor`
/// pre-creation); falls back to plain `CreateCoreWebView2CompositionController`
/// on runtimes without `Environment10` (then the first paint may start
/// transparent — log and rebuild on mode switch).
pub fn create_composition_controller(
    env: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    hwnd_sys: isize,
    opaque_rgb: Option<(u8, u8, u8)>,
) -> Result<
    webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    String,
> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2CompositionController, ICoreWebView2Environment10,
        ICoreWebView2Environment3,
    };
    use windows::Win32::Foundation::HWND;
    let (tx, rx) = mpsc::channel();
    let handler =
        webview2_com::CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(
            move |error_code, controller| {
                let result: Result<ICoreWebView2CompositionController, String> = (|| {
                    error_code.map_err(|e| format!("{e}"))?;
                    controller.ok_or_else(|| "null composition controller".to_string())
                })();
                let _ = tx.send(result);
                Ok(())
            },
        ));
    let hwnd = HWND(hwnd_sys as *mut _);
    let create = if let Ok(env10) = windows_core::Interface::cast::<ICoreWebView2Environment10>(env)
    {
        (|| {
            let opts = unsafe { env10.CreateCoreWebView2ControllerOptions() }?;
            creation_background(&opts, opaque_rgb)?;
            unsafe {
                env10.CreateCoreWebView2CompositionControllerWithOptions(hwnd, &opts, &handler)
            }
        })()
    } else if let Ok(env3) = windows_core::Interface::cast::<ICoreWebView2Environment3>(env) {
        crate::logging::warn(
            "[nex] Environment10 missing: composition controller without creation-time background",
        );
        unsafe { env3.CreateCoreWebView2CompositionController(hwnd, &handler) }
    } else {
        Err(windows_core::Error::from(windows_core::imp::E_NOINTERFACE))
    };
    std::mem::forget(handler);
    if let Err(e) = create {
        return Err(format!("CreateCoreWebView2CompositionController: {e}"));
    }
    webview2_com::wait_with_pump(rx)
        .map_err(|e| format!("CreateCoreWebView2CompositionController: {e}"))?
}

// ── Page view: controller + core + compositor ────────────────────────────

use std::borrow::Cow;
use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CompositionController, ICoreWebView2Controller,
    ICoreWebView2Environment, ICoreWebView2WebResourceRequestedEventArgs,
};

/// `serve_asset`-shaped responder (lives in host.rs; bridged in so this
/// module stays free of page assets). wry's `http` types are reused for
/// the bridge — no new crates.
pub type ServeAsset = Box<
    dyn Fn(wry::http::Request<Vec<u8>>) -> wry::http::Response<Cow<'static, [u8]>> + Send,
>;

/// The live page: composition controller + core + WinComp tree. `_env`
/// is never read — it keeps the WebView2 environment alive (controllers
/// die with it).
pub struct CompositionView {
    pub(crate) controller: ICoreWebView2CompositionController,
    pub(crate) core: ICoreWebView2,
    pub(crate) compositor: CompositorHost,
    _env: ICoreWebView2Environment,
}

/// Take a `CoTaskMem`-allocated `PWSTR` from WebView2 and free it.
/// Mirrors wry's `take_pwstr`.
unsafe fn take_pwstr(ptr: windows_core::PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while unsafe { *ptr.0.add(len) } != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr.0, len) });
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(ptr.0 as _)) };
    s
}

/// Null-terminated UTF-16 for `PCWSTR` params; keep the vec alive through
/// the call.
fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `http://nexasset.localhost/<path>` → `nexasset://localhost/<path>`
/// (what `serve_asset` routes on). Inverse of [`apply_protocol_workaround`].
pub fn revert_protocol_workaround(uri: &str) -> String {
    uri.replace("http://nexasset.", &format!("{NEXASSET_SCHEME}://"))
}

// ── JS→Rust: WebMessageReceived → handle_ipc ────────────────────────────

/// Subscribe page→Rust messages (`window.ipc.postMessage` via
/// [`IPC_SHIM_SCRIPT`); delivers the raw JSON string to `on_message`
/// (host's `handle_ipc`). Handler is intentionally leaked: it lives as
/// long as the process-lifetime page.
pub fn attach_ipc_handler(
    core: &ICoreWebView2,
    on_message: Box<dyn Fn(String) + Send>,
) -> Result<(), String> {
    let handler = webview2_com::WebMessageReceivedEventHandler::create(Box::new(
        move |_sender, args| {
            if let Some(args) = args {
                let mut msg = windows_core::PWSTR::null();
                if unsafe { args.TryGetWebMessageAsString(&mut msg) }.is_ok() {
                    on_message(unsafe { take_pwstr(msg) });
                }
            }
            Ok(())
        },
    ));
    let mut token = 0i64;
    let sub = unsafe { core.add_WebMessageReceived(&handler, &mut token) };
    std::mem::forget(handler);
    sub.map_err(|e| format!("add_WebMessageReceived: {e}"))
}

// ── Rust→page assets: WebResourceRequested → serve_asset ─────────────────

/// Filter `http://nexasset.localhost/*` into [`serve_protocol_request`].
/// Handler intentionally leaked (process-lifetime page).
pub fn attach_protocol_handler(
    core: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    serve: ServeAsset,
) -> Result<(), String> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
    let filter = wide_null(PROTOCOL_FILTER);
    unsafe {
        core.AddWebResourceRequestedFilter(
            windows_core::PCWSTR::from_raw(filter.as_ptr()),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )
    }
    .map_err(|e| format!("AddWebResourceRequestedFilter: {e}"))?;
    let env = env.clone();
    let handler = webview2_com::WebResourceRequestedEventHandler::create(Box::new(
        move |_sender, args| {
            if let Some(args) = args {
                serve_protocol_request(&env, &serve, &args);
            }
            Ok(())
        },
    ));
    let mut token = 0i64;
    let sub = unsafe { core.add_WebResourceRequested(&handler, &mut token) };
    std::mem::forget(handler);
    sub.map_err(|e| format!("add_WebResourceRequested: {e}"))
}

fn serve_protocol_request(
    env: &ICoreWebView2Environment,
    serve: &ServeAsset,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
) {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequest;
    let respond = |resp: &wry::http::Response<Cow<'static, [u8]>>| {
        let status = resp.status().as_u16() as i32;
        let reason = wide_null(resp.status().canonical_reason().unwrap_or("OK"));
        let mut headers = String::new();
        for (name, value) in resp.headers().iter() {
            if let Ok(v) = value.to_str() {
                headers.push_str(&format!("{}: {}\n", name.as_str(), v));
            }
        }
        let headers = wide_null(&headers);
        let stream = if resp.body().is_empty() {
            None
        } else {
            mem_stream(resp.body())
        };
        let response = unsafe {
            env.CreateWebResourceResponse(
                stream.as_ref(),
                status,
                windows_core::PCWSTR::from_raw(reason.as_ptr()),
                windows_core::PCWSTR::from_raw(headers.as_ptr()),
            )
        };
        match response {
            Ok(r) => {
                let _ = unsafe { args.SetResponse(&r) };
            }
            Err(e) => crate::logging::warn(&format!("[nex] protocol response failed: {e}")),
        }
    };
    let request_iface: ICoreWebView2WebResourceRequest = match unsafe { args.Request() } {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut uri_p = windows_core::PWSTR::null();
    if unsafe { request_iface.Uri(&mut uri_p) }.is_err() {
        return;
    }
    let uri = unsafe { take_pwstr(uri_p) };
    if workaround_path(&uri).is_none() {
        return; // not ours — let WebView2 handle it.
    }
    match build_serve_request(&request_iface, &uri) {
        Ok(req) => respond(&(serve)(req)),
        Err(e) => {
            crate::logging::warn(&format!("[nex] protocol request failed: {e}"));
            respond(&err_response(&e));
        }
    }
}

/// Rebuild the `http` request `serve_asset` expects: method + headers +
/// body, with the workaround URI reverted to `nexasset://…`. Mirrors
/// wry's `prepare_request` (same `http` crate via the `wry::http`
/// re-export, so the builder calls type-check identically).
fn build_serve_request(
    request_iface: &webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequest,
    uri: &str,
) -> Result<wry::http::Request<Vec<u8>>, String> {
    let mut builder = wry::http::Request::builder();
    let mut method = windows_core::PWSTR::null();
    unsafe { request_iface.Method(&mut method) }.map_err(|e| format!("method: {e}"))?;
    builder = builder.method(unsafe { take_pwstr(method) }.as_str());
    let headers = unsafe { request_iface.Headers() }.map_err(|e| format!("headers: {e}"))?;
    let iter = unsafe { headers.GetIterator() }.map_err(|e| format!("header iter: {e}"))?;
    let mut has = windows_core::BOOL::default();
    unsafe { iter.HasCurrentHeader(&mut has) }.map_err(|e| format!("has header: {e}"))?;
    while has.as_bool() {
        let mut k = windows_core::PWSTR::null();
        let mut v = windows_core::PWSTR::null();
        unsafe { iter.GetCurrentHeader(&mut k, &mut v) }.map_err(|e| format!("get header: {e}"))?;
        builder = builder.header(
            unsafe { take_pwstr(k) }.as_str(),
            unsafe { take_pwstr(v) }.as_str(),
        );
        unsafe { iter.MoveNext(&mut has) }.map_err(|e| format!("next header: {e}"))?;
    }
    let mut body = Vec::new();
    if let Ok(content) = unsafe { request_iface.Content() } {
        let mut buf = [0u8; 1024];
        loop {
            let mut read = 0u32;
            let r = unsafe {
                content.Read(
                    buf.as_mut_ptr() as *mut core::ffi::c_void,
                    buf.len() as u32,
                    Some(&mut read),
                )
            };
            if r.is_err() || read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
        }
    }
    builder
        .uri(revert_protocol_workaround(uri))
        .body(body)
        .map_err(|e| format!("request: {e}"))
}

fn err_response(msg: &str) -> wry::http::Response<Cow<'static, [u8]>> {
    wry::http::Response::builder()
        .status(400u16)
        .header("Content-Type", "text/plain")
        .body(Cow::Owned(msg.as_bytes().to_vec()))
        .unwrap_or_else(|_| wry::http::Response::new(Cow::Borrowed(&b""[..])))
}

// ── Init scripts + fire-and-forget ExecuteScript ─────────────────────────

/// Register one document-start script and wait for confirmation (wry
/// parity: scripts must land before `Navigate` returns content).
pub fn add_init_script(core: &ICoreWebView2, js: &str) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let handler =
        webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
            move |error_code, _id| {
                let result: Result<(), String> =
                    error_code.map_err(|e| format!("{e}")).map(|_| ());
                let _ = tx.send(result);
                Ok(())
            },
        ));
    let script = windows_core::HSTRING::from(js);
    let add = unsafe { core.AddScriptToExecuteOnDocumentCreated(&script, &handler) };
    std::mem::forget(handler);
    if let Err(e) = add {
        return Err(format!("AddScriptToExecuteOnDocumentCreated: {e}"));
    }
    webview2_com::wait_with_pump(rx)
        .map_err(|e| format!("AddScriptToExecuteOnDocumentCreated: {e}"))?
}

/// Fire-and-forget `ExecuteScript` (focus ping, future toasts). The
/// result is dropped on the floor. Never use for calls whose return
/// value matters.
pub fn execute_script(core: &ICoreWebView2, js: &str) -> windows_core::Result<()> {
    let handler = webview2_com::ExecuteScriptCompletedHandler::create(Box::new(
        move |_error_code, _result| Ok(()),
    ));
    let script = windows_core::HSTRING::from(js);
    let r = unsafe { core.ExecuteScript(&script, &handler) };
    std::mem::forget(handler);
    r
}

// ── Full assembly ────────────────────────────────────────────────────────

/// Build the live page in order: environment → controller (opaque surface
/// from birth) → WinComp tree → root visual → protocol filter/handler →
/// IPC handler → init scripts → navigate → visible. Must run on the tao
/// main thread (completions pump through its message loop).
pub fn build_composition_view(
    hwnd_sys: isize,
    scale: f64,
    window_width_lp: f64,
    height_lp: f64,
    opaque_rgb: Option<(u8, u8, u8)>,
    init_scripts: &[&str],
    on_message: Box<dyn Fn(String) + Send>,
    serve: ServeAsset,
) -> Result<CompositionView, String> {
    let env = create_environment()?;
    crate::logging::info("[nex] composition: environment ready");
    let controller = create_composition_controller(&env, hwnd_sys, opaque_rgb)?;
    crate::logging::info("[nex] composition: controller ready");
    // NAV_URL is the workaround form of the page origin — keep both
    // spellings pinned together.
    debug_assert_eq!(NAV_URL, apply_protocol_workaround("nexasset://localhost/"));
    let base: ICoreWebView2Controller = windows_core::Interface::cast(&controller)
        .map_err(|e| format!("cast ICoreWebView2Controller: {e}"))?;
    let core: ICoreWebView2 =
        unsafe { base.CoreWebView2() }.map_err(|e| format!("CoreWebView2: {e}"))?;
    let compositor = CompositorHost::new(hwnd_sys)?;
    crate::logging::info("[nex] composition: compositor ready");
    let (w, h) = panel_size_px(window_width_lp, height_lp, scale);
    compositor.resize(w, h, corner_radius_px(scale))?;
    set_root_visual(&controller, compositor.content_visual())
        .map_err(|e| format!("root visual: {e}"))?;
    crate::logging::info("[nex] composition: root visual set");
    set_rasterization_scale(&controller, scale).map_err(|e| format!("rasterization: {e}"))?;
    set_bounds_px(&controller, w as i32, h as i32).map_err(|e| format!("bounds: {e}"))?;
    attach_protocol_handler(&core, &env, serve)?;
    crate::logging::info("[nex] composition: protocol attached");
    attach_ipc_handler(&core, on_message)?;
    crate::logging::info("[nex] composition: ipc attached");
    for js in init_scripts {
        add_init_script(&core, js)?;
    }
    crate::logging::info("[nex] composition: init scripts registered");
    unsafe {
        core.Navigate(&windows_core::HSTRING::from(apply_protocol_workaround(
            "nexasset://localhost/",
        )))
    }
    .map_err(|e| format!("Navigate: {e}"))?;
    crate::logging::info("[nex] composition: navigated");
    set_visible(&controller, true).map_err(|e| format!("visible: {e}"))?;
    Ok(CompositionView {
        controller,
        core,
        compositor,
        _env: env,
    })
}

/// Resize the page for `apply_window_height`: clip + bounds +
/// rasterization, one call. Replaces `keep_webview_viewport_max` (exact
/// sizing — no child HWND left to snap) and both region helpers.
pub fn resize_view(
    view: &CompositionView,
    window_width_lp: f64,
    height_lp: f64,
    scale: f64,
) -> Result<(), String> {
    let (w, h) = panel_size_px(window_width_lp, height_lp, scale);
    view.compositor.resize(w, h, corner_radius_px(scale))?;
    set_bounds_px(&view.controller, w as i32, h as i32).map_err(|e| format!("bounds: {e}"))?;
    set_rasterization_scale(&view.controller, scale).map_err(|e| format!("rasterization: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_token_matches_panel_spec() {
        assert_eq!(PANEL_RADIUS_LP, 20.0);
        assert_eq!(corner_radius_px(1.0), 20.0);
        assert_eq!(corner_radius_px(1.5), 30.0);
    }

    #[test]
    fn panel_size_scales_like_region_math() {
        let (w, h) = panel_size_px(700.0, 60.0, 1.0);
        assert_eq!((w, h), (700.0, 60.0));
        let (w2, _) = panel_size_px(700.0, 60.0, 1.5);
        assert_eq!(w2, 1050.0);
    }

    #[test]
    fn opaque_surface_is_fully_opaque() {
        let black = surface_color(Some((0, 0, 0)));
        assert_eq!((black.A, black.R, black.G, black.B), (255, 0, 0, 0));
        let white = surface_color(Some((255, 255, 255)));
        assert_eq!((white.A, white.R, white.G, white.B), (255, 255, 255, 255));
    }

    #[test]
    fn glass_surface_is_fully_transparent() {
        let t = surface_color(None);
        assert_eq!((t.A, t.R, t.G, t.B), (0, 0, 0, 0));
    }

    #[test]
    fn workaround_round_trips_through_serve_asset_paths() {
        // wry navigates nexasset://localhost/… as http://nexasset.localhost/…
        assert_eq!(
            apply_protocol_workaround("nexasset://localhost/index.html"),
            "http://nexasset.localhost/index.html"
        );
        // serve_asset routes on the path remainder:
        assert_eq!(
            workaround_path("http://nexasset.localhost/style.css"),
            Some("/style.css")
        );
        assert_eq!(workaround_path("https://example.com/"), None);
        // …and the handler reverts to the original scheme before serving:
        assert_eq!(
            revert_protocol_workaround("http://nexasset.localhost/style.css"),
            "nexasset://localhost/style.css"
        );
    }

    #[test]
    fn mouse_kinds_match_win32_messages() {
        // Spot-check against Win32 values (WM_LBUTTONDOWN=513, …) so a
        // typo here can't silently misroute clicks on cutover.
        assert_eq!(mouse_forward_args(&MouseForward::LeftDown { x_px: 1, y_px: 2 }).0, 513);
        assert_eq!(mouse_forward_args(&MouseForward::LeftUp { x_px: 1, y_px: 2 }).0, 514);
        assert_eq!(mouse_forward_args(&MouseForward::Move { x_px: 1, y_px: 2 }).0, 512);
        assert_eq!(mouse_forward_args(&MouseForward::Leave).0, 675);
        assert_eq!(
            mouse_forward_point(&MouseForward::Wheel { x_px: 3, y_px: 4, delta: 120 }),
            Some((3, 4))
        );
        assert_eq!(mouse_forward_point(&MouseForward::Leave), None);
    }
}
