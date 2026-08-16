//! LRU icon cache serving the WebView `nexasset://icon/…` route.
//!
//! Each entry is keyed by file path and stores PNG-encoded bytes
//! (decoded from `.ico` or `.png` on first access). No Iced dependency.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;

use crate::overlay::model::OverlayRow;

const DEFAULT_MAX_ENTRIES: usize = 96;
const DEFAULT_IDLE_TRIM_MS: u32 = 90_000;

/// Target square canvas size for normalized icons. Crisp at 2-3x DPI
/// when CSS displays at 30px. PNG is ~3-8KB each — fits the LRU budget.
const TARGET_ICON_SIZE: u32 = 128;
/// Extraction request size for IShellItemImageFactory and
/// PrivateExtractIconsW (primary high-res paths). 256px is the
/// Windows jumbo icon size; the Lanczos downscale to
/// TARGET_ICON_SIZE (128) produces a clean, sharp result on HiDPI.
const EXTRACT_ICON_SIZE: i32 = 256;

pub struct IconCache {
    inner: Mutex<Inner>,
}

struct Inner {
    png: LruCache<PathBuf, Arc<Vec<u8>>>,
    last_touch: HashMap<PathBuf, Instant>,
    max_entries: NonZeroUsize,
    idle_trim: Duration,
}

impl Inner {
    fn touch(&mut self, key: PathBuf) {
        self.last_touch.insert(key, Instant::now());
    }

    /// Remove last_touch entries whose keys are no longer in the LRU.
    /// Called after put() to prevent unbounded HashMap growth when
    /// LRU eviction removes png entries but last_touch retains them.
    fn clean_orphaned_touches(&mut self) {
        if self.last_touch.len() <= self.png.cap().get() {
            return; // No orphans possible
        }
        self.last_touch.retain(|k, _| self.png.contains(k));
    }
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_IDLE_TRIM_MS)
    }
}

impl IconCache {
    pub(crate) fn new(max_entries: usize, idle_trim_ms: u32) -> Self {
        let max_entries = NonZeroUsize::new(max_entries.max(1)).unwrap();
        Self {
            inner: Mutex::new(Inner {
                png: LruCache::new(max_entries),
                last_touch: HashMap::new(),
                max_entries,
                idle_trim: Duration::from_millis(idle_trim_ms as u64),
            }),
        }
    }

    /// Decode `path` (.ico/.png) and return PNG-encoded bytes for the
    /// WebView `nexasset://icon/...` route. Cached in an LRU keyed by
    /// path. Returns `None` on empty path or decode failure.
    pub fn png_bytes(&self, path: &str) -> Option<Arc<Vec<u8>>> {
        if path.is_empty() {
            return None;
        }
        let key = PathBuf::from(path);
        if let Ok(mut inner) = self.inner.lock() {
            let bytes = inner.png.get(&key).cloned();
            if bytes.is_some() {
                inner.touch(key);
                return bytes;
            }
        }
        let bytes = Arc::new(decode_png(&key)?);
        if let Ok(mut inner) = self.inner.lock() {
            inner.png.put(key.clone(), bytes.clone());
            inner.touch(key);
            inner.clean_orphaned_touches();
        }
        Some(bytes)
    }

    /// Same as `png_bytes` but never blocks — returns `None` if the
    /// icon has not been decoded yet.  The background prefetch thread
    /// fills the cache; the caller re-renders when it completes.
    pub fn png_bytes_cached(&self, path: &str) -> Option<Arc<Vec<u8>>> {
        if path.is_empty() {
            return None;
        }
        let key = PathBuf::from(path);
        let mut inner = self.inner.lock().ok()?;
        let bytes = inner.png.get(&key).cloned()?;
        inner.touch(key);
        Some(bytes)
    }

    pub(crate) fn trim_unused(&self) -> usize {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return 0,
        };
        let cutoff = Instant::now()
            .checked_sub(inner.idle_trim)
            .unwrap_or_else(Instant::now);
        // Remove any stale entries from both the LRU and the touch map.
        // Also clean up touch entries that were left behind by LRU eviction.
        let stale: Vec<PathBuf> = inner
            .last_touch
            .iter()
            .filter_map(|(k, t)| {
                let expired = *t < cutoff;
                let evicted = !inner.png.contains(k);
                (expired || evicted).then(|| k.clone())
            })
            .collect();
        for k in &stale {
            inner.png.pop(k);
            inner.last_touch.remove(k);
        }
        stale.len()
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.png.clear();
            inner.last_touch.clear();
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|i| i.png.len())
            .unwrap_or(0)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reconfigure cache limits from runtime config values.
    /// `max_entries` is derived from `active_memory_target_mb`.
    /// `trim_ms` comes directly from `idle_cache_trim_ms`.
    pub(crate) fn reconfigure(&self, max_entries: usize, trim_ms: u32) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let new_max = NonZeroUsize::new(max_entries.max(1)).unwrap();
        inner.max_entries = new_max;
        inner.idle_trim = Duration::from_millis(trim_ms as u64);
        if inner.png.cap().get() != new_max.get() {
            inner.png.resize(new_max);
        }
    }

    /// Compute icon cache capacity from the configured memory target.
    /// Each icon ~4KB. Reserve ~10% of memory target for icons.
    pub(crate) fn icon_cache_capacity_from_memory_target(target_mb: u16) -> usize {
        let budget = (target_mb as usize).saturating_mul(1024 * 1024) / 10;
        (budget / 4096).max(32).min(512)
    }
}

