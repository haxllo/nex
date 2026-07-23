#![cfg(target_os = "windows")]

use std::collections::HashSet;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED,
    FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, WaitForSingleObject, INFINITE,
};

const BUFFER_BYTES: usize = 16 * 1024;
const DEBOUNCE_WINDOW_MS: u64 = 200;
const MAX_EVENTS_PER_BATCH: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherEventKind {
    Added,
    Modified,
    Removed,
    Renamed,
}

impl WatcherEventKind {
    fn from_action(action: u32) -> Option<Self> {
        match action {
            x if x == FILE_ACTION_ADDED => Some(Self::Added),
            x if x == FILE_ACTION_MODIFIED => Some(Self::Modified),
            x if x == FILE_ACTION_REMOVED => Some(Self::Removed),
            x if x == FILE_ACTION_RENAMED_OLD_NAME || x == FILE_ACTION_RENAMED_NEW_NAME => {
                Some(Self::Renamed)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WatcherEvent {
    pub kind: WatcherEventKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub root: PathBuf,
    pub excluded_roots: Vec<PathBuf>,
}

impl WatcherConfig {
    pub fn new(root: PathBuf, excluded_roots: Vec<PathBuf>) -> Self {
        Self {
            root,
            excluded_roots,
        }
    }
}

struct WatcherHandles {
    directory: HANDLE,
    event: HANDLE,
    overlapped: OVERLAPPED,
    buffer: [u8; BUFFER_BYTES],
}

unsafe impl Send for WatcherHandles {}

impl Drop for WatcherHandles {
    fn drop(&mut self) {
        unsafe {
            if !self.event.is_null() {
                CloseHandle(self.event);
            }
            if !self.directory.is_null() && self.directory != INVALID_HANDLE_VALUE {
                CloseHandle(self.directory);
            }
        }
    }
}

struct PendingBatch {
    events: Vec<WatcherEvent>,
    timer_start: Option<Instant>,
}

pub struct DirectoryWatcher {
    stop: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
}

impl DirectoryWatcher {
    pub fn start(
        config: WatcherConfig,
    ) -> Result<(Self, Receiver<Vec<WatcherEvent>>), WatcherError> {
        if config.root.as_os_str().is_empty() {
            return Err(WatcherError::NoRoots);
        }

        let (mut handles, directory) = unsafe { create_watcher_handles(&config.root)? };
        handles.overlapped.hEvent = handles.event;
        let _ = directory;

        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let thread = std::thread::Builder::new()
            .name("nex-dir-watcher".to_string())
            .spawn(move || {
                run_watch_loop(
                    config,
                    handles,
                    thread_stop,
                    tx,
                )
            })
            .map_err(|e| WatcherError::ThreadSpawnFailed(e.to_string()))?;

        let watcher = Self {
            stop: Some(stop),
            thread: Some(thread),
        };
        Ok((watcher, rx))
    }

    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug)]
pub enum WatcherError {
    NoRoots,
    OpenFailed { path: PathBuf, code: i32 },
    EventCreateFailed,
    ThreadSpawnFailed(String),
}

impl std::fmt::Display for WatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRoots => write!(f, "no root to watch"),
            Self::OpenFailed { path, code } => {
                write!(f, "CreateFileW failed for {}: code={}", path.display(), code)
            }
            Self::EventCreateFailed => write!(f, "CreateEventW returned NULL"),
            Self::ThreadSpawnFailed(msg) => write!(f, "thread spawn failed: {}", msg),
        }
    }
}

impl std::error::Error for WatcherError {}

unsafe fn create_watcher_handles(
    root: &Path,
) -> Result<(WatcherHandles, HANDLE), WatcherError> {
    let wide_path: Vec<u16> = root.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: root path is valid UTF-16, handles follow Win32 error convention
    let directory = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        )
    };
    if directory == INVALID_HANDLE_VALUE {
        return Err(WatcherError::OpenFailed {
            path: root.to_path_buf(),
            code: last_error() as i32,
        });
    }

    // SAFETY: null ptr for name creates unnamed event
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    if event.is_null() {
        unsafe { CloseHandle(directory) };
        return Err(WatcherError::EventCreateFailed);
    }

    // SAFETY: zeroed OVERLAPPED is valid init state
    let overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let buffer = [0u8; BUFFER_BYTES];
    Ok((
        WatcherHandles {
            directory,
            event,
            overlapped,
            buffer,
        },
        directory,
    ))
}

