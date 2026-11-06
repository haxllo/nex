//! Indexer end of the helper's persistent MFT command channel.
//!
//! The indexer (Medium IL) owns the pipe `\\.\pipe\NexMftCmd\<ui pid>` and the
//! elevated helper connects as its client: UIPI blocks a Medium-IL process from
//! writing into a High-IL pipe, but a High-IL client may write into this one.
//! One duplex connection lives per helper session; requests are serialized and
//! answered with framed snapshot streams (see `mft_protocol`).

#![cfg(target_os = "windows")]

use std::fs::File;
use std::io::{BufReader, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PeekNamedPipe};

use crate::mft_protocol::{
    CommandRequest, FrameReader, MftRecord, ReadError, RequestKind, SnapshotMeta, command_pipe_name,
    read_snapshot,
};

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_TYPE_BYTE: u32 = 0;
const PIPE_READMODE_BYTE: u32 = 0;
const PIPE_WAIT: u32 = 0;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const ERROR_PIPE_CONNECTED: u32 = 535;
const PIPE_BUFFER_BYTES: u32 = 1 << 20;

#[derive(Debug)]
pub(crate) enum ChannelError {
    /// No helper connection, or it dropped before the stream began.
    NotConnected(String),
    /// The helper answered, but the snapshot stream failed or was refused.
    Stream(ReadError),
}

struct ChannelState {
    connection: Mutex<Option<Arc<File>>>,
    /// Serializes requests so overlapping passes cannot interleave frames.
    request_lock: Mutex<()>,
    next_id: AtomicU64,
}

static STATE: OnceLock<Arc<ChannelState>> = OnceLock::new();

fn state() -> &'static Arc<ChannelState> {
    STATE.get_or_init(|| {
        let state = Arc::new(ChannelState {
            connection: Mutex::new(None),
            request_lock: Mutex::new(()),
            next_id: AtomicU64::new(0),
        });
        let accept_state = Arc::clone(&state);
        let spawned = std::thread::Builder::new()
            .name("NexMft-cmd-accept".into())
            .spawn(move || accept_loop(&accept_state));
        if let Err(error) = spawned {
            crate::logging::warn(&format!("[nex] mft command channel spawn failed: {error}"));
        }
        state
    })
}

/// Start listening for the helper. Idempotent; safe to call before any request.
pub(crate) fn ensure_started() {
    let _ = state();
}

/// The UI pid the helper derives the pipe name from: the indexer's parent.
fn ui_pid() -> u32 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|arg| arg == "--parent-pid")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(std::process::id)
}

fn wide(text: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn accept_loop(state: &ChannelState) {
    let name = wide(&command_pipe_name(ui_pid()));
    let mut first_instance = true;
    let mut create_failures = 0u32;
    loop {
        let open_mode = PIPE_ACCESS_DUPLEX
            | if first_instance {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            create_failures += 1;
            if create_failures == 1 {
                crate::logging::warn(&format!(
                    "[nex] mft command channel create failed err={}",
                    unsafe { GetLastError() }
                ));
            }
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        first_instance = false;
        create_failures = 0;

        if unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_CONNECTED {
                crate::logging::warn(&format!(
                    "[nex] mft command channel accept failed err={error}"
                ));
                unsafe { CloseHandle(handle) };
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }
        let file = unsafe { File::from_raw_handle(handle as *mut _) };
        // A newer connection replaces the old one; a request still running on
        // the old handle fails on its own once that helper is gone.
        if let Ok(mut slot) = state.connection.lock() {
            *slot = Some(Arc::new(file));
        }
        crate::logging::info("[nex] mft command channel connected");
    }
}

/// True when a helper is attached and its end is still open.
pub(crate) fn is_connected() -> bool {
    let Some(state) = STATE.get() else {
        ensure_started();
        return false;
    };
    let Ok(mut slot) = state.connection.lock() else {
        return false;
    };
    let alive = slot.as_ref().is_some_and(|file| peer_open(file));
    if !alive && slot.take().is_some() {
        crate::logging::info("[nex] mft command channel disconnected");
    }
    alive
}

/// PeekNamedPipe fails with ERROR_BROKEN_PIPE once the client closed.
fn peer_open(file: &File) -> bool {
    let handle = file.as_raw_handle() as HANDLE;
    unsafe {
        PeekNamedPipe(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) != 0
    }
}

fn drop_connection(state: &ChannelState, failed: &Arc<File>) {
    if let Ok(mut slot) = state.connection.lock() {
        if slot.as_ref().is_some_and(|current| Arc::ptr_eq(current, failed)) {
            *slot = None;
        }
    }
}

/// Run one request over the command channel and stream its records.
pub(crate) fn request_snapshot<F>(
    letter: char,
    kind: RequestKind,
    on_record: F,
) -> Result<SnapshotMeta, ChannelError>
where
    F: FnMut(&MftRecord) -> Result<(), String>,
{
    let state = state();
    let _serial = state
        .request_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let connection = state
        .connection
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or_else(|| ChannelError::NotConnected("no helper connection".to_string()))?;

    let id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let request = CommandRequest { id, letter, kind };
    let mut writer: &File = &connection;
    if let Err(error) = writer.write_all(&request.encode()).and_then(|_| writer.flush()) {
        drop_connection(state, &connection);
        return Err(ChannelError::NotConnected(format!("send request: {error}")));
    }

    let mut reader = FrameReader::new(BufReader::with_capacity(256 * 1024, &*connection), id);
    let result = read_snapshot(&mut reader, on_record).and_then(|meta| {
        reader
            .drain()
            .map(|()| meta)
            .map_err(|error| ReadError::from_io("finish stream", error))
    });
    match result {
        Ok(meta) => Ok(meta),
        Err(error) => {
            // A helper-reported refusal leaves the stream aligned; any other
            // failure may have left unread frames, so reconnect.
            if !error.remote {
                drop_connection(state, &connection);
            }
            Err(ChannelError::Stream(error))
        }
    }
}