/// Decode an icon file to PNG-encoded bytes.
/// Uses [`ExtractIconExW`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-extracticonexw)
/// for all file types (`.exe`, `.lnk`, `.ico`, `shell:AppsFolder\…`),
/// which is the only reliable cross-format approach.  Falls back to
/// direct decode for `.png` images (which carry no Windows icon
/// resource).
fn decode_png(path: &PathBuf) -> Option<Vec<u8>> {
    let path_str = path.to_string_lossy();

    // ms-settings:{uri} — synthetic Settings page icons: render the
    // page's Segoe Fluent glyph instead of extracting a shell icon.
    if let Some(uri) = path_str.strip_prefix("ms-settings:") {
        return settings_glyph_png(uri);
    }

    // .png files don't have embedded Windows icons; decode directly.
    if path_str.to_ascii_lowercase().ends_with(".png") {
        if let Ok(bytes) = std::fs::read(path) {
            return decode_image_bytes(&bytes);
        }
    }

    // Everything else: extract the shell icon.
    #[cfg(target_os = "windows")]
    {
        let png = extract_shell_icon_png(&path_str);
        if png.is_some() {
            return png;
        }
    }

    // Fallback: try direct read + decode (for .ico, cached PNGs, etc.)
    if let Ok(bytes) = std::fs::read(path) {
        return decode_image_bytes(&bytes);
    }
    None
}

fn decode_image_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        image::load_from_memory(bytes)
    }))
    .ok()
    .and_then(|result| result.ok())?;
    normalize_to_square_png(img.into_rgba8())
}

/// Normalize any RGBA image to a consistent square canvas:
/// Lanczos-resize to TARGET × TARGET (upscaling small sources,
/// downscaling large ones), centered on a transparent canvas. Every
/// result row gets a uniform square icon with consistent padding —
/// Raycast look. Without the upscale, small shell extractions (e.g.
/// Windows Settings) would render smaller than 256px sources (Spotify).
///
/// Content-aware upscale for sparse glyph icons: if visible pixels
/// fill < 35% of the source image (small glyph centered in large
/// transparent padding), crop to bounding box first then upscale the
/// cropped region to fill TARGET × TARGET. This prevents Magnifier/
/// JPEGView-style icons from rendering as tiny dots.
///
/// Full-bleed icons (Notepad ~58% fill) keep existing behavior.
/// Degenerate case (all-transparent) returns native PNG as-is.
fn normalize_to_square_png(img: image::RgbaImage) -> Option<Vec<u8>> {
    let target = TARGET_ICON_SIZE;
    let (w, h) = (img.width(), img.height());

    // ── Compute fill ratio (alpha > 8 = visible) ──
    let mut content_px: u64 = 0;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 8 {
                content_px += 1;
            }
        }
    }
    let fill_ratio = content_px as f32 / ((w * h) as f32);

    // ── Compute core bounding box (alpha > 128 = solid) ──
    // Use a stricter threshold to find the actual glyph core — anti-aliased
    // border pixels (alpha 9-128) can span the full image, making the bbox
    // useless for cropping. Core bbox captures the opaque glyph content.
    let mut core_min_x = w;
    let mut core_min_y = h;
    let mut core_max_x: u32 = 0;
    let mut core_max_y: u32 = 0;
    let mut core_px: u64 = 0;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 128 {
                core_px += 1;
                if x < core_min_x { core_min_x = x; }
                if y < core_min_y { core_min_y = y; }
                if x > core_max_x { core_max_x = x; }
                if y > core_max_y { core_max_y = y; }
            }
        }
    }
    let core_bbox_valid = core_px > 0 && core_max_x >= core_min_x && core_max_y >= core_min_y;

    // ── Sparse glyph: crop to core bbox, upscale to target ──
    // fill_ratio uses alpha>8 (permissive), core_bbox uses alpha>128 (strict).
    // Both must agree: low fill AND core bbox significantly smaller than image.
    if core_bbox_valid && fill_ratio < 0.35 {
        let bbox_w = core_max_x - core_min_x + 1;
        let bbox_h = core_max_y - core_min_y + 1;
        // Only crop if core bbox is actually smaller than the source —
        // otherwise we'd just be resizing the same dimensions.
        if bbox_w < w || bbox_h < h {
            use image::imageops::{self, FilterType};
            let pad: u32 = 2;
            let crop_x = core_min_x.saturating_sub(pad);
            let crop_y = core_min_y.saturating_sub(pad);
            let crop_x2 = (core_max_x + pad).min(w - 1);
            let crop_y2 = (core_max_y + pad).min(h - 1);
            let crop_w = crop_x2 - crop_x + 1;
            let crop_h = crop_y2 - crop_y + 1;

            let crop = image::imageops::crop_imm(&img, crop_x, crop_y, crop_w, crop_h).to_image();

            // Upscale cropped region to fill TARGET × TARGET.
            let resized = imageops::resize(&crop, target, target, FilterType::Lanczos3);
            let mut canvas = image::RgbaImage::from_pixel(target, target, image::Rgba([0, 0, 0, 0]));
            let x = target.saturating_sub(resized.width()) / 2;
            let y = target.saturating_sub(resized.height()) / 2;
            imageops::overlay(&mut canvas, &resized, x as i64, y as i64);
            return rgba_to_png(canvas);
        }
    }

    // Degenerate: all-transparent image → return native as-is.
    if content_px == 0 {
        return rgba_to_png(img);
    }

    // ── Full-bleed / normal path ──
    // Uniform canvas: upscale small sources, downscale large ones —
    // every row icon renders at the same CSS size no matter what size
    // the shell extraction returned (some app icons come back small,
    // e.g. Windows Settings, while others are 256px).
    use image::imageops::{self, FilterType};
    let resized = imageops::resize(&img, target, target, FilterType::Lanczos3);
    let mut canvas = image::RgbaImage::from_pixel(target, target, image::Rgba([0, 0, 0, 0]));
    let x = target.saturating_sub(resized.width()) / 2;
    let y = target.saturating_sub(resized.height()) / 2;
    imageops::overlay(&mut canvas, &resized, x as i64, y as i64);
    rgba_to_png(canvas)
}

