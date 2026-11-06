//! Wire format between `NexIndexerProcess` and `NexHelper` for NTFS MFT/USN
//! snapshots. Std-only and platform independent: the helper pulls this file in
//! with `#[path]`, and the framing is unit-tested on any host.
//!
//! Snapshot byte stream (unchanged since v3, used by both transports):
//!   header:  b"NXMF" | version u16 | letter u8 | mode u8 (0 full, 1 delta) |
//!            journal_id u64 | start_usn i64 | next_usn i64
//!   record:  frn u64 | parent u64 | attributes u32 | timestamp i64 |
//!            reason u32 | name_len u16 | path_len u16 | name | path
//!   trailer: a record prefix with frn=0, no name/path, reason=u32::MAX,
//!            followed by the final next_usn i64
//!
//! Command channel (the indexer creates `\\.\pipe\NexMftCmd\<ui pid>`, the
//! elevated helper connects as the client; a High-IL client may write into a
//! Medium-IL pipe, the reverse is blocked by UIPI):
//!   request (indexer -> helper, 32 bytes): b"NXRQ" | version u16 (1) |
//!            mode u8 | letter u8 | request_id u64 | journal_id u64 | start_usn i64
//!   frame (helper -> indexer): kind u8 | request_id u64 | len u32 | payload
//!     DATA(1)  payload is a slice of the snapshot byte stream
//!     END(2)   no payload; the snapshot stream is complete
//!     ERROR(3) payload: code u8 | utf8 message; the request failed
//! One request is in flight at a time; every frame echoes its request id.

#![allow(dead_code)]

use std::io::{self, Read, Write};

pub const HEADER_MAGIC: &[u8; 4] = b"NXMF";
pub const PROTOCOL_VERSION: u16 = 3;
pub const HEADER_LEN: usize = 32;
pub const RECORD_PREFIX_LEN: usize = 36;
pub const FULL_MODE: u8 = 0;
pub const DELTA_MODE: u8 = 1;

pub const REQUEST_MAGIC: &[u8; 4] = b"NXRQ";
pub const CHANNEL_VERSION: u16 = 1;
pub const REQUEST_LEN: usize = 32;
pub const FRAME_HEADER_LEN: usize = 13;
pub const MAX_FRAME_PAYLOAD: usize = 4 << 20;
pub const DATA_FRAME_TARGET: usize = 256 << 10;

pub const FRAME_DATA: u8 = 1;
pub const FRAME_END: u8 = 2;
pub const FRAME_ERROR: u8 = 3;

pub const ERROR_FAILED: u8 = 0;
pub const ERROR_JOURNAL_INVALID: u8 = 1;

/// Prefix a producer puts on a stream error to flag an unusable journal
/// position; `error_code_for` maps it to `ERROR_JOURNAL_INVALID`.
pub const JOURNAL_INVALID_PREFIX: &str = "journal_invalid: ";

pub fn command_pipe_name(ui_pid: u32) -> String {
    format!(r"\\.\pipe\NexMftCmd\{ui_pid}")
}

