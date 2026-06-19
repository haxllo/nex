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
    rgba_to_png(img.into_rgba8())
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
fn icon_to_rgba_png(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, CreateDIBSection, SelectObject, DeleteObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::DrawIconEx;

    const ICON_SIZE: i32 = 32;
    unsafe {
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        if hdc.is_null() { return None; }

        // Create a 32-bit BGRA DIB section to render the icon into.
        let mut header: BITMAPINFOHEADER = std::mem::zeroed();
        header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.biWidth = ICON_SIZE;
        header.biHeight = -ICON_SIZE; // top-down
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
        let pixel_count = (ICON_SIZE * ICON_SIZE) as usize;
        std::ptr::write_bytes(bits, 0, pixel_count * 4);

        DrawIconEx(hdc, 0, 0, hicon, ICON_SIZE, ICON_SIZE, 0, std::ptr::null_mut(), 0x0003);

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

        let img = image::RgbaImage::from_raw(ICON_SIZE as u32, ICON_SIZE as u32, rgba)?;
        rgba_to_png(img)
    }
}

#[cfg(not(target_os = "windows"))]
fn icon_to_rgba_png(_hicon: *mut std::ffi::c_void) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "windows")]
fn extract_shell_icon_png(shell_path: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::Shell::{
        SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW,
        SHGFI_ICON, SHGFI_LARGEICON, SHGFI_PIDL,
    };
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;
    use windows_sys::Win32::System::Com::CoTaskMemFree;

    let wide: Vec<u16> = shell_path.encode_utf16().chain(std::iter::once(0)).collect();

    // Parse the display name (shell URI or filesystem path) to a PIDL.
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

    let png = icon_to_rgba_png(sfi.hIcon as windows_sys::Win32::UI::WindowsAndMessaging::HICON);
    unsafe { DestroyIcon(sfi.hIcon); }
    png
}

#[cfg(not(target_os = "windows"))]
fn extract_shell_icon_png(_shell_path: &str) -> Option<Vec<u8>> {
    None
}

pub(crate) fn prefetch_rows(cache: &IconCache, rows: &[OverlayRow]) {
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = windows_sys::Win32::System::Com::CoInitializeEx(
            std::ptr::null(),
            2, // COINIT_APARTMENTTHREADED
        );
    }
    for row in rows {
        if !row.icon_path.is_empty() {
            cache.png_bytes(&row.icon_path);
        }
    }
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