fn run_watch_loop(
    config: WatcherConfig,
    mut handles: WatcherHandles,
    stop: Arc<AtomicBool>,
    tx: Sender<Vec<WatcherEvent>>,
) {
    let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_CREATION;

    let excluded: Vec<String> = config
        .excluded_roots
        .iter()
        .map(|p| normalize_path(p))
        .collect();

    let root_lower = normalize_path(&config.root);

    let mut batch = PendingBatch {
        events: Vec::new(),
        timer_start: None,
    };

    'outer: loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let buffer_ptr = handles.buffer.as_mut_ptr() as *mut core::ffi::c_void;
        let buffer_len = handles.buffer.len();
        let queued = unsafe {
            ReadDirectoryChangesW(
                handles.directory,
                buffer_ptr,
                buffer_len as u32,
                1,
                notify_filter,
                std::ptr::null_mut(),
                &mut handles.overlapped,
                None,
            )
        };
        if queued == 0 {
            log_watcher_error("ReadDirectoryChangesW", last_error() as i32);
            break;
        }

        loop {
            let wait_ms = if batch.timer_start.is_some() {
                DEBOUNCE_WINDOW_MS as u32
            } else {
                INFINITE
            };
            let wait_result = unsafe { WaitForSingleObject(handles.event, wait_ms) };

            if stop.load(Ordering::SeqCst) {
                break 'outer;
            }

            if wait_result == WAIT_OBJECT_0 {
                let mut bytes_returned: u32 = 0;
                let ok = unsafe {
                    GetOverlappedResult(
                        handles.directory,
                        &mut handles.overlapped,
                        &mut bytes_returned,
                        1,
                    )
                };
                if ok == 0 {
                    log_watcher_error("GetOverlappedResult", last_error() as i32);
                    break 'outer;
                }
                if bytes_returned == 0 {
                    continue;
                }
                let slice = unsafe {
                    std::slice::from_raw_parts(buffer_ptr as *const u8, bytes_returned as usize)
                };
                append_notifications(slice, &root_lower, &excluded, &mut batch);
                batch.timer_start = Some(Instant::now());
            } else if wait_result == WAIT_TIMEOUT {
                if let Some(start) = batch.timer_start {
                    if start.elapsed() >= Duration::from_millis(DEBOUNCE_WINDOW_MS) {
                        flush_batch(&mut batch, &tx);
                    }
                }
            } else {
                log_watcher_error("WaitForSingleObject", last_error() as i32);
                break 'outer;
            }
        }
    }

    flush_batch(&mut batch, &tx);
}

fn append_notifications(
    buffer: &[u8],
    root_lower: &str,
    excluded: &[String],
    batch: &mut PendingBatch,
) {
    let mut offset: usize = 0;
    while offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() <= buffer.len() {
        let info = unsafe { &*(buffer.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION) };
        let name_byte_len = info.FileNameLength as usize;
        let name_u16_len = name_byte_len / 2;
        if name_u16_len == 0 {
            break;
        }
        let name_slice =
            unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_u16_len) };
        let name = String::from_utf16_lossy(name_slice);
        let full_path = join_under_root(root_lower, &name);

        if let Some(kind) = WatcherEventKind::from_action(info.Action) {
            if !is_under_excluded(&full_path, excluded) {
                batch.events.push(WatcherEvent {
                    kind,
                    path: full_path,
                });
                if batch.events.len() > MAX_EVENTS_PER_BATCH {
                    batch.events.truncate(MAX_EVENTS_PER_BATCH);
                }
            }
        }

        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
}

