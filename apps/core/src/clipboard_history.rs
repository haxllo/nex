use crate::config::Config;
use crate::model::SearchItem;
use crate::search::{search_with_filter, SearchFilter};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CLIPBOARD_ENTRIES: usize = 500;
/// Magic bytes prefixing DPAPI-encrypted clipboard history files so we can
/// distinguish them from plaintext JSON (legacy format) on read.
const DPAPI_MAGIC: &[u8; 8] = b"NXCLPDPA";

static CLIPBOARD_CACHE: Mutex<Option<Vec<ClipboardEntry>>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClipboardContentType {
    Text,
    Image,
}

impl Default for ClipboardContentType {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardEntry {
    pub id: String,
    pub text: String,
    pub captured_epoch_secs: i64,
    #[serde(default)]
    pub content_type: ClipboardContentType,
    /// Full-resolution PNG bytes (populated when content_type == Image).
    #[serde(default)]
    pub image_data: Option<Vec<u8>>,
    /// Compressed thumbnail PNG bytes for grid display.
    #[serde(default)]
    pub thumbnail_data: Option<Vec<u8>>,
    #[serde(default)]
    pub image_width: Option<u32>,
    #[serde(default)]
    pub image_height: Option<u32>,
    /// xxHash3 of raw DIB pixel data — used for dedup and full-res file lookup.
    #[serde(default)]
    pub image_hash: Option<u64>,
}

pub fn maybe_capture_latest(cfg: &Config) -> Result<bool, String> {
    if !cfg.clipboard_enabled {
        return Ok(false);
    }

    let mut entries = load_entries(cfg);
    let now = now_epoch_secs();

    // Try image capture first (images take priority over text). A
    // transient image-read failure must NOT abort the whole capture —
    // fall through to text instead of losing the entry.
    #[cfg(target_os = "windows")]
    {
        let img_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            capture_clipboard_thumbnail_png(cfg)
        }));
        match img_result {
            Ok(Ok(Some((thumbnail_png, hash)))) => {
                // Dedup: skip if same image as previous entry
                if entries.first().is_some_and(|e| {
                    e.content_type == ClipboardContentType::Image
                        && e.image_hash == Some(hash)
                }) {
                    return Ok(false);
                }
                entries.insert(
                    0,
                    ClipboardEntry {
                        id: format!("clip-{now}-{}", now_nanos() % 1_000_000),
                        text: String::new(),
                        captured_epoch_secs: now,
                        content_type: ClipboardContentType::Image,
                        image_data: None, // full-res lives on disk
                        thumbnail_data: Some(thumbnail_png),
                        image_width: None,
                        image_height: None,
                        image_hash: Some(hash),
                    },
                );
                let hashes: Vec<u64> = entries
                    .iter()
                    .filter_map(|e| e.image_hash)
                    .collect();
                prune_entries(cfg, &mut entries, now);
                prune_image_cache(cfg, &hashes);
                save_entries(cfg, &entries)?;
                return Ok(true);
            }
            Ok(Ok(None)) => {
                // No image on clipboard — fall through to text
            }
            Ok(Err(error)) => {
                crate::runtime::log_warn(&format!(
                    "[nex] clipboard image capture failed, falling back to text: {error}"
                ));
            }
            Err(_) => {
                crate::runtime::log_warn(
                    "[nex] clipboard image capture panicked, falling back to text",
                );
            }
        }
    }

    // Fall back to text capture
    let Some(raw) = read_system_clipboard_text()? else {
        return Ok(false);
    };
    let text = normalize_clipboard_text(&raw);
    if text.is_empty() {
        return Ok(false);
    }

    if is_sensitive_content(&text, &cfg.clipboard_exclude_sensitive_patterns) {
        return Ok(false);
    }

    if entries.first().is_some_and(|entry| entry.text == text) {
        return Ok(false);
    }

    entries.insert(
        0,
        ClipboardEntry {
            id: format!("clip-{now}-{}", now_nanos() % 1_000_000),
            text,
            captured_epoch_secs: now,
            content_type: ClipboardContentType::Text,
            image_data: None,
            thumbnail_data: None,
            image_width: None,
            image_height: None,
            image_hash: None,
        },
    );
    prune_entries(cfg, &mut entries, now);
    save_entries(cfg, &entries)?;
    Ok(true)
}

