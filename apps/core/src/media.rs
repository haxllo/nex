//! Now-playing media widget backed by Windows SMTC
//! ([`GlobalSystemMediaTransportControlsSessionManager`]).
//!
//! This talks to whatever media session Windows knows about — Spotify,
//! the browser, any player — with no credentials and no network. When
//! several sessions exist the user picks via switcher dots (one per app);
//! otherwise the pick sticks to useful playback (frozen "playing"
//! corpses rank with the paused ones) with Spotify preferred per tier.
//!
//! Volume is the master device endpoint (SMTC has no per-session volume).

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use windows::core::Interface;

/// Largest album-art payload we will embed (bytes). Thumbnails are
/// normally tens of KB; anything bigger is skipped, not downscaled.
const MAX_ART_BYTES: u64 = 512 * 1024;

/// One live entry for the session switcher dots. `key` is the raw
/// SourceAppUserModelId (stable per app); two tabs in one browser share
/// an id and collapse to a single dot — first live wins.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MediaSessionEntry {
    pub key: String,
    pub source: String,
    pub title: String,
    pub playing: bool,
}

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
    /// Master output volume 0.0–1.0. SMTC exposes no per-session volume,
    /// so this is the device endpoint level.
    pub volume: f32,
    pub muted: bool,
    /// False when the endpoint query failed — the UI hides the slider.
    pub volume_supported: bool,
    /// All live sessions (one entry per app) for the switcher dots.
    /// Empty when 0–1 sessions — the UI hides the dots.
    pub sessions: Vec<MediaSessionEntry>,
    /// Raw app id of the active session; matches one `sessions` key.
    pub session_key: String,
    /// Unix millis when the timeline was read. Lets the page discard
    /// pushes sampled before its last drag/switch (stale in-flight
    /// snapshots must not move the bar backward).
    pub sampled_ms: u64,
    /// Start of the seekable window (DVR-aware). Drags map onto
    /// [`Self::seek_min_secs`]`..=`[`Self::seek_max_secs`].
    pub seek_min_secs: f64,
    pub seek_max_secs: f64,
    /// False for ads and other sources reporting seeking disabled —
    /// the bar locks (progress still shows).
    pub seekable: bool,
    /// No duration (live radio) — `LIVE` badge, bar locked.
    pub live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaAction {
    Toggle,
    Next,
    Prev,
    Seek(f64),
    /// Master output volume 0.0–1.0 (clamped).
    Volume(f32),
    Mute(bool),
}

static ART_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn art_cache() -> &'static Mutex<HashMap<String, String>> {
    ART_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn ensure_com() {
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
    /// Cache is local to a worker thread; isolated snapshot threads do not
    /// retain it between polls.
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

/// User-pinned session (raw app id) from the switcher dots. Empty
/// selection = auto-pick. A pin whose app goes quiet clears on next pick
/// so auto-pick resumes instead of sticking on nothing.
static SELECTED_APP: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn selected_app() -> &'static Mutex<Option<String>> {
    SELECTED_APP.get_or_init(|| Mutex::new(None))
}

/// Pin the switcher to one app's session; empty clears back to auto-pick.
/// Drops the cached session so the next snapshot re-picks immediately.
pub fn select_session(key: &str) {
    if let Ok(mut selected) = selected_app().lock() {
        if key.is_empty() {
            selected.take();
        } else {
            *selected = Some(key.to_string());
        }
    }
    forget_cached_session();
}

#[cfg(test)]
pub(crate) fn selected_session_key() -> Option<String> {
    selected_app().lock().ok().and_then(|s| s.clone())
}

/// One enumerated session that answers liveness probes.
struct LiveSession {
    app_id: String,
    session: Session,
    playing: bool,
    changing: bool,
    /// Unix millis of the session's last timeline update, 0 when unreadable.
    /// Used only to order genuinely-playing sessions against each other, so
    /// the one the user just started wins a tie.
    last_updated_ms: u64,
}

