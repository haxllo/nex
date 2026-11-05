//! NTFS MFT snapshot server.
//!
//! The helper runs at High IL, so it is the only Nex process allowed to open a
//! volume and enumerate its Master File Table. `NexIndexerProcess` (Medium IL)
//! connects to a read-only byte pipe and rebuilds paths from FRN/parent pairs,
//! which turns a cold 80k-file directory walk into a sequential metadata read.
//!
//! Pipe protocol (byte stream):
//!   header: b"NXMF" | version u16 | letter u8 | reserved u8 | journal_id u64 | next_usn i64
//!   record: frn u64 | parent_frn u64 | attributes u32 | timestamp i64 | name_len u16 | utf8 name
//! The server closes the pipe after the snapshot, so EOF marks the end.

use std::ffi::c_void;
use std::io::Write;
use std::os::windows::io::FromRawHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, GetLogicalDrives,
    GetVolumeInformationW, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW};

use crate::debug_log;

const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00f4;
const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00b3;
const ERROR_HANDLE_EOF: u32 = 38;
const ERROR_PIPE_CONNECTED: u32 = 535;
const DRIVE_FIXED: u32 = 3;
const GENERIC_READ: u32 = 0x8000_0000;
// Pipe access rights live in Storage::FileSystem; the helpers above use the
// constants the helper already declares, kept local here for clarity.
const PIPE_ACCESS_OUTBOUND: u32 = 0x0000_0002;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const HEADER_MAGIC: &[u8; 4] = b"NXMF";
const RECORD_PREFIX_LEN: usize = 30;
const ENUM_BUFFER_BYTES: usize = 1 << 20;

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
    let mut writer = std::io::BufWriter::with_capacity(256 * 1024, pipe);
    let stats = stream_volume(letter, &mut writer)?;
    writer.flush().map_err(|e| format!("flush: {e}"))?;
    debug_log(&format!(
        "[mft] {letter}: streamed records={} skipped={} bytes={} elapsed_ms={}",
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

struct StreamStats {
    records: u64,
    skipped: u64,
    bytes: u64,
    elapsed_ms: u128,
}

fn stream_volume(letter: char, writer: &mut impl Write) -> Result<StreamStats, String> {
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
        let mut header = [0u8; 24];
        header[0..4].copy_from_slice(HEADER_MAGIC);
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6] = letter as u8;
        header[8..16].copy_from_slice(&journal.usn_journal_id.to_le_bytes());
        header[16..24].copy_from_slice(&journal.next_usn.to_le_bytes());
        writer.write_all(&header).map_err(|e| format!("write header: {e}"))?;

        let mut input = MftEnumDataV0 {
            start_file_reference_number: 0,
            low_usn: 0,
            high_usn: journal.next_usn,
        };
        let mut buffer = vec![0u8; ENUM_BUFFER_BYTES];
        let mut records = 0u64;
        let mut skipped = 0u64;
        let mut bytes_written = header.len() as u64;

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
                    // ReFS uses USN_RECORD_V3 with 128-bit references.
                    skipped += 1;
                    offset += record_len;
                    continue;
                }
                let frn = u64::from_le_bytes(record[8..16].try_into().unwrap());
                let parent = u64::from_le_bytes(record[16..24].try_into().unwrap());
                let timestamp = i64::from_le_bytes(record[32..40].try_into().unwrap());
                let attributes = u32::from_le_bytes(record[52..56].try_into().unwrap());
                let name_len = u16::from_le_bytes(record[56..58].try_into().unwrap()) as usize;
                let name_off = u16::from_le_bytes(record[58..60].try_into().unwrap()) as usize;
                if name_len == 0 || name_off + name_len > record_len {
                    skipped += 1;
                    offset += record_len;
                    continue;
                }
                let name_wide: Vec<u16> = record[name_off..name_off + name_len]
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                let name = String::from_utf16_lossy(&name_wide);
                if name.is_empty() || name == "." || name == ".." {
                    offset += record_len;
                    continue;
                }
                let name_bytes = name.as_bytes();
                if name_bytes.len() > u16::MAX as usize {
                    skipped += 1;
                    offset += record_len;
                    continue;
                }
                let mut prefix = [0u8; RECORD_PREFIX_LEN];
                prefix[0..8].copy_from_slice(&frn.to_le_bytes());
                prefix[8..16].copy_from_slice(&parent.to_le_bytes());
                prefix[16..20].copy_from_slice(&attributes.to_le_bytes());
                prefix[20..28].copy_from_slice(&timestamp.to_le_bytes());
                prefix[28..30].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                writer
                    .write_all(&prefix)
                    .and_then(|_| writer.write_all(name_bytes))
                    .map_err(|e| format!("write record: {e}"))?;
                bytes_written += (RECORD_PREFIX_LEN + name_bytes.len()) as u64;
                records += 1;
                offset += record_len;
            }

            if next_frn == 0 || next_frn == input.start_file_reference_number {
                break;
            }
            input.start_file_reference_number = next_frn;
        }

        Ok(StreamStats {
            records,
            skipped,
            bytes: bytes_written,
            elapsed_ms: started.elapsed().as_millis(),
        })
    })();

    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(handle);
    }
    result
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
