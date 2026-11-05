//! NTFS MFT + USN snapshot server.
//!
//! The helper runs at High IL, so it is the only Nex process allowed to open a
//! volume and enumerate its Master File Table or read its change journal.
//! `NexIndexerProcess` connects to a read-only byte pipe and receives either a
//! full MFT snapshot or USN deltas, with paths resolved helper-side.
//!
//! Pipe protocol v2 (byte stream):
//!   header: b"NXMF" | version u16 (2) | letter u8 | mode u8 (0 full, 1 delta) |
//!           journal_id u64 | start_usn i64 | next_usn i64
//!   record: frn u64 | parent_frn u64 | attributes u32 | timestamp i64 |
//!           reason u32 | name_len u16 | path_len u16 | utf8 name | utf8 path
//!   trailer: frn=0, name_len=0, path_len=0, reason=u32::MAX, then next_usn i64
//!
//! Delta requests come from `%APPDATA%\Nex\mft-<letter>.req`:
//!   journal_id=<u64>\nstart_usn=<i64>\n   (or "full")
//! When missing/invalid the server streams a full snapshot. Delta records carry
//! the resolved full path; full records carry the name only and the indexer
//! rebuilds paths from parent references.

use std::ffi::c_void;
use std::io::Write;
use std::os::windows::io::FromRawHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, GetLogicalDrives,
    GetVolumeInformationW, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW};

use crate::debug_log;

const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00f4;
const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00b3;
const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00bb;
const ERROR_HANDLE_EOF: u32 = 38;
const ERROR_PIPE_CONNECTED: u32 = 535;
const DRIVE_FIXED: u32 = 3;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const FILE_SHARE_ALL: u32 = 0x0000_0007;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
// Pipe access rights are file access rights, but windows-sys keeps the pipe
// mode constants in System::Pipes and the access bits here.
const PIPE_ACCESS_OUTBOUND: u32 = 0x0000_0002;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const HEADER_MAGIC: &[u8; 4] = b"NXMF";
const PROTOCOL_VERSION: u16 = 2;
const HEADER_LEN: usize = 32;
const RECORD_PREFIX_LEN: usize = 36;
const ENUM_BUFFER_BYTES: usize = 1 << 20;
const ROOT_RECORD_NUMBER: u64 = 5;
const FRN_RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const DELTA_MODE: u8 = 1;
const FULL_MODE: u8 = 0;

#[repr(C)]
struct MftEnumDataV0 {
    start_file_reference_number: u64,
    low_usn: i64,
    high_usn: i64,
}

#[repr(C)]
#[derive(Default)]
struct UsnJournalDataV0 {
    usn_journal_id: u64,
    first_usn: i64,
    next_usn: i64,
    lowest_valid_usn: i64,
    max_usn: i64,
    maximum_size: u64,
    allocation_delta: u64,
}

#[repr(C)]
struct ReadUsnJournalDataV0 {
    start_usn: i64,
    reason_mask: u32,
    return_only_on_close: u32,
    timeout: u64,
    bytes_to_wait_for: u64,
    usn_journal_id: u64,
}

