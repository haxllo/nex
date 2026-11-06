//! Client for the helper's NTFS MFT snapshots: the persistent command channel
//! (`mft_channel`) with the legacy per-volume pipes (`\\.\pipe\NexMft-<letter>`)
//! as fallback. Only Windows builds call into it.

#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    GENERIC_READ, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, GetVolumeInformationW,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const ROOT_RECORD_NUMBER: u64 = 5;
const FRN_RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const MAX_PARENT_DEPTH: usize = 256;
const ERROR_SEM_TIMEOUT: u32 = 121;

pub(crate) use crate::mft_protocol::{MftRecord, SnapshotMeta};
use crate::mft_protocol::{RequestKind, read_snapshot};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct Snapshot {
    pub delta: bool,
    pub journal_id: u64,
    pub start_usn: i64,
    pub next_usn: i64,
    pub records: Vec<MftRecord>,
}

/// What the caller asks the helper for.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SnapshotRequest {
    Full,
    Delta { journal_id: u64, start_usn: i64 },
}

#[derive(Debug)]
pub(crate) enum FetchError {
    /// Neither a command-channel connection nor a legacy pipe is available:
    /// the elevated helper is not up (yet). Retrying later can succeed.
    HelperNotReady(String),
    /// The helper refused the saved journal position (id mismatch, truncated).
    JournalInvalid(String),
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HelperNotReady(message)
            | Self::JournalInvalid(message)
            | Self::Failed(message) => f.write_str(message),
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// `C:\Users\me` -> `C`.
pub(crate) fn root_volume_letter(root: &Path) -> Option<char> {
    let text = root.to_string_lossy();
    let mut chars = text.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() || chars.next()? != ':' {
        return None;
    }
    Some(letter.to_ascii_uppercase())
}

/// NTFS is the only filesystem whose MFT/USN shape matches this protocol.
pub(crate) fn is_ntfs_volume(letter: char) -> bool {
    let root = wide(&format!("{letter}:\\"));
    let mut fs_buf = [0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_buf.as_mut_ptr(),
            fs_buf.len() as u32,
        )
    };
    if ok == 0 {
        return false;
    }
    let len = fs_buf.iter().position(|&c| c == 0).unwrap_or(fs_buf.len());
    String::from_utf16_lossy(&fs_buf[..len]).eq_ignore_ascii_case("NTFS")
}

/// Fixed volumes only; removable/network roots stay on the walkdir path.
pub(crate) fn is_fixed_volume(letter: char) -> bool {
    let root = wide(&format!("{letter}:\\"));
    unsafe { GetDriveTypeW(root.as_ptr()) == 3 }
}

fn legacy_pipe_name(letter: char) -> Vec<u16> {
    wide(&format!("\\\\.\\pipe\\NexMft-{letter}"))
}

/// Non-blocking probe for the helper's legacy per-volume pipe: present and
/// free, or present and busy serving another client.
fn legacy_pipe_exists(letter: char) -> bool {
    let name = legacy_pipe_name(letter);
    if unsafe { WaitNamedPipeW(name.as_ptr(), 1) } != 0 {
        return true;
    }
    let error = unsafe { GetLastError() };
    error == ERROR_SEM_TIMEOUT
}

/// True when the helper can serve `letters` now: attached to the command
/// channel, or exposing a legacy snapshot pipe.
pub(crate) fn helper_is_up(letters: &[char]) -> bool {
    crate::mft_channel::is_connected() || letters.iter().any(|&letter| legacy_pipe_exists(letter))
}

