//! Native liquid-glass layer: a top-level window under the WebView that
//! renders the *real* desktop behind the panel as refracting glass.
//!
//! History: a transparent D3D11 *child* beneath the WebView presented
//! fine but never reached the screen (input worked, pixels didn't —
//! under flip, bitblt and GDI alike). The glass is a top-level window:
//! borderless, click-through, never activating, hidden from taskbar and
//! Alt-Tab, kept pixel-aligned below the main panel.
//!
//! Pipeline (one worker thread, all work on the GPU):
//!
//!   capture ─ DXGI Desktop Duplication (whole monitor)
//!          └─ CopySubresourceRegion → sharpTex   (panel rect only)
//!   frost   ─ separable Gaussian, H then V, at half resolution
//!   glass   ─ one pixel shader: rounded-rect bevel height field →
//!            normal → thin-lens refraction with a per-channel IOR
//!            (real dispersion), Fresnel rim light, key specular,
//!            grazing-path absorption (internal tint), chroma bleed,
//!            corner feathering and dither.
//!
//! Nothing about the pane is baked into a texture: the lens, the
//! highlights and the tint are evaluated per pixel from the live
//! backdrop, so the material bends whatever is actually behind it.
//!
//! Idle cost is zero. When the desktop produces no new frame *and* the
//! interaction springs have settled, the worker does no GPU work at
//! all — it just waits on the next capture.
//!
//! Refraction is enabled by default. Set `NEX_REFRACT_LAB=0` to force
//! the Acrylic fallback. Logs use the `[nex][refract]` prefix.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::overlay::host::UiCommand;
use tao::event_loop::EventLoopProxy;
use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET,
    D3D11_CREATE_DEVICE_DEBUG, D3D11_CREATE_DEVICE_FLAG,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_RENDER_TARGET_VIEW_DESC, D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_RASTERIZER_DESC,
    D3D11_SAMPLER_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
    ID3D11Buffer, ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader,
    ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_WAIT_TIMEOUT, DXGI_PRESENT,
    DXGI_SCALING, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT, DXGI_SWAP_EFFECT_DISCARD,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIFactory,
    IDXGIFactory2, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WDA_EXCLUDEFROMCAPTURE, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::{core::w, Win32::System::LibraryLoader::GetModuleHandleW};

/// Refraction is on by default; an explicit zero keeps the Acrylic fallback.
pub fn enabled() -> bool {
    std::env::var("NEX_REFRACT_LAB")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

// ── optics ────────────────────────────────────────────────────────────
// Lengths are device pixels taken from the 8 px corner-radius token the
// panel's CSS already uses, multiplied by the panel's scale factor so
// the material keeps its proportions at 125/150/200%.

/// Width of the rolled edge. The bevel profile peaks mid-band, so this
/// also sets how much of the pane counts as "near the edge".
const RIM_WIDTH: f64 = 30.0;
/// Peak sample displacement at the rim, in px.
const REFRACT_PX: f64 = 30.0;
/// Bevel height in px. Sets how steep the surface gets: the profile
/// spans RIM_WIDTH, so peak slope is BEVEL/(RIM_WIDTH/2) and the
/// displacement across the band is that slope times REFRACT_PX.
const BEVEL: f64 = 38.0;
/// Whole-pane convexity, so the flat middle still reads as solid glass.
const DOME: f32 = 0.35;
/// Frosted layer Gaussian sigma in px: wide enough to separate the
/// backdrop from the UI, tight enough to keep type legible through it.
const BLUR_SIGMA: f64 = 9.0;
/// How far beyond the pane the capture reaches, in px. Desktop
/// duplication renders our own (capture-excluded) windows as black, so
/// the pixels directly under the pane are unavailable by construction.
/// Capturing a margin and sampling into it is the only way to see real
/// desktop content through the material — sampling the pane's own rect
/// would give a feedback mirror of the panel above it.
const CROP_MARGIN: f64 = 120.0;
/// How far the pane's centre samples outward, in px: the interior
/// takes its backdrop from the surround, the way light entering a
/// window continues across it. Must stay under CROP_MARGIN minus the
/// rim bend or sampling walks off the captured region.
const PUSH_MAX: f64 = 64.0;
/// Extra outward offset at the rim itself, in px. The pane's own edge
/// is inside the excluded rectangle, so the rim has to reach past it
/// before it has anything real to bend.
const RIM_PUSH: f64 = 10.0;
/// Chroma bleed radius in px — how far background colour spreads along
/// the rim before the sharp layer takes over.
const BLEED_PX: f64 = 11.0;
/// Extra displacement concentrated into the outer band (1 = none).
const EDGE_BOOST: f32 = 1.55;
/// Per-channel IOR spread. 0.055 splits red/blue by ~10% of the total
/// bend at the rim, which is the part of dispersion the eye can see.
const DISPERSION: f32 = 0.055;
/// Pointer influence radius in px (Gaussian falloff).
const POINTER_SIGMA: f64 = 120.0;
/// Rim-light gain, and Blinn specular gain/power. The power is low on
/// purpose: surface normals here only sweep about 40°, so a tight
/// lobe would clip to a single blown pixel instead of a sheen.
const RIM_GAIN: f32 = 1.1;
const SPEC_GAIN: f32 = 0.60;
const SPEC_POWER: f32 = 18.0;
/// Key light direction on screen (y up): top-left, the convention every
/// UI gloss render uses.
const LIGHT: [f32; 2] = [-0.45, 0.72];
/// Saturation lift applied to whatever the glass transmits.
const SATURATION: f32 = 1.38;
/// Absorption per unit path length, per channel. Real glass eats red
/// first, so the long grazing paths at the rim read faintly cool.
const ABSORB: [f32; 3] = [0.021, 0.016, 0.013];
/// Output dither; breaks banding in the smooth falloffs.
const GRAIN: f32 = 1.6 / 255.0;

/// Render cap: live frames present at most this often.
const MIN_FRAME_MS: u128 = 16;

/// Host -> worker config. `main_sys == 0` and `scale <= 0.0` both mean
/// "unchanged".
struct GlassConfig {
    visible: bool,
    main_sys: isize,
    scale: f64,
}

/// Host-facing handle. Every method is a UI-thread window op plus a
/// config send; all D3D objects live on the worker.
pub struct GlassLayer {
    window: GlassWindow,
    tx: Sender<GlassConfig>,
    ready: Arc<AtomicBool>,
    /// Last visibility pushed to the worker. `sync_to_main` runs on every
    /// resize including while the panel is hidden, and must not use that
    /// to switch the capture loop back on.
    visible: AtomicBool,
}

struct GlassWindow {
    hwnd: HWND,
    scale: f64,
}

impl GlassLayer {
    /// Create the glass top-level window + worker. `None` when explicitly
    /// disabled or on any failure (warn-logged, caller falls back to
    /// Acrylic). Starts hidden; the host shows it with the panel.
    pub fn create_for_window(
        main_sys: isize,
        logical_w: f64,
        logical_h: f64,
        scale: f64,
        proxy: EventLoopProxy<UiCommand>,
    ) -> Option<GlassLayer> {
        if !enabled() {
            crate::logging::info("[nex][refract] disabled by environment - acrylic path");
            return None;
        }
        match GlassWindow::create(main_sys, logical_w, logical_h, scale) {
            Ok(window) => {
                // The main window's own DWM backdrop would blur our live
                // pixels along with the desktop (that's the acrylic wash
                // hiding the glass). Hand the background to the glass;
                // Acrylic stays as-is when this call fails or glass dies.
                disable_main_backdrop(main_sys);
                let (tx, rx) = channel();
                let ready = Arc::new(AtomicBool::new(false));
                let worker_ready = Arc::clone(&ready);
                // HWND is a raw pointer (not Send): move it as isize and
                // rebuild it on the worker.
                let hwnd_sys = window.hwnd().0 as isize;
                match std::thread::Builder::new()
                    .name("glass-capture".into())
                    .spawn(move || worker_main(hwnd_sys, main_sys, rx, proxy, worker_ready))
                {
                    Ok(_) => {
                        crate::logging::info("[nex][refract] capture worker spawned");
                        Some(GlassLayer {
                            window,
                            tx,
                            ready,
                            visible: AtomicBool::new(false),
                        })
                    }
                    Err(error) => {
                        crate::logging::warn(&format!(
                            "[nex][refract] disabled, acrylic fallback: worker spawn failed: {error}"
                        ));
                        None
                    }
                }
            }
            Err(error) => {
                crate::logging::warn(&format!("[nex][refract] disabled, acrylic fallback: {error}"));
                None
            }
        }
    }

    /// Align to the main window. Called on show/resize; carries the
    /// current visibility through so a resize while hidden does not
    /// restart capture for a pane nobody can see.
    pub fn sync_to_main(&self, main_sys: isize, scale: f64) {
        self.window.move_below(main_sys);
        let _ = self.tx.send(GlassConfig {
            visible: self.visible.load(Ordering::Acquire),
            main_sys,
            scale,
        });
    }

    /// Show/hide with the panel. No animation, no activation.
    ///
    /// The glass is shown unconditionally on show: the host keeps the
    /// panel itself hidden until the worker reports a real presented
    /// frame, so revealing the layer early costs nothing but a frame of
    /// latency — and holding it back would deadlock, since a hidden
    /// window is exactly the one DXGI refuses to present for.
    pub fn set_visible(&self, show: bool) {
        self.window.show(show);
        self.visible.store(show, Ordering::Release);
        let _ = self.tx.send(GlassConfig {
            visible: show,
            main_sys: 0,
            scale: 0.0,
        });
    }

    /// Reveal only after the renderer has presented a captured frame.
    pub fn show_ready_frame(&self) {
        if self.ready.load(Ordering::Acquire) {
            self.window.show(true);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

/// Hand the background to the glass: the main window's DWM backdrop
/// would otherwise blur our live pixels into the acrylic wash.
fn disable_main_backdrop(main_sys: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
    };
    let backdrop = DWMSBT_NONE;
    let hr = unsafe {
        DwmSetWindowAttribute(
            main_sys as *mut std::ffi::c_void,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&backdrop) as u32,
        )
    };
    if hr == 0 {
        crate::logging::info("[nex][refract] main DWM backdrop off (glass owns the background)");
    } else {
        crate::logging::warn(&format!(
            "[nex][refract] backdrop off failed: HRESULT {hr} (glass will wash out)"
        ));
    }
}

impl GlassWindow {
    fn create(
        main_sys: isize,
        logical_w: f64,
        logical_h: f64,
        scale: f64,
    ) -> Result<GlassWindow, String> {
        let instance: HMODULE =
            unsafe { GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e:?}"))? };
        let class = w!("NexGlassLayer");
        let wndclass = WNDCLASSW {
            style: windows::Win32::UI::WindowsAndMessaging::WNDCLASS_STYLES(0),
            lpfnWndProc: Some(glass_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance.into(),
            hIcon: windows::Win32::UI::WindowsAndMessaging::HICON(std::ptr::null_mut()),
            hCursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR(std::ptr::null_mut()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(std::ptr::null_mut()),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: class,
        };
        let atom = unsafe { RegisterClassW(&wndclass) };
        if atom == 0 {
            crate::logging::info(&format!(
                "[nex][refract] RegisterClassW returned 0, continuing (last error {})",
                last_error_code()
            ));
        }

        let main = HWND(main_sys as *mut std::ffi::c_void);
        let (x, y, width, height) = main_rect(main_sys)
            .map(|(x, y, w, h)| (x, y, w, h))
            .unwrap_or((0, 0, physical_px(logical_w, scale), physical_px(logical_h, scale)));

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class,
                w!("NexGlass"),
                WS_POPUP,
                x,
                y,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| {
                format!("CreateWindowExW glass failed: {e:?} (last error {})", last_error_code())
            })?
        };
        // Self-exclusion is mandatory, not optional: the glass window
        // sits directly under the panel, so a capture that includes
        // either one feeds the pane's own pixels back into itself and
        // the material smears into a feedback mirror. Both windows are
        // marked before the first capture is taken.
        for (label, target) in [("glass", hwnd), ("main", main)] {
            match unsafe { SetWindowDisplayAffinity(target, WDA_EXCLUDEFROMCAPTURE) } {
                Ok(()) => {
                    crate::logging::info(&format!("[nex][refract] {label} excluded from capture"))
                }
                Err(error) => crate::logging::warn(&format!(
                    "[nex][refract] {label} exclusion failed: {error:?} (feedback risk)"
                )),
            }
        }
        crate::logging::info(&format!(
            "[nex][refract] glass top-level hwnd={hwnd:?} {width}x{height}px at ({x},{y})"
        ));
        Ok(GlassWindow { hwnd, scale })
    }

    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Same rect as main, ordered directly below it, never activating.
    fn move_below(&self, main_sys: isize) {
        align_glass_to_main(self.hwnd, main_sys);
        if let Some((_, _, w, h)) = main_rect(main_sys) {
            self.apply_round_region(w, h);
        }
    }

    /// Clip to the panel's rounded corners. The shader also feathers the
    /// corner in alpha, but the only swapchain this box accepts is the
    /// bitblt one, whose backbuffer carries no alpha — so the window
    /// region is what actually shapes the pane here. Both use the same
    /// radius token as the page's `border-radius`, so they agree.
    fn apply_round_region(&self, w_px: u32, h_px: u32) {
        use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
        // Panel radius token is 8 logical px; the region wants diameters.
        let d = ((8.0 * self.scale * 2.0).round() as i32).max(2);
        let rgn =
            unsafe { CreateRoundRectRgn(0, 0, w_px as i32 + 1, h_px as i32 + 1, d, d) };
        if rgn.is_null() {
            return;
        }
        // Ownership passes to the OS on success; delete only on failure.
        if unsafe { SetWindowRgn(self.hwnd.0, rgn, 1) } == 0 {
            unsafe {
                DeleteObject(rgn);
            }
        }
    }

    fn show(&self, show: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if show { SW_SHOWNA } else { SW_HIDE });
        }
    }
}