#[repr(C)]
struct FileIdDescriptor {
    size: u32,
    id_type: u32,
    file_id: u64,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenFileById(
        volume_hint: HANDLE,
        descriptor: *const FileIdDescriptor,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const c_void,
        flags: u32,
    ) -> HANDLE;
    fn GetFinalPathNameByHandleW(
        file: HANDLE,
        path: *mut u16,
        path_len: u32,
        flags: u32,
    ) -> u32;
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Start one snapshot server per fixed NTFS volume. Missing volumes (locked
/// BitLocker, non-NTFS, network) are skipped; the indexer falls back to walkdir.
pub(crate) fn spawn_snapshot_servers() {
    let mask = unsafe { GetLogicalDrives() };
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let root = format!("{letter}:\\");
        let root_wide = wide(&root);
        let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
        if drive_type != DRIVE_FIXED {
            continue;
        }
        let mut fs_buf = [0u16; 32];
        let ok = unsafe {
            GetVolumeInformationW(
                root_wide.as_ptr(),
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
            continue;
        }
        let fs_len = fs_buf.iter().position(|&c| c == 0).unwrap_or(fs_buf.len());
        let file_system = String::from_utf16_lossy(&fs_buf[..fs_len]);
        if !file_system.eq_ignore_ascii_case("NTFS") {
            continue;
        }
        let _ = std::thread::Builder::new()
            .name(format!("NexMft-{letter}"))
            .spawn(move || serve_volume(letter));
        debug_log(&format!("[mft] serving {letter}: snapshots"));
    }
}

fn serve_volume(letter: char) {
    let pipe_name = format!("\\\\.\\pipe\\NexMft-{letter}");
    loop {
        if let Err(error) = serve_one_client(&pipe_name, letter) {
            debug_log(&format!("[mft] {letter}: {error}"));
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

fn serve_one_client(pipe_name: &str, letter: char) -> Result<(), String> {
    let pipe = create_pipe(pipe_name)?;
    wait_for_client(&pipe)?;
    let request = read_request(letter);
    let mut writer = std::io::BufWriter::with_capacity(256 * 1024, pipe);
    let stats = stream_volume(letter, request, &mut writer)?;
    writer.flush().map_err(|e| format!("flush: {e}"))?;
    debug_log(&format!(
        "[mft] {letter}: mode={} records={} skipped={} bytes={} elapsed_ms={}",
        if stats.delta { "delta" } else { "full" },
        stats.records,
        stats.skipped,
        stats.bytes,
        stats.elapsed_ms
    ));
    // Dropping the writer closes the pipe; the indexer reads that as EOF.
    Ok(())
}

fn create_pipe(pipe_name: &str) -> Result<std::fs::File, String> {
    let name = wide(pipe_name);
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            ENUM_BUFFER_BYTES as u32,
            0,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(format!("CreateNamedPipeW failed: {}", unsafe { GetLastError() }));
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle as *mut _) })
}

fn wait_for_client(pipe: &std::fs::File) -> Result<(), String> {
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(pipe) as HANDLE;
    let ok = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    if ok == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_PIPE_CONNECTED {
            return Err(format!("ConnectNamedPipe failed: {error}"));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Request {
    journal_id: u64,
    start_usn: i64,
}

fn request_path(letter: char) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(
        std::path::PathBuf::from(base)
            .join("Nex")
            .join(format!("mft-{letter}.req")),
    )
}

/// `Some(request)` only when the indexer left a valid journal position.
fn read_request(letter: char) -> Option<Request> {
    let path = request_path(letter)?;
    let content = std::fs::read_to_string(&path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("full") {
        return None;
    }
    let mut journal_id = None;
    let mut start_usn = None;
    for line in trimmed.lines() {
        let (key, value) = line.split_once('=')?;
        match key.trim() {
            "journal_id" => journal_id = value.trim().parse::<u64>().ok(),
            "start_usn" => start_usn = value.trim().parse::<i64>().ok(),
            _ => {}
        }
    }
    Some(Request {
        journal_id: journal_id?,
        start_usn: start_usn?,
    })
}

struct StreamStats {
    delta: bool,
    records: u64,
    skipped: u64,
    bytes: u64,
    elapsed_ms: u128,
}

fn stream_volume(
    letter: char,
    request: Option<Request>,
    writer: &mut impl Write,
) -> Result<StreamStats, String> {
    let started = Instant::now();
    let volume_path = format!("\\\\.\\{letter}:");
    let volume_name = wide(&volume_path);
    let handle = unsafe {
        CreateFileW(
            volume_name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "open {volume_path} failed: {} (needs elevation or unlocked volume)",
            unsafe { GetLastError() }
        ));
    }

    let result = (|| -> Result<StreamStats, String> {
        let journal = query_journal(handle)?;
        let delta_request = request.filter(|request| {
            request.journal_id == journal.usn_journal_id
                && request.start_usn >= journal.lowest_valid_usn
                && request.start_usn <= journal.next_usn
        });

        let mut header = [0u8; HEADER_LEN];
        header[0..4].copy_from_slice(HEADER_MAGIC);
        header[4..6].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        header[6] = letter as u8;
        header[7] = if delta_request.is_some() {
            DELTA_MODE
        } else {
            FULL_MODE
        };
        header[8..16].copy_from_slice(&journal.usn_journal_id.to_le_bytes());
        header[16..24].copy_from_slice(
            &delta_request
                .map(|request| request.start_usn)
                .unwrap_or(0)
                .to_le_bytes(),
        );
        header[24..32].copy_from_slice(&journal.next_usn.to_le_bytes());
        writer.write_all(&header).map_err(|e| format!("write header: {e}"))?;

        let mut stats = StreamStats {
            delta: delta_request.is_some(),
            records: 0,
            skipped: 0,
            bytes: header.len() as u64,
            elapsed_ms: 0,
        };
        let next_usn = match delta_request {
            Some(request) => stream_deltas(handle, letter, request.start_usn, &journal, writer, &mut stats)?,
            None => stream_full(handle, letter, journal.next_usn, writer, &mut stats)?,
        };
        write_trailer(writer, next_usn, &mut stats)?;
        stats.elapsed_ms = started.elapsed().as_millis();
        Ok(stats)
    })();

    unsafe {
        CloseHandle(handle);
    }
    result
}

fn stream_full(
    handle: HANDLE,
    letter: char,
    high_usn: i64,
    writer: &mut impl Write,
    stats: &mut StreamStats,
) -> Result<i64, String> {
    let mut input = MftEnumDataV0 {
        start_file_reference_number: 0,
        low_usn: 0,
        high_usn,
    };
    let mut buffer = vec![0u8; ENUM_BUFFER_BYTES];
    loop {
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                &input as *const _ as *const c_void,
                std::mem::size_of::<MftEnumDataV0>() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_HANDLE_EOF {
                break;
            }
            return Err(format!("FSCTL_ENUM_USN_DATA failed: {error}"));
        }
        let returned = returned as usize;
        if returned <= 8 {
            break;
        }
        let next_frn = u64::from_le_bytes(buffer[0..8].try_into().unwrap());
        let mut offset = 8usize;
        while offset + 4 <= returned {
            let record_len =
                u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
            if record_len < 60 || offset + record_len > returned {
                break;
            }
            let record = &buffer[offset..offset + record_len];
            let major = u16::from_le_bytes(record[4..6].try_into().unwrap());
            if major != 2 {
                stats.skipped += 1;
                offset += record_len;
                continue;
            }
            let frn = u64::from_le_bytes(record[8..16].try_into().unwrap());
            let parent = u64::from_le_bytes(record[16..24].try_into().unwrap());
            let timestamp = i64::from_le_bytes(record[32..40].try_into().unwrap());
            let attributes = u32::from_le_bytes(record[52..56].try_into().unwrap());
            let name_len = u16::from_le_bytes(record[56..58].try_into().unwrap()) as usize;
            let name_off = u16::from_le_bytes(record[58..60].try_into().unwrap()) as usize;
            if let Some(name) = record_name(record, record_len, name_off, name_len) {
                write_record(
                    writer, frn, parent, attributes, timestamp, 0, &name, None, stats,
                )?;
            } else {
                stats.skipped += 1;
            }
            offset += record_len;
        }
        if next_frn == 0 || next_frn == input.start_file_reference_number {
            break;
        }
        input.start_file_reference_number = next_frn;
    }
    let _ = letter;
    Ok(high_usn)
}

fn stream_deltas(
    handle: HANDLE,
    letter: char,
    start_usn: i64,
    journal: &UsnJournalDataV0,
    writer: &mut impl Write,
    stats: &mut StreamStats,
) -> Result<i64, String> {
    let mut buffer = vec![0u8; ENUM_BUFFER_BYTES];
    let mut next = start_usn;
    let mut final_next = start_usn;
    for _ in 0..100_000 {
        let input = ReadUsnJournalDataV0 {
            start_usn: next,
            reason_mask: u32::MAX,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            usn_journal_id: journal.usn_journal_id,
        };
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                &input as *const _ as *const c_void,
                std::mem::size_of::<ReadUsnJournalDataV0>() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_HANDLE_EOF {
                break;
            }
            return Err(format!("FSCTL_READ_USN_JOURNAL failed: {error}"));
        }
        let returned = returned as usize;
        if returned <= 8 {
            break;
        }
        let new_next = i64::from_le_bytes(buffer[0..8].try_into().unwrap());
        let mut offset = 8usize;
        while offset + 4 <= returned {
            let record_len =
                u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
            if record_len < 60 || offset + record_len > returned {
                break;
            }
            let record = &buffer[offset..offset + record_len];
            let major = u16::from_le_bytes(record[4..6].try_into().unwrap());
            if major != 2 {
                stats.skipped += 1;
                offset += record_len;
                continue;
            }
            let frn = u64::from_le_bytes(record[8..16].try_into().unwrap());
            let parent = u64::from_le_bytes(record[16..24].try_into().unwrap());
            let timestamp = i64::from_le_bytes(record[32..40].try_into().unwrap());
            let reason = u32::from_le_bytes(record[40..44].try_into().unwrap());
            let attributes = u32::from_le_bytes(record[52..56].try_into().unwrap());
            let name_len = u16::from_le_bytes(record[56..58].try_into().unwrap()) as usize;
            let name_off = u16::from_le_bytes(record[58..60].try_into().unwrap()) as usize;
            let Some(name) = record_name(record, record_len, name_off, name_len) else {
                stats.skipped += 1;
                offset += record_len;
                continue;
            };
            let path = resolve_parent_path(handle, letter, parent)
                .map(|parent_path| join_path(&parent_path, &name));
            write_record(
                writer,
                frn,
                parent,
                attributes,
                timestamp,
                reason,
                &name,
                path.as_deref(),
                stats,
            )?;
            offset += record_len;
        }
        final_next = new_next;
        if new_next <= next {
            break;
        }
        next = new_next;
    }
    Ok(final_next)
}

fn record_name(record: &[u8], record_len: usize, offset: usize, len: usize) -> Option<String> {
    if len == 0 || offset + len > record_len {
        return None;
    }
    let name_wide: Vec<u16> = record[offset..offset + len]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let name = String::from_utf16_lossy(&name_wide);
    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name)
    }
}

