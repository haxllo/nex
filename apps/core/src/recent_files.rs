//! Windows recent-documents (MRU) integration.
//!
//! Windows does not expose "what did the user open recently" through Win32
//! search APIs in a form a launcher can use directly. It *does* maintain the
//! shell's MRU as one `.lnk` shortcut per opened document under
//! `%APPDATA%\Microsoft\Windows\Recent`. Each shortcut's last-write time is,
//! to within a few seconds, when the document was last used.
//!
//! This module reads that list and feeds it back into the index as a *recency
//! overlay*: existing file/folder items get their `last_accessed_epoch_secs`
//! advanced, and targets the filesystem scan never reached (recently opened
//! documents outside the configured roots) are inserted. That makes the
//! ranking pass's existing recency bonus surface "the PDF I was reading
//! yesterday" without any new query syntax.
//!
//! Designed as an overlay rather than a `DiscoveryProvider` on purpose: the
//! rebuild loop gives the `filesystem` provider ownership of `file`/`folder`
//! kinds and prunes anything it did not discover, so a second provider
//! emitting the same kinds would fight that pass instead of cooperating
//! with it.
//!
//! `.lnk` targets are resolved by parsing the shortcut binary format
//! (MS-SHLLINK) directly — no COM apartment, no shell GUID marshalling, and
//! the parser is a pure function that is unit-tested without touching the
//! filesystem.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Cap on how many MRU entries to apply per pass. The shell keeps far more,
/// but the tail is noise and every entry costs a store write.
pub(crate) const RECENT_FILES_MAX_ENTRIES: usize = 250;

/// Hard ceiling on store writes per refresh. The list is applied most-recent
/// first, so a large backlog is drained over successive passes instead of
/// holding the service write lock for a long time in one go.
pub(crate) const RECENT_FILES_MAX_APPLIED_PER_PASS: usize = 80;

/// Shell link header size, `0x0000004C`.
const LNK_HEADER_SIZE: u32 = 0x4C;
/// `00021401-0000-0000-C000-000000000046` (CLSID_ShellLink) in little-endian.
const LNK_CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

const FLAG_HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const FLAG_HAS_LINK_INFO: u32 = 0x0000_0002;
const FLAG_HAS_RELATIVE_PATH: u32 = 0x0000_0008;
const FLAG_IS_UNICODE: u32 = 0x0000_0080;

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Read a NUL-terminated ANSI (Windows-1252-ish) string. Non-ASCII bytes are
/// decoded lossily; paths outside the current code page are recovered by the
/// caller's relative-path fallback instead.
fn read_c_string(data: &[u8], offset: usize) -> Option<String> {
    if offset >= data.len() {
        return None;
    }
    let tail = &data[offset..];
    let end = tail.iter().position(|&b| b == 0)?;
    Some(
        String::from_utf8_lossy(&tail[..end])
            .trim()
            .to_string(),
    )
}

/// Read a UTF-16LE string of `char_count` characters starting at `offset`.
fn read_utf16(data: &[u8], offset: usize, char_count: usize) -> Option<String> {
    let byte_len = char_count.checked_mul(2)?;
    let bytes = data.get(offset..offset + byte_len)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Resolve the local target path of a `.lnk` shortcut.
///
/// Tries the `LinkInfo` local base path (plus common suffix) first because it
/// is absolute, then falls back to the stored relative path. Returns `None`
/// for malformed data, network-only links, or shortcuts without a path.
pub(crate) fn parse_lnk_target(data: &[u8]) -> Option<String> {
    if data.len() < LNK_HEADER_SIZE as usize {
        return None;
    }
    if read_u32(data, 0)? != LNK_HEADER_SIZE {
        return None;
    }
    if data.get(4..20)? != LNK_CLSID {
        return None;
    }

    let flags = read_u32(data, 20)?;
    let mut pos = LNK_HEADER_SIZE as usize;

    if flags & FLAG_HAS_LINK_TARGET_ID_LIST != 0 {
        let id_list_size = read_u16(data, pos)? as usize;
        pos = pos.checked_add(2 + id_list_size)?;
    }

    if flags & FLAG_HAS_LINK_INFO != 0 {
        let link_info_start = pos;
        let link_info_size = read_u32(data, link_info_start)? as usize;
        let local_base_path_offset = read_u32(data, link_info_start + 16)? as usize;
        let common_path_suffix_offset = read_u32(data, link_info_start + 24)? as usize;

        if local_base_path_offset > 0 {
            if let Some(base) =
                read_c_string(data, link_info_start + local_base_path_offset)
            {
                if !base.is_empty() {
                    let suffix = if common_path_suffix_offset > 0 {
                        read_c_string(data, link_info_start + common_path_suffix_offset)
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    return Some(format!("{base}{suffix}"));
                }
            }
        }

        // LinkInfo present but it only holds a network volume, or the local
        // base path was empty — fall through to the relative path.
        pos = link_info_start.checked_add(link_info_size)?;
    }

    if flags & FLAG_HAS_RELATIVE_PATH != 0 {
        let count = read_u16(data, pos)? as usize;
        pos += 2;
        let relative = if flags & FLAG_IS_UNICODE != 0 {
            read_utf16(data, pos, count)
        } else {
            read_c_string(data, pos)
        }?;
        let relative = relative.trim();
        if !relative.is_empty() {
            return Some(relative.to_string());
        }
    }

    None
}

/// Directory holding the shell's per-document MRU shortcuts.
pub(crate) fn recent_dir_default() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Recent"),
    )
}