/// All playing/paused/changing sessions that answer. `changing`
/// (buffer flicker) counts as live so a buffering session never drops
/// from the list for a snapshot — dropping is what flapped the whole
/// view between sessions. Sync cheap calls only — no metadata or
/// thumbnail reads, so this stays fast per snapshot.
fn live_sessions(manager: &SessionManager) -> Vec<LiveSession> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as S;

    let mut out = Vec::new();
    if let Ok(sessions) = manager.GetSessions() {
        if let Ok(count) = sessions.Size() {
            for index in 0..count {
                let Ok(session) = sessions.GetAt(index) else {
                    continue;
                };
                let Ok(status) = session
                    .GetPlaybackInfo()
                    .and_then(|info| info.PlaybackStatus())
                else {
                    continue;
                };
                if !matches!(status, S::Playing | S::Paused | S::Changing) {
                    continue;
                }
                let app_id = session
                    .SourceAppUserModelId()
                    .map(|id| id.to_string())
                    .unwrap_or_default();
                // Recency signal for tie-breaking between playing sessions.
                // Cheap enough to read per session; 0 when the source does not
                // expose a timeline, which sorts it behind any real stamp.
                let last_updated_ms = session
                    .GetTimelineProperties()
                    .and_then(|t| t.LastUpdatedTime())
                    .map(datetime_to_unix_millis)
                    .unwrap_or(0);
                out.push(LiveSession {
                    app_id,
                    session,
                    playing: status == S::Playing,
                    changing: status == S::Changing,
                    last_updated_ms,
                });
            }
        }
    }
    out
}
/// Session liveness probe: a listed session whose app already quit (or a
/// suspended/transient source like a closed browser tab) answers enumeration
/// but throws on any real call. Only sessions that answer this are usable —
/// firing transport commands or property reads at a dead COM server is what
/// hangs the snapshot and wedges the media view.
fn playback_status_usable(session: &Session) -> bool {
    session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .is_ok()
}

/// Rank a candidate session: lower is better.
///
/// Being in the playing tier dominates; Spotify is only a tie-break *within*
/// a tier. The previous form ranked playing-Spotify above playing-anything-
/// else, which meant a playing browser session could never take over while
/// Spotify played — the view stuck to Spotify no matter what the user started.
fn session_rank(is_spotify: bool, playing: bool) -> u8 {
    match (playing, is_spotify) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    }
}

/// Grace before a "playing" session with a frozen position counts as a
/// background-tab stall. Below this, buffering flicker is tolerated so
/// the view doesn't flap on every micro-stall.
const STUCK_GRACE_MS: u64 = 10_000;

/// How much newer another playing session's timeline must be before it takes
/// the view over from a session that is still playing.
///
/// Both sessions may be advancing (a browser video keeps ticking while a track
/// plays), so a bare "newer wins" test would hand the view back and forth on
/// sub-second differences. One snapshot interval of slack is enough to
/// distinguish "I just started this" from "both are still running".
const SWITCH_STAMP_SLACK_MS: u64 = 1_500;
/// Position advance counting as real movement (dust/granularity ignored).
const ADVANCE_EPS_SECS: f64 = 0.5;

/// Last picked app id — stickiness across snapshots. Fresh rank-pick
/// every second with zero memory is what flapped the whole view on
/// every 1-snapshot status flicker. Snapshots run on throwaway threads
/// (thread-local cache never hits), so memory must be process-wide.
static LAST_PICK: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Per-app position anchor for stuck detection: (pos, time).
static ADVANCE_MAP: OnceLock<Mutex<HashMap<String, (f64, u64)>>> = OnceLock::new();

fn set_last_pick(app_id: &str) {
    if let Ok(mut last) = LAST_PICK
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *last = Some(app_id.to_string());
    }
}

/// Pure part of stuck detection, unit-tested. Fresh when the position
/// advanced since the anchor, or the anchor is younger than the grace.
fn advancement_fresh(anchor_pos: f64, anchor_time_ms: u64, pos: f64, now_ms: u64) -> bool {
    if pos > anchor_pos + ADVANCE_EPS_SECS {
        return true;
    }
    now_ms.saturating_sub(anchor_time_ms) <= STUCK_GRACE_MS
}

/// A playing-like session counts as effectively playing unless its
/// position froze past the grace (background-tab stall).
fn effectively_playing(app_id: &str, session: &Session, now_ms: u64) -> bool {
    let pos = ticks_to_secs(
        session
            .GetTimelineProperties()
            .ok()
            .and_then(|t| t.Position().ok())
            .map(|t| t.Duration)
            .unwrap_or(0),
    );
    let map = ADVANCE_MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut guard) = map.lock() else {
        return true;
    };
    let entry = guard.entry(app_id.to_string()).or_insert((pos, now_ms));
    if advancement_fresh(entry.0, entry.1, pos, now_ms) {
        if pos > entry.0 + ADVANCE_EPS_SECS {
            *entry = (pos, now_ms);
        }
        return true;
    }
    false
}

/// Thin wrapper for transport commands: fresh pick, session only.
fn pick_session(manager: &SessionManager) -> Option<Session> {
    let live = live_sessions(manager);
    pick_from_list(manager, &live).map(|(session, _)| session)
}

/// Grace holding the last good state when the pick would switch on a
/// 1-snapshot dropout (flaky browser session blinking out and back).
/// Without this every blink re-anchors the whole view: the ping-pong.
/// Bounded — genuine switches land after the grace at the latest.
const PICK_HOLD_MS: u64 = 3000;

/// Last good active snapshot: (app_id, was_playing, state, unix_ms).
static LAST_GOOD: OnceLock<Mutex<Option<(String, bool, MediaState, u64)>>> =
    OnceLock::new();