fn write_record(
    writer: &mut impl Write,
    frn: u64,
    parent: u64,
    attributes: u32,
    timestamp: i64,
    reason: u32,
    name: &str,
    path: Option<&str>,
    stats: &mut StreamStats,
) -> Result<(), String> {
    let name_bytes = name.as_bytes();
    let path_bytes = path.unwrap_or("").as_bytes();
    if name_bytes.len() > u16::MAX as usize || path_bytes.len() > u16::MAX as usize {
        stats.skipped += 1;
        return Ok(());
    }
    let mut prefix = [0u8; RECORD_PREFIX_LEN];
    prefix[0..8].copy_from_slice(&frn.to_le_bytes());
    prefix[8..16].copy_from_slice(&parent.to_le_bytes());
    prefix[16..20].copy_from_slice(&attributes.to_le_bytes());
    prefix[20..28].copy_from_slice(&timestamp.to_le_bytes());
    prefix[28..32].copy_from_slice(&reason.to_le_bytes());
    prefix[32..34].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    prefix[34..36].copy_from_slice(&(path_bytes.len() as u16).to_le_bytes());
    writer
        .write_all(&prefix)
        .and_then(|_| writer.write_all(name_bytes))
        .and_then(|_| writer.write_all(path_bytes))
        .map_err(|e| format!("write record: {e}"))?;
    stats.records += 1;
    stats.bytes += (RECORD_PREFIX_LEN + name_bytes.len() + path_bytes.len()) as u64;
    Ok(())
}