fn rgba_to_png(rgba: image::RgbaImage) -> Option<Vec<u8>> {
    let (width, height) = rgba.dimensions();
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        rgba.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .ok()?;
    Some(out.into_inner())
}

#[cfg(target_os = "windows")]
fn icon_to_rgba_png(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON, size: i32) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, CreateDIBSection, SelectObject, DeleteObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::DrawIconEx;

    unsafe {
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        if hdc.is_null() { return None; }

        // Create a 32-bit BGRA DIB section to render the icon into.
        let mut header: BITMAPINFOHEADER = std::mem::zeroed();
        header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.biWidth = size;
        header.biHeight = -size; // top-down
        header.biPlanes = 1;
        header.biBitCount = 32;
        header.biCompression = BI_RGB;

        let mut bmpinfo: BITMAPINFO = std::mem::zeroed();
        bmpinfo.bmiHeader = header;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(
            hdc,
            &bmpinfo,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if hbmp.is_null() || bits.is_null() {
            DeleteDC(hdc);
            return None;
        }

        let old_bmp = SelectObject(hdc, hbmp as _);
        // Fill with transparent black.
        let pixel_count = (size * size) as usize;
        std::ptr::write_bytes(bits, 0, pixel_count * 4);

        DrawIconEx(hdc, 0, 0, hicon, size, size, 0, std::ptr::null_mut(), 0x0003);

        SelectObject(hdc, old_bmp);

        // Read back the BGRA pixels, swap to RGBA.
        let pixels = std::slice::from_raw_parts(bits as *const u8, pixel_count * 4);
        let mut rgba = vec![0u8; pixel_count * 4];
        for (i, chunk) in pixels.chunks_exact(4).enumerate() {
            rgba[i * 4] = chunk[2];     // R ← B
            rgba[i * 4 + 1] = chunk[1]; // G ← G
            rgba[i * 4 + 2] = chunk[0]; // B ← R
            rgba[i * 4 + 3] = chunk[3]; // A ← A
        }

        DeleteObject(hbmp as _);
        DeleteDC(hdc);

        let img = image::RgbaImage::from_raw(size as u32, size as u32, rgba)?;
        normalize_to_square_png(img)
    }
}

#[cfg(not(target_os = "windows"))]
fn icon_to_rgba_png(_hicon: *mut std::ffi::c_void, _size: i32) -> Option<Vec<u8>> {
    None
}

/// Render a Settings page glyph (`ms-settings:{uri}`) as a white
/// Segoe Fluent Icons / Segoe MDL2 Assets glyph on a transparent
/// 128px canvas, normalized to PNG. The overlay tints it per theme
/// via CSS (`img.icon.glyph`).
#[cfg(target_os = "windows")]
fn settings_glyph_png(uri: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, CreateDIBSection, SelectObject, DeleteObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        CreateFontW, DrawTextW, DT_CENTER, DT_NOCLIP, DT_SINGLELINE, DT_VCENTER,
        SetBkMode, TRANSPARENT, GetTextFaceW, SetTextColor,
    };

    const SIZE: i32 = 128;
    let glyph: u16 = crate::settings_catalog::settings_glyph(uri);
    let text = [glyph, 0u16];

    unsafe {
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        if hdc.is_null() { return None; }

        let mut header: BITMAPINFOHEADER = std::mem::zeroed();
        header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.biWidth = SIZE;
        header.biHeight = -SIZE; // top-down
        header.biPlanes = 1;
        header.biBitCount = 32;
        header.biCompression = BI_RGB;

        let mut bmpinfo: BITMAPINFO = std::mem::zeroed();
        bmpinfo.bmiHeader = header;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(
            hdc,
            &bmpinfo,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if hbmp.is_null() || bits.is_null() {
            DeleteDC(hdc);
            return None;
        }
        let old_bmp = SelectObject(hdc, hbmp as _);
        let pixel_count = (SIZE * SIZE) as usize;
        std::ptr::write_bytes(bits, 0, pixel_count * 4);

        // Segoe Fluent Icons on Win11; Segoe MDL2 Assets on Win10.
        let mut font_name = "Segoe Fluent Icons".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let mut hfont = CreateFontW(
            -96, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 4, 0, font_name.as_ptr(),
        );
        if !hfont.is_null() {
            // Confirm the face actually resolved (missing font → GDI
            // substitutes and the glyph renders as a tofu box).
            let mut face = [0u16; 64];
            let old_font = SelectObject(hdc, hfont as _);
            let face_len = GetTextFaceW(hdc, face.len() as i32, face.as_mut_ptr());
            let resolved = face_len > 0
                && face[..face_len as usize]
                    == "Segoe Fluent Icons".encode_utf16().collect::<Vec<u16>>()[..];
            SelectObject(hdc, old_font);
            if !resolved {
                DeleteObject(hfont as _);
                hfont = std::ptr::null_mut();
            }
        }
        if hfont.is_null() {
            font_name = "Segoe MDL2 Assets".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
            hfont = CreateFontW(
                -96, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 4, 0, font_name.as_ptr(),
            );
        }
        if hfont.is_null() {
            SelectObject(hdc, old_bmp);
            DeleteObject(hbmp as _);
            DeleteDC(hdc);
            return None;
        }

        let old_font = SelectObject(hdc, hfont as _);
        SetBkMode(hdc, TRANSPARENT as i32);
        let rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: SIZE,
            bottom: SIZE,
        };
        // White glyph; the web layer tints per theme.
        SetTextColor(hdc, 0x00FF_FFFF);
        let _ = DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &rect as *const _ as *mut _,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
        );

        SelectObject(hdc, old_font);
        SelectObject(hdc, old_bmp);

        // BGRA → RGBA.
        let pixels = std::slice::from_raw_parts(bits as *const u8, pixel_count * 4);
        let mut rgba = vec![0u8; pixel_count * 4];
        for (i, chunk) in pixels.chunks_exact(4).enumerate() {
            rgba[i * 4] = chunk[2];
            rgba[i * 4 + 1] = chunk[1];
            rgba[i * 4 + 2] = chunk[0];
            rgba[i * 4 + 3] = chunk[3];
        }

        DeleteObject(hfont as _);
        DeleteObject(hbmp as _);
        DeleteDC(hdc);

        let img = image::RgbaImage::from_raw(SIZE as u32, SIZE as u32, rgba)?;
        normalize_to_square_png(img)
    }
}