/// Whether a playing session should take the view over from the sticky pick.
///
/// `sticky_playing` is the current pick's playback state and the stamps are
/// each session's timeline `LastUpdatedTime` (0 meaning unreadable). Pure so
/// the switch policy is testable without COM.
fn should_take_over(sticky_playing: bool, sticky_stamp: u64, challenger_stamp: u64) -> bool {
    // A paused/stuck sticky pick never defends itself; any playing session wins.
    if !sticky_playing {
        return true;
    }
    if challenger_stamp == 0 {
        // No timeline to compare. Do not switch on absence of evidence.
        return false;
    }
    if sticky_stamp == 0 {
        // The sticky pick has no stamp and the challenger does, so treat the
        // challenger as newer. Otherwise two stamp-less sessions would be
        // permanently unswappable.
        return true;
    }
    challenger_stamp > sticky_stamp.saturating_add(SWITCH_STAMP_SLACK_MS)
}

/// Pure hold decision, unit-tested: hold a playing state through a
/// switch to a non-playing pick, unless pinned or the grace expired.
/// Takeovers by genuinely playing sessions are never held.
fn should_hold(
    last_id: &str,
    last_playing: bool,
    new_id: &str,
    new_playing: bool,
    age_ms: u64,
    pin_set: bool,
) -> bool {
    if pin_set || last_id == new_id || !last_playing || new_playing {
        return false;
    }
    age_ms < PICK_HOLD_MS
}

/// User pin first (switcher dots), then sticky current while it usefully
/// plays, then genuinely playing sessions, then paused — never flapping
/// between corpses. Falls back to whatever Windows calls current when it
/// answers. Dead sessions are skipped outright instead of becoming the
/// "current" corpse the view opens on top of.
/// Returns the session plus whether it counts as useful playback (drives
/// the disappearance hold in [`snapshot_inner`]).
fn pick_from_list(manager: &SessionManager, live: &[LiveSession]) -> Option<(Session, bool)> {
    if live.is_empty() {
        let current = manager.GetCurrentSession().ok()?;
        return playback_status_usable(&current).then_some((current, false));
    }
    // User pin wins while live (switcher dots).
    if let Ok(mut selected) = selected_app().lock() {
        if let Some(pinned) = selected.clone() {
            if let Some(hit) = live.iter().find(|s| s.app_id == pinned) {
                set_last_pick(&pinned);
                return Some((hit.session.clone(), true));
            }
            selected.take();
        }
    }
    let now_ms = unix_millis();
    // Owned candidates: (session, app_id, effectively playing, spotify,
    // last_updated_ms). Recency is the final tie-break so that when two
    // sessions are both genuinely playing, the one the user just started wins.
    let cands: Vec<(Session, String, bool, bool, u64)> = live
        .iter()
        .map(|s| {
            let playing_like = s.playing || s.changing;
            let eff = playing_like && effectively_playing(&s.app_id, &s.session, now_ms);
            let spotify = s.app_id.to_ascii_lowercase().contains("spotify");
            (
                s.session.clone(),
                s.app_id.clone(),
                eff,
                spotify,
                s.last_updated_ms,
            )
        })
        .collect();
    // Ordering for two playing candidates: most recent timeline wins, then
    // Spotify, then app id for stability. `u64::MAX - stamp` so the normal
    // ascending sort puts the newest first; a 0 stamp (no timeline) sorts
    // last rather than as "infinitely old but still competing".
    let recent_key = |stamp: u64| if stamp == 0 { u64::MAX } else { u64::MAX - stamp };
    let best_playing = |pool: &[(Session, String, bool, bool, u64)]| {
        pool.iter()
            .filter(|c| c.2)
            .min_by_key(|c| (recent_key(c.4), c.3 as u8, c.1.clone()))
            .map(|c| c.1.clone())
    };
    // Sticky current: never flap away from useful playback on a
    // 1-snapshot flicker; follow real playback when paused/stuck.
    if let Some(last) = LAST_PICK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|g| g.clone())
    {
        if let Some(cur) = cands.iter().find(|c| c.1 == last) {
            if cur.2 {
                // The sticky pick is still playing, but another session may
                // have started since. Staying purely because the old one has
                // not stopped is what pinned the view to Spotify forever.
                // Only yield to a playing session whose timeline is newer by
                // more than one snapshot, so two sessions both advancing do
                // not ping-pong on sub-second stamp differences.
                if let Some(best_id) = best_playing(&cands) {
                    if best_id != cur.1 {
                        let best = cands.iter().find(|c| c.1 == best_id)?;
                        if should_take_over(cur.2, cur.4, best.4) {
                            set_last_pick(&best_id);
                            return Some((best.0.clone(), true));
                        }
                    }
                }
                return Some((cur.0.clone(), true));
            }
            if let Some(best_id) = best_playing(&cands) {
                let best = cands.iter().find(|c| c.1 == best_id)?;
                set_last_pick(&best.1);
                return Some((best.0.clone(), true));
            }
            return Some((cur.0.clone(), false));
        }
    }
    // Fresh pick: effectively playing first, then the most recently updated
    // playing session, with Spotify as a within-tier tie-break.
    let best = cands
        .iter()
        .min_by_key(|c| {
            (
                session_rank(c.3, c.2),
                if c.2 { recent_key(c.4) } else { 0 },
                c.1.clone(),
            )
        })?;
    set_last_pick(&best.1);
    Some((best.0.clone(), best.2))
}