fn write_trailer(writer: &mut impl Write, next_usn: i64, stats: &mut StreamStats) -> Result<(), String> {
    let mut prefix = [0u8; RECORD_PREFIX_LEN];
    prefix[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    writer
        .write_all(&prefix)
        .and_then(|_| writer.write_all(&next_usn.to_le_bytes()))
        .map_err(|e| format!("write trailer: {e}"))?;
    stats.bytes += (RECORD_PREFIX_LEN + 8) as u64;
    Ok(())
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.ends_with('\\') || parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}\\{name}")
    }
}

/// Resolve a parent directory's full path from its file reference number.
fn resolve_parent_path(volume: HANDLE, letter: char, parent_frn: u64) -> Option<String> {
    if parent_frn & FRN_RECORD_MASK == ROOT_RECORD_NUMBER {
        return Some(format!("{letter}:\\"));
    }
    let descriptor = FileIdDescriptor {
        size: std::mem::size_of::<FileIdDescriptor>() as u32,
        id_type: 1, // FileIdType
        file_id: parent_frn,
    };
    let file = unsafe {
        OpenFileById(
            volume,
            &descriptor,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_ALL,
            std::ptr::null(),
            FILE_FLAG_BACKUP_SEMANTICS,
        )
    };
    if file == INVALID_HANDLE_VALUE || file.is_null() {
        return None;
    }
    let mut buffer = vec![0u16; 32_768];
    let len = unsafe { GetFinalPathNameByHandleW(file, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    unsafe {
        CloseHandle(file);
    }
    if len == 0 || len as usize >= buffer.len() {
        return None;
    }
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    Some(
        path.strip_prefix(r"\\?\")
            .unwrap_or(path.as_str())
            .to_string(),
    )
}

fn query_journal(handle: HANDLE) -> Result<UsnJournalDataV0, String> {
    let mut journal = UsnJournalDataV0::default();
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            &mut journal as *mut _ as *mut c_void,
            std::mem::size_of::<UsnJournalDataV0>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!("FSCTL_QUERY_USN_JOURNAL failed: {}", unsafe {
            GetLastError()
        }));
    }
    Ok(journal)
}

#[allow(dead_code)]
fn is_directory(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_DIRECTORY != 0
}
