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
    root: windows::UI::Composition::ContainerVisual,
    content: windows::UI::Composition::ContainerVisual,
    geometry: windows::UI::Composition::CompositionRoundedRectangleGeometry,
    _target: windows::UI::Composition::Desktop::DesktopWindowTarget,
}

impl CompositorHost {
    /// Build the tree on `hwnd` (tao main-window HWND as isize).
    /// `topmost=true` matches the always-on-top overlay.
    pub fn new(hwnd_sys: isize) -> Result<Self, String> {
        use windows::UI::Composition::Compositor;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
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
            root,
            content,
            geometry,
            _target: target,
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

use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::{AtomicU32, Ordering};

/// Pump window messages until `rx` delivers or 120s elapse. Mirrors
/// `webview2_com::wait_with_pump` (WebView2 fires completions through the
/// calling thread's message loop), plus a deadline so a wedged runtime
/// fails loudly instead of hanging launch forever.
fn wait_with_pump<T>(rx: Receiver<T>, what: &str) -> Result<T, String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageA, PeekMessageA, TranslateMessage, MSG, PM_REMOVE,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut msg = MSG::default();
    loop {
        if let Ok(v) = rx.try_recv() {
            return Ok(v);
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("{what}: timed out after 120s"));
        }
        let mut pumped = false;
        while unsafe { PeekMessageA(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            pumped = true;
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageA(&msg);
            }
            if let Ok(v) = rx.try_recv() {
                return Ok(v);
            }
        }
        if !pumped {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

macro_rules! completed_handler {
    ($name:ident, $handler_iface:ty, $vtbl:ty, $result_iface:ty, $iid_of:ty) => {
        #[repr(C)]
        struct $name {
            vtbl: *const $vtbl,
            ref_count: AtomicU32,
            tx: mpsc::Sender<Result<$result_iface, String>>,
        }

        impl $name {
            unsafe extern "system" fn add_ref(this: *mut core::ffi::c_void) -> u32 {
                let h = unsafe { &*(this as *const $name) };
                h.ref_count.fetch_add(1, Ordering::Relaxed) + 1
            }

            unsafe extern "system" fn release(this: *mut core::ffi::c_void) -> u32 {
                let h = unsafe { &*(this as *const $name) };
                let prev = h.ref_count.fetch_sub(1, Ordering::Release);
                if prev == 1 {
                    unsafe {
                        drop(Box::from_raw(this as *mut $name));
                    }
                }
                prev - 1
            }

            unsafe extern "system" fn query_interface(
                this: *mut core::ffi::c_void,
                iid: *const windows_core::GUID,
                out: *mut *mut core::ffi::c_void,
            ) -> windows_core::HRESULT {
                if out.is_null() || iid.is_null() {
                    return windows_core::imp::E_POINTER;
                }
                unsafe {
                    *out = core::ptr::null_mut();
                }
                let iid = unsafe { &*iid };
                if *iid == <windows_core::IUnknown as windows_core::Interface>::IID
                    || *iid == <$iid_of as windows_core::Interface>::IID
                {
                    unsafe {
                        *out = this;
                    }
                    unsafe {
                        Self::add_ref(this);
                    }
                    windows_core::imp::S_OK
                } else {
                    windows_core::imp::E_NOINTERFACE
                }
            }

            unsafe extern "system" fn invoke(
                this: *mut core::ffi::c_void,
                error: windows_core::HRESULT,
                result: *mut core::ffi::c_void,
            ) -> windows_core::HRESULT {
                let h = unsafe { &*(this as *const $name) };
                let iface: windows_core::Result<$result_iface> =
                    unsafe { windows_core::Type::from_abi(result) };
                let value: Result<$result_iface, String> = if error.is_ok() && !result.is_null() {
                    iface.map_err(|e| format!("creation failed: {e}"))
                } else {
                    Err(format!("creation failed: {error:?}"))
                };
                let _ = h.tx.send(value);
                windows_core::imp::S_OK
            }
        }
    };
}

/// Create the shared WebView2 environment (default user-data dir, same as
/// wry's default). Must run on the tao main thread — completions arrive
/// through its message loop.
pub fn create_environment() -> Result<
    webview2_com_sys::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    String,
> {
    use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2Environment, ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler,
        ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler_Vtbl,
        ICoreWebView2Environment,
    };
    completed_handler!(
        EnvCompleted,
        ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler,
        ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler_Vtbl,
        ICoreWebView2Environment,
        ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler
    );
    static ENV_VTBL: ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler_Vtbl =
        ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler_Vtbl {
            base__: windows_core::IUnknown_Vtbl {
                QueryInterface: EnvCompleted::query_interface,
                AddRef: EnvCompleted::add_ref,
                Release: EnvCompleted::release,
            },
            Invoke: EnvCompleted::invoke,
        };
    let (tx, rx) = mpsc::channel();
    let handler = Box::new(EnvCompleted {
        vtbl: &ENV_VTBL as *const _,
        ref_count: AtomicU32::new(1),
        tx,
    });
    let raw = Box::into_raw(handler) as *mut core::ffi::c_void;
    let com: ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler =
        unsafe { core::mem::transmute_copy(&raw) };
    let create = unsafe { CreateCoreWebView2Environment(&com) };
    std::mem::forget(com);
    if let Err(e) = create {
        unsafe {
            drop(Box::from_raw(raw as *mut EnvCompleted));
        }
        return Err(format!("CreateCoreWebView2Environment: {e}"));
    }
    wait_with_pump(rx, "CreateCoreWebView2Environment")?
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
        ICoreWebView2CompositionController,
        ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler,
        ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler_Vtbl,
        ICoreWebView2Environment10, ICoreWebView2Environment3,
    };
    use windows::Win32::Foundation::HWND;
    completed_handler!(
        CompositionCompleted,
        ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler,
        ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler_Vtbl,
        ICoreWebView2CompositionController,
        ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler
    );
    static COMPOSITION_VTBL: ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler_Vtbl =
        ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler_Vtbl {
            base__: windows_core::IUnknown_Vtbl {
                QueryInterface: CompositionCompleted::query_interface,
                AddRef: CompositionCompleted::add_ref,
                Release: CompositionCompleted::release,
            },
            Invoke: CompositionCompleted::invoke,
        };
    let (tx, rx) = mpsc::channel();
    let handler = Box::new(CompositionCompleted {
        vtbl: &COMPOSITION_VTBL as *const _,
        ref_count: AtomicU32::new(1),
        tx,
    });
    let raw = Box::into_raw(handler) as *mut core::ffi::c_void;
    let com: ICoreWebView2CreateCoreWebView2CompositionControllerCompletedHandler =
        unsafe { core::mem::transmute_copy(&raw) };
    let hwnd = HWND(hwnd_sys as *mut _);
    let create = if let Ok(env10) = windows_core::Interface::cast::<ICoreWebView2Environment10>(env)
    {
        (|| {
            let opts = unsafe { env10.CreateCoreWebView2ControllerOptions() }?;
            creation_background(&opts, opaque_rgb)?;
            unsafe {
                env10.CreateCoreWebView2CompositionControllerWithOptions(hwnd, &opts, &com)
            }
        })()
    } else if let Ok(env3) = windows_core::Interface::cast::<ICoreWebView2Environment3>(env) {
        crate::logging::warn(
            "[nex] Environment10 missing: composition controller without creation-time background",
        );
        unsafe { env3.CreateCoreWebView2CompositionController(hwnd, &com) }
    } else {
        Err(windows_core::Error::from(windows_core::imp::E_NOINTERFACE))
    };
    std::mem::forget(com);
    if let Err(e) = create {
        unsafe {
            drop(Box::from_raw(raw as *mut CompositionCompleted));
        }
        return Err(format!("CreateCoreWebView2CompositionController: {e}"));
    }
    wait_with_pump(rx, "CreateCoreWebView2CompositionController")?
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