fn join_under_root(root_lower: &str, name: &str) -> PathBuf {
    if root_lower.is_empty() {
        return PathBuf::from(name);
    }
    if name.starts_with('\\') || name.contains(':') {
        return PathBuf::from(name);
    }
    let mut path = PathBuf::from(root_lower);
    path.push(name);
    path
}

fn normalize_path(p: &Path) -> String {
    p.to_string_lossy()
        .to_ascii_lowercase()
        .trim_end_matches(['\\', '/'])
        .to_string()
}

fn is_under_excluded(path: &Path, excluded: &[String]) -> bool {
    let lower = normalize_path(path);
    for ex in excluded {
        if lower == *ex {
            return true;
        }
        if lower.starts_with(ex.as_str()) {
            let next = lower.as_bytes().get(ex.len()).copied();
            if matches!(next, Some(b'\\') | Some(b'/')) {
                return true;
            }
        }
    }
    let segments: Vec<&str> = lower.split(['\\', '/']).collect();
    for ex in excluded {
        let ex_segments: Vec<&str> = ex.split(['\\', '/']).collect();
        if !ex_segments.is_empty()
            && segments
                .windows(ex_segments.len())
                .any(|w| w == ex_segments.as_slice())
        {
            return true;
        }
    }
    false
}

fn flush_batch(batch: &mut PendingBatch, tx: &Sender<Vec<WatcherEvent>>) {
    if batch.events.is_empty() {
        batch.timer_start = None;
        return;
    }
    let mut deduped: Vec<WatcherEvent> = Vec::with_capacity(batch.events.len());
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for ev in batch.events.drain(..) {
        if seen.insert(ev.path.clone()) {
            deduped.push(ev);
        }
    }
    let _ = tx.send(deduped);
    batch.timer_start = None;
}

fn last_error() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

fn log_watcher_error(op: &str, code: i32) {
    eprintln!("[nex-dir-watcher] {} failed (code={})", op, code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_event_kind_maps_actions() {
        assert_eq!(
            WatcherEventKind::from_action(FILE_ACTION_ADDED),
            Some(WatcherEventKind::Added)
        );
        assert_eq!(
            WatcherEventKind::from_action(FILE_ACTION_MODIFIED),
            Some(WatcherEventKind::Modified)
        );
        assert_eq!(
            WatcherEventKind::from_action(FILE_ACTION_REMOVED),
            Some(WatcherEventKind::Removed)
        );
        assert_eq!(
            WatcherEventKind::from_action(FILE_ACTION_RENAMED_OLD_NAME),
            Some(WatcherEventKind::Renamed)
        );
        assert_eq!(WatcherEventKind::from_action(99), None);
    }

    #[test]
    fn join_under_root_appends_relative_name() {
        let joined = join_under_root("c:\\users\\admin", "documents\\file.txt");
        assert_eq!(
            joined.to_string_lossy().to_ascii_lowercase(),
            "c:\\users\\admin\\documents\\file.txt"
        );
    }

    #[test]
    fn join_under_root_passthrough_absolute() {
        let joined = join_under_root("c:\\users\\admin", "C:\\absolute\\path.txt");
        assert_eq!(joined, PathBuf::from("C:\\absolute\\path.txt"));
    }

    #[test]
    fn is_under_excluded_detects_path_prefix() {
        let excluded = vec![normalize_path(&PathBuf::from("C:\\Users\\Admin\\AppData"))];
        let path = PathBuf::from("C:\\Users\\Admin\\AppData\\Local\\Temp\\foo.txt");
        assert!(is_under_excluded(&path, &excluded));
    }

    #[test]
    fn is_under_excluded_returns_false_for_unrelated() {
        let excluded = vec![normalize_path(&PathBuf::from("C:\\Users\\Admin\\AppData"))];
        let path = PathBuf::from("C:\\Users\\Admin\\Documents\\file.txt");
        assert!(!is_under_excluded(&path, &excluded));
    }

    #[test]
    fn is_under_excluded_detects_segment_match() {
        let excluded = vec![normalize_path(&PathBuf::from("node_modules"))];
        let path = PathBuf::from("C:\\proj\\node_modules\\lodash\\index.js");
        assert!(is_under_excluded(&path, &excluded));
    }
}
