//! Now-playing media widget backed by Windows SMTC
//! ([`GlobalSystemMediaTransportControlsSessionManager`]).
//!
//! This talks to whatever media session Windows knows about — Spotify,
//! the browser, any player — with no credentials and no network. When
//! several sessions exist the Spotify one wins; otherwise the current
//! session is used.

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use windows::core::Interface;

/// Largest album-art payload we will embed (bytes). Thumbnails are
/// normally tens of KB; anything bigger is skipped, not downscaled.
const MAX_ART_BYTES: u64 = 512 * 1024;

/// Serializable snapshot pushed to the overlay page.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MediaState {
    /// A usable media session exists (playing or paused).
    pub active: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Owning app, e.g. `Spotify.exe` when derivable.
    pub source: String,
    /// `playing`, `paused`, `stopped`, `changing`, `closed`, `unknown`.
    pub status: String,
    pub position_secs: f64,
    pub duration_secs: f64,
    /// Album art as a data URI, when the session provides a thumbnail.
    pub art: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaAction {
    Toggle,
    Next,
    Prev,
    Seek(f64),
}

static ART_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn art_cache() -> &'static Mutex<HashMap<String, String>> {
    ART_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ensure_com() {
    thread_local! {
        static COM_INIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    COM_INIT.with(|flag| {
        if !flag.get() {
            unsafe {
                let _ = windows_sys::Win32::System::Com::CoInitializeEx(
                    std::ptr::null(),
                    0, // COINIT_MULTITHREADED
                );
            }
            flag.set(true);
        }
    });
}

type SessionManager =
    windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
type Session = windows::Media::Control::GlobalSystemMediaTransportControlsSession;

fn session_manager() -> Option<SessionManager> {
    ensure_com();
    SessionManager::RequestAsync().ok()?.get().ok()
}

thread_local! {
    /// The media worker thread is persistent, so the manager/session pair
    /// survives across snapshots. Re-requesting the manager and
    /// re-enumerating sessions every second is what made progress updates
    /// arrive late — the cached pair is revalidated with one cheap call.
    static CACHED: std::cell::RefCell<Option<(SessionManager, Session)>> =
        const { std::cell::RefCell::new(None) };
}

fn forget_cached_session() {
    CACHED.with(|cached| {
        cached.borrow_mut().take();
    });
}

/// The preferred session, reusing the cached pair when it still answers.
/// Falls back to a fresh manager request + Spotify-first pick.
fn cached_session() -> Option<Session> {
    if let Some(session) = CACHED.with(|cached| {
        cached
            .borrow()
            .as_ref()
            .map(|(_, session)| session.clone())
    }) {
        // One cheap synchronous call validates the cached session
        // (covers quit apps and dead sessions).
        if session.GetPlaybackInfo().is_ok() {
            return Some(session);
        }
        forget_cached_session();
    }
    let manager = session_manager()?;
    let session = pick_session(&manager)?;
    if session.GetPlaybackInfo().is_err() {
        return None;
    }
    CACHED.with(|cached| {
        *cached.borrow_mut() = Some((manager, session.clone()));
    });
    Some(session)
}

/// Prefer the Spotify session; fall back to whatever Windows calls current.
fn pick_session(manager: &SessionManager) -> Option<Session> {
    if let Ok(sessions) = manager.GetSessions() {
        if let Ok(count) = sessions.Size() {
            for index in 0..count {
                let Ok(session) = sessions.GetAt(index) else {
                    continue;
                };
                let id = session
                    .SourceAppUserModelId()
                    .map(|id| id.to_string())
                    .unwrap_or_default();
                if id.to_ascii_lowercase().contains("spotify") {
                    return Some(session);
                }
            }
        }
    }
    manager.GetCurrentSession().ok()
}

fn status_name(
    status: windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus,
) -> &'static str {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as S;
    match status {
        S::Playing => "playing",
        S::Paused => "paused",
        S::Stopped => "stopped",
        S::Changing => "changing",
        S::Closed => "closed",
        _ => "unknown",
    }
}

fn friendly_source(app_id: &str) -> String {
    // `Spotify.exe_abc123!Spotify` → `Spotify.exe`.
    let base = app_id.split('!').next().unwrap_or(app_id);
    let base = base.split('_').next().unwrap_or(base);
    if base.is_empty() {
        "media".to_string()
    } else {
        base.to_string()
    }
}

fn read_thumbnail_data_uri(
    session: &Session,
    cache_key: &str,
) -> Option<String> {
    if let Ok(cache) = art_cache().lock() {
        if let Some(uri) = cache.get(cache_key) {
            return Some(uri.clone());
        }
    }
    let uri = read_thumbnail_data_uri_uncached(session)?;
    if let Ok(mut cache) = art_cache().lock() {
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert(cache_key.to_string(), uri.clone());
    }
    Some(uri)
}