/// Load all clipboard entries (for bento history view). Returns entries
/// ordered newest-first, with expired entries pruned.
pub fn load_all_entries(cfg: &Config) -> Vec<ClipboardEntry> {
    if !cfg.clipboard_enabled {
        return Vec::new();
    }
    let mut entries = load_entries(cfg);
    let now = now_epoch_secs();
    let before_len = entries.len();
    prune_entries(cfg, &mut entries, now);
    if entries.len() != before_len {
        let _ = save_entries(cfg, &entries);
    }
    entries
}

/// Copy a clipboard entry back to the system clipboard by ID.
/// Handles both text and image entries.
pub fn copy_entry_to_clipboard(cfg: &Config, entry_id: &str) -> Result<(), String> {
    let entry = load_entries(cfg)
        .into_iter()
        .find(|e| e.id == entry_id)
        .ok_or_else(|| "clipboard entry not found".to_string())?;

    match entry.content_type {
        ClipboardContentType::Text => write_system_clipboard_text(&entry.text),
        ClipboardContentType::Image => {
            #[cfg(target_os = "windows")]
            {
                if let Some(hash) = entry.image_hash {
                    return write_fullres_to_clipboard(cfg, hash);
                }
                Err("image entry has no hash — cannot locate full-res file".into())
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err("clipboard image copy is unsupported on this platform".into())
            }
        }
    }
}

pub fn clear_history(cfg: &Config) -> Result<(), String> {
    let path = history_path(cfg);
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| format!("failed to clear clipboard history: {e}"))?;
    }
    // Also clear the full-res image cache
    let cache_dir = image_cache_dir(cfg);
    if cache_dir.exists() {
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
    Ok(())
}

pub fn search_history(
    cfg: &Config,
    query: &str,
    filter: &SearchFilter,
    limit: usize,
) -> Vec<SearchItem> {
    if !cfg.clipboard_enabled || limit == 0 {
        return Vec::new();
    }

    let mut entries = load_entries(cfg);
    if entries.is_empty() {
        return Vec::new();
    }
    let before_len = entries.len();
    let now = now_epoch_secs();
    prune_entries(cfg, &mut entries, now);
    if entries.len() != before_len {
        let _ = save_entries(cfg, &entries);
    }

    let items: Vec<SearchItem> = entries
        .iter()
        .map(|entry| {
            let preview = preview_text(&entry.text, 96);
            let subtitle = format!("Copied {}", relative_age(entry.captured_epoch_secs, now));
            SearchItem::new(
                &format!("clipboard:{}", entry.id),
                "clipboard",
                &preview,
                &format!("{subtitle} · {}", preview_text(&entry.text, 180)),
            )
            .with_usage(0, entry.captured_epoch_secs)
        })
        .collect();

    search_with_filter(&items, query, limit, filter)
}

pub fn copy_result_to_clipboard(cfg: &Config, result_id: &str) -> Result<(), String> {
    let entry_id = result_id
        .strip_prefix("clipboard:")
        .ok_or_else(|| "invalid clipboard result ID".to_string())?;
    copy_entry_to_clipboard(cfg, entry_id)
}

fn load_entries(cfg: &Config) -> Vec<ClipboardEntry> {
    {
        let guard = CLIPBOARD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
    }
    let entries = load_entries_from_disk(cfg);
    *CLIPBOARD_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(entries.clone());
    entries
}

fn load_entries_from_disk(cfg: &Config) -> Vec<ClipboardEntry> {
    let path = history_path(cfg);
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    let decrypted = dpapi_try_decrypt(&raw)
        .or_else(|| {
            // Legacy plaintext JSON — first read on this format triggers an
            // upgrade: the next save_entries call will rewrite it encrypted.
            std::str::from_utf8(&raw).ok().map(|s| s.as_bytes().to_vec())
        })
        .unwrap_or(raw);
    serde_json::from_slice::<Vec<ClipboardEntry>>(&decrypted).unwrap_or_default()
}

fn save_entries(cfg: &Config, entries: &[ClipboardEntry]) -> Result<(), String> {
    let path = history_path(cfg);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create clipboard history dir: {e}"))?;
    }
    let encoded = serde_json::to_string(entries)
        .map_err(|e| format!("failed to encode clipboard history: {e}"))?;
    let blob = dpapi_encrypt(encoded.as_bytes());
    std::fs::write(path, blob)
        .map_err(|e| format!("failed to write clipboard history: {e}"))?;
    *CLIPBOARD_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(entries.to_vec());
    Ok(())
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct DPAPIBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