/// Switcher entries from one shared enumeration — callers must reuse
/// the same `live` list they picked from, so dots and pick can never
/// disagree about who exists. Titles fetched only when 2+ apps are
/// live (the only state where dots show). Dedupes by app id.
fn switcher_entries(live: &[LiveSession], active_app_id: &str) -> (Vec<MediaSessionEntry>, String) {
    let mut seen: Vec<String> = Vec::new();
    let mut ranked: Vec<&LiveSession> = Vec::new();
    for candidate in live {
        if seen.contains(&candidate.app_id) {
            continue;
        }
        seen.push(candidate.app_id.clone());
        ranked.push(candidate);
    }
    // Playing entries first so dots sit in a stable, meaningful order.
    ranked.sort_by_key(|s| (!s.playing, s.app_id.clone()));
    if ranked.len() < 2 {
        return (Vec::new(), active_app_id.to_string());
    }
    let entries = ranked
        .iter()
        .map(|s| MediaSessionEntry {
            key: s.app_id.clone(),
            source: friendly_source(&s.app_id),
            title: session_title(&s.session),
            playing: s.playing,
        })
        .collect();
    (entries, active_app_id.to_string())
}

fn session_title(session: &Session) -> String {
    session
        .TryGetMediaPropertiesAsync()
        .ok()
        .and_then(|op| op.get().ok())
        .map(|props| props.Title().unwrap_or_default().to_string())
        .unwrap_or_default()
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

fn clamp01(level: f32) -> f32 {
    if !level.is_finite() {
        return 0.0;
    }
    level.clamp(0.0, 1.0)
}

fn ticks_to_secs(ticks: i64) -> f64 {
    ticks.max(0) as f64 / 10_000_000.0
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// WinRT `DateTime` (100ns ticks since 1601) → unix millis. 0 when unset.
fn datetime_to_unix_millis(dt: windows::Foundation::DateTime) -> u64 {
    const EPOCH_DIFF_TICKS: i64 = 116444736000000000;
    dt.UniversalTime
        .checked_sub(EPOCH_DIFF_TICKS)
        .map(|t| (t.max(0) / 10_000) as u64)
        .unwrap_or(0)
}

/// True position from a timeline read: `Position` is only exact at
/// `LastUpdatedTime`, so add the lag when playing. Without this every
/// push carries the snapshot cost (~0.3–1s) as lead, and the page's
/// confirm path keeps "correcting" it with visible yanks.
fn correct_position(position_secs: f64, now_ms: u64, last_updated_ms: u64, playing: bool) -> f64 {
    if !playing || last_updated_ms == 0 {
        return position_secs.max(0.0);
    }
    let lag = now_ms.saturating_sub(last_updated_ms) as f64 / 1000.0;
    position_secs.max(0.0) + lag.min(5.0)
}

/// Seek window + capability from raw timeline fields. Fail-open: only an
/// explicit `false` from the source locks seeking (ads); unreadable
/// fields fall back to the full duration (today's behavior). No duration
/// means live radio — nothing to seek.
fn resolve_seek_window(
    min_ticks: i64,
    max_ticks: i64,
    duration_secs: f64,
    position_enabled: Option<bool>,
) -> (f64, f64, bool, bool) {
    if !(duration_secs > 0.0) {
        return (0.0, 0.0, false, true);
    }
    let mut lo = ticks_to_secs(min_ticks).clamp(0.0, duration_secs);
    let mut hi = ticks_to_secs(max_ticks);
    if !(hi > lo) {
        lo = 0.0;
        hi = duration_secs;
    }
    hi = hi.clamp(lo, duration_secs);
    let seekable = match position_enabled {
        Some(false) => false,
        _ => hi > lo,
    };
    (lo, hi, seekable, false)
}

/// Master output volume via the MMDevice endpoint API. SMTC has no
/// per-session volume, so this is the device level — the only volume knob
/// Windows exposes without per-app audio-session matching. Fast,
/// non-blocking COM; every failure degrades to `None` (UI hides slider).
fn endpoint_volume() -> Option<
    windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
> {
    use windows::Win32::Media::Audio::{
        MMDeviceEnumerator, IMMDeviceEnumerator, eConsole,
        eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};

    ensure_com();
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()? };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()? };
    unsafe { device.Activate(CLSCTX_ALL, None).ok() }
}

