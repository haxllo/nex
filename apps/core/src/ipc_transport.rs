//! Transport for the UI to indexer link.
//!
//! This module moves whole request and response lines and knows nothing about
//! what is inside them. The protocol (versions, methods, parameters) lives in
//! [`crate::ipc`], so a change to the wire format does not touch a `CreateFileW`
//! and a change to the transport does not touch a handler.
//!
//! # Why a pipe and not a socket
//!
//! The link used to be an unauthenticated loopback TCP socket on a fixed port.
//! That exposed an unauthenticated control channel to every process on the
//! machine — an unrelated process could read a 360,000-item snapshot of indexed
//! paths, or call `shutdown` — and because a port is per machine while the UI's
//! singleton is per session, a second Windows session could reach the first
//! session's indexer. See CIQ-242.
//!
//! A named pipe is scoped by name and guarded by a security descriptor, so the
//! link is reachable only by the user who owns the index. The name carries the
//! session id as well, so two sessions are two separate links rather than one
//! contested port.
//!
//! # Framing
//!
//! One request line and one response line per connection, newline terminated,
//! UTF-8 JSON inside. Byte mode means a frame may arrive in arbitrary chunks,
//! which is why every read accumulates until it sees the terminator rather than
//! assuming one `ReadFile` is one frame.
//!
//! Frame caps exist because an unbounded read is a memory hazard. The request
//! cap is small and the response cap is generous: a full snapshot is
//! legitimately tens of megabytes, so its cap is a runaway guard rather than a
//! protocol limit.

use std::time::Duration;

/// Largest request this end will accept. Requests carry a query, a generation
/// and a limit, so a megabyte is already orders of magnitude too much.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Largest response this end will accept. Deliberately generous: `snapshot`
/// legitimately answers with the whole item store.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;

/// A bound, listening endpoint. Creating one claims the endpoint name for this
/// process; a second claimant gets `None` instead of a second server.
pub struct Listener {
    imp: imp::Listener,
}

/// Bind `endpoint` for this user and session, or `None` when another process
/// already owns it. A successful bind means the endpoint is already
/// connectable.
pub fn bind(endpoint: &str) -> Option<Listener> {
    imp::Listener::bind(endpoint).map(|imp| Listener { imp })
}

impl Listener {
    /// Serve connections until the process exits. `handler` receives one
    /// request line and returns one response line, without the newline.
    pub fn serve<F>(self, handler: F)
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        self.imp.serve(handler);
    }
}

/// Send one request line and wait for one response line.
pub fn call(endpoint: &str, request_line: &str, timeout: Duration) -> Result<String, String> {
    if request_line.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "request of {} bytes exceeds the {} byte frame cap",
            request_line.len(),
            MAX_REQUEST_BYTES
        ));
    }
    imp::call(endpoint, request_line, timeout)
}