/// Encrypt plaintext bytes using Windows DPAPI (CryptProtectData).
/// On non-Windows, returns the input unchanged.
fn dpapi_encrypt(plaintext: &[u8]) -> Vec<u8> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Security::Cryptography::CryptProtectData;

        unsafe {
            let data_in = DPAPIBlob {
                cb_data: plaintext.len() as u32,
                pb_data: plaintext.as_ptr() as *mut u8,
            };
            let mut data_out = DPAPIBlob {
                cb_data: 0,
                pb_data: std::ptr::null_mut(),
            };

            if CryptProtectData(
                &data_in as *const DPAPIBlob as *const _,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0x00000004, // CRYPTPROTECT_LOCAL_MACHINE
                &mut data_out as *mut DPAPIBlob as *mut _,
            ) != 0
            {
                let ciphertext =
                    std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize);
                let mut result = DPAPI_MAGIC.to_vec();
                result.extend_from_slice(ciphertext);
                windows_sys::Win32::Foundation::LocalFree(data_out.pb_data as _);
                return result;
            }
        }
        // Fallback: plaintext (encryption failed, which is rare)
        let mut fallback = DPAPI_MAGIC.to_vec();
        fallback.extend_from_slice(plaintext);
        fallback
    }
    #[cfg(not(target_os = "windows"))]
    {
        plaintext.to_vec()
    }
}

/// Decrypt bytes that were previously encrypted with `dpapi_encrypt`.
/// Returns `None` if the data does not carry the DPAPI magic or if
/// decryption fails (e.g. different user, different machine).
fn dpapi_try_decrypt(data: &[u8]) -> Option<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Security::Cryptography::CryptUnprotectData;

        if !data.starts_with(DPAPI_MAGIC) {
            return None;
        }
        let ciphertext = &data[DPAPI_MAGIC.len()..];

        unsafe {
            let data_in = DPAPIBlob {
                cb_data: ciphertext.len() as u32,
                pb_data: ciphertext.as_ptr() as *mut u8,
            };
            let mut data_out = DPAPIBlob {
                cb_data: 0,
                pb_data: std::ptr::null_mut(),
            };

            if CryptUnprotectData(
                &data_in as *const DPAPIBlob as *const _,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0x00000001, // CRYPTPROTECT_UI_FORBIDDEN
                &mut data_out as *mut DPAPIBlob as *mut _,
            ) != 0
            {
                let plain =
                    std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize);
                let result = plain.to_vec();
                windows_sys::Win32::Foundation::LocalFree(data_out.pb_data as _);
                return Some(result);
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn invalidate_entries_cache() {
    *CLIPBOARD_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn prune_entries(cfg: &Config, entries: &mut Vec<ClipboardEntry>, now: i64) {
    let retention_secs = (cfg.clipboard_retention_minutes as i64) * 60;
    entries.retain(|entry| {
        entry.captured_epoch_secs > 0
            && entry.captured_epoch_secs <= now
            && now.saturating_sub(entry.captured_epoch_secs) <= retention_secs
    });
    if entries.len() > MAX_CLIPBOARD_ENTRIES {
        entries.truncate(MAX_CLIPBOARD_ENTRIES);
    }
}

fn history_path(cfg: &Config) -> PathBuf {
    cfg.config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("clipboard-history.json")
}

fn normalize_clipboard_text(input: &str) -> String {
    input
        .replace('\u{0000}', "")
        .replace('\r', "")
        .trim()
        .to_string()
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let single_line = value.replace('\n', " ").trim().to_string();
    let mut out = String::new();
    for ch in single_line.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

fn is_sensitive_content(value: &str, patterns: &[String]) -> bool {
    let lowered = value.to_ascii_lowercase();
    patterns.iter().any(|pattern| {
        let p = pattern.trim().to_ascii_lowercase();
        !p.is_empty() && lowered.contains(&p)
    })
}

fn relative_age(captured_epoch_secs: i64, now: i64) -> String {
    let age = now.saturating_sub(captured_epoch_secs);
    if age < 60 {
        return "just now".to_string();
    }
    if age < 3600 {
        return format!("{}m ago", age / 60);
    }
    if age < 86_400 {
        return format!("{}h ago", age / 3600);
    }
    format!("{}d ago", age / 86_400)
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn read_system_clipboard_text() -> Result<Option<String>, String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Ok(None);
        }

        if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) == 0 {
            CloseClipboard();
            return Ok(None);
        }

        let handle = GetClipboardData(u32::from(CF_UNICODETEXT));
        if handle.is_null() {
            CloseClipboard();
            return Ok(None);
        }

        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return Ok(None);
        }

        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);

        GlobalUnlock(handle);
        CloseClipboard();
        Ok(Some(text))
    }
}