// ── shaders ──────────────────────────────────────────────────────────

/// Fullscreen triangle from `SV_VertexID` — no vertex buffer, no input
/// layout, one draw call per pass.
const VS_HLSL: &str = r#"
struct VSOut { float4 pos : SV_Position; };
VSOut VSMain(uint vid : SV_VertexID) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    VSOut o;
    o.pos = float4(p * 2.0 - 1.0, 0.0, 1.0);
    return o;
}
"#;

/// Separable Gaussian. Taps ride at sigma/2 spacing so eight bilinear
/// samples cover +-2 sigma: each pair stands in for the two texels its
/// offset straddles, which is what keeps the tap count low without the
/// kernel looking banded.
const BLUR_HLSL: &str = r#"
cbuffer B : register(b0) {
    float4 axis;   // sigmaU, sigmaV, dirX, dirY
    float4 invOut; // 1/width, 1/height
};
Texture2D srcTex : register(t0);
SamplerState samp : register(s0);

float4 PSMain(float4 pos : SV_Position) : SV_Target {
    float2 uv  = pos.xy * invOut.xy;
    float2 dir = axis.zw;
    float sig  = (dir.x != 0.0) ? axis.x : axis.y;
    float3 acc = srcTex.Sample(samp, uv).rgb;
    float wsum = 1.0;
    [unroll]
    for (int k = 1; k <= 4; ++k) {
        float w = exp(-0.125 * k * k);          // g(k*sigma/2) = e^(-k^2/8)
        float2 o = dir * (sig * k * 0.5);
        acc += srcTex.Sample(samp, uv + o).rgb * w;
        acc += srcTex.Sample(samp, uv - o).rgb * w;
        wsum += 2.0 * w;
    }
    return float4(acc / wsum, 1.0);
}
"#;

/// The glass itself. Everything below derives from the live pixels in
/// `sharpTex` (the desktop) and `frostTex` (its blurred twin).
const GLASS_HLSL: &str = r#"
cbuffer C : register(b0) {
    float4 invOut;      // 1/cropW, 1/cropH, grain, unused
    float4 sizeRadius;  // paneW, paneH, corner radius, rim width
    float4 crop;        // marginX, marginY, pushMax, rimPush
    float4 state;       // hover, reveal, press, hasPointer
    float4 pointer;     // pointerX, pointerY, tiltX, pointerSigma
    float4 tiltRefract; // tiltY, refract px, bevel, dome
    float4 tintSat;     // absorbR, absorbG, absorbB, saturation
    float4 bleedEdge;   // bleed px, edge boost, dispersion, rim gain
    float4 lightSpec;   // lightX, lightY, spec gain, spec power
};
Texture2D sharpTex : register(t0);
Texture2D frostTex : register(t1);
SamplerState samp : register(s0);

static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);
static const float3 KEY_TINT  = float3(1.0, 0.985, 0.955);
static const float3 FILL_TINT = float3(0.86, 0.91, 1.0);