/// Stream one snapshot into `on_record`, waiting up to `wait_ms` for the helper.
///
/// A helper attached to the command channel is preferred; otherwise the
/// request-file + per-volume pipe path serves helpers that predate the channel.
pub(crate) fn stream_snapshot_into<F>(
    letter: char,
    request: SnapshotRequest,
    wait_ms: u32,
    on_record: F,
) -> Result<SnapshotMeta, FetchError>
where
    F: FnMut(&MftRecord) -> Result<(), String>,
{
    use std::time::{Duration, Instant};

    crate::mft_channel::ensure_started();
    let name = legacy_pipe_name(letter);
    // WaitNamedPipeW only waits for a free instance of an existing pipe; the
    // helper creates its endpoints asynchronously after the UI starts, so poll
    // for both transports until the deadline.
    let deadline = Instant::now() + Duration::from_millis(u64::from(wait_ms.max(1)));
    loop {
        if crate::mft_channel::is_connected() {
            let kind = match request {
                SnapshotRequest::Full => RequestKind::Full,
                SnapshotRequest::Delta {
                    journal_id,
                    start_usn,
                } => RequestKind::Delta {
                    journal_id,
                    start_usn,
                },
            };
            return match crate::mft_channel::request_snapshot(letter, kind, on_record) {
                Ok(meta) => Ok(meta),
                Err(crate::mft_channel::ChannelError::NotConnected(message)) => {
                    Err(FetchError::HelperNotReady(message))
                }
                Err(crate::mft_channel::ChannelError::Stream(error)) if error.journal_invalid => {
                    Err(FetchError::JournalInvalid(error.to_string()))
                }
                Err(crate::mft_channel::ChannelError::Stream(error)) => {
                    Err(FetchError::Failed(error.to_string()))
                }
            };
        }
        if unsafe { WaitNamedPipeW(name.as_ptr(), 200) } != 0 {
            return stream_legacy(letter, request, &name, on_record);
        }
        if Instant::now() >= deadline {
            let last_error = unsafe { GetLastError() };
            return Err(FetchError::HelperNotReady(format!(
                "no command channel connection and legacy pipe not ready (waited {wait_ms} ms, last err {last_error})"
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn stream_legacy<F>(
    letter: char,
    request: SnapshotRequest,
    name: &[u16],
    on_record: F,
) -> Result<SnapshotMeta, FetchError>
where
    F: FnMut(&MftRecord) -> Result<(), String>,
{
    match request {
        SnapshotRequest::Full => write_full_request(letter),
        SnapshotRequest::Delta {
            journal_id,
            start_usn,
        } => write_delta_request(letter, journal_id, start_usn),
    }
    let handle: HANDLE = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(FetchError::Failed(format!("open pipe failed: {}", unsafe {
            GetLastError()
        })));
    }
    let mut file = unsafe { File::from_raw_handle(handle as *mut _) };
    read_snapshot(&mut file, on_record).map_err(|error| FetchError::Failed(error.to_string()))
}

/// Collect a delta snapshot; the legacy path answers a stale position with a
/// full stream, which the caller detects through `Snapshot::delta`.
pub(crate) fn fetch_delta(
    letter: char,
    journal_id: u64,
    start_usn: i64,
    wait_ms: u32,
) -> Result<Snapshot, FetchError> {
    let mut records = Vec::new();
    let meta = stream_snapshot_into(
        letter,
        SnapshotRequest::Delta {
            journal_id,
            start_usn,
        },
        wait_ms,
        |record| {
            records.push(record.clone());
            Ok(())
        },
    )?;
    Ok(Snapshot {
        delta: meta.delta,
        journal_id: meta.journal_id,
        start_usn: meta.start_usn,
        next_usn: meta.next_usn,
        records,
    })
}

fn is_root_reference(frn: u64) -> bool {
    (frn & FRN_RECORD_MASK) == ROOT_RECORD_NUMBER
}

/// Resolve every record to a full path and keep entries under `root`.
/// Returns `(path, is_dir)` with case-insensitive de-duplication.
pub(crate) fn paths_under_root(
    snapshot: &Snapshot,
    letter: char,
    root: &Path,
) -> Vec<(PathBuf, bool)> {
    let by_frn: HashMap<u64, &MftRecord> = snapshot
        .records
        .iter()
        .map(|record| (record.frn, record))
        .collect();
    let base = PathBuf::from(format!("{letter}:\\"));
    let mut memo: HashMap<u64, Option<PathBuf>> = HashMap::new();
    let root_text = root.to_string_lossy().to_ascii_lowercase();
    // A volume root ("g:\") already ends in a separator, so the next character
    // is the first path component, not a boundary.
    let root_ends_separator = root_text.ends_with('\\') || root_text.ends_with('/');
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    for record in &snapshot.records {
        let Some(path) = resolve_path(record.frn, &by_frn, &mut memo, &base, 0) else {
            continue;
        };
        let text = path.to_string_lossy();
        let lower = text.to_ascii_lowercase();
        if lower == root_text || !lower.starts_with(&root_text) {
            continue;
        }
        if !root_ends_separator {
            match lower.as_bytes().get(root_text.len()) {
                Some(b'\\') | Some(b'/') => {}
                _ => continue,
            }
        }
        if !seen.insert(lower) {
            continue;
        }
        out.push((path, record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0));
    }
    out
}

fn resolve_path(
    frn: u64,
    by_frn: &HashMap<u64, &MftRecord>,
    memo: &mut HashMap<u64, Option<PathBuf>>,
    base: &Path,
    depth: usize,
) -> Option<PathBuf> {
    if depth > MAX_PARENT_DEPTH {
        return None;
    }
    if let Some(cached) = memo.get(&frn) {
        return cached.clone();
    }
    let Some(record) = by_frn.get(&frn) else {
        // The root record itself is not part of the stream (it has no name).
        return if is_root_reference(frn) {
            Some(base.to_path_buf())
        } else {
            None
        };
    };
    let resolved = if is_root_reference(record.parent) {
        Some(base.join(&record.name))
    } else {
        resolve_path(record.parent, by_frn, memo, base, depth + 1)
            .map(|parent| parent.join(&record.name))
    };
    memo.insert(frn, resolved.clone());
    resolved
}

fn nex_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Nex"))
}

fn request_path(letter: char) -> Option<PathBuf> {
    Some(nex_dir()?.join(format!("mft-{letter}.req")))
}

fn processed_path(letter: char) -> Option<PathBuf> {
    Some(nex_dir()?.join(format!("mft-processed-{letter}.txt")))
}

/// Ask the helper for a full snapshot for this volume.
pub(crate) fn write_full_request(letter: char) {
    let Some(path) = request_path(letter) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, b"full\n");
}

/// Ask the helper for journal deltas from a saved position.
#[allow(dead_code)]
pub(crate) fn write_delta_request(letter: char, journal_id: u64, start_usn: i64) {
    let Some(path) = request_path(letter) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        path,
        format!("journal_id={journal_id}\nstart_usn={start_usn}\n"),
    );
}

/// Volume position after the last successful full snapshot or delta pass.
#[allow(dead_code)]
pub(crate) fn read_processed_state(letter: char) -> Option<(u64, i64)> {
    let content = std::fs::read_to_string(processed_path(letter)?).ok()?;
    let mut journal_id = None;
    let mut next_usn = None;
    for line in content.lines() {
        let (key, value) = line.split_once('=')?;
        match key.trim() {
            "journal_id" => journal_id = value.trim().parse::<u64>().ok(),
            "next_usn" => next_usn = value.trim().parse::<i64>().ok(),
            _ => {}
        }
    }
    Some((journal_id?, next_usn?))
}

pub(crate) fn write_processed_state(letter: char, journal_id: u64, next_usn: i64) {
    let Some(path) = processed_path(letter) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        path,
        format!("journal_id={journal_id}\nnext_usn={next_usn}\n"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(frn: u64, parent: u64, name: &str, is_dir: bool) -> MftRecord {
        MftRecord {
            frn,
            parent,
            attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            reason: 0,
            name: name.to_string(),
            path: None,
        }
    }

    #[test]
    fn root_volume_letter_parses_drive_paths() {
        assert_eq!(root_volume_letter(Path::new(r"C:\Users\me")), Some('C'));
        assert_eq!(root_volume_letter(Path::new(r"g:\")), Some('G'));
        assert_eq!(root_volume_letter(Path::new(r"\\server\share")), None);
    }

    #[test]
    fn paths_resolve_below_root_from_parent_references() {
        let root_frn = (5u64 << 48) | 5;
        let users_frn = (7u64 << 48) | 42;
        let snapshot = Snapshot {
            delta: false,
            journal_id: 1,
            start_usn: 0,
            next_usn: 0,
            records: vec![
                record(users_frn, root_frn, "Users", true),
                record((9u64 << 48) | 100, users_frn, "me", true),
                record((9u64 << 48) | 101, (9u64 << 48) | 100, "note.txt", false),
            ],
        };
        let paths = paths_under_root(&snapshot, 'C', Path::new(r"C:\Users\me"));
        let note = paths
            .iter()
            .find(|(path, _)| path.ends_with("note.txt"))
            .expect("note.txt resolved");
        assert_eq!(note.0, PathBuf::from(r"C:\Users\me\note.txt"));
        assert!(!note.1);
        assert!(paths.iter().all(|(path, _)| path.starts_with(r"C:\Users\me")));
    }

    #[test]
    fn volume_root_matches_without_an_extra_separator() {
        let root_frn = (5u64 << 48) | 5;
        let pictures_frn = (7u64 << 48) | 42;
        let snapshot = Snapshot {
            delta: false,
            journal_id: 1,
            start_usn: 0,
            next_usn: 0,
            records: vec![record(pictures_frn, root_frn, "Pictures", true)],
        };
        let paths = paths_under_root(&snapshot, 'G', Path::new(r"G:\"));
        assert_eq!(
            paths,
            vec![(PathBuf::from(r"G:\Pictures"), true)],
            "volume-root prefix must not require a second separator"
        );
    }

    #[test]
    fn unresolved_parents_are_dropped() {
        let snapshot = Snapshot {
            delta: false,
            journal_id: 1,
            start_usn: 0,
            next_usn: 0,
            records: vec![record(77, 999, "orphan.txt", false)],
        };
        assert!(paths_under_root(&snapshot, 'C', Path::new(r"C:\")).is_empty());
    }
}