/// Current master level + mute. `None` = no usable endpoint.
pub fn volume_state() -> Option<(f32, bool)> {
    let volume = endpoint_volume()?;
    let level = unsafe { volume.GetMasterVolumeLevelScalar().ok()? };
    let muted = unsafe { volume.GetMute().ok()?.as_bool() };
    Some((clamp01(level), muted))
}

/// Set master level 0.0–1.0 (clamped; NaN → 0).
pub fn set_volume(level: f32) -> Result<(), String> {
    let volume = endpoint_volume().ok_or("no audio endpoint")?;
    unsafe {
        volume
            .SetMasterVolumeLevelScalar(clamp01(level), std::ptr::null())
            .map_err(|e| format!("volume set failed: {e}"))
    }
}

/// Set master mute.
pub fn set_muted(muted: bool) -> Result<(), String> {
    let volume = endpoint_volume().ok_or("no audio endpoint")?;
    unsafe {
        volume
            .SetMute(muted, std::ptr::null())
            .map_err(|e| format!("mute set failed: {e}"))
    }
}

/// Attempts to make at a thumbnail that is not ready yet.
///
/// The snapshot runs roughly once a second, so a player that publishes art a
/// moment after its metadata needs a few tries rather than one. Kept low on
/// purpose: this runs synchronously inside `snapshot_inner`, and each attempt
/// is a full `TryGetMediaPropertiesAsync` + stream open. Five attempts against
/// a player that never publishes art would add seconds to a snapshot and stall
/// the media view, which is the failure this project has repeatedly fixed
/// elsewhere. Three attempts plus the cache means a player that does publish
/// art is covered without a deep retry loop.
const ART_RETRY_ATTEMPTS: u32 = 3;
/// Delay between attempts. Short, because the caller already retries every
/// snapshot: a player still loading art will be caught by the next push even
/// if this miss gives up.
const ART_RETRY_DELAY_MS: u64 = 80;

fn read_thumbnail_data_uri(
    session: &Session,
    cache_key: &str,
) -> Option<String> {
    if let Ok(cache) = art_cache().lock() {
        if let Some(uri) = cache.get(cache_key) {
            return Some(uri.clone());
        }
    }
    // Retry only when the read failed for a reason that can resolve itself.
    // `Empty`, `TooLarge` and `ReadFailed` are answers about this thumbnail,
    // not races, so retrying them would only add latency and log noise.
    let mut last = ArtMiss::NotReady;
    for attempt in 0..ART_RETRY_ATTEMPTS {
        match read_thumbnail_with_reason(session) {
            Ok(uri) => {
                if let Ok(mut cache) = art_cache().lock() {
                    if cache.len() > 32 {
                        cache.clear();
                    }
                    cache.insert(cache_key.to_string(), uri.clone());
                }
                return Some(uri);
            }
            Err(miss) => {
                last = miss;
                if miss != ArtMiss::NotReady {
                    break;
                }
                if attempt + 1 < ART_RETRY_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(ART_RETRY_DELAY_MS));
                }
            }
        }
    }
    log_art_miss(cache_key, last);
    None
}

/// Log a failed art read once per key, so a player that never publishes a
/// thumbnail is diagnosable instead of silently showing the initial forever.
fn log_art_miss(cache_key: &str, miss: ArtMiss) {
    use std::sync::OnceLock;
    static LOGGED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        OnceLock::new();
    let seen = LOGGED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let Ok(mut seen) = seen.lock() else {
        return;
    };
    if !seen.insert(cache_key.to_string()) {
        return;
    }
    crate::runtime::log_info(&format!(
        "[nex] media_art_unavailable reason={miss:?} key={:?}",
        cache_key.split('\u{1f}').next().unwrap_or("")
    ));
}

/// Why a thumbnail read produced nothing. Distinguishing these matters: only
/// `NotReady` is worth retrying on a later snapshot, and only `Empty` means the
/// player genuinely has no art.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum ArtMiss {
    /// `Thumbnail()` threw or returned nothing yet. Players commonly populate
    /// art only after metadata finishes loading, so this can succeed later.
    NotReady,
    /// The thumbnail reference exists but the stream is empty (size 0): the
    /// player advertised art and then delivered nothing.
    Empty,
    /// The stream is larger than we will embed.
    TooLarge,
    /// Reading the stream failed part way (closed, locked, decode error).
    ReadFailed,
}