#[cfg(not(target_os = "windows"))]
fn settings_glyph_png(_uri: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "windows")]
/// COM vtable for IShellItemImageFactory.
/// GUID: {BCC18B79-BA16-442F-80C4-8A59C30C463B}
#[repr(C)]
struct IShellItemImageFactoryVtbl {
    // IUnknown
    query_interface: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        riid: *const windows_sys::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> windows_sys::core::HRESULT,
    add_ref: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    // IShellItemImageFactory
    get_image: unsafe extern "system" fn(
        this: *mut std::ffi::c_void,
        size: windows_sys::Win32::Foundation::SIZE,
        flags: u32,
        phbitmap: *mut windows_sys::Win32::Graphics::Gdi::HBITMAP,
    ) -> windows_sys::core::HRESULT,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct IShellItemImageFactory {
    lp_vtbl: *const IShellItemImageFactoryVtbl,
}

#[cfg(target_os = "windows")]
const IID_ISHELL_ITEM_IMAGE_FACTORY: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0xBCC18B79_BA16_442F_80C4_8A59C30C463B);

#[cfg(target_os = "windows")]
const SIIGBF_ICONONLY: u32 = 0x00000100;
#[cfg(target_os = "windows")]
const SIIGBF_BIGGERSIZEOK: u32 = 0x00000001;

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn SHCreateItemFromParsingName(
        pszPath: *const u16,
        pbc: *const std::ffi::c_void,
        riid: *const windows_sys::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> windows_sys::core::HRESULT;
}

/// Request 256 for every item; files whose shell icon lacks hi-res just
/// return their native smaller bitmap (no upscale) thanks to BIGGERSIZEOK
/// without RESIZETOFIT.
fn extraction_size_for_path(path: &str) -> i32 {
    let _ = path;
    EXTRACT_ICON_SIZE
}

/// Primary icon extraction: IShellItemImageFactory::GetImage.
/// Returns HBITMAP that we render into an RGBA buffer via GDI.
/// Works for all shell types: .exe, .lnk, .ico, shell:AppsFolder\...
#[cfg(target_os = "windows")]
fn shell_item_image_factory_png(shell_path: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        DeleteObject, HBITMAP,
    };

    let wide: Vec<u16> = shell_path.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut item: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = SHCreateItemFromParsingName(
            wide.as_ptr(),
            std::ptr::null(),
            &IID_ISHELL_ITEM_IMAGE_FACTORY,
            &mut item,
        );
        if hr < 0 || item.is_null() {
            return None;
        }
        let factory = &*(item as *const IShellItemImageFactory);

        let size_px = extraction_size_for_path(shell_path);
        let size = windows_sys::Win32::Foundation::SIZE {
            cx: size_px,
            cy: size_px,
        };

        let mut hbmp: HBITMAP = std::ptr::null_mut();
        // SIIGBF_ICONONLY: never let the shell substitute a content
        // thumbnail (image/video/PDF files). SIIGBF_BIGGERSIZEOK without
        // RESIZETOFIT returns the native bitmap (no forced upscale):
        // items with real 256px icons yield hi-res, small 16/32px natives
        // pass through untouched at native size (crisp at 24px CSS).
        let hr = ((*factory.lp_vtbl).get_image)(
            item,
            size,
            SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            &mut hbmp,
        );
        ((*factory.lp_vtbl).release)(item);

        if hr < 0 || hbmp.is_null() {
            return None;
        }

        let png = hbitmap_to_rgba_png(hbmp, size_px);
        DeleteObject(hbmp as _);
        png
    }
}

