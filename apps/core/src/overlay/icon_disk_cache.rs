//! Bounded on-disk store for normalized icon PNGs.
//!
//! Sits behind the in-memory LRU in [`super::icons::IconCache`]: a memory
//! miss consults this store before falling back to shell extraction, so
//! the first result set after a restart or a warm-release trim repaints
//! from disk instead of re-extracting every icon.
//!
//! Each entry is one file named by a hash of the source path. The file
//! header records the full source path and a [`Fingerprint`] of the
//! source's on-disk state (size + mtime, plus whatever else the caller
//! mixes in). A load only succeeds when both match, so an icon whose
//! source changed is never served; the stale file is deleted on sight.
//!
//! Writes go to a temp file and are renamed into place, so readers never
//! see a torn entry. The directory is bounded by total bytes: when a write
//! pushes it over budget, the least recently used entries (by file mtime,
//! refreshed on every hit) are removed until it is at 3/4 of the budget.
//! [`IconDiskCache::clear`] removes every entry and is safe to call while
//! other threads load or store; they just miss.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 4] = b"NXIC";
const FORMAT_VERSION: u8 = 1;
const HEADER_LEN: usize = 4 + 1 + 8 + 4;
const ENTRY_EXT: &str = "nxicon";
const TMP_EXT: &str = "tmp";
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// Temp files older than this are leftovers from a crashed write.
const TMP_MAX_AGE: Duration = Duration::from_secs(60);

pub(crate) const MIN_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// FNV-1a over everything that decides what an icon looks like. Stable
/// across builds (unlike `DefaultHasher`), which matters for a persistent
/// key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint(u64);