float sdRound(float2 p, float2 half_, float r) {
    float2 q = abs(p) - half_ + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

// Rolled edge: 0 at the rim, 1 across the flat middle. smoothstep puts
// the steepest slope mid-band, so the lens peaks just inside the cut
// instead of diverging on the outermost pixel.
float lensT(float2 p) {
    float d = -sdRound(p - sizeRadius.xy * 0.5, sizeRadius.xy * 0.5, sizeRadius.z);
    return smoothstep(0.0, sizeRadius.w, d);
}

float hash12(float2 p) {
    float3 p3 = frac(p.xyx * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return frac((p3.x + p3.y) * p3.z);
}

float4 PSMain(float4 pos : SV_Position) : SV_Target {
    float2 px = pos.xy;
    float2 half_ = sizeRadius.xy * 0.5;
    float2 rel   = px - half_;

    float sd   = sdRound(rel, half_, sizeRadius.z);
    float d    = -sd;                      // >0 inside, 0 at the cut
    float lens = lensT(px);
    float rim  = 1.0 - lens;               // 1 at the very edge
    float reveal = state.y;

    // The captured frame is the pane plus a margin, and the pane's own
    // rect is black in it (our windows are capture-excluded). So the
    // material reads its backdrop from the margin: every fetch is
    // offset outward, growing toward the centre, the way light crossing
    // a window keeps arriving from outside it. Without this the pane
    // samples its own excluded rectangle and either goes black or
    // mirrors the panel stacked above it.
    float2 outward = normalize(rel + float2(1e-4, 1e-4)) * (crop.z * lens + crop.w);
    float2 uv = (crop.xy + px + outward) * invOut.xy;

    // ── surface normal from the bevel height field ───────────────────
    float hx = lensT(px + float2(1.0, 0.0)) - lensT(px - float2(1.0, 0.0));
    float hy = lensT(px + float2(0.0, 1.0)) - lensT(px - float2(0.0, 1.0));
    float2 grad = float2(hx, hy) * (tiltRefract.z * 0.5);

    // Whole-pane convexity. Added with a minus so it slopes *inward*
    // like the bevel does, which is what makes the flat middle read as
    // a solid rather than a hole punched in the desktop.
    float rN = saturate(length(rel) / max(min(half_.x, half_.y), 1.0));
    grad -= normalize(rel + float2(1e-4, 0.0)) * rN * tiltRefract.w;

    // Pointer: a local swell plus a directional lean. The springs
    // upstream are critically damped, so this reads as liquid following
    // the cursor rather than as a UI animation.
    // Pointer response. This drives the highlight and a whisper of tilt
    // only: the gradient it feeds is multiplied by the same `strength`
    // that carries the rim bend, so anything larger reads as the whole
    // backdrop swimming when the cursor moves rather than as material
    // reacting to touch. Kept well under the bevel's own slope.
    float hover = 0.0;
    if (state.w > 0.5) {
        float2 toP = pointer.xy - px;
        float dist = length(toP);
        hover = exp(-(dist * dist) / (2.0 * pointer.w * pointer.w)) * state.x;
        grad += (toP / max(dist, 1.0)) * hover * (0.008 + 0.022 * state.z);
        grad += float2(pointer.z, tiltRefract.x) * hover * 0.006;
    }
    grad *= reveal;

    // Fold the screen's downward y back out: a convex bevel's normal
    // points away from the centre once you are looking at the surface.
    float2 n2 = float2(-grad.x, grad.y);
    float3 N  = normalize(float3(n2, 1.0));

    // ── refraction ───────────────────────────────────────────────────
    // Thin-slab limit: the apparent shift of the backdrop is the surface
    // tilt times (n - 1). Offsets are computed in pane pixels and
    // converted to the crop's uv, which is the space `uv` lives in.
    float edge    = bleedEdge.y * rim * rim;
    float strength = tiltRefract.y * (0.18 + 0.82 * edge);
    float2 oG = (-n2 * strength) * invOut.xy;
    float2 oR = oG * (1.0 - bleedEdge.z);
    float2 oB = oG * (1.0 + bleedEdge.z);

    float3 frost = frostTex.Sample(samp, uv + oG * 0.35).rgb;

    // Chroma bleed: a wide, low-weight average of the frost layer. The
    // positive deviation carries bright background colour outward along
    // the rim without lifting the whole pane.
    float2 bs = float2(bleedEdge.x * invOut.x, bleedEdge.x * invOut.y);
    float3 halo =
        (frostTex.Sample(samp, uv + oG * 0.35 + float2( bs.x, 0.0)).rgb +
         frostTex.Sample(samp, uv + oG * 0.35 + float2(-bs.x, 0.0)).rgb +
         frostTex.Sample(samp, uv + oG * 0.35 + float2(0.0,  bs.y)).rgb +
         frostTex.Sample(samp, uv + oG * 0.35 + float2(0.0, -bs.y)).rgb) * 0.25;
    frost += max(halo - frost, 0.0) * (0.35 * rim);

    // Per-channel fetch is real dispersion: red bends least, blue most.
    float3 sharp;
    sharp.r = sharpTex.Sample(samp, uv + oR).r;
    sharp.g = sharpTex.Sample(samp, uv + oG).g;
    sharp.b = sharpTex.Sample(samp, uv + oB).b;

    // Frost across the flat middle, refracted detail at the rim. The
    // centre leans on frost because the sharp fetch there would be a
    // magnified copy of one point; the rim wants the sharp layer so the
    // bent edge stays crisp and coloured.
    float mixSharp = saturate(0.06 + 0.88 * rim * rim);
    float3 col = lerp(frost, sharp, mixSharp);

    // ── colour ───────────────────────────────────────────────────────
    float lum = dot(col, LUMA);
    col = lerp(lum.xxx, col, tintSat.w);

    // Internal tint. A curved surface turns the view ray away from
    // normal, so the path through the glass lengthens; absorption (which
    // eats red first) then shows up as a cool, denser edge.
    float path = 1.0 / max(N.z, 0.30);
    col *= exp(-tintSat.xyz * (tiltRefract.z * (path - 1.0)));

    // ── light ────────────────────────────────────────────────────────
    float fres = pow(1.0 - saturate(N.z), 2.0);
    float2 nOut = normalize(n2 + float2(1e-5, 1e-5));
    float2 L = normalize(lightSpec.xy);
    // Half vector for a viewer straight ahead: H is (L + V) normalized.
    float3 H = normalize(float3(L.x, L.y, 2.0));

    float spec  = pow(saturate(dot(N, H)), lightSpec.w) * lightSpec.z;
    float lit   = fres * (0.30 + 1.05 * saturate(dot(nOut, L))) * rim;
    float bounce = fres * (0.30 + 1.05 * saturate(dot(-nOut, L))) * rim;

    col += lit * bleedEdge.w * KEY_TINT;
    col += bounce * bleedEdge.w * 0.30 * FILL_TINT;
    col += spec * KEY_TINT;

    // Pressing swells the surface under the pointer; a hair of lift
    // keeps the press legible without turning into a glow.
    col += hover * state.z * float3(0.05, 0.055, 0.06);

    // ── output ───────────────────────────────────────────────────────
    // Corners are shaped by the window region (radius token shared
    // with the page's border-radius), which every swapchain this can
    // fall back to honours — so the pane itself is simply opaque.
    col = saturate(col) + (hash12(px) - 0.5) * invOut.z;
    return float4(col, 1.0);
}
"#;

// `d3dcompiler_47.dll` ships with Windows, so the shaders compile at
// run time instead of dragging the Windows SDK's build toolchain into
// the build graph. Shader model 5.0 is the lowest level every Windows
// 10+ GPU supports.
//
// Argument order and types are d3dcompiler.h's exactly: two separate
// flag words, and ANSI string arguments (not wide).
#[link(name = "d3dcompiler")]
unsafe extern "system" {
    fn D3DCompile(
        src: *const std::ffi::c_void,
        src_len: usize,
        source_name: *const u8,
        defines: *const u8,
        include: *const std::ffi::c_void,
        entry: *const u8,
        target: *const u8,
        flags1: u32,
        flags2: u32,
        code: *mut *mut std::ffi::c_void,
        errors: *mut *mut std::ffi::c_void,
    ) -> i32;
}

const D3DCOMPILE_OPTIMIZATION_LEVEL3: u32 = 0x1000;
const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 0x0002;

/// `entry` must exist in `source`; `profile` is the shader stage, which
/// D3DCompile validates against the entry point's return semantic.
fn compile(source: &str, entry: &str, profile: &str) -> Result<Vec<u8>, String> {
    let wide_entry: Vec<u8> = entry.bytes().chain(std::iter::once(0)).collect();
    let wide_profile: Vec<u8> = profile.bytes().chain(std::iter::once(0)).collect();
    let mut code: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut errors: *mut std::ffi::c_void = std::ptr::null_mut();
    let hr = unsafe {
        D3DCompile(
            source.as_ptr() as *const std::ffi::c_void,
            source.len(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            wide_entry.as_ptr(),
            wide_profile.as_ptr(),
            D3DCOMPILE_OPTIMIZATION_LEVEL3 | D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut code,
            &mut errors,
        )
    };
    if !errors.is_null() {
        let blob = unsafe { <ID3DBlob as Interface>::from_raw(errors) };
        let text = unsafe {
            let ptr = blob.GetBufferPointer();
            let len = blob.GetBufferSize();
            String::from_utf8_lossy(std::slice::from_raw_parts(ptr as *const u8, len))
                .trim()
                .to_string()
        };
        if !text.is_empty() {
            return Err(format!("{profile} {entry}: {text}"));
        }
    }
    if hr != 0 {
        return Err(format!("{profile} {entry}: D3DCompile failed 0x{hr:08X}"));
    }
    let blob = unsafe { <ID3DBlob as Interface>::from_raw(code) };
    let bytes = unsafe {
        let ptr = blob.GetBufferPointer();
        let len = blob.GetBufferSize();
        std::slice::from_raw_parts(ptr as *const u8, len).to_vec()
    };
    Ok(bytes)
}

// ── engine ───────────────────────────────────────────────────────────

/// A texture plus the views the passes bind to it. `rtv` is absent for
/// the capture texture, which is only ever read.
struct Target {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    rtv: Option<ID3D11RenderTargetView>,
}

fn make_target(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    render_target: bool,
    format: DXGI_FORMAT,
) -> Result<Target, String> {
    let width = width.max(1);
    let height = height.max(1);
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32
                        | if render_target {
                            D3D11_BIND_RENDER_TARGET.0 as u32
                        } else {
                            0
                        },
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                },
                None,
                Some(&mut tex as *mut _),
            )
            .map_err(|e| format!("CreateTexture2D {width}x{height} failed: {e:?}"))?;
    }
    let tex = tex.ok_or("CreateTexture2D returned no texture")?;
    // A null descriptor is legal for single-mip, single-sample textures
    // and infers exactly the view wanted here — one less union to get
    // wrong, and the same call works for both views.
    let mut srv: Option<ID3D11ShaderResourceView> = None;
    unsafe {
        device
            .CreateShaderResourceView(
                &tex,
                None::<*const D3D11_SHADER_RESOURCE_VIEW_DESC>,
                Some(&mut srv as *mut _),
            )
            .map_err(|e| format!("CreateShaderResourceView failed: {e:?}"))?;
    }
    let srv = srv.ok_or("CreateShaderResourceView returned nothing")?;
    let rtv = if render_target {
        let mut view: Option<ID3D11RenderTargetView> = None;
        unsafe {
            device
                .CreateRenderTargetView(
                    &tex,
                    None::<*const D3D11_RENDER_TARGET_VIEW_DESC>,
                    Some(&mut view as *mut _),
                )
                .map_err(|e| format!("CreateRenderTargetView failed: {e:?}"))?;
        }
        Some(view.ok_or("CreateRenderTargetView returned nothing")?)
    } else {
        None
    };
    Ok(Target { tex, srv, rtv })
}