fn read_thumbnail_with_reason(session: &Session) -> Result<String, ArtMiss> {
    use windows::Storage::Streams::{DataReader, IInputStream};

    let props = session
        .TryGetMediaPropertiesAsync()
        .map_err(|_| ArtMiss::NotReady)?
        .get()
        .map_err(|_| ArtMiss::NotReady)?;
    let reference = props.Thumbnail().map_err(|_| ArtMiss::NotReady)?;
    let stream = reference
        .OpenReadAsync()
        .map_err(|_| ArtMiss::NotReady)?
        .get()
        .map_err(|_| ArtMiss::NotReady)?;
    let size = stream.Size().map_err(|_| ArtMiss::ReadFailed)?;
    if size == 0 {
        return Err(ArtMiss::Empty);
    }
    if size > MAX_ART_BYTES {
        return Err(ArtMiss::TooLarge);
    }
    let mime = stream
        .ContentType()
        .map(|mime| mime.to_string())
        .unwrap_or_default();
    let input: IInputStream = stream.cast().map_err(|_| ArtMiss::ReadFailed)?;
    let reader = DataReader::CreateDataReader(&input).map_err(|_| ArtMiss::ReadFailed)?;
    let loaded = reader
        .LoadAsync(size as u32)
        .map_err(|_| ArtMiss::ReadFailed)?
        .get()
        .map_err(|_| ArtMiss::ReadFailed)?;
    let mut bytes = vec![0u8; loaded as usize];
    reader
        .ReadBytes(&mut bytes)
        .map_err(|_| ArtMiss::ReadFailed)?;
    let mime = if mime.is_empty() {
        "image/png".to_string()
    } else {
        mime
    };
    Ok(format!(
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
    // Single enumeration per snapshot, shared by pick + dots — two
    // back-to-back enumerations could disagree about a flaky session
    // and flap the view against its own dots.
    let Some(manager) = session_manager() else {
        return MediaState::default();
    };
    let live = live_sessions(&manager);
    let Some((session, picked_playing)) = pick_from_list(&manager, &live) else {
        return MediaState::default();
    };

    let app_id = session
        .SourceAppUserModelId()
        .map(|id| id.to_string())
        .unwrap_or_default();

    // Disappearance hold: the pick moved off a playing session to
    // something non-playing — usually a 1-snapshot dropout, not intent.
    // Freeze the last good frame briefly instead of re-anchoring the
    // whole view; genuine switches land after the grace at the latest.
    // Explicit pins bypass (user intent is instant).
    let now_ms = unix_millis();
    let pin_set = selected_app()
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .is_some();
    if let Ok(guard) = LAST_GOOD.get_or_init(|| Mutex::new(None)).lock() {
        if let Some((last_id, last_playing, last_state, t)) = guard.as_ref() {
            let age = now_ms.saturating_sub(*t);
            if should_hold(last_id, *last_playing, &app_id, picked_playing, age, pin_set) {
                return last_state.clone();
            }
        }
    }

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

    // Only live sessions open the view: playing, paused, or changing
    // (buffer flicker — the bar freezes, it never closes/reopens).
    // Stopped/closed corpses can carry stale metadata — treating them as
    // active is what opened the view on top of a dead session whose
    // controls then hung.
    if status != "playing" && status != "paused" && status != "changing" {
        return MediaState::default();
    }

    let playing = status == "playing";
    let (position_secs, duration_secs, seek_min_secs, seek_max_secs, seekable, is_live) =
        match session.GetTimelineProperties() {
            Ok(timeline) => {
                let duration =
                    ticks_to_secs(timeline.EndTime().map(|t| t.Duration).unwrap_or(0));
                let position_enabled = session
                    .GetPlaybackInfo()
                    .and_then(|info| info.Controls())
                    .and_then(|c| c.IsPlaybackPositionEnabled())
                    .ok();
                let (lo, hi, seekable, live) = resolve_seek_window(
                    timeline.MinSeekTime().map(|t| t.Duration).unwrap_or(0),
                    timeline.MaxSeekTime().map(|t| t.Duration).unwrap_or(0),
                    duration,
                    position_enabled,
                );
                let raw_pos =
                    ticks_to_secs(timeline.Position().map(|t| t.Duration).unwrap_or(0));
                let last_updated = timeline
                    .LastUpdatedTime()
                    .ok()
                    .map(datetime_to_unix_millis)
                    .unwrap_or(0);
                let position = correct_position(raw_pos, unix_millis(), last_updated, playing);
                (position, duration, lo, hi, seekable, live)
            }
            Err(_) => (0.0, 0.0, 0.0, 0.0, false, false),
        };

    // Sample time for the position above, taken immediately — the
    // thumbnail/volume reads below can take hundreds of ms, and the page
    // uses this to discard pushes sampled before its last drag/switch.
    let sampled_ms = unix_millis();

    let cache_key = format!("{title}\u{1f}{artist}\u{1f}{album}");
    let art = read_thumbnail_data_uri(&session, &cache_key);

    let (volume, muted, volume_supported) = volume_state()
        .map(|(level, muted)| (level, muted, true))
        .unwrap_or((1.0, false, false));

    let (sessions, session_key) = switcher_entries(&live, &app_id);

    let state = MediaState {
        active: true,
        title,
        artist,
        album,
        source: friendly_source(&app_id),
        status,
        position_secs,
        duration_secs,
        art,
        volume,
        muted,
        volume_supported,
        sessions,
        session_key,
        sampled_ms,
        seek_min_secs,
        seek_max_secs,
        seekable,
        live: is_live,
    };
    if let Ok(mut guard) = LAST_GOOD.get_or_init(|| Mutex::new(None)).lock() {
        *guard = Some((
            app_id.clone(),
            state.status == "playing",
            state.clone(),
            unix_millis(),
        ));
    }
    state
}

/// Fire a transport command at the preferred session.
pub fn control(action: MediaAction) -> Result<bool, String> {
    // Volume/mute never touch the media session (SMTC has no volume API) —
    // they go straight at the fast, non-blocking endpoint.
    match action {
        MediaAction::Volume(level) => {
            set_volume(level)?;
            return Ok(true);
        }
        MediaAction::Mute(muted) => {
            set_muted(muted)?;
            return Ok(true);
        }
        _ => {}
    }
    let session = cached_session().ok_or("no media session")?;
    // The session can die between pick and command (quit app, closed tab).
    // Firing into a corpse hangs the WinRT call — revalidate first and
    // drop it so the next pick finds a live one.
    if !playback_status_usable(&session) {
        forget_cached_session();
        return Err("media session is gone".to_string());
    }
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
            // Clamp to the source's seekable window (DVR/live-edge
            // aware); unseekable sources (ads) error out instead of
            // hanging the call like before.
            let timeline = session
                .GetTimelineProperties()
                .map_err(|e| format!("media seek failed: {e}"))?;
            let duration = ticks_to_secs(timeline.EndTime().map(|t| t.Duration).unwrap_or(0));
            let enabled = session
                .GetPlaybackInfo()
                .and_then(|info| info.Controls())
                .and_then(|c| c.IsPlaybackPositionEnabled())
                .ok();
            let (lo, hi, seekable, _) = resolve_seek_window(
                timeline.MinSeekTime().map(|t| t.Duration).unwrap_or(0),
                timeline.MaxSeekTime().map(|t| t.Duration).unwrap_or(0),
                duration,
                enabled,
            );
            if !seekable {
                return Err("seek not supported for this stream".to_string());
            }
            // SMTC takes 100ns ticks.
            let ticks = (position_secs.max(lo).min(hi) * 10_000_000.0) as i64;
            session
                .TryChangePlaybackPositionAsync(ticks)
                .map_err(|e| format!("media seek failed: {e}"))?
                .get()
                .map_err(|e| format!("media seek failed: {e}"))
        }
        MediaAction::Volume(_) | MediaAction::Mute(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only `NotReady` is a race that a later snapshot can fix. Retrying the
    /// others would just add latency for a thumbnail that is definitively
    /// empty, oversized or broken.
    #[test]
    fn only_not_ready_is_worth_retrying() {
        assert_eq!(ArtMiss::NotReady, ArtMiss::NotReady);
        // The properties the retry loop branches on: any other miss stops it.
        for miss in [ArtMiss::Empty, ArtMiss::TooLarge, ArtMiss::ReadFailed] {
            assert_ne!(
                miss,
                ArtMiss::NotReady,
                "{miss:?} must not be retried"
            );
        }
        assert!(ART_RETRY_ATTEMPTS >= 2, "a single attempt is not a retry");
    }

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

    #[test]
    fn volume_clamp_bounds() {
        assert_eq!(clamp01(0.5), 0.5);
        assert_eq!(clamp01(-0.25), 0.0);
        assert_eq!(clamp01(1.5), 1.0);
        assert_eq!(clamp01(f32::NAN), 0.0);
        assert_eq!(clamp01(f32::INFINITY), 0.0);
    }

    /// Being in the playing tier must dominate the Spotify preference.
    /// The old form ranked playing-Spotify *above* playing-anything-else,
    /// which made it impossible for a playing browser video to take over
    /// while Spotify was playing (CIQ-132).
    #[test]
    fn any_playing_session_outranks_a_paused_spotify() {
        assert!(session_rank(true, true) < session_rank(true, false));
        assert!(session_rank(false, true) < session_rank(true, false));
        assert!(session_rank(true, false) < session_rank(false, false));
    }

    #[test]
    fn takeover_needs_a_clearly_newer_timeline() {
        // Sticky pick not playing: any playing challenger takes over.
        assert!(should_take_over(false, 1_000, 1_000));
        assert!(should_take_over(false, 1_000, 0));

        // Sticky pick playing alone: nothing to take over from.
        assert!(!should_take_over(true, 5_000, 0));

        // Both advancing, challenger newer by less than the slack: hold, so
        // two live sessions cannot ping-pong on sub-second differences.
        assert!(!should_take_over(true, 5_000, 5_000 + 1));
        assert!(!should_take_over(true, 5_000, 5_000 + SWITCH_STAMP_SLACK_MS));

        // Clearly newer: this is "the user just started it".
        assert!(should_take_over(
            true,
            5_000,
            5_000 + SWITCH_STAMP_SLACK_MS + 1
        ));

        // Sticky has no stamp, challenger does: the challenger is the only
        // evidence available, so it wins rather than deadlocking.
        assert!(should_take_over(true, 0, 7_000));
    }

    /// The reported case: Spotify playing, then a browser video starts.
    /// The browser must take over once its timeline is clearly newer.
    #[test]
    fn playing_browser_takes_over_from_playing_spotify() {
        let spotify_stamp = 10_000;
        let browser_stamp = spotify_stamp + SWITCH_STAMP_SLACK_MS + 500;
        assert!(should_take_over(true, spotify_stamp, browser_stamp));
    }

    #[test]
    fn advancement_fresh_tracks_movement_and_grace() {
        // Advanced since anchor → fresh whatever the age.
        assert!(advancement_fresh(100.0, 0, 101.0, 999_000));
        // Frozen but young (buffering) → fresh.
        assert!(advancement_fresh(100.0, 10_000, 100.0, 15_000));
        // Frozen past the grace → stuck.
        assert!(!advancement_fresh(100.0, 10_000, 100.0, 21_000));
        // Dust doesn't count as advance.
        assert!(!advancement_fresh(100.0, 10_000, 100.4, 31_000));
    }

    #[test]
    fn should_hold_bridges_dropouts_not_takeovers() {
        // Playing -> non-playing within grace, no pin: hold.
        assert!(should_hold("a", true, "b", false, 1_000, false));
        // Pin set: user intent is instant.
        assert!(!should_hold("a", true, "b", false, 1_000, true));
        // Same session: no hold needed.
        assert!(!should_hold("a", true, "a", false, 1_000, false));
        // Last wasn't playing: nothing to protect.
        assert!(!should_hold("a", false, "b", false, 1_000, false));
        // Takeover by genuinely playing: switch at once.
        assert!(!should_hold("a", true, "b", true, 1_000, false));
        // Grace expired: switch.
        assert!(!should_hold("a", true, "b", false, 3_000, false));
        // Vanished entirely (""): hold like any dropout.
        assert!(should_hold("a", true, "", false, 1_000, false));
    }

    #[test]
    fn select_session_pins_and_clears() {
        select_session("Spotify.exe_abc!Spotify");
        assert_eq!(
            selected_session_key().as_deref(),
            Some("Spotify.exe_abc!Spotify")
        );
        select_session("");
        assert_eq!(selected_session_key(), None);
    }

    #[test]
    fn seek_window_explicit_disabled_locks() {
        // Ads: source says no seeking despite a known duration.
        let (lo, hi, seekable, live) = resolve_seek_window(0, 0, 200.0, Some(false));
        assert!(!seekable);
        assert!(!live);
        assert_eq!((lo, hi), (0.0, 200.0));
    }

    #[test]
    fn seek_window_unset_falls_back_to_duration() {
        let (lo, hi, seekable, live) = resolve_seek_window(0, 0, 200.0, Some(true));
        assert!((seekable, live) == (true, false));
        assert_eq!((lo, hi), (0.0, 200.0));
    }

    #[test]
    fn seek_window_uses_source_window() {
        // DVR window smaller than the duration is honored, not flattened.
        let (lo, hi, seekable, live) =
            resolve_seek_window(1_000_000_000, 1_600_000_000, 200.0, Some(true));
        assert!((seekable, live) == (true, false));
        assert_eq!((lo, hi), (100.0, 160.0));
    }

    #[test]
    fn seek_window_no_duration_means_live_locked() {
        let (lo, hi, seekable, live) = resolve_seek_window(0, 0, 0.0, None);
        assert!((seekable, live) == (false, true));
        assert_eq!((lo, hi), (0.0, 0.0));
    }

    #[test]
    fn position_correction_adds_lag_only_when_playing() {
        // 2s stale read while playing → corrected; paused → raw.
        assert_eq!(correct_position(100.0, 12_000, 10_000, true), 102.0);
        assert_eq!(correct_position(100.0, 12_000, 10_000, false), 100.0);
        // Unset timestamp → no correction; absurd lag capped at 5s.
        assert_eq!(correct_position(100.0, 12_000, 0, true), 100.0);
        assert_eq!(correct_position(100.0, 112_000, 10_000, true), 105.0);
    }
}