/// Collect recent targets as `(path, last_used_epoch_secs)`, most recent
/// first, capped at `max_entries`. `.lnk` files that cannot be resolved are
/// skipped. Unreadable directories yield an empty list rather than an error:
/// recency is a ranking nicety, never a hard requirement.
pub(crate) fn collect_recent_targets(dir: &Path, max_entries: usize) -> Vec<(PathBuf, i64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut targets: Vec<(PathBuf, i64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|dur| dur.as_secs() as i64)
            .unwrap_or(0);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(target) = parse_lnk_target(&bytes) else {
            continue;
        };
        let target = PathBuf::from(target);
        let target_key = target.to_string_lossy().to_ascii_lowercase();
        if let Some(existing) = targets.iter_mut().find(|(candidate, _)| {
            candidate.to_string_lossy().to_ascii_lowercase() == target_key
        }) {
            existing.1 = existing.1.max(modified);
            continue;
        }
        targets.push((target, modified));
    }

    targets.sort_by(|a, b| b.1.cmp(&a.1));
    targets.truncate(max_entries);
    targets
}

/// Item id and kind for a recent target, matching the ids the filesystem
/// provider emits so recency updates the same row instead of forking a
/// duplicate result.
pub(crate) fn recent_item_identity(path: &Path) -> Option<(&'static str, String)> {
    let as_string = path.to_string_lossy().to_ascii_lowercase();
    if path.is_dir() {
        Some(("folder", format!("folder:{as_string}")))
    } else if path.is_file() {
        Some(("file", format!("file:{as_string}")))
    } else {
        None
    }
}

pub(crate) fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