fn make_constant(device: &ID3D11Device) -> Result<ID3D11Buffer, String> {
    let mut buffer: Option<ID3D11Buffer> = None;
    unsafe {
        device
            .CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 256,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                    StructureByteStride: 0,
                },
                None,
                Some(&mut buffer as *mut _),
            )
            .map_err(|e| format!("constant CreateBuffer failed: {e:?}"))?;
    }
    buffer.ok_or_else(|| "constant CreateBuffer returned nothing".to_string())
}

/// Constant-buffer scratch. Every cbuffer member is a float4, so the
/// layout is trivially 16-byte aligned rows.
fn write_params(buf: &mut [u8], rows: &[[f32; 4]]) {
    for (i, row) in rows.iter().enumerate() {
        let off = i * 16;
        for (j, value) in row.iter().enumerate() {
            buf[off + j * 4..off + j * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
    }
}

/// Worker-owned renderer: every D3D object. Prewarmed while hidden and
/// rebuilt on size change.
struct Engine {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain1,
    back: Option<ID3D11RenderTargetView>,
    back_tex: Option<ID3D11Texture2D>,
    sharp: Target,
    frost_a: Target,
    frost_b: Target,
    vs: ID3D11VertexShader,
    ps_blur: ID3D11PixelShader,
    ps_glass: ID3D11PixelShader,
    samp: ID3D11SamplerState,
    raster: ID3D11RasterizerState,
    cb_blur_h: ID3D11Buffer,
    cb_blur_v: ID3D11Buffer,
    cb_glass: ID3D11Buffer,
    glass_params: [u8; 256],
    scale: f64,
    width: u32,
    height: u32,
    /// Capture margin around the pane, in device px. See CROP_MARGIN:
    /// the pane's own rect is black in a duplication frame because both
    /// of our windows are capture-excluded, so the material has to be
    /// fed from the surround.
    margin: u32,
    /// Format of the duplicated desktop frames. `sharp` must match it
    /// exactly: CopySubresourceRegion refuses to convert, and it fails
    /// quietly rather than raising an error.
    source_format: DXGI_FORMAT,
    /// Frames drawn; drives the readback probe cadence.
    frame: u64,
    /// NEX_REFRACT_DEBUG=probe: log real pixels out of each pass.
    debug: bool,
}

impl Engine {
    /// Build everything for a panel size: device, swapchain (first
    /// working desc wins), shaders and size-dependent targets.
    fn build(hwnd: HWND, width: u32, height: u32, scale: f64) -> Result<Engine, String> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;
        // D3D11 debug layer, opt-in: it names every invalid state call
        // instead of silently producing a black pane, but it needs the
        // SDK gear DLL, which most installs do not ship.
        let debug = std::env::var("NEX_REFRACT_DEBUG")
            .map(|v| matches!(v.trim(), "1" | "layer"))
            .unwrap_or(false);
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT
                    | if debug {
                        D3D11_CREATE_DEVICE_DEBUG
                    } else {
                        D3D11_CREATE_DEVICE_FLAG(0)
                    },
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device as *mut _),
                Some(&mut feature_level as *mut _),
                Some(&mut context as *mut _),
            )
            .map_err(|e| format!("D3D11CreateDevice failed: {e:?}"))?;
        }
        let device = device.ok_or("D3D11CreateDevice returned no device")?;
        let context = context.ok_or("D3D11CreateDevice returned no context")?;

        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
            .map_err(|e| format!("CreateDXGIFactory2 failed: {e:?}"))?;
        let mut desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let variants: [(&str, DXGI_SCALING, DXGI_ALPHA_MODE, u32, DXGI_SWAP_EFFECT); 4] = [
            (
                "premultiplied+stretch",
                DXGI_SCALING_STRETCH,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
                2,
                DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ),
            (
                "premultiplied+none",
                DXGI_SCALING_NONE,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
                2,
                DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ),
            (
                "bitblt+discard",
                DXGI_SCALING_STRETCH,
                DXGI_ALPHA_MODE_UNSPECIFIED,
                1,
                DXGI_SWAP_EFFECT_DISCARD,
            ),
            (
                "unspecified+stretch",
                DXGI_SCALING_STRETCH,
                DXGI_ALPHA_MODE_UNSPECIFIED,
                2,
                DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ),
        ];
        let mut swapchain: Option<IDXGISwapChain1> = None;
        for (name, scaling, alpha, buffer_count, effect) in variants {
            desc.Scaling = scaling;
            desc.AlphaMode = alpha;
            desc.BufferCount = buffer_count;
            desc.SwapEffect = effect;
            match unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) } {
                Ok(chain) => {
                    crate::logging::info(&format!("[nex][refract] swapchain ok via {name}"));
                    swapchain = Some(chain);
                    break;
                }
                Err(error) => crate::logging::warn(&format!(
                    "[nex][refract] swapchain {name} failed: {error:?}"
                )),
            }
        }
        let swapchain = swapchain.ok_or("all swapchain desc variants failed")?;

        let vs_bytecode = compile(VS_HLSL, "VSMain", "vs_5_0")?;
        let blur_bytecode = compile(BLUR_HLSL, "PSMain", "ps_5_0")?;
        let glass_bytecode = compile(GLASS_HLSL, "PSMain", "ps_5_0")?;
        let mut vs: Option<ID3D11VertexShader> = None;
        let mut ps_blur: Option<ID3D11PixelShader> = None;
        let mut ps_glass: Option<ID3D11PixelShader> = None;
        unsafe {
            device
                .CreateVertexShader(&vs_bytecode, None, Some(&mut vs as *mut _))
                .map_err(|e| format!("CreateVertexShader failed: {e:?}"))?;
            device
                .CreatePixelShader(&blur_bytecode, None, Some(&mut ps_blur as *mut _))
                .map_err(|e| format!("CreatePixelShader(blur) failed: {e:?}"))?;
            device
                .CreatePixelShader(&glass_bytecode, None, Some(&mut ps_glass as *mut _))
                .map_err(|e| format!("CreatePixelShader(glass) failed: {e:?}"))?;
        }
        let (vs, ps_blur, ps_glass) = (
            vs.ok_or("no vertex shader")?,
            ps_blur.ok_or("no blur pixel shader")?,
            ps_glass.ok_or("no glass pixel shader")?,
        );

        let mut samp: Option<ID3D11SamplerState> = None;
        unsafe {
            device
                .CreateSamplerState(
                    &D3D11_SAMPLER_DESC {
                        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                        MipLODBias: 0.0,
                        MaxAnisotropy: 1,
                        ComparisonFunc: D3D11_COMPARISON_NEVER,
                        BorderColor: [0.0; 4],
                        MinLOD: 0.0,
                        MaxLOD: 0.0,
                    },
                    Some(&mut samp as *mut _),
                )
                .map_err(|e| format!("CreateSamplerState failed: {e:?}"))?;
        }
        let samp = samp.ok_or("CreateSamplerState returned nothing")?;

        // Explicit rasterizer state. The D3D11 default culls back faces,
        // and the winding of the SV_VertexID fullscreen triangle is
        // sensitive to that; CULL_NONE makes the pass independent of it.
        let mut raster: Option<ID3D11RasterizerState> = None;
        unsafe {
            device
                .CreateRasterizerState(
                    &D3D11_RASTERIZER_DESC {
                        FillMode: D3D11_FILL_SOLID,
                        CullMode: D3D11_CULL_NONE,
                        FrontCounterClockwise: BOOL(0),
                        DepthBias: 0,
                        DepthBiasClamp: 0.0,
                        SlopeScaledDepthBias: 0.0,
                        DepthClipEnable: BOOL(1),
                        ScissorEnable: BOOL(0),
                        MultisampleEnable: BOOL(0),
                        AntialiasedLineEnable: BOOL(0),
                    },
                    Some(&mut raster as *mut _),
                )
                .map_err(|e| format!("CreateRasterizerState failed: {e:?}"))?;
        }
        let raster = raster.ok_or("CreateRasterizerState returned nothing")?;

        // Built at 1x1 and replaced by rebuild_content with the real
        // panel size; `back` is filled in there too, and `sharp` is
        // re-made by set_source_format once the capture reports the
        // desktop's actual pixel format.
        let sharp = make_target(&device, 1, 1, false, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        let frost_a = make_target(&device, 1, 1, true, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        let frost_b = make_target(&device, 1, 1, true, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        let cb_blur_h = make_constant(&device)?;
        let cb_blur_v = make_constant(&device)?;
        let cb_glass = make_constant(&device)?;
        let mut engine = Engine {
            device,
            context,
            swapchain,
            back: None,
            back_tex: None,
            sharp,
            frost_a,
            frost_b,
            vs,
            ps_blur,
            ps_glass,
            samp,
            raster,
            cb_blur_h,
            cb_blur_v,
            cb_glass,
            glass_params: [0u8; 256],
            scale,
            width,
            height,
            margin: (CROP_MARGIN * scale).round() as u32,
            source_format: DXGI_FORMAT_B8G8R8A8_UNORM,
            frame: 0,
            debug: std::env::var("NEX_REFRACT_DEBUG")
                .map(|v| matches!(v.trim(), "probe" | "1" | "layer"))
                .unwrap_or(false),
        };
        engine.rebuild_content()?;
        let rim = RIM_WIDTH * scale;
        let refract = REFRACT_PX * scale;
        let sigma = BLUR_SIGMA * scale;
        crate::logging::info(&format!(
            "[nex][refract] glass ready {width}x{height}px scale={scale:.2} rim={rim:.0}px refract={refract:.0}px sigma={sigma:.1}px"
        ));
        Ok(engine)
    }

    /// Rebuild the size-dependent targets, the backbuffer view and the
    /// blur constants.
    fn rebuild_content(&mut self) -> Result<(), String> {
        // Capture the pane plus a margin of real desktop around it.
        self.sharp = make_target(
            &self.device,
            self.crop_width(),
            self.crop_height(),
            false,
            self.source_format,
        )?;
        // Frost runs at half resolution: it is a blur, so the extra
        // samples buy nothing, and bilinear upsampling reads softer than
        // full resolution would - which is what a frosted layer wants.
        let hw = self.crop_width().div_ceil(2).max(1);
        let hh = self.crop_height().div_ceil(2).max(1);
        self.frost_a = make_target(&self.device, hw, hh, true, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        self.frost_b = make_target(&self.device, hw, hh, true, DXGI_FORMAT_B8G8R8A8_UNORM)?;

        let texture: ID3D11Texture2D = unsafe {
            self.swapchain
                .GetBuffer(0)
                .map_err(|e| format!("swapchain GetBuffer failed: {e:?}"))?
        };
        let mut back: Option<ID3D11RenderTargetView> = None;
        unsafe {
            self.device
                .CreateRenderTargetView(
                    &texture,
                    None::<*const D3D11_RENDER_TARGET_VIEW_DESC>,
                    Some(&mut back as *mut _),
                )
                .map_err(|e| format!("backbuffer view failed: {e:?}"))?;
        }
        self.back = back;
        self.back_tex = Some(texture);

        // Blur runs over the capture crop, not the pane, so its sigma is
        // expressed in the crop's uv.
        let sigma = (BLUR_SIGMA * self.scale) as f32;
        for (buffer, dir) in [
            (self.cb_blur_h.clone(), (1.0f32, 0.0f32)),
            (self.cb_blur_v.clone(), (0.0f32, 1.0f32)),
        ] {
            let mut params = [0u8; 256];
            write_params(
                &mut params,
                &[
                    [
                        sigma / self.crop_width() as f32,
                        sigma / self.crop_height() as f32,
                        dir.0,
                        dir.1,
                    ],
                    [1.0 / hw as f32, 1.0 / hh as f32, 0.0, 0.0],
                ],
            );
            unsafe {
                self.context.UpdateSubresource(
                    &buffer,
                    0,
                    None,
                    params.as_ptr() as *const std::ffi::c_void,
                    0,
                    0,
                );
            }
        }
        Ok(())
    }

    /// Log a horizontal strip of real pixels out of `tex`. This is the
    /// one place a black pane can be traced to its cause: black in the
    /// duplication texture means Windows really did hand us a black
    /// frame, black only after the copy means the copy is at fault,
    /// black only in the output means the passes are.
    fn read_pixels(
        &self,
        tex: &ID3D11Texture2D,
        format: DXGI_FORMAT,
        box_: D3D11_BOX,
    ) -> Option<String> {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };
        const N: u32 = 10;
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            let _ = self.device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: N,
                    Height: 1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                },
                None,
                Some(&mut staging as *mut _),
            );
        }
        let staging = staging?;
        unsafe {
            self.context.CopySubresourceRegion(
                &staging,
                0,
                0,
                0,
                0,
                tex,
                0,
                Some(&box_ as *const _),
            );
        }
        let mut mapped: D3D11_MAPPED_SUBRESOURCE = unsafe { std::mem::zeroed() };
        if unsafe { self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() {
            return None;
        }
        let pixels = unsafe {
            let p = mapped.pData as *const u8;
            let text = (0..N)
                .map(|i| {
                    format!(
                        "{:02x}{:02x}{:02x}",
                        *p.add(i as usize * 4 + 2),
                        *p.add(i as usize * 4 + 1),
                        *p.add(i as usize * 4)
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            self.context.Unmap(&staging, 0);
            text
        };
        Some(pixels)
    }

    /// Probe the whole chain in one go: duplication frame → capture
    /// texture → finished composite.
    fn probe(&self, acquired: &ID3D11Texture2D, region: D3D11_BOX) {
        let (hw, hh) = (
            self.width.div_ceil(2).max(1),
            self.height.div_ceil(2).max(1),
        );
        let mid = (self.height / 2).min(hh.saturating_sub(1));
        let local = D3D11_BOX {
            left: 0,
            top: mid,
            right: 10.min(self.width),
            bottom: mid + 1,
            front: 0,
            back: 1,
        };
        let local_frost = D3D11_BOX {
            left: 0,
            top: mid,
            right: 10.min(hw),
            bottom: mid + 1,
            front: 0,
            back: 1,
        };
        let remote = D3D11_BOX {
            left: region.left,
            top: region.top + mid,
            right: region.left + 10.min(region.right - region.left),
            bottom: region.top + mid + 1,
            front: 0,
            back: 1,
        };
        let dup = self.read_pixels(acquired, self.source_format, remote).unwrap_or_default();
        let cap = self.read_pixels(&self.sharp.tex, self.source_format, local).unwrap_or_default();
        let frost = self
            .read_pixels(&self.frost_b.tex, DXGI_FORMAT_B8G8R8A8_UNORM, local_frost)
            .unwrap_or_default();
        let out = self
            .back_tex
            .as_ref()
            .and_then(|t| self.read_pixels(t, DXGI_FORMAT_B8G8R8A8_UNORM, local))
            .unwrap_or_default();
        crate::logging::info(&format!(
            "[nex][refract] probe dup[{dup}] cap[{cap}] frost[{frost}] out[{out}]"
        ));
    }

    /// Point the capture texture at the desktop's real pixel format.
    /// No-ops once it already matches, so the per-frame call site can
    /// stay unconditional.
    fn set_source_format(&mut self, format: DXGI_FORMAT) -> Result<(), String> {
        if self.source_format == format {
            return Ok(());
        }
        crate::logging::info(&format!(
            "[nex][refract] capture format {:?} -> {:?}",
            self.source_format, format
        ));
        self.source_format = format;
        self.sharp = make_target(
            &self.device,
            self.crop_width(),
            self.crop_height(),
            false,
            format,
        )?;
        Ok(())
    }

    /// Size of the captured region: pane plus margin on both sides.
    fn crop_width(&self) -> u32 {
        self.width + self.margin * 2
    }

    fn crop_height(&self) -> u32 {
        self.height + self.margin * 2
    }

    /// Crop one captured monitor frame down to the panel, then draw it
    /// as glass. The copy is GPU-to-GPU straight out of the duplication
    /// texture: no staging readback, no per-frame allocation, and the
    /// CPU never touches a pixel.
    fn render(
        &mut self,
        acquired: &ID3D11Texture2D,
        region: D3D11_BOX,
        motion: &Motion,
        pointer: (f32, f32),
        pointer_in: bool,
    ) -> Result<Frame, String> {
        unsafe {
            self.context.CopySubresourceRegion(
                &self.sharp.tex,
                0,
                0,
                0,
                0,
                acquired,
                0,
                Some(&region as *const _),
            );
        }
        self.draws(motion, pointer, pointer_in);
        self.frame += 1;
        // Diagnostic readback (NEX_REFRACT_DEBUG=probe). The interesting
        // states are "starting up hidden" and "on screen", not every
        // frame, and the sync readback is far too slow to run always.
        if self.debug && (self.frame <= 3 || self.frame % 150 == 0) {
            self.probe(acquired, region);
        }
        self.present()
    }

    /// Frost + glass over whatever crop is currently resident, with no
    /// present. Split out so the caller can inspect the finished frame
    /// before it reaches the screen — a DISCARD swapchain leaves the
    /// backbuffer undefined the moment Present returns.
    fn draws(&mut self, motion: &Motion, pointer: (f32, f32), pointer_in: bool) {
        let (w, h, scale) = (self.width, self.height, self.scale);
        let (hw, hh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
        // Every optical constant is scaled here and nowhere else.
        let mut params = self.glass_params;
        write_params(
            &mut params,
            &[
                [
                    1.0 / self.crop_width() as f32,
                    1.0 / self.crop_height() as f32,
                    GRAIN,
                    0.0,
                ],
                [w as f32, h as f32, 8.0 * scale as f32, (RIM_WIDTH * scale) as f32],
                [
                    self.margin as f32,
                    self.margin as f32,
                    (PUSH_MAX * scale) as f32,
                    (RIM_PUSH * scale) as f32,
                ],
                [motion.hover, motion.reveal, motion.press, f32::from(pointer_in)],
                [
                    pointer.0,
                    pointer.1,
                    motion.tilt.0,
                    (POINTER_SIGMA * scale) as f32,
                ],
                [motion.tilt.1, (REFRACT_PX * scale) as f32, (BEVEL * scale) as f32, DOME],
                [ABSORB[0], ABSORB[1], ABSORB[2], SATURATION],
                [
                    (BLEED_PX * scale) as f32,
                    EDGE_BOOST,
                    DISPERSION,
                    RIM_GAIN,
                ],
                [LIGHT[0], LIGHT[1], SPEC_GAIN, SPEC_POWER],
            ],
        );
        // The context is a deferred queue, so the constant upload is
        // recorded ahead of the draws that read it.
        unsafe {
            self.context.UpdateSubresource(
                &self.cb_glass,
                0,
                None,
                params.as_ptr() as *const std::ffi::c_void,
                0,
                0,
            );
            self.context.VSSetShader(&self.vs, None);
            self.context.PSSetSamplers(0, Some(&[Some(self.samp.clone())]));
            self.context.IASetInputLayout(None);
            self.context.RSSetState(&self.raster);

            self.context.PSSetShader(&self.ps_blur, None);
            // D3D11 has no usable default viewport: leaving it unset
            // rasterizes nothing at all, which shows up as a pane that
            // is uniformly black rather than as an obvious error.
            self.set_viewport(hw, hh);
            self.context.OMSetRenderTargets(
                Some(&[self.frost_a.rtv.clone()]),
                None::<&ID3D11DepthStencilView>,
            );
            self.context.PSSetConstantBuffers(0, Some(&[Some(self.cb_blur_h.clone())]));
            self.context.PSSetShaderResources(0, Some(&[Some(self.sharp.srv.clone())]));
            self.context.Draw(3, 0);

            self.set_viewport(hw, hh);
            self.context.OMSetRenderTargets(
                Some(&[self.frost_b.rtv.clone()]),
                None::<&ID3D11DepthStencilView>,
            );
            self.context.PSSetConstantBuffers(0, Some(&[Some(self.cb_blur_v.clone())]));
            self.context.PSSetShaderResources(0, Some(&[Some(self.frost_a.srv.clone())]));
            self.context.Draw(3, 0);

            self.context.PSSetShader(&self.ps_glass, None);
            // The pane is the viewport; the crop only feeds its samplers.
            self.set_viewport(w, h);
            self.context.OMSetRenderTargets(
                Some(&[self.back.clone()]),
                None::<&ID3D11DepthStencilView>,
            );
            self.context.PSSetConstantBuffers(0, Some(&[Some(self.cb_glass.clone())]));
            self.context.PSSetShaderResources(
                0,
                Some(&[Some(self.sharp.srv.clone()), Some(self.frost_b.srv.clone())]),
            );
            self.context.Draw(3, 0);
        }
    }

    fn set_viewport(&self, width: u32, height: u32) {
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width as f32,
            Height: height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe { self.context.RSSetViewports(Some(&[viewport])) };
    }

    /// Present the frame. Present(0) skips the vsync wait, which in a
    /// composed window otherwise throttles the glass to ~20 FPS when the
    /// desktop is also presenting video.
    fn present(&self) -> Result<Frame, String> {
        let hr = unsafe { self.swapchain.Present(0, DXGI_PRESENT(0)) };
        if hr.is_ok() {
            Ok(Frame::Presented)
        } else if hr == DXGI_ERROR_DEVICE_SUSPENDED_HR {
            Ok(Frame::Skipped)
        } else {
            Err(format!("Present failed: 0x{:08X}", hr.0 as u32))
        }
    }

    fn compose(
        &mut self,
        motion: &Motion,
        pointer: (f32, f32),
        pointer_in: bool,
    ) -> Result<Frame, String> {
        self.draws(motion, pointer, pointer_in);
        self.present()
    }

    /// ResizeBuffers + content rebuild for a new panel size.
    fn resize(&mut self, width: u32, height: u32, scale: f64) -> Result<(), String> {
        // Both references have to go first: DXGI refuses to resize while
        // anything still points at the backbuffer, and it reports that
        // refusal as a generic invalid-call rather than saying why.
        self.back = None;
        self.back_tex = None;
        unsafe { self.context.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>) };
        unsafe {
            self.swapchain
                .ResizeBuffers(0, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))
        }
        .map_err(|e| format!("ResizeBuffers failed: {e:?}"))?;
        self.width = width;
        self.height = height;
        self.scale = scale;
        self.rebuild_content()
    }
}

// ── interaction ──────────────────────────────────────────────────────

/// Critically damped follow. No overshoot, no oscillation: liquid
/// settles rather than bounces, which is the whole difference between
/// this and a generic spring animation.
fn spring(value: &mut f32, velocity: &mut f32, target: f32, dt: f32, omega: f32) {
    let accel = -2.0 * omega * *velocity - omega * omega * (*value - target);
    *velocity += accel * dt;
    *value += *velocity * dt;
}

/// Pointer tilt, press and reveal. `reveal` opens partway so the first
/// visible frame is already materialised instead of a flat frost that
/// pops into a lens.
#[derive(Clone, Copy)]
struct Motion {
    reveal: f32,
    reveal_v: f32,
    press: f32,
    press_v: f32,
    hover: f32,
    hover_v: f32,
    tilt: (f32, f32),
    tilt_v: (f32, f32),
}

impl Motion {
    fn new() -> Motion {
        Motion {
            reveal: 0.35,
            reveal_v: 0.0,
            press: 0.0,
            press_v: 0.0,
            hover: 0.0,
            hover_v: 0.0,
            tilt: (0.0, 0.0),
            tilt_v: (0.0, 0.0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        dt: f32,
        visible: bool,
        pointer_in: bool,
        pointer: (f32, f32),
        down: bool,
        size: (f32, f32),
    ) {
        // A stalled frame (host hitch, debugger break) must not blow the
        // integrator up.
        let dt = dt.clamp(0.0, 1.0 / 30.0);
        spring(
            &mut self.reveal,
            &mut self.reveal_v,
            if visible { 1.0 } else { 0.0 },
            dt,
            14.0,
        );
        spring(
            &mut self.press,
            &mut self.press_v,
            if pointer_in && down { 1.0 } else { 0.0 },
            dt,
            24.0,
        );
        spring(
            &mut self.hover,
            &mut self.hover_v,
            if pointer_in { 1.0 } else { 0.0 },
            dt,
            12.0,
        );
        let target = if pointer_in {
            (
                ((pointer.0 - size.0 * 0.5) / 110.0).clamp(-1.0, 1.0),
                ((pointer.1 - size.1 * 0.5) / 110.0).clamp(-1.0, 1.0),
            )
        } else {
            (0.0, 0.0)
        };
        spring(&mut self.tilt.0, &mut self.tilt_v.0, target.0, dt, 10.0);
        spring(&mut self.tilt.1, &mut self.tilt_v.1, target.1, dt, 10.0);
    }

    /// True once nothing is moving, so the worker can skip the GPU.
    fn settled(&self, visible: bool, pointer_in: bool, down: bool) -> bool {
        let still = |v: f32, target: f32| (v - target).abs() < 0.002;
        still(self.reveal, if visible { 1.0 } else { 0.0 })
            && still(self.press, if pointer_in && down { 1.0 } else { 0.0 })
            && still(self.hover, if pointer_in { 1.0 } else { 0.0 })
            && self.tilt_v.0.abs() < 0.002
            && self.tilt_v.1.abs() < 0.002
    }

    /// Snap to rest — used when the panel hides so the next show starts
    /// from a settled material instead of mid-spring.
    fn settle(&mut self) {
        self.reveal = 1.0;
        self.reveal_v = 0.0;
        self.press = 0.0;
        self.press_v = 0.0;
        self.hover = 0.0;
        self.hover_v = 0.0;
        self.tilt = (0.0, 0.0);
        self.tilt_v = (0.0, 0.0);
    }
}

// ── capture ──────────────────────────────────────────────────────────

/// Why a present did or did not reach the screen.
enum Frame {
    Presented,
    /// The window is not on screen (hidden, or the compositor parked
    /// the device). Not an error — the next capture will retry.
    Skipped,
}

/// Not projected by the windows crate; the numeric value is stable and
/// public in dxgi.h.
const DXGI_ERROR_DEVICE_SUSPENDED_HR: windows::core::HRESULT =
    windows::core::HRESULT(0x887A_0005u32 as i32);

/// Worker-owned capture: a duplication session for one monitor.
struct Capture {
    duplication: IDXGIOutputDuplication,
    frame_w: u32,
    frame_h: u32,
    /// Pixel format of the duplicated frames.
    format: DXGI_FORMAT,
    origin_x: i32,
    origin_y: i32,
    monitor: HMONITOR,
    protected_logged: bool,
}

/// Cursor position and left-button state. Read straight from the OS
/// rather than routed over IPC: the glass window is click-through and
/// the panel owns the mouse, so this is the only place the two meet.
fn cursor_state() -> ((i32, i32), bool) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut point) };
    let down = unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } < 0;
    if ok == 0 {
        return ((0, 0), down);
    }
    ((point.x, point.y), down)
}