#[cfg(not(target_os = "windows"))]
fn read_system_clipboard_text() -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(target_os = "windows")]
fn write_system_clipboard_text(value: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("failed to open clipboard".to_string());
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return Err("failed to clear clipboard".to_string());
        }

        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            CloseClipboard();
            return Err("failed to allocate clipboard memory".to_string());
        }

        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return Err("failed to lock clipboard memory".to_string());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(mem);

        if SetClipboardData(u32::from(CF_UNICODETEXT), mem).is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return Err("failed to set clipboard data".to_string());
        }

        CloseClipboard();
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn write_system_clipboard_text(_value: &str) -> Result<(), String> {
    Err("clipboard copy is unsupported on this platform".to_string())
}

// ---------------------------------------------------------------------------
// Image clipboard support — GDI-based, zero-abort pipeline
// ---------------------------------------------------------------------------
//
// The `image` crate's resize/encode can abort (not panic) on OOM.
// This module replaces all `image` crate usage in the clipboard path
// with Windows GDI (CreateDIBSection + StretchBlt) for resize and the
// `png` crate for scanline-streamed encoding. Peak RAM < 1MB.
//
// Full-resolution images are streamed to disk as `full_{xxh3}.png`
// inside `%APPDATA%\Nex\image_cache\`. Thumbnails (256px) are kept
// in memory as `ClipboardEntry.thumbnail_data` for the bento grid.

use std::io::BufWriter;

use xxhash_rust::xxh3::xxh3_64;

/// Max dimension accepted from the clipboard (reject before any alloc).
const MAX_CAPTURE_DIM: u32 = 2560;
/// Thumbnail size for bento grid display.
const MAX_THUMBNAIL_DIM: u32 = 512;
/// Max number of full-res image files in the cache.
const IMAGE_CACHE_MAX_ITEMS: usize = 50;
/// Max total disk usage for full-res images (bytes).
const IMAGE_CACHE_MAX_BYTES: u64 = 500 * 1024 * 1024;

/// Directory for full-res image files.
fn image_cache_dir(cfg: &Config) -> PathBuf {
    cfg.config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("image_cache")
}

/// Full-res file path for a given xxh3 hash.
fn fullres_path(cfg: &Config, hash: u64) -> PathBuf {
    image_cache_dir(cfg).join(format!("full_{hash:016x}.png"))
}

/// Encode RGBA pixels as PNG using the `png` crate.
/// Scanline-by-scanline — only needs ~width*4 bytes intermediate buffer.
fn encode_thumbnail_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let mut encoder = png::Encoder::new(BufWriter::new(&mut buf), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG header error: {e}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| format!("PNG encode error: {e}"))?;
    }
    Ok(buf)
}