impl Fingerprint {
    pub(crate) fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) fn mix(mut self, bytes: &[u8]) -> Self {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        // Length-delimit so ("ab","c") and ("a","bc") differ.
        for byte in (bytes.len() as u64).to_le_bytes() {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        self
    }

    /// Mix in `path`'s size and mtime; `None` if it cannot be stat'ed.
    pub(crate) fn mix_existing(self, path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Some(
            self.mix(&[1])
                .mix(&meta.len().to_le_bytes())
                .mix(&mtime.to_le_bytes()),
        )
    }

    /// Like [`mix_existing`](Self::mix_existing), but a missing path is
    /// itself part of the state (it may appear later).
    pub(crate) fn mix_optional(self, path: &Path) -> Self {
        self.mix_existing(path).unwrap_or_else(|| self.mix(&[0]))
    }

    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

fn path_hash(source: &str) -> u64 {
    Fingerprint::new().mix(source.as_bytes()).value()
}

struct State {
    max_bytes: u64,
    /// Sum of entry sizes; `None` until the directory is first scanned.
    total_bytes: Option<u64>,
}

pub(crate) struct IconDiskCache {
    dir: PathBuf,
    state: Mutex<State>,
}

impl IconDiskCache {
    /// Store entries under `root/<version>`. Sibling directories from other
    /// versions are removed on the first write, since a normalization
    /// change makes their contents unreachable.
    pub(crate) fn new(root: &Path, version: &str, max_bytes: u64) -> Self {
        Self {
            dir: root.join(version),
            state: Mutex::new(State {
                max_bytes: clamp_budget(max_bytes),
                total_bytes: None,
            }),
        }
    }

    #[cfg(test)]
    fn with_exact_budget(root: &Path, version: &str, max_bytes: u64) -> Self {
        let cache = Self::new(root, version, 0);
        cache.state.lock().unwrap().max_bytes = max_bytes;
        cache
    }

    fn entry_path(&self, source: &str) -> PathBuf {
        self.dir
            .join(format!("{:016x}.{ENTRY_EXT}", path_hash(source)))
    }

    /// PNG bytes for `source` if an entry exists with the same path and
    /// fingerprint. A mismatched fingerprint means the source changed:
    /// the entry is deleted and the caller re-extracts.
    pub(crate) fn load(&self, source: &str, fingerprint: Fingerprint) -> Option<Vec<u8>> {
        let file = self.entry_path(source);
        let bytes = fs::read(&file).ok()?;
        match parse_entry(&bytes, source, fingerprint) {
            Parsed::Hit(png) => {
                // Refresh mtime so pruning evicts least recently used.
                let _ = fs::File::options()
                    .write(true)
                    .open(&file)
                    .and_then(|f| f.set_modified(SystemTime::now()));
                Some(png.to_vec())
            }
            Parsed::OtherSource => None,
            Parsed::Invalid => {
                if fs::remove_file(&file).is_ok() {
                    self.account_removed(bytes.len() as u64);
                }
                None
            }
        }
    }

    pub(crate) fn store(&self, source: &str, fingerprint: Fingerprint, png: &[u8]) {
        if !png.starts_with(PNG_SIGNATURE) {
            return;
        }
        if self.ensure_scanned().is_none() {
            return;
        }
        let file = self.entry_path(source);
        let mut entry = Vec::with_capacity(HEADER_LEN + source.len() + png.len());
        entry.extend_from_slice(MAGIC);
        entry.push(FORMAT_VERSION);
        entry.extend_from_slice(&fingerprint.value().to_le_bytes());
        entry.extend_from_slice(&(source.len() as u32).to_le_bytes());
        entry.extend_from_slice(source.as_bytes());
        entry.extend_from_slice(png);

        static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = self.dir.join(format!(
            "{:016x}.{}-{}.{TMP_EXT}",
            path_hash(source),
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::File::create(&tmp)
            .and_then(|mut f| f.write_all(&entry))
            .is_ok();
        let replaced = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        if !written || fs::rename(&tmp, &file).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }

        let Ok(mut state) = self.state.lock() else { return };
        let total = state
            .total_bytes
            .unwrap_or(0)
            .saturating_sub(replaced)
            .saturating_add(entry.len() as u64);
        state.total_bytes = Some(total);
        if total > state.max_bytes {
            self.prune_locked(&mut state);
        }
    }

    /// Remove every entry. Returns the number of entry files removed.
    pub(crate) fn clear(&self) -> usize {
        let Ok(mut state) = self.state.lock() else { return 0 };
        let mut removed = 0;
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str());
                if matches!(ext, Some(ENTRY_EXT) | Some(TMP_EXT))
                    && fs::remove_file(&path).is_ok()
                    && ext == Some(ENTRY_EXT)
                {
                    removed += 1;
                }
            }
        }
        state.total_bytes = Some(0);
        removed
    }

    pub(crate) fn set_max_bytes(&self, max_bytes: u64) {
        let Ok(mut state) = self.state.lock() else { return };
        state.max_bytes = clamp_budget(max_bytes);
        if state.total_bytes.is_some_and(|t| t > state.max_bytes) {
            self.prune_locked(&mut state);
        }
    }

    #[cfg(test)]
    fn total_bytes(&self) -> Option<u64> {
        self.state.lock().unwrap().total_bytes
    }

    fn account_removed(&self, len: u64) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(total) = state.total_bytes.as_mut() {
                *total = total.saturating_sub(len);
            }
        }
    }

    /// Create the directory and compute the current size once. Also the
    /// point where other versions' directories are dropped.
    fn ensure_scanned(&self) -> Option<()> {
        let mut state = self.state.lock().ok()?;
        if state.total_bytes.is_some() {
            return Some(());
        }
        fs::create_dir_all(&self.dir).ok()?;
        if let (Some(root), Some(own)) = (self.dir.parent(), self.dir.file_name()) {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.filter_map(Result::ok) {
                    if entry.file_name() != own && entry.path().is_dir() {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
        let total = self.scan().iter().map(|(_, len, _)| len).sum();
        state.total_bytes = Some(total);
        if total > state.max_bytes {
            self.prune_locked(&mut state);
        }
        Some(())
    }

    /// Entry files as (path, len, mtime). Stale temp files are removed.
    fn scan(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let now = SystemTime::now();
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else { return out };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            match path.extension().and_then(|e| e.to_str()) {
                Some(ENTRY_EXT) => out.push((path, meta.len(), mtime)),
                Some(TMP_EXT) => {
                    if now.duration_since(mtime).unwrap_or_default() > TMP_MAX_AGE {
                        let _ = fs::remove_file(&path);
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn prune_locked(&self, state: &mut State) {
        let mut entries = self.scan();
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        let target = state.max_bytes / 4 * 3;
        entries.sort_by_key(|(_, _, mtime)| *mtime);
        for (path, len, _) in entries {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
        state.total_bytes = Some(total);
    }
}

fn clamp_budget(max_bytes: u64) -> u64 {
    max_bytes.clamp(MIN_MAX_BYTES, MAX_MAX_BYTES)
}

enum Parsed<'a> {
    Hit(&'a [u8]),
    /// Hash collision with a different source path: leave it alone.
    OtherSource,
    /// Corrupt, old format, or stale fingerprint.
    Invalid,
}

fn parse_entry<'a>(bytes: &'a [u8], source: &str, fingerprint: Fingerprint) -> Parsed<'a> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC || bytes[4] != FORMAT_VERSION {
        return Parsed::Invalid;
    }
    let stored_fp = u64::from_le_bytes(bytes[5..13].try_into().unwrap());
    let path_len = u32::from_le_bytes(bytes[13..17].try_into().unwrap()) as usize;
    let Some(path) = bytes.get(HEADER_LEN..HEADER_LEN + path_len) else {
        return Parsed::Invalid;
    };
    if path != source.as_bytes() {
        return Parsed::OtherSource;
    }
    let png = &bytes[HEADER_LEN + path_len..];
    if stored_fp != fingerprint.value() || !png.starts_with(PNG_SIGNATURE) {
        return Parsed::Invalid;
    }
    Parsed::Hit(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(fill: u8, len: usize) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.resize(len.max(PNG_SIGNATURE.len()), fill);
        bytes
    }

    fn fp(n: u64) -> Fingerprint {
        Fingerprint::new().mix(&n.to_le_bytes())
    }

    #[test]
    fn round_trips_matching_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let cache = IconDiskCache::new(dir.path(), "v1", 0);
        cache.store(r"C:\a.exe", fp(1), &png(7, 64));
        assert_eq!(cache.load(r"C:\a.exe", fp(1)), Some(png(7, 64)));
        assert_eq!(cache.load(r"C:\b.exe", fp(1)), None);
    }

    #[test]
    fn stale_fingerprint_is_never_served_and_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let cache = IconDiskCache::new(dir.path(), "v1", 0);
        cache.store(r"C:\a.exe", fp(1), &png(7, 64));
        assert_eq!(cache.load(r"C:\a.exe", fp(2)), None);
        assert!(!cache.entry_path(r"C:\a.exe").exists());
        assert_eq!(cache.load(r"C:\a.exe", fp(1)), None);
        assert_eq!(cache.total_bytes(), Some(0));
    }

    #[test]
    fn corrupt_entry_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = IconDiskCache::new(dir.path(), "v1", 0);
        cache.store(r"C:\a.exe", fp(1), &png(7, 64));
        let file = cache.entry_path(r"C:\a.exe");
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, &bytes[..HEADER_LEN + 3]).unwrap();
        assert_eq!(cache.load(r"C:\a.exe", fp(1)), None);
        // Non-PNG payloads are refused at store time.
        cache.store(r"C:\b.exe", fp(1), b"not a png");
        assert!(!cache.entry_path(r"C:\b.exe").exists());
    }

    #[test]
    fn fingerprint_tracks_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("icon.ico");
        fs::write(&file, b"one").unwrap();
        let first = Fingerprint::new().mix_existing(&file).unwrap();
        assert_eq!(first, Fingerprint::new().mix_existing(&file).unwrap());
        fs::write(&file, b"three").unwrap();
        assert_ne!(first, Fingerprint::new().mix_existing(&file).unwrap());
        let same_len = Fingerprint::new().mix_existing(&file).unwrap();
        let f = fs::File::options().write(true).open(&file).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
        assert_ne!(same_len, Fingerprint::new().mix_existing(&file).unwrap());
        assert!(Fingerprint::new().mix_existing(&dir.path().join("gone")).is_none());
        assert_ne!(
            Fingerprint::new().mix_optional(&dir.path().join("gone")),
            Fingerprint::new().mix_optional(&file)
        );
    }

    #[test]
    fn size_is_bounded_and_prunes_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let cache = IconDiskCache::with_exact_budget(dir.path(), "v1", 4096);
        let old = SystemTime::now() - Duration::from_secs(600);
        for i in 0..4u64 {
            cache.store(&format!("p{i}"), fp(i), &png(i as u8, 900));
            let f = fs::File::options()
                .write(true)
                .open(cache.entry_path(&format!("p{i}")))
                .unwrap();
            f.set_modified(old + Duration::from_secs(i)).unwrap();
        }
        // A hit makes p0 the most recently used.
        assert!(cache.load("p0", fp(0)).is_some());
        cache.store("p4", fp(4), &png(4, 900));
        let total = cache.total_bytes().unwrap();
        assert!(total <= 3072, "pruned to 3/4 budget, got {total}");
        assert!(cache.load("p0", fp(0)).is_some(), "recently used entry kept");
        assert!(cache.load("p4", fp(4)).is_some(), "newest entry kept");
        assert!(cache.load("p1", fp(1)).is_none(), "oldest entry pruned");
        let on_disk: u64 = cache.scan().iter().map(|(_, len, _)| len).sum();
        assert_eq!(on_disk, total);
    }

    #[test]
    fn clear_removes_entries_and_drops_other_versions() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("v0")).unwrap();
        fs::write(dir.path().join("v0").join("x.nxicon"), b"old").unwrap();
        let cache = IconDiskCache::new(dir.path(), "v1", 0);
        cache.store("a", fp(1), &png(1, 64));
        cache.store("b", fp(2), &png(2, 64));
        assert!(!dir.path().join("v0").exists());
        assert_eq!(cache.clear(), 2);
        assert_eq!(cache.load("a", fp(1)), None);
        assert_eq!(cache.total_bytes(), Some(0));
        // Still usable after a clear.
        cache.store("a", fp(1), &png(1, 64));
        assert!(cache.load("a", fp(1)).is_some());
    }

    #[test]
    fn budget_is_clamped() {
        assert_eq!(clamp_budget(0), MIN_MAX_BYTES);
        assert_eq!(clamp_budget(u64::MAX), MAX_MAX_BYTES);
    }
}