/// The capture rectangle: the pane plus `margin` of surrounding desktop,
/// clipped to the duplicated output. Clipping shrinks the margin on any
/// side the pane runs off, so the box is the single source of truth for
/// where the shader's mapping lands. Takes plain numbers rather than a
/// `Capture` so it stays testable off-device.
fn capture_box_in(
    frame_w: u32,
    frame_h: u32,
    origin_x: i32,
    origin_y: i32,
    mx: i32,
    my: i32,
    mw: u32,
    mh: u32,
    margin: u32,
) -> Option<D3D11_BOX> {
    let m = margin as i32;
    let (px, py) = (mx - origin_x, my - origin_y);
    let left = (px - m).max(0);
    let top = (py - m).max(0);
    // Clamp the pane's own edges to the output, then grow by whatever
    // margin still fits. Doing it in this order is what keeps the pane
    // inside the box when it overhangs an edge: clamping the grown box
    // instead would leave the pane's far side uncovered.
    let pane_right = (px + mw as i32).min(frame_w as i32);
    let pane_bottom = (py + mh as i32).min(frame_h as i32);
    let right = (pane_right + m).min(frame_w as i32);
    let bottom = (pane_bottom + m).min(frame_h as i32);
    if right <= left || bottom <= top {
        return None;
    }
    Some(D3D11_BOX {
        left: left as u32,
        top: top as u32,
        right: right as u32,
        bottom: bottom as u32,
        front: 0,
        back: 1,
    })
}