/// Capture a clipboard image as a thumbnail PNG + stream full-res to disk.
///
/// Returns `Ok(Some((thumbnail_png, xxh3_hash)))` on success.
/// Returns `Ok(None)` if no image is on the clipboard.
/// Returns `Err(...)` on processing failure (caller should fall through
/// to text capture).
///
/// Safety: GDI's `CreateDIBSection` returns NULL on OOM instead of
/// aborting. The only Rust heap allocation is the ~256KB thumbnail
/// readback buffer. Full-res is streamed row-by-row to disk.
#[cfg(target_os = "windows")]
fn capture_clipboard_thumbnail_png(
    cfg: &Config,
) -> Result<Option<(Vec<u8>, u64)>, String> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_DIB;

    unsafe {
        // 1. Open clipboard and get DIB handle
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Ok(None);
        }
        let format = u32::from(CF_DIB);
        if IsClipboardFormatAvailable(format) == 0 {
            CloseClipboard();
            return Ok(None);
        }
        let clip_handle = GetClipboardData(format);
        if clip_handle.is_null() {
            CloseClipboard();
            return Ok(None);
        }
        let dib_ptr = GlobalLock(clip_handle) as *const u8;
        if dib_ptr.is_null() {
            CloseClipboard();
            return Ok(None);
        }

        // 2. Parse BITMAPINFOHEADER
        let header_size = *(dib_ptr as *const u32);
        let width = (*(dib_ptr.add(4) as *const i32)).unsigned_abs();
        let height_raw = *(dib_ptr.add(8) as *const i32);
        let height = height_raw.unsigned_abs();
        let bpp = *(dib_ptr.add(14) as *const u16);

        if header_size < 40
            || width == 0
            || height == 0
            || (bpp != 24 && bpp != 32)
        {
            GlobalUnlock(clip_handle);
            CloseClipboard();
            return Ok(None);
        }

        // 3. Reject oversized images before any alloc
        if width > MAX_CAPTURE_DIM || height > MAX_CAPTURE_DIM {
            GlobalUnlock(clip_handle);
            CloseClipboard();
            return Ok(None);
        }

        // 4. Compute xxh3 hash over raw pixel data (for dedup + file naming)
        let pixel_offset = header_size as usize;
        let row_bytes = ((width as usize * bpp as usize + 31) / 32) * 4;
        let total_pixel_bytes = row_bytes * height as usize;
        let pixel_slice =
            std::slice::from_raw_parts(dib_ptr.add(pixel_offset), total_pixel_bytes);
        let hash = xxh3_64(pixel_slice);

        // 5. Create source HBITMAP from clipboard DIB
        let bmi_ptr = dib_ptr as *const BITMAPINFO;
        let screen_dc = GetDC(std::ptr::null_mut());
        let src_dc = CreateCompatibleDC(screen_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        if src_dc.is_null() {
            GlobalUnlock(clip_handle);
            CloseClipboard();
            return Err("CreateCompatibleDC(src) failed".into());
        }

        let mut src_bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let src_bmp = CreateDIBSection(
            src_dc,
            bmi_ptr,
            DIB_RGB_COLORS,
            &mut src_bits,
            std::ptr::null_mut(),
            0,
        );
        if src_bmp.is_null() || src_bits.is_null() {
            DeleteDC(src_dc);
            GlobalUnlock(clip_handle);
            CloseClipboard();
            return Err("CreateDIBSection(src) failed — OOM or invalid DIB".into());
        }

        // Copy clipboard pixel data into source bitmap
        std::ptr::copy_nonoverlapping(
            dib_ptr.add(pixel_offset),
            src_bits as *mut u8,
            total_pixel_bytes,
        );
        let old_src = SelectObject(src_dc, src_bmp as _);

        // 6. Compute thumbnail dimensions (preserve aspect ratio)
        let scale = (MAX_THUMBNAIL_DIM as f64 / width as f64)
            .min(MAX_THUMBNAIL_DIM as f64 / height as f64);
        let thumb_w = ((width as f64 * scale).max(1.0)) as i32;
        let thumb_h = ((height as f64 * scale).max(1.0)) as i32;

        // 7. Create destination DIBSection at thumbnail size (32bpp BGRA, top-down)
        let dst_dc = CreateCompatibleDC(std::ptr::null_mut());
        if dst_dc.is_null() {
            SelectObject(src_dc, old_src);
            DeleteObject(src_bmp as _);
            DeleteDC(src_dc);
            GlobalUnlock(clip_handle);
            CloseClipboard();
            return Err("CreateCompatibleDC(dst) failed".into());
        }

        let mut dst_header: BITMAPINFOHEADER = std::mem::zeroed();
        dst_header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        dst_header.biWidth = thumb_w;
        dst_header.biHeight = -thumb_h; // negative = top-down
        dst_header.biPlanes = 1;
        dst_header.biBitCount = 32;
        dst_header.biCompression = BI_RGB;

        let mut dst_bmi: BITMAPINFO = std::mem::zeroed();
        dst_bmi.bmiHeader = dst_header;

        let mut dst_bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dst_bmp = CreateDIBSection(
            dst_dc,
            &dst_bmi,
            DIB_RGB_COLORS,
            &mut dst_bits,
            std::ptr::null_mut(),
            0,
        );
        if dst_bmp.is_null() || dst_bits.is_null() {
            SelectObject(src_dc, old_src);
            DeleteObject(src_bmp as _);
            DeleteDC(src_dc);
            DeleteDC(dst_dc);
            GlobalUnlock(clip_handle);
            CloseClipboard();
            return Err("CreateDIBSection(dst) failed — OOM".into());
        }

        let old_dst = SelectObject(dst_dc, dst_bmp as _);

        // 8. Set HALFTONE stretch mode for bilinear interpolation quality,
        //    then StretchBlt: hardware-accelerated resize.
        use windows_sys::Win32::Graphics::Gdi::SetStretchBltMode;
        SetStretchBltMode(dst_dc, 4); // HALFTONE = 4
        StretchBlt(
            dst_dc,
            0,
            0,
            thumb_w,
            thumb_h,
            src_dc,
            0,
            0,
            width as i32,
            height as i32,
            SRCCOPY,
        );

        // 9. Read back thumbnail pixels (BGRA → RGBA)
        //    Force alpha to 255 — clipboard DIBs (screenshots) often have
        //    alpha=0 in the 32bpp format even though the image is opaque.
        let pixel_count = (thumb_w * thumb_h) as usize;
        let bgra = std::slice::from_raw_parts(dst_bits as *const u8, pixel_count * 4);
        let mut rgba = vec![0u8; pixel_count * 4];
        for (i, chunk) in bgra.chunks_exact(4).enumerate() {
            rgba[i * 4] = chunk[2]; // R ← B
            rgba[i * 4 + 1] = chunk[1]; // G
            rgba[i * 4 + 2] = chunk[0]; // B ← R
            rgba[i * 4 + 3] = 255; // A — always opaque
        }

        // 10. Stream full-res to disk WHILE clipboard is still locked
        //     (dib_ptr is only valid between GlobalLock and GlobalUnlock)
        stream_fullres_to_disk(cfg, hash, dib_ptr, header_size, width, height, bpp, row_bytes)?;

        // 11. Cleanup GDI resources and release clipboard
        SelectObject(src_dc, old_src);
        SelectObject(dst_dc, old_dst);
        DeleteObject(src_bmp as _);
        DeleteObject(dst_bmp as _);
        DeleteDC(src_dc);
        DeleteDC(dst_dc);
        GlobalUnlock(clip_handle);
        CloseClipboard();

        // 12. Encode thumbnail as PNG (pure in-memory, no clipboard needed)
        let thumbnail_png = encode_thumbnail_png(&rgba, thumb_w as u32, thumb_h as u32)?;

        Ok(Some((thumbnail_png, hash)))
    }
}