/// Append `chunk` to `pending` and return the frame once its newline arrives.
///
/// Shared by both transports because the framing rule is the same on each, and
/// a platform that gets it subtly wrong is a bug nobody reproduces. On `Ok(None)`
/// the caller keeps reading.
fn take_complete_frame(
    pending: &mut Vec<u8>,
    chunk: &[u8],
    cap: usize,
) -> Result<Option<Vec<u8>>, String> {
    pending.extend_from_slice(chunk);
    if let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
        let frame = pending[..position].to_vec();
        return Ok(Some(frame));
    }
    if pending.len() > cap {
        return Err(format!("frame exceeds the {cap} byte cap"));
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{take_complete_frame, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
    use std::ptr;
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING,
        ERROR_MORE_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
        GENERIC_READ, GENERIC_WRITE,
        HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject,
    };

    /// Chunk size for pipe transfers, and the buffer size for each direction.
    const CHUNK_BYTES: usize = 64 * 1024;

    /// How many connections can be in flight at once, and therefore how many
    /// pipe instances exist.
    ///
    /// Only one caller exists in practice (the UI), but a long-poll `sync`
    /// parks for up to 30 seconds, so a single instance would make a keystroke
    /// arriving in that window read as an unreachable indexer. Four leaves room
    /// for a parked `sync`, a keystroke seed, a teardown `shutdown` and a
    /// background snapshot at the same time.
    const WORKERS: usize = 4;

    /// Applies only to clients that call `WaitNamedPipe` without their own
    /// timeout; this transport always passes one.
    const PIPE_DEFAULT_TIMEOUT_MS: u32 = 0;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn current_session_id() -> u32 {
        let mut session = 0u32;
        // SAFETY: `session` is a valid out pointer for the duration of the call.
        unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
        session
    }

    /// The current user's SID in the string form SDDL expects.
    fn current_user_sid() -> Option<String> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: `token` is a valid out pointer; the handle is closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let sid = user_sid_from_token(token);
        // SAFETY: `token` was opened above and is not used afterwards.
        unsafe { CloseHandle(token) };
        sid
    }

    fn user_sid_from_token(token: HANDLE) -> Option<String> {
        let mut needed = 0u32;
        // SAFETY: a null buffer with length 0 is the documented size query; it
        // fails with ERROR_INSUFFICIENT_BUFFER and fills `needed`.
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: `buffer` is writable for `needed` bytes, which is the size the
        // API itself reported.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: on success the buffer holds one TOKEN_USER whose SID lives in
        // the same allocation, still owned by `buffer`.
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: `user.User.Sid` points inside `buffer`, alive for this scope.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
            return None;
        }
        let mut len = 0usize;
        // SAFETY: a NUL-terminated UTF-16 string allocated by the API.
        unsafe {
            while *text.add(len) != 0 {
                len += 1;
            }
        }
        // SAFETY: `text` is valid for `len` code units; LocalFree takes the
        // allocation back immediately after the copy.
        let sid = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, len)) };
        // SAFETY: allocated by ConvertSidToStringSidW, which mandates LocalFree.
        unsafe { LocalFree(text.cast()) };
        Some(sid)
    }

    /// `"<session>-<sid hash>"`, which makes an endpoint unique per user *and*
    /// per session without putting a full SID in a name other processes list.
    fn scope_tag() -> &'static str {
        static TAG: OnceLock<String> = OnceLock::new();
        TAG.get_or_init(|| {
            let sid = current_user_sid().unwrap_or_default();
            // FNV-1a: stable across runs and cheap. This is a name component,
            // not a security primitive — the security descriptor does that.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in sid.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            format!("{}-{:08x}", current_session_id(), hash as u32)
        })
    }

    fn pipe_name(endpoint: &str) -> Vec<u16> {
        wide(&format!(r"\\.\pipe\{endpoint}-v1-{}", scope_tag()))
    }

    fn guard_name(endpoint: &str) -> Vec<u16> {
        // `Local\` keeps the ownership flag per session, matching the pipe.
        wide(&format!(r"Local\{endpoint}-v1-{}", scope_tag()))
    }

    /// Security descriptor restricting the pipe to this user and LocalSystem.
    ///
    /// A `None` here is not fatal — the name is still session and SID scoped, so
    /// the link is not the fixed, machine-wide channel it replaced — but it is
    /// logged, because a permissive DACL is the difference between "only this
    /// user can reach it" and "the process default applies".
    fn security_attributes() -> Option<(SECURITY_ATTRIBUTES, *mut core::ffi::c_void)> {
        let sid = current_user_sid()?;
        // `P` marks the DACL protected, so nothing is inherited from the parent.
        let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
        let mut descriptor: *mut core::ffi::c_void = ptr::null_mut();
        // SAFETY: `sddl` is NUL terminated and outlives the call.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return None;
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        Some((attributes, descriptor))
    }

    fn create_instance(name: &[u16], attributes: *const SECURITY_ATTRIBUTES) -> Option<HANDLE> {
        // SAFETY: `name` is NUL terminated and `attributes`, when not null,
        // points at a descriptor the caller keeps alive.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                CHUNK_BYTES as u32,
                CHUNK_BYTES as u32,
                PIPE_DEFAULT_TIMEOUT_MS,
                attributes,
            )
        };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(handle)
        }
    }

    pub(super) struct Listener {
        name: Vec<u16>,
        attributes: Option<SECURITY_ATTRIBUTES>,
        descriptor: *mut core::ffi::c_void,
        /// Held for the process lifetime so a second indexer cannot claim the
        /// endpoint. The socket this replaced used the port being taken as its
        /// ownership flag, which made ownership a side effect rather than a
        /// stated intent.
        guard: HANDLE,
        /// Created in `bind`, not in `serve`: a listener that reports success
        /// must already be connectable, because callers treat a successful bind
        /// as "ready" and a gap between the two reads as a dead indexer.
        ///
        /// A fixed set, one per worker, each reused for every connection it
        /// serves. That is the shape of this whole transport: a worker owns one
        /// instance for its lifetime and loops over connections. Creating an
        /// instance per connection instead left a thread parked in
        /// `ConnectNamedPipe` for every connection ever accepted, which grows at
        /// scheduler speed and starves the rest of the process.
        instances: Vec<HANDLE>,
    }

    // SAFETY: every handle in this struct is owned exclusively by it — created
    // here, reached only through `&self`, and closed exactly once in `Drop`. A
    // Windows kernel handle has no thread affinity, so moving the listener to
    // the thread that serves it transfers ownership without sharing it.
    unsafe impl Send for Listener {}

    impl Drop for Listener {
        fn drop(&mut self) {
            // SAFETY: every handle here is owned by this struct and dropped
            // once. `instances` is empty after `serve` takes it, and a listener
            // that served is never dropped because `serve` parks.
            unsafe {
                CloseHandle(self.guard);
                if !self.descriptor.is_null() {
                    LocalFree(self.descriptor);
                }
                for instance in &self.instances {
                    DisconnectNamedPipe(*instance);
                    CloseHandle(*instance);
                }
            }
        }
    }

    impl Listener {
        pub(super) fn bind(endpoint: &str) -> Option<Self> {
            let name = guard_name(endpoint);
            // SAFETY: `name` is NUL terminated; no initial owner requested.
            let guard = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
            if guard.is_null() {
                return None;
            }
            // ERROR_ALREADY_EXISTS means another indexer already owns this
            // endpoint for this user and session.
            if unsafe { GetLastError() } == 183 {
                // SAFETY: `guard` was just created and is not used again.
                unsafe { CloseHandle(guard) };
                return None;
            }

            let (attributes, descriptor) = match security_attributes() {
                Some((attributes, descriptor)) => (Some(attributes), descriptor),
                None => {
                    crate::logging::warn(
                        "[nex-ipc] could not restrict the indexer link to this user; \
                         falling back to the default pipe security",
                    );
                    (None, ptr::null_mut())
                }
            };

            let name = pipe_name(endpoint);
            let attributes_ptr = attributes
                .as_ref()
                .map_or(ptr::null(), |attributes| attributes as *const _);
            let mut instances = Vec::with_capacity(WORKERS);
            for _ in 0..WORKERS {
                match create_instance(&name, attributes_ptr) {
                    Some(instance) => instances.push(instance),
                    None => {
                        // SAFETY: every handle in this branch is owned here and
                        // not used afterwards.
                        unsafe {
                            for instance in &instances {
                                CloseHandle(*instance);
                            }
                            CloseHandle(guard);
                            if !descriptor.is_null() {
                                LocalFree(descriptor);
                            }
                        }
                        return None;
                    }
                }
            }

            Some(Self {
                name,
                attributes,
                descriptor,
                guard,
                instances,
            })
        }

        pub(super) fn serve<F>(mut self, handler: F)
        where
            F: Fn(&str) -> String + Send + Sync + 'static,
        {
            let handler = Arc::new(handler);
            let instances = std::mem::take(&mut self.instances);
            for instance in instances {
                let handler = Arc::clone(&handler);
                let raw = instance as usize;
                let spawned = std::thread::Builder::new()
                    .name("nex-indexer-conn".into())
                    .spawn(move || serve_worker(raw as HANDLE, handler));
                if spawned.is_err() {
                    // SAFETY: the thread never ran, so this call still owns the
                    // instance.
                    unsafe {
                        DisconnectNamedPipe(instance);
                        CloseHandle(instance);
                    }
                }
            }
            // The ownership mutex, the security descriptor and the instance
            // names must outlive the workers. Serving is a process-lifetime
            // job with nothing to return to, so park rather than drop them.
            loop {
                std::thread::park();
            }
        }
    }

    /// Wait until the client has drained the reply before disconnecting.
    ///
    /// This is the handshake the whole transport depends on.
    /// `DisconnectNamedPipe` discards data the client has not read yet, and the
    /// reply is written on a thread that tears the instance down immediately —
    /// so without this the client's read fails with ERROR_PIPE_NOT_CONNECTED
    /// and a fast answer looks like a dead indexer. A pipe server's
    /// `FlushFileBuffers` blocks exactly until the client has consumed
    /// everything, which makes one-request-per-connection deterministic instead
    /// of racy.
    ///
    /// Not bounded, and it does not need to be: the indexer's watchdog and the
    /// client's own read timeout both end a peer that connects and never reads.
    fn flush_before_disconnect(handle: HANDLE) {
        // SAFETY: `handle` is a connected server pipe owned by this call.
        unsafe { FlushFileBuffers(handle) };
    }

    /// Serve connections forever on one pipe instance.
    ///
    /// One instance per worker, reused: `ConnectNamedPipe` waits for a client,
    /// the request is answered, `DisconnectNamedPipe` resets the instance for
    /// the next one. The only exit is the process ending, which is what the
    /// UI's teardown does.
    fn serve_worker<F>(handle: HANDLE, handler: Arc<F>)
    where
        F: Fn(&str) -> String,
    {
        loop {
            // SAFETY: `handle` is an unconnected pipe instance owned by this
            // worker.
            let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) };
            // ERROR_PIPE_CONNECTED means a client arrived between create and
            // connect, which is a success rather than a failure.
            let ready =
                connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !ready {
                return;
            }
            if let Ok(request) = read_frame_blocking(handle) {
                let response = handler(&request);
                if write_all_blocking(handle, response.as_bytes()).is_ok()
                    && write_all_blocking(handle, b"\n").is_ok()
                {
                    flush_before_disconnect(handle);
                }
            }
            // SAFETY: `handle` is still owned by this worker, and is reset
            // rather than closed so the next connection reuses it.
            unsafe { DisconnectNamedPipe(handle) };
        }
    }

    fn read_frame_blocking(handle: HANDLE) -> Result<String, String> {
        let mut frame: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; CHUNK_BYTES];
        loop {
            let mut read = 0u32;
            // SAFETY: `chunk` is writable for its whole length.
            let ok = unsafe {
                ReadFile(
                    handle,
                    chunk.as_mut_ptr(),
                    chunk.len() as u32,
                    &mut read,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let error = unsafe { GetLastError() };
                // Byte mode may report a partial message as ERROR_MORE_DATA;
                // the bytes it read are still valid.
                if error != ERROR_MORE_DATA {
                    return Err(format!("indexer link read failed ({error})"));
                }
            }
            if read == 0 {
                return Err("indexer link closed before a full request arrived".into());
            }
            if let Some(line) =
                take_complete_frame(&mut frame, &chunk[..read as usize], MAX_REQUEST_BYTES)?
            {
                return String::from_utf8(line)
                    .map_err(|error| format!("request not UTF-8: {error}"));
            }
        }
    }

    fn write_all_blocking(handle: HANDLE, bytes: &[u8]) -> Result<(), String> {
        let mut offset = 0usize;
        while offset < bytes.len() {
            let mut written = 0u32;
            // SAFETY: the slice is readable for the length passed.
            let ok = unsafe {
                WriteFile(
                    handle,
                    bytes[offset..].as_ptr(),
                    (bytes.len() - offset) as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(format!(
                    "indexer link write failed ({})",
                    unsafe { GetLastError() }
                ));
            }
            if written == 0 {
                return Err("indexer link stopped accepting data".into());
            }
            offset += written as usize;
        }
        Ok(())
    }

    pub(super) fn call(
        endpoint: &str,
        request_line: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let name = pipe_name(endpoint);
        let deadline = Instant::now() + timeout;
        let handle = open_client(&name, deadline)?;
        let mut frame = Vec::with_capacity(request_line.len() + 1);
        frame.extend_from_slice(request_line.as_bytes());
        frame.push(b'\n');
        let result = write_all_overlapped(handle, &frame, deadline)
            .and_then(|()| read_frame_overlapped(handle, deadline));
        // SAFETY: the handle is owned by this call.
        unsafe { CloseHandle(handle) };
        result
    }

    fn open_client(name: &[u16], deadline: Instant) -> Result<HANDLE, String> {
        loop {
            // SAFETY: `name` is NUL terminated; the pipe carries its own
            // security, so no attributes are needed to open it.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    ptr::null_mut(),
                )
            };
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                return Ok(handle);
            }
            let error = unsafe { GetLastError() };
            if error == ERROR_PIPE_BUSY {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("indexer link is busy and the deadline passed".into());
                }
                let millis = remaining.as_millis().min(u128::from(u32::MAX)) as u32;
                // SAFETY: `name` is NUL terminated.
                unsafe { WaitNamedPipeW(name.as_ptr(), millis.max(1)) };
                continue;
            }
            return Err(match error {
                ERROR_FILE_NOT_FOUND => "indexer is not running".to_string(),
                other => format!("could not open the indexer link ({other})"),
            });
        }
    }

    /// A manual-reset event, used to give each overlapped transfer a deadline.
    struct IoEvent(HANDLE);

    impl IoEvent {
        fn new() -> Option<Self> {
            // SAFETY: no security attributes, manual reset, initially unsignaled.
            let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            (!handle.is_null()).then_some(Self(handle))
        }
    }

    impl Drop for IoEvent {
        fn drop(&mut self) {
            // SAFETY: the event is owned by this struct.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Why a single transfer attempt failed.
    enum IoFailure {
        /// The client end is connected but the server has not yet accepted the
        /// instance. Overlapped I/O refuses until it has, so this is transient.
        NotAccepted,
        Fatal(String),
    }

    /// One overlapped transfer, bounded by `deadline`.
    ///
    /// A blocking pipe read cannot be given a timeout, and running the call on
    /// a throwaway thread leaks that thread until the pipe breaks — which a
    /// live-but-silent indexer never does. Overlapped I/O with a wait on an
    /// event is the only way to keep the caller's budget honest, and these
    /// budgets are load-bearing: 250 ms for liveness, 500 ms for a keystroke
    /// seed and for shutdown.
    fn transfer(
        handle: HANDLE,
        buffer: *mut u8,
        len: u32,
        read: bool,
        deadline: Instant,
    ) -> Result<u32, String> {
        loop {
            match transfer_once(handle, buffer, len, read, deadline) {
                Ok(done) => return Ok(done),
                Err(IoFailure::Fatal(message)) => return Err(message),
                Err(IoFailure::NotAccepted) => {
                    // The connection exists from the client's side, so the
                    // server's `ConnectNamedPipe` is imminent. Spin briefly
                    // rather than reporting a dead indexer during startup.
                    if Instant::now() >= deadline {
                        return Err("timed out waiting for the indexer to accept".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn transfer_once(
        handle: HANDLE,
        buffer: *mut u8,
        len: u32,
        read: bool,
        deadline: Instant,
    ) -> Result<u32, IoFailure> {
        let event = IoEvent::new().ok_or_else(|| {
            IoFailure::Fatal("could not create an indexer link event".into())
        })?;
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;

        // SAFETY: `buffer` is valid for `len` bytes in the direction used, and
        // `overlapped` outlives the operation.
        let started = unsafe {
            if read {
                ReadFile(handle, buffer, len, ptr::null_mut(), &mut overlapped)
            } else {
                WriteFile(handle, buffer, len, ptr::null_mut(), &mut overlapped)
            }
        };

        if started == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_PIPE_NOT_CONNECTED {
                return Err(IoFailure::NotAccepted);
            }
            if error != ERROR_IO_PENDING {
                return Err(IoFailure::Fatal(format!(
                    "indexer link I/O failed ({error})"
                )));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let millis = remaining.as_millis().min(u128::from(u32::MAX)) as u32;
            // SAFETY: `event` is a valid handle owned by this function.
            let wait = unsafe { WaitForSingleObject(event.0, millis.max(1)) };
            if wait == WAIT_TIMEOUT {
                // SAFETY: the operation is still pending on this handle and
                // this OVERLAPPED; cancel before dropping either.
                unsafe { CancelIoEx(handle, &overlapped) };
                return Err(IoFailure::Fatal(
                    "timed out waiting for the indexer".into(),
                ));
            }
            if wait != WAIT_OBJECT_0 {
                return Err(IoFailure::Fatal(format!(
                    "indexer link wait failed ({wait})"
                )));
            }
        }

        let mut done = 0u32;
        // SAFETY: the transfer finished, so this only collects its result.
        let ok = unsafe { GetOverlappedResult(handle, &overlapped, &mut done, 0) };
        if ok == 0 {
            return Err(IoFailure::Fatal(format!(
                "indexer link transfer failed ({})",
                unsafe { GetLastError() }
            )));
        }
        Ok(done)
    }

    fn write_all_overlapped(
        handle: HANDLE,
        bytes: &[u8],
        deadline: Instant,
    ) -> Result<(), String> {
        let mut offset = 0usize;
        while offset < bytes.len() {
            if Instant::now() >= deadline {
                return Err("timed out writing to the indexer".into());
            }
            let remaining = (bytes.len() - offset).min(CHUNK_BYTES);
            // SAFETY: `WriteFile` only reads the buffer; the pointer passed is
            // the slice's own start and the length is its true remainder.
            let done = transfer(
                handle,
                bytes[offset..].as_ptr() as *mut u8,
                remaining as u32,
                false,
                deadline,
            )?;
            if done == 0 {
                return Err("indexer link stopped accepting data".into());
            }
            offset += done as usize;
        }
        Ok(())
    }

    fn read_frame_overlapped(handle: HANDLE, deadline: Instant) -> Result<String, String> {
        let mut frame: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; CHUNK_BYTES];
        loop {
            if Instant::now() >= deadline {
                return Err("timed out waiting for the indexer".into());
            }
            let done = transfer(
                handle,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                true,
                deadline,
            )?;
            if done == 0 {
                return Err("indexer closed the link before replying".into());
            }
            if let Some(line) =
                take_complete_frame(&mut frame, &chunk[..done as usize], MAX_RESPONSE_BYTES)?
            {
                return String::from_utf8(line)
                    .map_err(|error| format!("response not UTF-8: {error}"));
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::{take_complete_frame, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// The Windows transport is the shipped one; this exists so the crate still
    /// builds and its tests still run on a non-Windows host, with the same
    /// semantics over a Unix socket.
    fn socket_path(endpoint: &str) -> PathBuf {
        let tag = std::env::var("NEX_TEST_SCOPE").unwrap_or_else(|_| std::process::id().to_string());
        std::env::temp_dir().join(format!("nex-{endpoint}-{tag}.sock"))
    }

    pub(super) struct Listener {
        listener: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub(super) fn bind(endpoint: &str) -> Option<Self> {
            let path = socket_path(endpoint);
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).ok()?;
            Some(Self { listener, path })
        }

        pub(super) fn serve<F>(self, handler: F)
        where
            F: Fn(&str) -> String + Send + Sync + 'static,
        {
            let handler = std::sync::Arc::new(handler);
            for stream in self.listener.incoming() {
                let Ok(stream) = stream else { continue };
                let handler = std::sync::Arc::clone(&handler);
                std::thread::spawn(move || {
                    let mut reader = match stream.try_clone() {
                        Ok(clone) => BufReader::new(clone),
                        Err(_) => return,
                    };
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.len() > MAX_REQUEST_BYTES {
                        return;
                    }
                    let response = handler(line.trim_end_matches('\n'));
                    let mut stream = stream;
                    let _ = write!(stream, "{response}\n");
                    let _ = stream.flush();
                });
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub(super) fn call(
        endpoint: &str,
        request_line: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let stream = UnixStream::connect(socket_path(endpoint)).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
        write!(writer, "{request_line}\n").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;

        let mut reader = stream;
        let mut frame: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let read = reader.read(&mut chunk).map_err(|e| e.to_string())?;
            if read == 0 {
                return Err("indexer closed the link before replying".into());
            }
            if let Some(line) =
                take_complete_frame(&mut frame, &chunk[..read], MAX_RESPONSE_BYTES)?
            {
                return String::from_utf8(line).map_err(|e| e.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_split_across_reads_is_reassembled() {
        let mut pending = Vec::new();
        assert_eq!(
            take_complete_frame(&mut pending, b"{\"a\":", 1024).expect("fits"),
            None
        );
        assert_eq!(
            take_complete_frame(&mut pending, b"1}\n", 1024).expect("fits"),
            Some(b"{\"a\":1}".to_vec())
        );
    }

    #[test]
    fn a_frame_over_the_cap_is_refused() {
        let mut pending = Vec::new();
        let error = take_complete_frame(&mut pending, &vec![b'x'; 2048], 1024)
            .expect_err("over the cap");
        assert!(error.contains("cap"), "{error}");
    }

    /// The request bound exists so a peer cannot drive an unbounded allocation
    /// on the other end. It must reject before anything is sent.
    #[test]
    fn an_oversized_request_is_rejected_without_touching_the_transport() {
        let huge = "x".repeat(MAX_REQUEST_BYTES + 1);
        let error = call("nex-test-unreachable", &huge, Duration::from_millis(10))
            .expect_err("must be rejected");
        assert!(error.contains("frame cap"), "{error}");
    }

    /// A bound endpoint must be connectable immediately. The UI treats a
    /// successful bind as "ready", and a listener that only becomes reachable
    /// inside `serve` reads as a dead indexer to every early caller.
    #[test]
    fn a_bound_endpoint_answers_before_serve_is_called() {
        let endpoint = format!("nex-test-ready-{}", std::process::id());
        let listener = bind(&endpoint).expect("bind");
        let started = std::thread::spawn(move || {
            listener.serve(|request| format!("echo:{request}"))
        });
        let reply = call(&endpoint, "ping", Duration::from_secs(5)).expect("reply");
        assert_eq!(reply, "echo:ping");
        drop(started);
    }

    /// Serving with no client must cost nothing.
    ///
    /// The first version created a pipe instance *and* a thread parked in
    /// `ConnectNamedPipe` on every loop iteration, so an idle listener grew the
    /// handle table at scheduler speed until the process starved — the whole
    /// test suite stopped making progress while every individual test passed in
    /// its own process. A bounded pool fixes it; this measures the bound.
    #[cfg(target_os = "windows")]
    #[test]
    fn serving_without_clients_does_not_grow_the_handle_table() {
        fn handle_count() -> u32 {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, GetProcessHandleCount,
            };
            let mut count = 0u32;
            // SAFETY: the pseudo-handle is always valid and `count` is writable.
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
            count
        }

        let endpoint = format!("nex-test-idle-{}", std::process::id());
        let listener = bind(&endpoint).expect("bind");
        let before = handle_count();
        let serving = std::thread::spawn(move || listener.serve(|request| request.to_string()));
        std::thread::sleep(Duration::from_millis(400));
        let after = handle_count();
        // The pool is small; a leak of one handle per iteration would be in the
        // thousands by now. The margin covers other tests running in parallel.
        assert!(
            after <= before + 64,
            "idle listener grew the handle table from {before} to {after}"
        );
        drop(serving);
    }

    /// Only one process may own an endpoint at a time, and the loser must find
    /// out from `bind` rather than from a half-working link later.
    #[test]
    fn a_second_claim_on_the_same_endpoint_is_refused() {
        let endpoint = format!("nex-test-owner-{}", std::process::id());
        let _first = bind(&endpoint).expect("first bind");
        assert!(bind(&endpoint).is_none(), "second bind must be refused");
    }
}
