//! Client for the helper's NTFS MFT snapshot pipes (`\\.\pipe\NexMft-<letter>`).
//!
//! The helper streams FRN/parent pairs; this module rebuilds paths and keeps
//! entries under a configured root. Only Windows builds call into it.

#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
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

const HEADER_LEN: usize = 32;
const RECORD_PREFIX_LEN: usize = 36;
const PROTOCOL_VERSION: u16 = 2;
const DELTA_MODE: u8 = 1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const ROOT_RECORD_NUMBER: u64 = 5;
const FRN_RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const MAX_PARENT_DEPTH: usize = 256;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct MftRecord {
    pub frn: u64,
    pub parent: u64,
    pub attributes: u32,
    pub reason: u32,
    pub name: String,
    /// Helper-resolved full path (delta mode only).
    pub path: Option<String>,
}

pub(crate) struct Snapshot {
    pub delta: bool,
    pub journal_id: u64,
    pub start_usn: i64,
    pub next_usn: i64,
    pub records: Vec<MftRecord>,
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

/// Wait for the helper's pipe, then read one full snapshot.
pub(crate) fn fetch_snapshot(letter: char, timeout_ms: u32) -> Result<Snapshot, String> {
    use std::time::{Duration, Instant};

    let pipe_name = format!("\\\\.\\pipe\\NexMft-{letter}");
    let name = wide(&pipe_name);
    // WaitNamedPipeW only waits for a free instance of an existing pipe; the
    // helper creates the name asynchronously after the UI starts, so poll for
    // both cases until the deadline.
    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms.max(1)));
    let mut last_error = 0u32;
    loop {
        if unsafe { WaitNamedPipeW(name.as_ptr(), 200) } != 0 {
            break;
        }
        last_error = unsafe { GetLastError() };
        if Instant::now() >= deadline {
            return Err(format!(
                "pipe not ready (waited {timeout_ms} ms, last err {last_error})"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
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
        return Err(format!("open pipe failed: {}", unsafe { GetLastError() }));
    }
    let mut file = unsafe { File::from_raw_handle(handle as *mut _) };

    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header)
        .map_err(|e| format!("read header: {e}"))?;
    if &header[0..4] != b"NXMF" {
        return Err("snapshot header mismatch".to_string());
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != PROTOCOL_VERSION {
        return Err(format!("snapshot version {version} unsupported"));
    }
    let delta = header[7] == DELTA_MODE;
    let journal_id = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let start_usn = i64::from_le_bytes(header[16..24].try_into().unwrap());
    let mut next_usn = i64::from_le_bytes(header[24..32].try_into().unwrap());

    let mut records = Vec::with_capacity(120_000);
    let mut prefix = [0u8; RECORD_PREFIX_LEN];
    loop {
        match file.read_exact(&mut prefix) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("read record: {error}")),
        }
        let frn = u64::from_le_bytes(prefix[0..8].try_into().unwrap());
        let parent = u64::from_le_bytes(prefix[8..16].try_into().unwrap());
        let attributes = u32::from_le_bytes(prefix[16..20].try_into().unwrap());
        let reason = u32::from_le_bytes(prefix[28..32].try_into().unwrap());
        let name_len = u16::from_le_bytes([prefix[32], prefix[33]]) as usize;
        let path_len = u16::from_le_bytes([prefix[34], prefix[35]]) as usize;
        if frn == 0 && name_len == 0 && path_len == 0 && reason == u32::MAX {
            // Trailer: final journal position.
            let mut tail = [0u8; 8];
            file.read_exact(&mut tail)
                .map_err(|e| format!("read trailer: {e}"))?;
            next_usn = i64::from_le_bytes(tail);
            break;
        }
        let mut name_bytes = vec![0u8; name_len];
        file.read_exact(&mut name_bytes)
            .map_err(|e| format!("read name: {e}"))?;
        let mut path_bytes = vec![0u8; path_len];
        file.read_exact(&mut path_bytes)
            .map_err(|e| format!("read path: {e}"))?;
        let name = String::from_utf8_lossy(&name_bytes).into_owned();
        if name.is_empty() {
            continue;
        }
        records.push(MftRecord {
            frn,
            parent,
            attributes,
            reason,
            name,
            path: if path_len > 0 {
                Some(String::from_utf8_lossy(&path_bytes).into_owned())
            } else {
                None
            },
        });
    }

    Ok(Snapshot {
        delta,
        journal_id,
        start_usn,
        next_usn,
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