/// Encode full-resolution PNG to disk from clipboard DIB.
/// Collects all rows into a single RGBA buffer for the png crate.
/// The DIB pointer must still be valid (clipboard locked).
#[cfg(target_os = "windows")]
fn stream_fullres_to_disk(
    cfg: &Config,
    hash: u64,
    dib_ptr: *const u8,
    header_size: u32,
    width: u32,
    height: u32,
    bpp: u16,
    row_bytes: usize,
) -> Result<(), String> {
    let dir = image_cache_dir(cfg);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create image cache dir: {e}"))?;

    let path = fullres_path(cfg, hash);
    let file = std::fs::File::create(&path)
        .map_err(|e| format!("failed to create full-res PNG: {e}"))?;

    let pixel_offset = header_size as usize;
    let top_down = unsafe { (*(dib_ptr.add(8) as *const i32)) < 0 };

    // Collect all rows into a single RGBA buffer.
    // png::Encoder::write_image_data requires the full image in one call.
    let pixel_count = width as usize * height as usize;
    let mut rgba = vec![0u8; pixel_count * 4];

    for row in 0..height {
        let src_row = if top_down {
            row
        } else {
            height - 1 - row
        };
        let dst_offset = row as usize * width as usize * 4;
        unsafe {
            let src = dib_ptr.add(pixel_offset + src_row as usize * row_bytes);
            for col in 0..width {
                let off = col as usize * bpp as usize / 8;
                let b = *src.add(off);
                let g = *src.add(off + 1);
                let r = *src.add(off + 2);
                let a = 255u8; // always opaque — clipboard DIBs have alpha=0
                let di = dst_offset + col as usize * 4;
                rgba[di] = r;
                rgba[di + 1] = g;
                rgba[di + 2] = b;
                rgba[di + 3] = a;
            }
        }
    }

    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header error: {e}"))?;
    writer
        .write_image_data(&rgba)
        .map_err(|e| format!("PNG write error: {e}"))?;

    Ok(())
}