#[cfg(target_os = "windows")]
/// Render an HBITMAP into an RGBA buffer via GetDIBits, then normalize to PNG.
/// Hi-res sources (>= 128px) fill the target canvas; small native bitmaps
/// render at native size to avoid blur from upscaling 16/32→256.
fn hbitmap_to_rgba_png(hbmp: windows_sys::Win32::Graphics::Gdi::HBITMAP, target_size: i32) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, SelectObject, ReleaseDC, DeleteObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, GetDIBits, GetDC,
        GetObjectW, BITMAP, StretchBlt, SRCCOPY, CreateDIBSection,
    };

    unsafe {
        // Query actual bitmap dimensions.
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(hbmp as _, std::mem::size_of::<BITMAP>() as i32, (&mut bm) as *mut _ as _) == 0 {
            return None;
        }
        let src_w = bm.bmWidth;
        let src_h = bm.bmHeight;
        if src_w <= 0 || src_h <= 0 {
            return None;
        }

        // Small native icons (16/32px — most generic file types) must NOT
        // be stretched to the 256px canvas: that produces blur. Only
        // hi-res sources (>= 128px) fill the target canvas; small sources
        // render at native size — normalize_to_square_png passes them
        // through untouched and CSS object-fit handles display.
        let src_min = src_w.min(src_h);
        let canvas = if src_min >= 128 { target_size } else { src_w };

        let screen_dc = GetDC(std::ptr::null_mut());
        let src_dc = CreateCompatibleDC(screen_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        if src_dc.is_null() {
            return None;
        }

        let old_src = SelectObject(src_dc, hbmp as _);

        // Create a 32-bit BGRA DIB sized to the canvas (source or target).
        let mut header: BITMAPINFOHEADER = std::mem::zeroed();
        header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.biWidth = canvas;
        header.biHeight = -canvas; // top-down
        header.biPlanes = 1;
        header.biBitCount = 32;
        header.biCompression = BI_RGB;

        let mut bmpinfo: BITMAPINFO = std::mem::zeroed();
        bmpinfo.bmiHeader = header;

        let dst_dc = CreateCompatibleDC(std::ptr::null_mut());
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dst_bmp = CreateDIBSection(
            dst_dc, &bmpinfo, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0,
        );
        if dst_bmp.is_null() || bits.is_null() {
            SelectObject(src_dc, old_src);
            DeleteDC(src_dc);
            DeleteDC(dst_dc);
            return None;
        }
        let old_dst = SelectObject(dst_dc, dst_bmp as _);

        // Fill with transparent black, then blit the source into the canvas.
        let pixel_count = (canvas * canvas) as usize;
        std::ptr::write_bytes(bits, 0, pixel_count * 4);
        StretchBlt(dst_dc, 0, 0, canvas, canvas, src_dc, 0, 0, src_w, src_h, SRCCOPY);

        SelectObject(src_dc, old_src);
        DeleteDC(src_dc);
        SelectObject(dst_dc, old_dst);

        // Read back the BGRA pixels, swap to RGBA.
        let pixels = std::slice::from_raw_parts(bits as *const u8, pixel_count * 4);
        let mut rgba = vec![0u8; pixel_count * 4];
        for (i, chunk) in pixels.chunks_exact(4).enumerate() {
            rgba[i * 4] = chunk[2];     // R ← B
            rgba[i * 4 + 1] = chunk[1]; // G ← G
            rgba[i * 4 + 2] = chunk[0]; // B ← R
            rgba[i * 4 + 3] = chunk[3]; // A ← A
        }

        DeleteObject(dst_bmp as _);
        DeleteDC(dst_dc);

        let img = image::RgbaImage::from_raw(canvas as u32, canvas as u32, rgba)?;
        normalize_to_square_png(img)
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_item_image_factory_png(_shell_path: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "windows")]
/// Extract the best quality icon from a file. Primary chain:
/// IShellItemImageFactory → PrivateExtractIconsW → SHGetFileInfo.
#[cfg(target_os = "windows")]
fn extract_shell_icon_png(shell_path: &str) -> Option<Vec<u8>> {
    // 1. IShellItemImageFactory — universal, handles all shell types.
    if let Some(png) = shell_item_image_factory_png(shell_path) {
        return Some(png);
    }

    // 2. PrivateExtractIconsW (high-res from .exe/.ico/.dll icon resources).
    let resolved_target = if shell_path.to_ascii_lowercase().ends_with(".lnk") {
        resolve_lnk_target(shell_path)
    } else {
        None
    };
    let resource_paths = resolved_target.as_deref().into_iter().chain(std::iter::once(shell_path));
    for path in resource_paths {
        if !path.starts_with("shell:") {
            if let Some(png) = private_extract_icons_png(path) {
                return Some(png);
            }
        }
    }

    // 3. SHGetFileInfo — last resort fallback.
    extract_shell_icon_fallback(shell_path)
}

#[cfg(target_os = "windows")]
fn resolve_lnk_target(path: &str) -> Option<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    if bytes.len() < 76 { return None; }

    // Validate Shell Link CLSID: {00021401-0000-0000-C000-000000000046}
    const EXPECTED_CLSID: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
    ];
    if bytes[4..20] != EXPECTED_CLSID { return None; }

    // Parse LinkFlags at offset 0x14 (DWORD, little-endian).
    let link_flags = u32::from_le_bytes(bytes[0x14..0x18].try_into().ok()?);
    let has_link_info = (link_flags & 0x02) != 0;
    if !has_link_info { return None; }

    let mut pos: usize = 76; // after fixed ShellLinkHeader

    // Skip LinkTargetIDList if present (flag 0x01).
    if (link_flags & 0x01) != 0 {
        loop {
            if pos + 2 > bytes.len() { return None; }
            let cb = u16::from_le_bytes(bytes[pos..pos + 2].try_into().ok()?);
            if cb == 0 { break; } // terminal ID
            pos += cb as usize;
        }
        pos += 2; // skip the terminal ID WORD
    }

    // Now at LinkInfo structure.
    if pos + 20 > bytes.len() { return None; }
    let link_info_size = u32::from_le_bytes(bytes[pos..pos + 4].try_into().ok()?);
    if pos + link_info_size as usize > bytes.len() { return None; }

    let local_base_path_offset = u32::from_le_bytes(bytes[pos + 16..pos + 20].try_into().ok()?);
    if local_base_path_offset == 0 { return None; }

    let base_start = pos + local_base_path_offset as usize;
    if base_start + 2 > bytes.len() { return None; }

    // Read null-terminated UTF-16 string at base_start.
    let mut end = base_start;
    while end + 2 <= bytes.len() {
        if bytes[end] == 0 && bytes[end + 1] == 0 { break; }
        end += 2;
    }

    let wide = bytes[base_start..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect::<Vec<_>>();
    let target = String::from_utf16(&wide).ok()?;
    if target.is_empty() { None } else { Some(target) }
}