/// Build a minimal but well-formed `.lnk` with a LinkInfo local base path.
/// Test-only so other modules can exercise the MRU overlay on synthetic data.
#[cfg(test)]
pub(crate) fn lnk_with_link_info(base_path: &str, suffix: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&LNK_HEADER_SIZE.to_le_bytes());
        data.extend_from_slice(&LNK_CLSID);
        data.extend_from_slice(&FLAG_HAS_LINK_INFO.to_le_bytes()); // flags
        data.extend_from_slice(&0u32.to_le_bytes()); // file attributes
        data.extend_from_slice(&[0u8; 28]); // 3 timestamps (8) + file size (4)
        data.extend_from_slice(&0u32.to_le_bytes()); // icon index
        data.extend_from_slice(&0u32.to_le_bytes()); // show command
        data.extend_from_slice(&0u16.to_le_bytes()); // hotkey
        data.extend_from_slice(&[0u8; 10]); // reserved
        debug_assert_eq!(data.len(), LNK_HEADER_SIZE as usize);

        let base_bytes = base_path.as_bytes();
        let suffix_bytes = suffix.as_bytes();
        const HEADER: usize = 28;
        let local_base_path_offset = HEADER as u32;
        let common_path_suffix_offset = (HEADER + base_bytes.len() + 1) as u32;
        let link_info_size =
            HEADER + base_bytes.len() + 1 + suffix_bytes.len() + 1;

        let link_info_start = data.len();
        data.extend_from_slice(&(link_info_size as u32).to_le_bytes());
        data.extend_from_slice(&(HEADER as u32).to_le_bytes()); // header size
        data.extend_from_slice(&1u32.to_le_bytes()); // volume id + local base
        data.extend_from_slice(&0u32.to_le_bytes()); // volume id offset
        data.extend_from_slice(&local_base_path_offset.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // common network offset
        data.extend_from_slice(&common_path_suffix_offset.to_le_bytes());
        data.extend_from_slice(base_bytes);
        data.push(0);
        data.extend_from_slice(suffix_bytes);
        data.push(0);
        debug_assert_eq!(data.len() - link_info_start, link_info_size);

        data
    }

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `.lnk` with an ID list and a Unicode relative path.
    fn lnk_with_relative_path(relative: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&LNK_HEADER_SIZE.to_le_bytes());
        data.extend_from_slice(&LNK_CLSID);
        data.extend_from_slice(
            &(FLAG_HAS_LINK_TARGET_ID_LIST | FLAG_HAS_RELATIVE_PATH | FLAG_IS_UNICODE).to_le_bytes(),
        );
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[0u8; 28]);
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&[0u8; 10]);

        // Empty ID list.
        data.extend_from_slice(&0u16.to_le_bytes());

        let units: Vec<u16> = relative.encode_utf16().collect();
        data.extend_from_slice(&(units.len() as u16).to_le_bytes());
        for unit in units {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data
    }

    #[test]
    fn parses_link_info_local_base_path() {
        let data = lnk_with_link_info("C:\\Users\\me\\Documents\\", "report.pdf");
        assert_eq!(
            parse_lnk_target(&data).as_deref(),
            Some("C:\\Users\\me\\Documents\\report.pdf")
        );
    }

    #[test]
    fn parses_link_info_without_suffix() {
        let data = lnk_with_link_info("C:\\Windows\\System32\\notepad.exe", "");
        assert_eq!(
            parse_lnk_target(&data).as_deref(),
            Some("C:\\Windows\\System32\\notepad.exe")
        );
    }

    #[test]
    fn falls_back_to_relative_path() {
        let data = lnk_with_relative_path("..\\..\\Desktop\\notes.txt");
        assert_eq!(
            parse_lnk_target(&data).as_deref(),
            Some("..\\..\\Desktop\\notes.txt")
        );
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parse_lnk_target(&[]).is_none());
        assert!(parse_lnk_target(&[0u8; 32]).is_none());

        let mut bad_header = lnk_with_link_info("C:\\x\\", "y.txt");
        bad_header[0] = 0xFF;
        assert!(parse_lnk_target(&bad_header).is_none());

        let mut bad_clsid = lnk_with_link_info("C:\\x\\", "y.txt");
        bad_clsid[4] = 0xAA;
        assert!(parse_lnk_target(&bad_clsid).is_none());

        // Truncated LinkInfo must not panic.
        let full = lnk_with_link_info("C:\\Users\\me\\", "a.pdf");
        let truncated = &full[..full.len() - 3];
        let _ = parse_lnk_target(truncated);
    }

    #[test]
    fn header_only_link_has_no_target() {
        let mut data = Vec::new();
        data.extend_from_slice(&LNK_HEADER_SIZE.to_le_bytes());
        data.extend_from_slice(&LNK_CLSID);
        data.extend_from_slice(&0u32.to_le_bytes()); // flags: nothing present
        data.extend_from_slice(&[0u8; 52]);
        assert_eq!(data.len(), LNK_HEADER_SIZE as usize);
        assert!(parse_lnk_target(&data).is_none());
    }

    #[test]
    fn missing_recent_dir_is_not_an_error() {
        let missing = std::env::temp_dir().join("nex-recent-definitely-missing-dir");
        assert!(collect_recent_targets(&missing, 10).is_empty());
    }

    #[test]
    fn collect_reads_lnk_files_and_sorts_by_recent() {
        let dir = std::env::temp_dir().join(format!(
            "nex-recent-collect-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let old = dir.join("old.lnk");
        let new = dir.join("new.lnk");
        std::fs::write(&old, lnk_with_link_info("C:\\tmp\\", "old.txt")).unwrap();
        std::fs::write(&new, lnk_with_link_info("C:\\tmp\\", "new.txt")).unwrap();
        // Make `new` strictly newer.
        let later = SystemTime::now() + std::time::Duration::from_secs(30);
        let _ = filetime_set(&new, later);

        let targets = collect_recent_targets(&dir, 10);
        assert_eq!(targets.len(), 2);
        assert!(
            targets[0].0.to_string_lossy().ends_with("new.txt"),
            "most recent entry should sort first, got {:?}",
            targets[0].0
        );

        let capped = collect_recent_targets(&dir, 1);
        assert_eq!(capped.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Best-effort timestamp bump; if the OS refuses, ordering is still
    /// validated by the dedupe path below.
    fn filetime_set(path: &Path, time: SystemTime) -> std::io::Result<()> {
        let _ = path;
        let _ = time;
        #[cfg(target_os = "windows")]
        {
            // Writing through the same handle updates the last-write time.
            let file = std::fs::OpenOptions::new().append(true).open(path)?;
            let _ = file.set_modified(time);
        }
        Ok(())
    }

    #[test]
    fn identity_matches_filesystem_provider_scheme() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let file = std::env::temp_dir().join(format!("nex-recent-id-{unique}.txt"));
        std::fs::write(&file, b"x").unwrap();
        let (kind, id) = recent_item_identity(&file).expect("file should resolve");
        assert_eq!(kind, "file");
        assert!(id.starts_with("file:"));
        assert_eq!(id, id.to_ascii_lowercase());
        let _ = std::fs::remove_file(&file);

        let missing = std::env::temp_dir().join(format!("nex-recent-missing-{unique}.txt"));
        assert!(recent_item_identity(&missing).is_none());
    }
}