/// Split a producer error into its wire code and message.
pub fn error_code_for(message: &str) -> (u8, &str) {
    match message.strip_prefix(JOURNAL_INVALID_PREFIX) {
        Some(rest) => (ERROR_JOURNAL_INVALID, rest),
        None => (ERROR_FAILED, message),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Full,
    Delta { journal_id: u64, start_usn: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandRequest {
    pub id: u64,
    pub letter: char,
    pub kind: RequestKind,
}

impl CommandRequest {
    pub fn encode(&self) -> [u8; REQUEST_LEN] {
        let mut out = [0u8; REQUEST_LEN];
        out[0..4].copy_from_slice(REQUEST_MAGIC);
        out[4..6].copy_from_slice(&CHANNEL_VERSION.to_le_bytes());
        let (mode, journal_id, start_usn) = match self.kind {
            RequestKind::Full => (FULL_MODE, 0, 0),
            RequestKind::Delta {
                journal_id,
                start_usn,
            } => (DELTA_MODE, journal_id, start_usn),
        };
        out[6] = mode;
        out[7] = self.letter as u8;
        out[8..16].copy_from_slice(&self.id.to_le_bytes());
        out[16..24].copy_from_slice(&journal_id.to_le_bytes());
        out[24..32].copy_from_slice(&start_usn.to_le_bytes());
        out
    }

    /// `Ok(None)` is a clean disconnect between requests.
    pub fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>> {
        let mut buf = [0u8; REQUEST_LEN];
        let mut filled = 0;
        while filled < buf.len() {
            match reader.read(&mut buf[filled..]) {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => filled += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        if &buf[0..4] != REQUEST_MAGIC {
            return Err(invalid("request magic mismatch"));
        }
        let version = u16::from_le_bytes([buf[4], buf[5]]);
        if version != CHANNEL_VERSION {
            return Err(invalid(&format!("command channel version {version} unsupported")));
        }
        let letter = buf[7];
        if !letter.is_ascii_alphabetic() {
            return Err(invalid("request letter invalid"));
        }
        let kind = match buf[6] {
            FULL_MODE => RequestKind::Full,
            DELTA_MODE => RequestKind::Delta {
                journal_id: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
                start_usn: i64::from_le_bytes(buf[24..32].try_into().unwrap()),
            },
            other => return Err(invalid(&format!("request mode {other} unknown"))),
        };
        Ok(Some(Self {
            id: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            letter: (letter as char).to_ascii_uppercase(),
            kind,
        }))
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Error frame received from the producer; carried inside an `io::Error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    pub code: u8,
    pub message: String,
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "helper error: {}", self.message)
    }
}

impl std::error::Error for RemoteError {}

/// Writes the snapshot stream as DATA frames; `finish`/`fail` close the request.
pub struct FrameWriter<W: Write> {
    inner: W,
    request_id: u64,
    buf: Vec<u8>,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W, request_id: u64) -> Self {
        Self {
            inner,
            request_id,
            buf: Vec::with_capacity(DATA_FRAME_TARGET),
        }
    }

    fn flush_data(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        write_frame(&mut self.inner, FRAME_DATA, self.request_id, &self.buf)?;
        self.buf.clear();
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        self.flush_data()?;
        write_frame(&mut self.inner, FRAME_END, self.request_id, &[])?;
        self.inner.flush()
    }

    /// Drops unsent data and reports the failure to the reader.
    pub fn fail(&mut self, code: u8, message: &str) -> io::Result<()> {
        self.buf.clear();
        let mut payload = Vec::with_capacity(message.len() + 1);
        payload.push(code);
        let bytes = message.as_bytes();
        payload.extend_from_slice(&bytes[..bytes.len().min(MAX_FRAME_PAYLOAD - 1)]);
        write_frame(&mut self.inner, FRAME_ERROR, self.request_id, &payload)?;
        self.inner.flush()
    }
}

impl<W: Write> Write for FrameWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = DATA_FRAME_TARGET - self.buf.len();
        let take = data.len().min(room);
        self.buf.extend_from_slice(&data[..take]);
        if self.buf.len() >= DATA_FRAME_TARGET {
            self.flush_data()?;
        }
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn write_frame(
    writer: &mut impl Write,
    kind: u8,
    request_id: u64,
    payload: &[u8],
) -> io::Result<()> {
    let mut head = [0u8; FRAME_HEADER_LEN];
    head[0] = kind;
    head[1..9].copy_from_slice(&request_id.to_le_bytes());
    head[9..13].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    writer.write_all(&head)?;
    writer.write_all(payload)
}

/// Presents the DATA frames of one request as a plain byte stream. An END frame
/// is EOF; an ERROR frame, a foreign request id or a connection that ends early
/// is an `io::Error` (never a clean EOF, so a truncated stream cannot be
/// mistaken for a complete one).
pub struct FrameReader<R: Read> {
    inner: R,
    request_id: u64,
    chunk: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R, request_id: u64) -> Self {
        Self {
            inner,
            request_id,
            chunk: Vec::new(),
            pos: 0,
            done: false,
        }
    }

    /// Consume the rest of the request (through END). The snapshot parser stops
    /// at its own trailer, so callers drain to confirm the producer finished.
    pub fn drain(&mut self) -> io::Result<()> {
        let mut scratch = [0u8; 4096];
        while self.read(&mut scratch)? > 0 {}
        Ok(())
    }

    fn next_frame(&mut self) -> io::Result<()> {
        let mut head = [0u8; FRAME_HEADER_LEN];
        read_exact_or_truncated(&mut self.inner, &mut head)?;
        let kind = head[0];
        let id = u64::from_le_bytes(head[1..9].try_into().unwrap());
        let len = u32::from_le_bytes(head[9..13].try_into().unwrap()) as usize;
        if id != self.request_id {
            return Err(invalid(&format!(
                "request id mismatch (expected {}, got {id})",
                self.request_id
            )));
        }
        if len > MAX_FRAME_PAYLOAD {
            return Err(invalid(&format!("frame payload {len} exceeds limit")));
        }
        let mut payload = vec![0u8; len];
        read_exact_or_truncated(&mut self.inner, &mut payload)?;
        match kind {
            FRAME_DATA => {
                self.chunk = payload;
                self.pos = 0;
                Ok(())
            }
            FRAME_END => {
                self.done = true;
                Ok(())
            }
            FRAME_ERROR => {
                let code = payload.first().copied().unwrap_or(ERROR_FAILED);
                let message = String::from_utf8_lossy(payload.get(1..).unwrap_or(&[])).into_owned();
                Err(io::Error::other(RemoteError { code, message }))
            }
            other => Err(invalid(&format!("frame kind {other} unknown"))),
        }
    }
}

impl<R: Read> Read for FrameReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.pos >= self.chunk.len() {
            if self.done {
                return Ok(0);
            }
            self.next_frame()?;
        }
        let take = out.len().min(self.chunk.len() - self.pos);
        out[..take].copy_from_slice(&self.chunk[self.pos..self.pos + take]);
        self.pos += take;
        Ok(take)
    }
}

fn read_exact_or_truncated(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<()> {
    reader.read_exact(buf).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            io::Error::new(io::ErrorKind::ConnectionAborted, "stream truncated before END")
        } else {
            error
        }
    })
}

pub fn encode_header(
    letter: char,
    delta: bool,
    journal_id: u64,
    start_usn: i64,
    next_usn: i64,
) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(HEADER_MAGIC);
    header[4..6].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    header[6] = letter as u8;
    header[7] = if delta { DELTA_MODE } else { FULL_MODE };
    header[8..16].copy_from_slice(&journal_id.to_le_bytes());
    header[16..24].copy_from_slice(&start_usn.to_le_bytes());
    header[24..32].copy_from_slice(&next_usn.to_le_bytes());
    header
}

/// Returns the number of bytes written.
#[allow(clippy::too_many_arguments)]
pub fn write_record(
    writer: &mut impl Write,
    frn: u64,
    parent: u64,
    attributes: u32,
    timestamp: i64,
    reason: u32,
    name: &str,
    path: &str,
) -> io::Result<usize> {
    let name_bytes = name.as_bytes();
    let path_bytes = path.as_bytes();
    let mut prefix = [0u8; RECORD_PREFIX_LEN];
    prefix[0..8].copy_from_slice(&frn.to_le_bytes());
    prefix[8..16].copy_from_slice(&parent.to_le_bytes());
    prefix[16..20].copy_from_slice(&attributes.to_le_bytes());
    prefix[20..28].copy_from_slice(&timestamp.to_le_bytes());
    prefix[28..32].copy_from_slice(&reason.to_le_bytes());
    prefix[32..34].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    prefix[34..36].copy_from_slice(&(path_bytes.len() as u16).to_le_bytes());
    writer.write_all(&prefix)?;
    writer.write_all(name_bytes)?;
    writer.write_all(path_bytes)?;
    Ok(RECORD_PREFIX_LEN + name_bytes.len() + path_bytes.len())
}

pub fn write_trailer(writer: &mut impl Write, next_usn: i64) -> io::Result<usize> {
    let mut prefix = [0u8; RECORD_PREFIX_LEN];
    prefix[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    writer.write_all(&prefix)?;
    writer.write_all(&next_usn.to_le_bytes())?;
    Ok(RECORD_PREFIX_LEN + 8)
}

#[derive(Debug, Clone)]
pub struct MftRecord {
    pub frn: u64,
    pub parent: u64,
    pub attributes: u32,
    pub reason: u32,
    pub name: String,
    /// Helper-resolved full path.
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub delta: bool,
    pub journal_id: u64,
    pub start_usn: i64,
    pub next_usn: i64,
    pub records: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub journal_invalid: bool,
    /// The producer reported the failure in an ERROR frame (stream stays aligned).
    pub remote: bool,
    pub message: String,
}

impl ReadError {
    pub fn plain(message: String) -> Self {
        Self {
            journal_invalid: false,
            remote: false,
            message,
        }
    }

    pub fn from_io(context: &str, error: io::Error) -> Self {
        if let Some(remote) = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<RemoteError>())
        {
            return Self {
                journal_invalid: remote.code == ERROR_JOURNAL_INVALID,
                remote: true,
                message: remote.to_string(),
            };
        }
        Self::plain(format!("{context}: {error}"))
    }
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Parse one snapshot stream, handing each record to `on_record`.
pub fn read_snapshot<R, F>(reader: &mut R, mut on_record: F) -> Result<SnapshotMeta, ReadError>
where
    R: Read,
    F: FnMut(&MftRecord) -> Result<(), String>,
{
    let mut header = [0u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|e| ReadError::from_io("read header", e))?;
    if &header[0..4] != HEADER_MAGIC {
        return Err(ReadError::plain("snapshot header mismatch".to_string()));
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ReadError::plain(format!("snapshot version {version} unsupported")));
    }
    let delta = header[7] == DELTA_MODE;
    let journal_id = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let start_usn = i64::from_le_bytes(header[16..24].try_into().unwrap());
    let mut next_usn = i64::from_le_bytes(header[24..32].try_into().unwrap());

    let mut record_count = 0usize;
    let mut prefix = [0u8; RECORD_PREFIX_LEN];
    loop {
        match reader.read_exact(&mut prefix) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(ReadError::from_io("read record", error)),
        }
        let frn = u64::from_le_bytes(prefix[0..8].try_into().unwrap());
        let parent = u64::from_le_bytes(prefix[8..16].try_into().unwrap());
        let attributes = u32::from_le_bytes(prefix[16..20].try_into().unwrap());
        let reason = u32::from_le_bytes(prefix[28..32].try_into().unwrap());
        let name_len = u16::from_le_bytes([prefix[32], prefix[33]]) as usize;
        let path_len = u16::from_le_bytes([prefix[34], prefix[35]]) as usize;
        if frn == 0 && name_len == 0 && path_len == 0 && reason == u32::MAX {
            let mut tail = [0u8; 8];
            reader
                .read_exact(&mut tail)
                .map_err(|e| ReadError::from_io("read trailer", e))?;
            next_usn = i64::from_le_bytes(tail);
            break;
        }
        let mut name_bytes = vec![0u8; name_len];
        reader
            .read_exact(&mut name_bytes)
            .map_err(|e| ReadError::from_io("read name", e))?;
        let mut path_bytes = vec![0u8; path_len];
        reader
            .read_exact(&mut path_bytes)
            .map_err(|e| ReadError::from_io("read path", e))?;
        let name = String::from_utf8_lossy(&name_bytes).into_owned();
        if name.is_empty() {
            continue;
        }
        let record = MftRecord {
            frn,
            parent,
            attributes,
            reason,
            name,
            path: (path_len > 0).then(|| String::from_utf8_lossy(&path_bytes).into_owned()),
        };
        record_count += 1;
        on_record(&record).map_err(ReadError::plain)?;
    }

    Ok(SnapshotMeta {
        delta,
        journal_id,
        start_usn,
        next_usn,
        records: record_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn snapshot_bytes(records: usize, delta: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&encode_header('C', delta, 7, 100, 900));
        for i in 0..records {
            write_record(
                &mut out,
                10 + i as u64,
                5,
                0x10,
                0,
                0,
                &format!("name{i}"),
                &format!(r"C:\dir\name{i}"),
            )
            .unwrap();
        }
        write_trailer(&mut out, 1234).unwrap();
        out
    }

    fn framed(request_id: u64, payload: &[u8]) -> Vec<u8> {
        let mut wire = Vec::new();
        let mut writer = FrameWriter::new(&mut wire, request_id);
        writer.write_all(payload).unwrap();
        writer.finish().unwrap();
        wire
    }

    fn parse(wire: Vec<u8>, request_id: u64) -> Result<(SnapshotMeta, Vec<MftRecord>), ReadError> {
        let mut reader = FrameReader::new(Cursor::new(wire), request_id);
        let mut records = Vec::new();
        let meta = read_snapshot(&mut reader, |record| {
            records.push(record.clone());
            Ok(())
        })?;
        reader.drain().map_err(|e| ReadError::from_io("drain", e))?;
        Ok((meta, records))
    }

    #[test]
    fn request_round_trips() {
        for kind in [
            RequestKind::Full,
            RequestKind::Delta {
                journal_id: u64::MAX - 3,
                start_usn: -42,
            },
        ] {
            let request = CommandRequest {
                id: 99,
                letter: 'D',
                kind,
            };
            let decoded = CommandRequest::read_from(&mut Cursor::new(request.encode().to_vec()))
                .unwrap()
                .unwrap();
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn request_reader_distinguishes_disconnect_from_truncation() {
        assert!(CommandRequest::read_from(&mut Cursor::new(Vec::new())).unwrap().is_none());
        let encoded = CommandRequest {
            id: 1,
            letter: 'C',
            kind: RequestKind::Full,
        }
        .encode();
        let error = CommandRequest::read_from(&mut Cursor::new(encoded[..10].to_vec())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        let mut bad = encoded;
        bad[0] = b'X';
        assert!(CommandRequest::read_from(&mut Cursor::new(bad.to_vec())).is_err());
    }

    #[test]
    fn framed_snapshot_round_trips_across_many_frames() {
        let payload = snapshot_bytes(20_000, true);
        assert!(payload.len() > DATA_FRAME_TARGET * 2, "needs several DATA frames");
        let (meta, records) = parse(framed(5, &payload), 5).unwrap();
        assert!(meta.delta);
        assert_eq!((meta.journal_id, meta.start_usn, meta.next_usn), (7, 100, 1234));
        assert_eq!(records.len(), 20_000);
        assert_eq!(records[19_999].name, "name19999");
        assert_eq!(records[0].path.as_deref(), Some(r"C:\dir\name0"));
    }

    #[test]
    fn foreign_request_id_is_rejected() {
        let error = parse(framed(5, &snapshot_bytes(3, false)), 6).unwrap_err();
        assert!(error.message.contains("request id mismatch"), "{error}");
        assert!(!error.journal_invalid);
    }

    #[test]
    fn error_frame_surfaces_code_and_message() {
        let mut wire = Vec::new();
        FrameWriter::new(&mut wire, 8)
            .fail(ERROR_JOURNAL_INVALID, "journal moved")
            .unwrap();
        let error = parse(wire, 8).unwrap_err();
        assert!(error.journal_invalid);
        assert!(error.message.contains("journal moved"), "{error}");

        let mut wire = Vec::new();
        FrameWriter::new(&mut wire, 8).fail(ERROR_FAILED, "open volume failed").unwrap();
        let error = parse(wire, 8).unwrap_err();
        assert!(!error.journal_invalid);
    }

    #[test]
    fn error_after_partial_data_discards_the_buffered_tail() {
        let mut wire = Vec::new();
        let mut writer = FrameWriter::new(&mut wire, 2);
        writer.write_all(&snapshot_bytes(5, false)[..50]).unwrap();
        writer.fail(ERROR_FAILED, "volume vanished").unwrap();
        let error = parse(wire, 2).unwrap_err();
        assert!(error.message.contains("volume vanished"), "{error}");
    }

    #[test]
    fn truncated_stream_is_an_error_not_a_clean_end() {
        let wire = framed(3, &snapshot_bytes(50, false));
        // Cut inside the last frame's payload and exactly before END.
        let end_frame = FRAME_HEADER_LEN;
        for cut in [wire.len() / 2, wire.len() - end_frame, wire.len() - 1] {
            let error = parse(wire[..cut].to_vec(), 3).unwrap_err();
            assert!(!error.journal_invalid);
            assert!(
                error.message.contains("truncated") || error.message.contains("read"),
                "{error}"
            );
        }
    }

    #[test]
    fn data_frame_without_trailer_still_requires_end() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&encode_header('C', false, 1, 0, 2));
        let mut wire = Vec::new();
        write_frame(&mut wire, FRAME_DATA, 4, &payload).unwrap();
        let mut reader = FrameReader::new(Cursor::new(wire), 4);
        let error = read_snapshot(&mut reader, |_| Ok(())).unwrap_err();
        assert!(error.message.contains("truncated"), "{error}");
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocation() {
        let mut wire = Vec::new();
        let mut head = [0u8; FRAME_HEADER_LEN];
        head[0] = FRAME_DATA;
        head[1..9].copy_from_slice(&1u64.to_le_bytes());
        head[9..13].copy_from_slice(&(u32::MAX).to_le_bytes());
        wire.extend_from_slice(&head);
        let error = parse(wire, 1).unwrap_err();
        assert!(error.message.contains("exceeds limit"), "{error}");
    }

    #[test]
    fn producer_error_codes_map_from_message_prefix() {
        let message = format!("{JOURNAL_INVALID_PREFIX}position rolled");
        assert_eq!(error_code_for(&message), (ERROR_JOURNAL_INVALID, "position rolled"));
        assert_eq!(error_code_for("boom"), (ERROR_FAILED, "boom"));
    }

    #[test]
    fn command_pipe_is_scoped_by_ui_pid() {
        assert_eq!(command_pipe_name(1234), r"\\.\pipe\NexMftCmd\1234");
    }
}