#[cfg(target_os = "windows")]
fn private_extract_icons_png(path: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hicon: HICON = std::ptr::null_mut();

    let count = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PrivateExtractIconsW(
            wide.as_ptr(),
            0,
            EXTRACT_ICON_SIZE,
            EXTRACT_ICON_SIZE,
            &mut hicon,
            std::ptr::null_mut(),
            1,
            0,
        )
    };

    if count == 0 || hicon.is_null() {
        return None;
    }

    let png = icon_to_rgba_png(hicon, EXTRACT_ICON_SIZE);
    unsafe { DestroyIcon(hicon); }
    png
}

#[cfg(target_os = "windows")]
fn extract_shell_icon_fallback(shell_path: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::Shell::{
        SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW,
        SHGFI_ICON, SHGFI_LARGEICON, SHGFI_PIDL,
    };
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;
    use windows_sys::Win32::System::Com::CoTaskMemFree;

    let wide: Vec<u16> = shell_path.encode_utf16().chain(std::iter::once(0)).collect();

    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    let hr = unsafe {
        SHParseDisplayName(wide.as_ptr(), std::ptr::null_mut(), &mut pidl, 0, std::ptr::null_mut())
    };
    if hr < 0 || pidl.is_null() {
        return None;
    }

    let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        SHGetFileInfoW(
            pidl as *const u16,
            0,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    unsafe { CoTaskMemFree(pidl as _); }

    if ret == 0 || sfi.hIcon.is_null() {
        return None;
    }

    let png = icon_to_rgba_png(sfi.hIcon as windows_sys::Win32::UI::WindowsAndMessaging::HICON, 64);
    unsafe { DestroyIcon(sfi.hIcon); }
    png
}

pub(crate) fn prefetch_rows(cache: &IconCache, rows: &[OverlayRow]) {
    // Initialize COM once per thread lifetime. The persistent
    // nex-icon-prefetch thread calls this repeatedly; calling
    // CoInitializeEx/CoUninitialize on every batch wastes cycles
    // and risks COM state churn. Using MTA (COINIT_MULTITHREADED)
    // so ExitProcess can terminate this thread without deadlocking
    // on COM apartment teardown.
    #[cfg(target_os = "windows")]
    {
        thread_local! {
            static COM_INIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        COM_INIT.with(|flag| {
            if !flag.get() {
                unsafe {
                    let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                        std::ptr::null(),
                        0, // COINIT_MULTITHREADED
                    );
                }
                flag.set(true);
            }
        });
    }
    for row in rows {
        if !row.icon_path.is_empty() {
            cache.png_bytes(&row.icon_path);
        }
    }
    // Note: CoUninitialize is intentionally omitted. COM is cleaned
    // up by ExitProcess when the process terminates. Calling
    // CoUninitialize here would undo the initialization for the
    // entire thread, requiring re-initialization on the next call.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_path_returns_none() {
        let cache = IconCache::default();
        assert!(cache.png_bytes("").is_none());
    }

    #[test]
    fn missing_file_returns_none() {
        let cache = IconCache::default();
        let path = std::env::temp_dir().join("nex-no-such-icon-99999.png");
        assert!(cache
            .png_bytes(path.to_string_lossy().as_ref())
            .is_none());
    }

    #[test]
    fn clear_resets_cache() {
        let cache = IconCache::new(4, 60_000);
        let _ = cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn trim_unused_returns_count_of_evicted() {
        let cache = IconCache::new(4, 0);
        let evicted = cache.trim_unused();
        assert_eq!(evicted, 0);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn bench_icon_extraction_paths() {
        let target = concat!(env!("windir"), "\\System32\\notepad.exe");
        let path = std::path::Path::new(target);
        if !path.exists() {
            eprintln!("SKIP: notepad.exe not found");
            return;
        }

        // COM must be initialized for SHCreateItemFromParsingName.
        unsafe {
            let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                0, // COINIT_MULTITHREADED
            );
        }

        use std::time::Instant;

        // Warmup: fill COM caches.
        let _ = shell_item_image_factory_png(target);
        let _ = private_extract_icons_png(target);

        let n = 20;
        let mut siif_times = Vec::with_capacity(n);
        let mut peiw_times = Vec::with_capacity(n);

        for _ in 0..n {
            let t0 = Instant::now();
            let r = shell_item_image_factory_png(target);
            siif_times.push(t0.elapsed().as_micros() as f64);
            assert!(r.is_some(), "IShellItemImageFactory should succeed for notepad.exe");
        }

        for _ in 0..n {
            let t0 = Instant::now();
            let r = private_extract_icons_png(target);
            peiw_times.push(t0.elapsed().as_micros() as f64);
            assert!(r.is_some(), "PrivateExtractIconsW should succeed for notepad.exe");
        }

        fn mean(v: &[f64]) -> f64 { v.iter().sum::<f64>() / v.len() as f64 }
        fn p95(v: &mut [f64]) -> f64 {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((v.len() as f64) * 0.95).round() as usize;
            v[idx.min(v.len() - 1)]
        }

        let siif_mean = mean(&siif_times);
        let peiw_mean = mean(&peiw_times);
        let mut siif_sorted = siif_times.clone();
        let mut peiw_sorted = peiw_times.clone();
        let siif_p95 = p95(&mut siif_sorted);
        let peiw_p95 = p95(&mut peiw_sorted);

        eprintln!("── Icon extraction benchmark ({n} iterations each) ──");
        eprintln!("IShellItemImageFactory: mean={siif_mean:.1}µs  p95={siif_p95:.1}µs");
        eprintln!("PrivateExtractIconsW:   mean={peiw_mean:.1}µs  p95={peiw_p95:.1}µs");

        // Assert both paths are in the same ballpark (within 3x of each other)
        let ratio = siif_mean.max(peiw_mean) / siif_mean.min(peiw_mean);
        assert!(ratio < 3.0, "IShellItemImageFactory (mean={siif_mean:.0}µs) vs PrivateExtractIconsW (mean={peiw_mean:.0}µs) differ by {ratio:.1}x — unexpected");
    }
}

#[cfg(test)]
mod probe_magnifier {
    use super::*;

    #[test]
    fn probe_magnifier_icon_size() {
        // COM must be initialized for IShellItemImageFactory paths.
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                0, // COINIT_MULTITHREADED
            );
        }

        // Build candidate paths.
        let exe_direct = concat!(env!("windir"), "\\System32\\Magnify.exe");

        let mut lnk_candidates: Vec<String> = Vec::new();
        // ProgramData path (shared across users).
        lnk_candidates.push(
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Accessories\\Magnifier.lnk"
                .to_string(),
        );
        // Per-user AppData path.
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            lnk_candidates.push(format!(
                "{userprofile}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Accessories\\Magnifier.lnk"
            ));
        } else {
            println!("magnifier probe: USERPROFILE env var not set, skipping AppData .lnk");
        }

        let mut paths: Vec<String> = Vec::new();
        paths.push(exe_direct.to_string());
        for lnk in &lnk_candidates {
            paths.push(lnk.clone());
        }

        for path in &paths {
            let exists = std::path::Path::new(path).exists();
            if !exists {
                println!("magnifier probe: path={path} -> file does not exist, skipping");
                continue;
            }

            let pb = PathBuf::from(path);
            match decode_png(&pb) {
                Some(bytes) => {
                    let non_empty = !bytes.is_empty();
                    match image::load_from_memory(&bytes) {
                        Ok(img) => {
                            let (w, h) = (img.width(), img.height());
                            println!(
                                "magnifier probe: path={path} png_bytes={} dims={}x{} non_empty={non_empty}",
                                bytes.len(), w, h,
                            );
                        }
                        Err(e) => {
                            println!(
                                "magnifier probe: path={path} png_bytes={} decode_error={e} non_empty={non_empty}",
                                bytes.len(),
                            );
                        }
                    }
                }
                None => {
                    println!("magnifier probe: path={path} -> None");
                }
            }
        }
    }

    /// Content-size hypothesis probe: decode icons, measure non-transparent
    /// bounding box, report how much of the 128x128 canvas is actually used.
    #[test]
    fn probe_content_bbox() {
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                0,
            );
        }

        let paths = [
            "shell:AppsFolder\\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\magnify.exe",
            "shell:AppsFolder\\{6D809377-6AF0-444B-8957-A3773F02200E}\\JPEGView\\JPEGView.exe",
            "C:\\Program Files\\JPEGView\\JPEGView.exe",
            "C:\\Windows\\System32\\Magnify.exe",
            "C:\\WINDOWS\\System32\\notepad.exe",
        ];

        for path in &paths {
            let pb = PathBuf::from(path);
            match decode_png(&pb) {
                Some(bytes) => {
                    match image::load_from_memory(&bytes) {
                        Ok(img) => {
                            let rgba = img.into_rgba8();
                            let (w, h) = rgba.dimensions();
                            let mut min_x = w;
                            let mut min_y = h;
                            let mut max_x: u32 = 0;
                            let mut max_y: u32 = 0;
                            let mut content_px: u64 = 0;
                            for y in 0..h {
                                for x in 0..w {
                                    let p = rgba.get_pixel(x, y);
                                    if p[3] > 8 {
                                        content_px += 1;
                                        if x < min_x { min_x = x; }
                                        if y < min_y { min_y = y; }
                                        if x > max_x { max_x = x; }
                                        if y > max_y { max_y = y; }
                                    }
                                }
                            }
                            if content_px == 0 {
                                println!(
                                    "bbox probe: path={path} dims={w}x{h} content_bbox=0:0:0:0 content_px=0 pct=0.00"
                                );
                            } else {
                                let bw = max_x - min_x + 1;
                                let bh = max_y - min_y + 1;
                                let total = (w as f64) * (h as f64);
                                let pct = (content_px as f64) / total * 100.0;
                                println!(
                                    "bbox probe: path={path} dims={w}x{h} content_bbox={min_x}:{min_y}:{bw}:{bh} content_px={content_px} pct={pct:.2}"
                                );
                            }
                        }
                        Err(e) => {
                            println!("bbox probe: path={path} decode_error={e}");
                        }
                    }
                }
                None => {
                    println!("bbox probe: path={path} -> None");
                }
            }
        }
    }

    /// Probe normalize_to_square_png: verify sparse glyph icons get
    /// content-aware upscaling (output fill ratio ~90%+), full-bleed
    /// icons (notepad) remain unchanged.
    #[test]
    fn probe_sparse_vs_fullbleed_after_normalize() {
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                0,
            );
        }

        let cases: Vec<(&str, &str)> = vec![
            ("magnify", "shell:AppsFolder\\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\magnify.exe"),
            ("jpegview", "shell:AppsFolder\\{6D809377-6AF0-444B-8957-A3773F02200E}\\JPEGView\\JPEGView.exe"),
            ("notepad", "C:\\Windows\\System32\\notepad.exe"),
        ];

        // Helper: scan image, return (fill_pct, core_bbox_w, core_bbox_h, core_fill_pct)
        fn scan_icon(rgba: &image::RgbaImage) -> (f64, f64, Option<(u32,u32,u32,u32)>) {
            let (w, h) = rgba.dimensions();
            let total = (w * h) as f64;
            let mut content_px: u64 = 0;
            let mut min_x = w; let mut min_y = h;
            let mut max_x: u32 = 0; let mut max_y: u32 = 0;
            let mut core_px: u64 = 0;
            let mut cmin_x = w; let mut cmin_y = h;
            let mut cmax_x: u32 = 0; let mut cmax_y: u32 = 0;
            for y in 0..h {
                for x in 0..w {
                    let a = rgba.get_pixel(x, y)[3];
                    if a > 8 {
                        content_px += 1;
                        if x < min_x { min_x = x; }
                        if y < min_y { min_y = y; }
                        if x > max_x { max_x = x; }
                        if y > max_y { max_y = y; }
                    }
                    if a > 128 {
                        core_px += 1;
                        if x < cmin_x { cmin_x = x; }
                        if y < cmin_y { cmin_y = y; }
                        if x > cmax_x { cmax_x = x; }
                        if y > cmax_y { cmax_y = y; }
                    }
                }
            }
            let fill_pct = content_px as f64 / total * 100.0;
            if core_px > 0 && cmax_x >= cmin_x && cmax_y >= cmin_y {
                let bw = cmax_x - cmin_x + 1;
                let bh = cmax_y - cmin_y + 1;
                let core_area = (bw as f64) * (bh as f64);
                let core_fill = core_px as f64 / core_area * 100.0;
                (fill_pct, core_fill, Some((cmin_x, cmin_y, bw, bh)))
            } else {
                (fill_pct, 0.0, None)
            }
        }

        for (label, path) in &cases {
            let pb = PathBuf::from(path);
            let Some(raw_bytes) = decode_png(&pb) else {
                println!("normalize probe [{label}]: path={path} -> decode_png returned None, skipping");
                continue;
            };
            let Ok(img) = image::load_from_memory(&raw_bytes) else {
                println!("normalize probe [{label}]: image decode failed, skipping");
                continue;
            };
            let rgba = img.into_rgba8();

            let (before_fill, before_core_fill, before_bbox) = scan_icon(&rgba);
            let (w, h) = rgba.dimensions();
            match before_bbox {
                Some((_bx, _by, bw, bh)) => {
                    println!("normalize probe [{label}] BEFORE: dims={w}x{h} fill={before_fill:.2}% core_bbox={bw}x{bh} core_fill={before_core_fill:.2}%");
                }
                None => {
                    println!("normalize probe [{label}] BEFORE: dims={w}x{h} fill={before_fill:.2}% (no core content)");
                }
            }

            let Some(normalized) = normalize_to_square_png(rgba) else {
                println!("normalize probe [{label}]: normalize_to_square_png returned None");
                continue;
            };

            let norm_img = image::load_from_memory(&normalized).expect("normalized PNG decodes");
            let norm_rgba = norm_img.into_rgba8();
            let (nw, nh) = norm_rgba.dimensions();
            let (after_fill, after_core_fill, after_bbox) = scan_icon(&norm_rgba);
            match after_bbox {
                Some((_bx, _by, bw, bh)) => {
                    println!("normalize probe [{label}]  AFTER: dims={nw}x{nh} fill={after_fill:.2}% core_bbox={bw}x{bh} core_fill={after_core_fill:.2}% png_bytes={}", normalized.len());
                }
                None => {
                    println!("normalize probe [{label}]  AFTER: dims={nw}x{nh} fill={after_fill:.2}% (no core content) png_bytes={}", normalized.len());
                }
            }
        }
    }

    #[test]
    fn probe_jpegview_and_missing() {
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                0,
            );
        }

        let candidates = vec![
            "C:\\Program Files\\JPEGView\\JPEGView.exe".to_string(),
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\JPEGView\\JPEGView.lnk"
                .to_string(),
            "C:\\Windows\\System32\\nonexistent_nex_probe.lnk".to_string(),
        ];

        for path in &candidates {
            let exists = std::path::Path::new(path).exists();
            if !exists {
                println!("jpegview probe: path={path} -> file does not exist, testing fallback");
            }
            let pb = PathBuf::from(path);
            match decode_png(&pb) {
                Some(bytes) => {
                    let non_empty = !bytes.is_empty();
                    match image::load_from_memory(&bytes) {
                        Ok(img) => {
                            let (w, h) = (img.width(), img.height());
                            println!(
                                "jpegview probe: path={path} png_bytes={} dims={}x{} non_empty={non_empty}",
                                bytes.len(), w, h,
                            );
                        }
                        Err(e) => {
                            println!(
                                "jpegview probe: path={path} png_bytes={} decode_error={e} non_empty={non_empty}",
                                bytes.len(),
                            );
                        }
                    }
                }
                None => {
                    println!("jpegview probe: path={path} -> None");
                }
            }
        }
    }
}
