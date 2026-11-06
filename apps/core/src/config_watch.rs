//! Push notification for config file edits.
//!
//! A thread blocks in `ReadDirectoryChangesW` on the config's directory and
//! calls back (debounced) when the config file itself changes. Callers keep a
//! slow mtime poll as a safety net for missed events. Off Windows this is a
//! no-op and only the poll applies.

use std::path::Path;
use std::time::Duration;

/// Coalesces the burst of events one save produces (write, truncate, rename).
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Start watching `config_path`; `on_change` runs on the watcher thread.
pub fn spawn(config_path: &Path, on_change: impl Fn() + Send + 'static) {
    #[cfg(target_os = "windows")]
    windows_impl::spawn(config_path, on_change);
    #[cfg(not(target_os = "windows"))]
    let _ = (config_path, on_change);
}

/// File names reported by one `FILE_NOTIFY_INFORMATION` buffer.
#[cfg(any(target_os = "windows", test))]
fn changed_names(buffer: &[u8]) -> Vec<String> {
    const HEADER: usize = 12;
    let mut names = Vec::new();
    let mut offset = 0_usize;
    while offset + HEADER <= buffer.len() {
        let read_u32 = |at: usize| {
            u32::from_le_bytes([buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]]) as usize
        };
        let next = read_u32(offset);
        let name_len = read_u32(offset + 8);
        let name_start = offset + HEADER;
        if name_start + name_len > buffer.len() {
            break;
        }
        let units: Vec<u16> = buffer[name_start..name_start + name_len]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        names.push(String::from_utf16_lossy(&units));
        if next == 0 {
            break;
        }
        offset += next;
    }
    names
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{changed_names, DEBOUNCE};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    const RETRY_DELAY: Duration = Duration::from_secs(5);

    pub(super) fn spawn(config_path: &Path, on_change: impl Fn() + Send + 'static) {
        let Some(dir) = config_path.parent().map(Path::to_path_buf) else {
            return;
        };
        let Some(file_name) = config_path.file_name().map(|n| n.to_string_lossy().into_owned())
        else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("nex-config-watch".into())
            .spawn(move || loop {
                watch_once(&dir, &file_name, &on_change);
                std::thread::sleep(RETRY_DELAY);
            });
        if let Err(error) = spawned {
            crate::logging::warn(&format!("[nex] config watcher thread failed: {error}"));
        }
    }

    /// Returns when the directory handle cannot be opened or read any more.
    fn watch_once(dir: &Path, file_name: &str, on_change: &impl Fn()) {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is NUL-terminated; the handle is closed below.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return;
        }
        // u32 elements keep FILE_NOTIFY_INFORMATION records aligned.
        let mut buffer = vec![0_u32; 4096];
        loop {
            let mut returned = 0_u32;
            // SAFETY: synchronous call (no OVERLAPPED); the buffer outlives it.
            let ok = unsafe {
                ReadDirectoryChangesW(
                    handle,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 4) as u32,
                    0,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_CREATION,
                    &mut returned,
                    std::ptr::null_mut(),
                    None,
                )
            };
            if ok == 0 {
                break;
            }
            // returned == 0 means the buffer overflowed: treat as a change.
            let relevant = returned == 0 || {
                // SAFETY: `returned` bytes of the u32 buffer were initialised.
                let bytes = unsafe {
                    std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), returned as usize)
                };
                changed_names(bytes)
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(file_name))
            };
            if relevant {
                std::thread::sleep(DEBOUNCE);
                on_change();
            }
        }
        // SAFETY: handle came from CreateFileW above.
        unsafe { CloseHandle(handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(next: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut out = Vec::new();
        out.extend_from_slice(&next.to_le_bytes());
        out.extend_from_slice(&3_u32.to_le_bytes());
        out.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
        for unit in units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out
    }

    #[test]
    fn parses_chained_records() {
        let first = record(0, "config.toml");
        let mut buffer = record(first.len() as u32, "config.toml");
        buffer.extend(record(0, "other.txt"));
        assert_eq!(changed_names(&buffer), vec!["config.toml", "other.txt"]);
    }

    #[test]
    fn truncated_buffer_is_ignored() {
        let mut buffer = record(0, "config.toml");
        buffer.truncate(14);
        assert!(changed_names(&buffer).is_empty());
    }
}