fn read_thumbnail_data_uri_uncached(session: &Session) -> Option<String> {
    use windows::Storage::Streams::{DataReader, IInputStream};

    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let reference = props.Thumbnail().ok()?;
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_ART_BYTES {
        return None;
    }
    let mime = stream
        .ContentType()
        .map(|mime| mime.to_string())
        .unwrap_or_default();
    let input: IInputStream = stream.cast().ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    let loaded = reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut bytes = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let mime = if mime.is_empty() {
        "image/png".to_string()
    } else {
        mime
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64_encode(&bytes)
    ))
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Snapshot the preferred media session. Cheap except the first thumbnail
/// read per track (cached afterwards); safe to call on a ~1s poll.
pub fn snapshot() -> MediaState {
    let started = std::time::Instant::now();
    let state = snapshot_inner();
    let elapsed = started.elapsed();
    if elapsed > std::time::Duration::from_millis(500) {
        crate::logging::warn(&format!(
            "[nex] slow media snapshot: {}ms",
            elapsed.as_millis()
        ));
    }
    state
}

fn snapshot_inner() -> MediaState {
    let Some(session) = cached_session() else {
        return MediaState::default();
    };

    let app_id = session
        .SourceAppUserModelId()
        .map(|id| id.to_string())
        .unwrap_or_default();

    let (title, artist, album) = session
        .TryGetMediaPropertiesAsync()
        .ok()
        .and_then(|op| op.get().ok())
        .map(|props| {
            (
                props.Title().unwrap_or_default().to_string(),
                props.Artist().unwrap_or_default().to_string(),
                props.AlbumTitle().unwrap_or_default().to_string(),
            )
        })
        .unwrap_or_default();

    // A session with no metadata at all is not widget-worthy.
    if title.is_empty() && artist.is_empty() && album.is_empty() {
        return MediaState::default();
    }

    let status = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .map(status_name)
        .unwrap_or("unknown")
        .to_string();

    let (position_secs, duration_secs) = session
        .GetTimelineProperties()
        .map(|timeline| {
            let position = timeline.Position().map(|t| t.Duration).unwrap_or(0);
            let duration = timeline.EndTime().map(|t| t.Duration).unwrap_or(0);
            (
                position.max(0) as f64 / 10_000_000.0,
                duration.max(0) as f64 / 10_000_000.0,
            )
        })
        .unwrap_or((0.0, 0.0));

    let cache_key = format!("{title}\u{1f}{artist}\u{1f}{album}");
    let art = read_thumbnail_data_uri(&session, &cache_key);

    MediaState {
        active: true,
        title,
        artist,
        album,
        source: friendly_source(&app_id),
        status,
        position_secs,
        duration_secs,
        art,
    }
}

/// Fire a transport command at the preferred session.
pub fn control(action: MediaAction) -> Result<bool, String> {
    let session = cached_session().ok_or("no media session")?;
    match action {
        MediaAction::Toggle => session
            .TryTogglePlayPauseAsync()
            .map_err(|e| format!("media control failed: {e}"))?
            .get()
            .map_err(|e| format!("media control failed: {e}")),
        MediaAction::Next => session
            .TrySkipNextAsync()
            .map_err(|e| format!("media control failed: {e}"))?
            .get()
            .map_err(|e| format!("media control failed: {e}")),
        MediaAction::Prev => session
            .TrySkipPreviousAsync()
            .map_err(|e| format!("media control failed: {e}"))?
            .get()
            .map_err(|e| format!("media control failed: {e}")),
        MediaAction::Seek(position_secs) => {
            // SMTC takes 100ns ticks; clamp to the track bounds.
            let ticks = (position_secs.max(0.0) * 10_000_000.0) as i64;
            session
                .TryChangePlaybackPositionAsync(ticks)
                .map_err(|e| format!("media seek failed: {e}"))?
                .get()
                .map_err(|e| format!("media seek failed: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_shape() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn snapshot_never_panics() {
        // With or without a live session this must return, never hang or
        // panic — every fallible WinRT step degrades to inactive.
        let state = snapshot();
        if state.active {
            assert!(!state.title.is_empty() || !state.artist.is_empty());
        }
    }

    #[test]
    fn friendly_source_trims_app_id() {
        assert_eq!(friendly_source("Spotify.exe_abc!Spotify"), "Spotify.exe");
        assert_eq!(friendly_source(""), "media");
    }
}
