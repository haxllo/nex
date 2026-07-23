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
/// Extraction request size for PrivateExtractIconsW (primary high-res
/// path). 256px is the Windows jumbo icon size; the Lanczos downscale
/// to TARGET_ICON_SIZE (128) produces a clean, sharp result on HiDPI.
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

/// Normalize large RGBA image down to consistent square:
/// Lanczos-resize preserving aspect to fit within TARGET × TARGET,
/// then center-composite onto transparent canvas. Every result row
/// gets uniform square icon with consistent padding — Raycast look.
///
/// Does NOT upscale sources smaller than TARGET — those pass through
/// at native size (CSS handles display). Upscaling small sources adds
/// Lanczos blur without benefit for the 30px CSS display container.
fn normalize_to_square_png(img: image::RgbaImage) -> Option<Vec<u8>> {
    let target = TARGET_ICON_SIZE;
    let (w, h) = (img.width(), img.height());

    // Source already at or below target → pass through at native size.
    // The CSS container (30px, object-fit: contain) handles display.
    if w <= target && h <= target {
        return rgba_to_png(img);
    }

    // Source larger than target → downscale to fit within target,
    // center on transparent canvas (consistent padding).
    use image::imageops::{self, FilterType};
    // imageops::resize preserves aspect ratio (fits within target×target)
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

#[cfg(target_os = "windows")]
/// Resolve a .lnk shortcut to its target executable path by parsing
/// the Shell Link binary format (MS-SHLLINK). Extracts the
/// `LocalBasePath` from the `LinkInfo` section when available.
/// Returns None for non-.lnk files or shortcuts without a local
/// base path (e.g. AppUserModelID-based or network paths).
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
/// Extract the best quality icon from a file using `ExtractIconExW`.
/// Tries to get the largest available icon size for sharper rendering.
#[cfg(target_os = "windows")]
fn extract_shell_icon_png(shell_path: &str) -> Option<Vec<u8>> {
    // For .lnk shortcuts, resolve the target executable so we can
    // attempt high-resolution extraction from the actual .exe.
    let resolved_target = if shell_path.to_ascii_lowercase().ends_with(".lnk") {
        resolve_lnk_target(shell_path)
    } else {
        None
    };

    // Primary high-res path: try on resolved .exe first, then on
    // the original path (for direct .exe/.ico/.dll).
    let high_res_paths = resolved_target.as_deref().into_iter().chain(std::iter::once(shell_path));
    for path in high_res_paths {
        if !path.starts_with("shell:") {
            if let Some(png) = private_extract_icons_png(path) {
                return Some(png);
            }
        }
    }

    use windows_sys::Win32::UI::Shell::ExtractIconExW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    // Convert shell path to file path for ExtractIconExW
    // shell:AppsFolder\{app_id} needs special handling
    let file_path = if shell_path.starts_with("shell:") {
        // For shell URIs, fall back to SHGetFileInfo
        return extract_shell_icon_fallback(shell_path);
    } else {
        shell_path
    };

    let wide: Vec<u16> = file_path.encode_utf16().chain(std::iter::once(0)).collect();

    // First call: get the number of icons
    let icon_count = unsafe { ExtractIconExW(wide.as_ptr(), 0, std::ptr::null_mut(), std::ptr::null_mut(), 0) };
    if icon_count <= 0 {
        return extract_shell_icon_fallback(shell_path);
    }

    // Allocate arrays for icon handles
    let mut large_icons: Vec<HICON> = vec![std::ptr::null_mut(); icon_count as usize];
    let mut small_icons: Vec<HICON> = vec![std::ptr::null_mut(); icon_count as usize];

    // Second call: get the actual icon handles
    let extracted = unsafe {
        ExtractIconExW(
            wide.as_ptr(),
            0,
            large_icons.as_mut_ptr(),
            small_icons.as_mut_ptr(),
            icon_count as u32,
        )
    };

    if extracted == 0 {
        return extract_shell_icon_fallback(shell_path);
    }

    // Use the first large icon (typically the highest quality)
    let best_hicon = large_icons.iter().find(|&&h| !h.is_null()).copied();

    let result = if let Some(hicon) = best_hicon {
        icon_to_rgba_png(hicon, 32)
    } else {
        None
    };

    // Clean up all icon handles
    for &hicon in &large_icons {
        if !hicon.is_null() {
            unsafe { DestroyIcon(hicon); }
        }
    }
    for &hicon in &small_icons {
        if !hicon.is_null() {
            unsafe { DestroyIcon(hicon); }
        }
    }

    result
}

/// Fallback using SHGetFileInfo for shell URIs.
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

    let png = icon_to_rgba_png(sfi.hIcon as windows_sys::Win32::UI::WindowsAndMessaging::HICON, 32);
    unsafe { DestroyIcon(sfi.hIcon); }
    png
}

#[cfg(not(target_os = "windows"))]
fn extract_shell_icon_png(_shell_path: &str) -> Option<Vec<u8>> {
    None
}

/// High-resolution icon extraction using PrivateExtractIconsW.
/// Requests a 256×256 HICON from .exe/.ico/.dll files that have
/// large icon resources. Returns None for paths without embedded
/// icon resources (e.g. .lnk, directories) — the caller falls back
/// to ExtractIconExW / SHGetFileInfoW.
///
/// PrivateExtractIconsW can return up to the exact size we request
/// (256) if the source has a matching icon resource, giving crisp
/// results on HiDPI displays even after normalization to 128×128.
#[cfg(target_os = "windows")]
fn private_extract_icons_png(path: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hicon: HICON = std::ptr::null_mut();

    let count = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PrivateExtractIconsW(
            wide.as_ptr(),
            0,               // first icon index
            EXTRACT_ICON_SIZE,
            EXTRACT_ICON_SIZE,
            &mut hicon,
            std::ptr::null_mut(),
            1,               // request one icon
            0,               // default flags
        )
    };

    if count == 0 || hicon.is_null() {
        return None;
    }

    let png = icon_to_rgba_png(hicon, EXTRACT_ICON_SIZE);
    unsafe { DestroyIcon(hicon); }
    png
}

#[cfg(not(target_os = "windows"))]
fn private_extract_icons_png(_path: &str) -> Option<Vec<u8>> {
    None
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
}