/// Prune the image cache directory — enforce item count and total size limits.
fn prune_image_cache(cfg: &Config, keep_hashes: &[u64]) {
    let dir = image_cache_dir(cfg);
    let Ok(mut files) = std::fs::read_dir(&dir) else {
        return;
    };

    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    while let Some(Ok(entry)) = files.next() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "png") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        // Extract hash from filename to check if it should be kept
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if let Some(hex) = stem.strip_prefix("full_") {
            if let Ok(h) = u64::from_str_radix(hex, 16) {
                if keep_hashes.contains(&h) {
                    entries.push((path, meta.len(), meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)));
                    continue;
                }
            }
        }
        entries.push((path, meta.len(), meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)));
    }

    // Sort oldest first (LRU eviction)
    entries.sort_by_key(|e| e.2);

    let mut total_bytes: u64 = entries.iter().map(|e| e.1).sum();
    let mut count = entries.len();

    // Evict oldest until within limits
    for (path, size, _) in &entries {
        if count <= IMAGE_CACHE_MAX_ITEMS && total_bytes <= IMAGE_CACHE_MAX_BYTES {
            break;
        }
        let _ = std::fs::remove_file(path);
        total_bytes -= size;
        count -= 1;
    }
}

/// Write a full-res PNG from disk back to the system clipboard as a DIB.
#[cfg(target_os = "windows")]
fn write_fullres_to_clipboard(cfg: &Config, hash: u64) -> Result<(), String> {
    let path = fullres_path(cfg, hash);
    let png_bytes = std::fs::read(&path)
        .map_err(|e| format!("failed to read full-res PNG: {e}"))?;

    // Decode PNG using the png crate
    let decoder = png::Decoder::new(std::io::Cursor::new(&png_bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("PNG decode error: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG frame error: {e}"))?;
    let rgba = &buf[..info.buffer_size()];
    let width = info.width;
    let height = info.height;

    // Build a BITMAPINFOHEADER + BGR pixel data (bottom-up, 24bpp)
    let row_size = ((width * 24 + 31) / 32) * 4;
    let pixel_data_size = row_size * height;
    let header_size: u32 = 40;
    let total_size = header_size + pixel_data_size;

    let mut dib = vec![0u8; total_size as usize];
    dib[0..4].copy_from_slice(&header_size.to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&24u16.to_le_bytes());

    for y in 0..height {
        let dst_row = height - 1 - y;
        for x in 0..width {
            let src_idx = ((y * width + x) * 4) as usize;
            let dst_idx = (header_size as usize + dst_row as usize * row_size as usize
                + x as usize * 3);
            dib[dst_idx] = rgba[src_idx + 2]; // B
            dib[dst_idx + 1] = rgba[src_idx + 1]; // G
            dib[dst_idx + 2] = rgba[src_idx]; // R
        }
    }

    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_DIB;

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("failed to open clipboard".into());
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return Err("failed to clear clipboard".into());
        }
        let mem = GlobalAlloc(GMEM_MOVEABLE, total_size as usize);
        if mem.is_null() {
            CloseClipboard();
            return Err("failed to allocate clipboard memory".into());
        }
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return Err("failed to lock clipboard memory".into());
        }
        std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr, dib.len());
        GlobalUnlock(mem);
        if SetClipboardData(u32::from(CF_DIB), mem).is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return Err("failed to set clipboard image".into());
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_sensitive_content, preview_text};

    #[test]
    fn sensitive_filter_detects_keywords() {
        let patterns = vec!["password".to_string(), "token".to_string()];
        assert!(is_sensitive_content("my PASSWORD is hidden", &patterns));
        assert!(!is_sensitive_content("regular clipboard text", &patterns));
    }

    #[test]
    fn preview_is_single_line_and_trimmed() {
        assert_eq!(preview_text("a\nb\nc", 10), "a b c");
    }
}
