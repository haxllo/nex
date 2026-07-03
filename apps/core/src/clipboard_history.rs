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
pub struct ClipboardEntry {
    pub id: String,
    pub text: String,
    pub captured_epoch_secs: i64,
}

pub fn maybe_capture_latest(cfg: &Config) -> Result<bool, String> {
    if !cfg.clipboard_enabled {
        return Ok(false);
    }

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

    let mut entries = load_entries(cfg);
    if entries.first().is_some_and(|entry| entry.text == text) {
        return Ok(false);
    }

    let now = now_epoch_secs();
    entries.insert(
        0,
        ClipboardEntry {
            id: format!("clip-{now}-{}", now_nanos() % 1_000_000),
            text,
            captured_epoch_secs: now,
        },
    );
    prune_entries(cfg, &mut entries, now);
    save_entries(cfg, &entries)?;
    Ok(true)
}

pub fn clear_history(cfg: &Config) -> Result<(), String> {
    let path = history_path(cfg);
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|e| format!("failed to clear clipboard history: {e}"))
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
    let Some(text) = resolve_text_for_result(cfg, result_id) else {
        return Err("clipboard entry not found".to_string());
    };
    write_system_clipboard_text(&text)
}

fn resolve_text_for_result(cfg: &Config, result_id: &str) -> Option<String> {
    let entry_id = result_id.strip_prefix("clipboard:")?;
    load_entries(cfg)
        .into_iter()
        .find(|entry| entry.id == entry_id)
        .map(|entry| entry.text)
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