fn capture_box(
    cap: &Capture,
    mx: i32,
    my: i32,
    mw: u32,
    mh: u32,
    margin: u32,
) -> Option<D3D11_BOX> {
    capture_box_in(
        cap.frame_w,
        cap.frame_h,
        cap.origin_x,
        cap.origin_y,
        mx,
        my,
        mw,
        mh,
        margin,
    )
}

fn worker_main(
    glass_sys: isize,
    initial_main_sys: isize,
    rx: Receiver<GlassConfig>,
    proxy: EventLoopProxy<UiCommand>,
    ready: Arc<AtomicBool>,
) {
    let hwnd = HWND(glass_sys as *mut std::ffi::c_void);
    crate::logging::info("[nex][refract] worker alive (D3D11/DXGI need no COM init here)");
    let mut engine: Option<Engine> = None;
    let mut capture: Option<Capture> = None;
    let mut motion = Motion::new();
    let mut visible = false;
    let mut main_sys: isize = initial_main_sys;
    let mut scale = 1.0f64;
    let mut pointer;
    let mut pointer_in;
    let mut down;
    // Build and render one frame while hidden, so the first show has a
    // ready surface instead of waiting on D3D/capture initialization.
    let mut prewarm_pending = true;
    let mut last_frame = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut stat_t = Instant::now();
    let (mut n_cap, mut n_draw) = (0u64, 0u64);
    // Last panel position the glass was aligned to. Re-align if the
    // window moves while the capture worker is active.
    let mut last_glass_at = (0i32, 0i32);
    // Straight failures in a row: a poisoned device or capture never
    // heals by retrying the same calls, so rebuild everything after a
    // few.
    let mut n_fail = 0u32;

    loop {
        while let Ok(cfg) = rx.try_recv() {
            visible = cfg.visible;
            if cfg.main_sys != 0 {
                main_sys = cfg.main_sys;
            }
            if cfg.scale > 0.0 {
                scale = cfg.scale;
            }
        }
        if (!visible && !prewarm_pending) || main_sys == 0 {
            if capture.take().is_some() {
                crate::logging::info("[nex][refract] capture stopped (hidden)");
            }
            motion.settle();
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let Some((mx, my, mw, mh)) = main_rect(main_sys) else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if (mx, my) != last_glass_at {
            align_glass_to_main(hwnd, main_sys);
            crate::logging::info(&format!("[nex][refract] glass follows panel to ({mx},{my})"));
            last_glass_at = (mx, my);
        }
        // Engine ensure / resize (buffers only when one exists).
        if engine.is_none() {
            match Engine::build(hwnd, mw, mh, scale) {
                Ok(e) => engine = Some(e),
                Err(error) => {
                    crate::logging::warn(&format!("[nex][refract] engine build failed: {error}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        } else if let Some(e) = engine.as_mut() {
            if e.width != mw || e.height != mh || (e.scale - scale).abs() > 0.01 {
                if let Err(error) = e.resize(mw, mh, scale) {
                    crate::logging::warn(&format!("[nex][refract] engine resize failed: {error}"));
                    engine = None;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }
        let eng = match engine.as_mut() {
            Some(e) => e,
            None => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        // Capture ensure (per monitor; restarts on move/ACCESS_LOST).
        let monitor_now = unsafe {
            MonitorFromWindow(HWND(main_sys as *mut std::ffi::c_void), MONITOR_DEFAULTTONEAREST)
        };
        if capture.as_ref().map(|c| c.monitor) != Some(monitor_now) {
            capture = None;
            match Capture::start(&eng.device, monitor_now) {
                Ok(c) => {
                    crate::logging::info(&format!(
                        "[nex][refract] capture live {}x{}px {:?}",
                        c.frame_w, c.frame_h, c.format
                    ));
                    if let Err(error) = eng.set_source_format(c.format) {
                        crate::logging::warn(&format!(
                            "[nex][refract] source format adopt failed: {error}"
                        ));
                    }
                    capture = Some(c);
                }
                Err(error) => {
                    crate::logging::warn(&format!("[nex][refract] capture start failed: {error}"));
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }
        let cap = match capture.as_mut() {
            Some(c) => c,
            None => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };

        // Pointer state. The glass window never sees mouse messages, so
        // the cursor is read here instead of pushed over IPC.
        let (cursor, pressed) = cursor_state();
        pointer = ((cursor.0 - mx) as f32, (cursor.1 - my) as f32);
        pointer_in = pointer.0 >= 0.0
            && pointer.1 >= 0.0
            && pointer.0 < mw as f32
            && pointer.1 < mh as f32;
        down = pressed;

        // Poll one frame (nonblocking).
        let mut info: windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO =
            unsafe { std::mem::zeroed() };
        let mut resource: Option<IDXGIResource> = None;
        let acquired = match unsafe {
            cap.duplication
                .AcquireNextFrame(0, &mut info as *mut _, &mut resource as *mut _)
        } {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                // The desktop did not change. Redraw only while an
                // interaction is still settling; otherwise this is the
                // idle path and it costs no GPU work at all.
                if !motion.settled(visible, pointer_in, down) {
                    let elapsed = last_frame.elapsed();
                    if elapsed >= Duration::from_millis(MIN_FRAME_MS as u64) {
                        motion.step(
                            elapsed.as_secs_f32(),
                            visible,
                            pointer_in,
                            pointer,
                            down,
                            (mw as f32, mh as f32),
                        );
                        last_frame = Instant::now();
                        if capture_box(cap, mx, my, mw, mh, eng.margin).is_some() {
                            if let Err(error) = eng.compose(&motion, pointer, pointer_in) {
                                crate::logging::warn(&format!(
                                    "[nex][refract] compose failed: {error}"
                                ));
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            Err(e) => {
                crate::logging::warn(&format!(
                    "[nex][refract] acquire failed: {e:?} - restarting capture"
                ));
                capture = None;
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            Ok(()) => resource,
        };
        n_cap += 1;
        if info.ProtectedContentMaskedOut.0 != 0 && !cap.protected_logged {
            cap.protected_logged = true;
            crate::logging::warn(
                "[nex][refract] protected pixels are masked by Windows; presenting the rest of each frame",
            );
        }
        // FPS cap.
        let elapsed = last_frame.elapsed();
        if elapsed < Duration::from_millis(MIN_FRAME_MS as u64) {
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        motion.step(
            elapsed.as_secs_f32(),
            visible,
            pointer_in,
            pointer,
            down,
            (mw as f32, mh as f32),
        );
        last_frame = Instant::now();

        let outcome = (|| -> Result<Frame, String> {
            let tex = acquired
                .as_ref()
                .ok_or("empty frame resource")?
                .cast::<ID3D11Texture2D>()
                .map_err(|e| format!("frame QI texture failed: {e:?}"))?;
            let Some(region) = capture_box(cap, mx, my, mw, mh, eng.margin) else {
                // Panel is partly off the duplicated output: keep the
                // last crop rather than smearing a stale half-screen.
                return Ok(Frame::Skipped);
            };
            eng.render(&tex, region, &motion, pointer, pointer_in)
        })();
        let _ = unsafe { cap.duplication.ReleaseFrame() };
        match outcome {
            Ok(Frame::Presented) => {
                n_draw += 1;
                n_fail = 0;
                // Readiness means "a frame actually reached the screen",
                // not "the pipeline is warm": the host holds the panel
                // back until then, so it must not open on a frame DXGI
                // silently dropped.
                if !ready.swap(true, Ordering::AcqRel) {
                    let _ = proxy.send_event(UiCommand::GlassReady);
                }
                if prewarm_pending {
                    prewarm_pending = false;
                    if !visible {
                        capture = None;
                        crate::logging::info("[nex][refract] first frame prewarmed while hidden");
                    }
                }
            }
            Ok(Frame::Skipped) => {}
            Err(error) => {
                n_fail += 1;
                crate::logging::warn(&format!("[nex][refract] frame failed: {error}"));
                if n_fail >= 3 {
                    crate::logging::warn(
                        "[nex][refract] 3 straight frame failures - full engine+capture reset",
                    );
                    capture = None;
                    engine = None;
                    n_fail = 0;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        if stat_t.elapsed() >= Duration::from_secs(1) {
            crate::logging::info(&format!(
                "[nex][refract] captured/s={n_cap} drawn/s={n_draw}"
            ));
            stat_t = Instant::now();
            n_cap = 0;
            n_draw = 0;
        }
    }
}

impl Capture {
    fn start(device: &ID3D11Device, monitor: HMONITOR) -> Result<Capture, String> {
        // Independent factory (not the device's parent): enumerate every
        // adapter/output and duplicate the first matching output that
        // accepts our device. Survives multi-GPU boxes where the device
        // and the output live on different adapters.
        let factory: IDXGIFactory = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
            .map_err(|e| format!("capture factory failed: {e:?}"))?;
        let mut duplication: Option<IDXGIOutputDuplication> = None;
        let mut ai = 0u32;
        'adapters: loop {
            let adapter: IDXGIAdapter = match unsafe { factory.EnumAdapters(ai) } {
                Ok(a) => a,
                Err(_) => break,
            };
            ai += 1;
            let mut oi = 0u32;
            loop {
                let output: IDXGIOutput = match unsafe { adapter.EnumOutputs(oi) } {
                    Ok(o) => o,
                    Err(_) => break,
                };
                oi += 1;
                let matches = unsafe {
                    output
                        .GetDesc()
                        .map(|desc| desc.Monitor == monitor)
                        .unwrap_or(false)
                };
                if !matches {
                    continue;
                }
                let output1: IDXGIOutput1 = match output.cast() {
                    Ok(o) => o,
                    Err(error) => {
                        crate::logging::warn(&format!(
                            "[nex][refract] output QI IDXGIOutput1 failed: {error:?}"
                        ));
                        continue;
                    }
                };
                match unsafe { output1.DuplicateOutput(device) } {
                    Ok(dup) => {
                        duplication = Some(dup);
                        break 'adapters;
                    }
                    Err(error) => crate::logging::warn(&format!(
                        "[nex][refract] DuplicateOutput refused (adapter mismatch?): {error:?}"
                    )),
                }
            }
        }
        let duplication = duplication.ok_or("no output on the panel monitor duplicated")?;
        let mode = unsafe { duplication.GetDesc() };
        let (frame_w, frame_h) = (mode.ModeDesc.Width, mode.ModeDesc.Height);
        if frame_w == 0 || frame_h == 0 {
            return Err("duplication mode is empty".to_string());
        }
        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut mi as *mut _) }.0 == 0 {
            return Err("GetMonitorInfoW failed".to_string());
        }
        Ok(Capture {
            duplication,
            frame_w,
            frame_h,
            format: mode.ModeDesc.Format,
            origin_x: mi.rcMonitor.left,
            origin_y: mi.rcMonitor.top,
            monitor,
            protected_logged: false,
        })
    }
}

unsafe extern "system" fn glass_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn last_error_code() -> u32 {
    unsafe { windows::Win32::Foundation::GetLastError().0 }
}

fn physical_px(logical: f64, scale: f64) -> u32 {
    ((logical * scale).round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader samples the capture crop outward from the pane by at
    /// most `PUSH_MAX * lens + RIM_PUSH`, where `lens` peaks at 1 in the
    /// centre. If that ever exceeds the captured margin, the pane starts
    /// reading past the edge of its own crop and the material degrades
    /// into clamped smears — which is the same failure mode as the
    /// feedback mirror it replaced, and much harder to see.
    #[test]
    fn outward_push_stays_inside_the_capture_margin() {
        let worst = PUSH_MAX + RIM_PUSH;
        assert!(
            worst < CROP_MARGIN,
            "outward push {worst}px reaches past the {CROP_MARGIN}px capture margin"
        );
        // And it must clear the rim bend, which is added on top of it.
        assert!(
            worst + REFRACT_PX < CROP_MARGIN,
            "push plus rim bend ({:.0}px) exceeds the {:.0}px margin",
            worst + REFRACT_PX,
            CROP_MARGIN
        );
    }

    /// The margin must also be wide enough for the blur kernel plus the
    /// chroma-bleed taps, or the frost layer samples clamped edges and
    /// the pane picks up a bright outline from the crop border.
    #[test]
    fn margin_clears_blur_and_bleed() {
        let reach = BLUR_SIGMA * 3.0 + BLEED_PX;
        assert!(
            reach < CROP_MARGIN,
            "blur+bleed reach {reach:.0}px exceeds the {CROP_MARGIN}px margin"
        );
    }

    /// The capture box must cover the pane itself, or the material has
    /// no backdrop at all. Clipping may shrink the margin but never the
    /// pane — a pane overhanging a screen edge is the case that breaks.
    #[test]
    fn capture_box_always_contains_the_pane() {
        let (fw, fh) = (1920u32, 1080u32);
        let box_of = |x, y, w, h| {
            capture_box_in(fw, fh, 0, 0, x, y, w, h, 120).expect("pane overlaps the output")
        };

        // Interior: full margin all round.
        let b = box_of(600, 200, 700, 500);
        assert_eq!((b.left, b.top, b.right, b.bottom), (480, 80, 1420, 820));

        // Top-left corner: margin clips away on two sides, pane intact.
        let b = box_of(0, 0, 700, 500);
        assert!(b.left == 0 && b.top == 0);
        assert!(b.right >= 700 && b.bottom >= 500, "pane must stay covered");

        // Overhanging the bottom-right: the pane's visible part still has
        // to be inside the box, and the box still has to stay on-screen.
        let b = box_of(1800, 1000, 700, 500);
        assert!(b.left <= 1800 && b.top <= 1000);
        assert!(b.right <= fw && b.bottom <= fh);
        assert!(b.right == 1920 && b.bottom == 1080, "box must hug the output edge");

        // A second monitor at a negative origin shifts everything.
        let ox = -1920i32;
        let b = capture_box_in(fw, fh, ox, 0, -1300, 300, 700, 500, 120).expect("on monitor 2");
        let pane_left = (-1300 - ox) as u32;
        assert!(b.left <= pane_left && b.top <= 300);
        assert!(b.right >= pane_left + 700);

        // Fully off the output yields nothing rather than a bogus box.
        assert!(capture_box_in(fw, fh, 0, 0, 3000, 0, 700, 500, 120).is_none());
    }
}

/// Align a glass window to the main panel: same rect, ordered directly
/// below it, never activating. Shared by the host (show/resize) and the
/// worker (position follows — nothing else re-syncs after a drag).
fn align_glass_to_main(glass: HWND, main_sys: isize) -> Option<(u32, u32)> {
    let (x, y, width, height) = main_rect(main_sys)?;
    let main = HWND(main_sys as *mut std::ffi::c_void);
    if let Err(error) = unsafe {
        SetWindowPos(
            glass,
            Some(main),
            x,
            y,
            width as i32,
            height as i32,
            SWP_NOACTIVATE,
        )
    } {
        crate::logging::warn(&format!("[nex][refract] align failed: {error:?}"));
        return None;
    }
    Some((width, height))
}

fn main_rect(main_sys: isize) -> Option<(i32, i32, u32, u32)> {
    // Extended frame bounds, not GetWindowRect: the latter includes the
    // invisible DWM drop shadow (~8px per side), which made the glass
    // poke out around the panel. RECT is the windows (not windows_sys)
    // shape already imported here.
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            main_sys as *mut std::ffi::c_void,
            windows_sys::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut rect as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    } == 0;
    if !ok {
        return None;
    }
    let (w, h) = (
        (rect.right - rect.left).max(1),
        (rect.bottom - rect.top).max(1),
    );
    Some((rect.left, rect.top, w as u32, h as u32))
}
