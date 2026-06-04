//! LRU image cache for the result-row icons.
//!
//! Each entry is keyed by file path and stores the decoded RGBA8
//! bitmap plus an `iced::widget::image::Handle` that points at it.
//! `image::load_from_memory` decodes `.png` directly; for `.ico` we
//! fall back to the first available frame.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use iced::widget::image::Handle;
use lru::LruCache;

use crate::overlay::model::OverlayRow;

const DEFAULT_MAX_ENTRIES: usize = 96;
const DEFAULT_IDLE_TRIM_MS: u32 = 90_000;

pub(crate) struct IconCache {
    inner: Mutex<Inner>,
}

struct Inner {
    lru: LruCache<PathBuf, Entry>,
    last_touch: HashMap<PathBuf, Instant>,
    max_entries: NonZeroUsize,
    idle_trim: Duration,
}

#[derive(Clone)]
struct Entry {
    handle: Handle,
    width: u32,
    height: u32,
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_IDLE_TRIM_MS)
    }
}

impl IconCache {
    pub(crate) fn new(max_entries: usize, idle_trim_ms: u32) -> Self {
        let max_entries = NonZeroUsize::new(max_entries.max(1)).unwrap();
        Self {
            inner: Mutex::new(Inner {
                lru: LruCache::new(max_entries),
                last_touch: HashMap::new(),
                max_entries,
                idle_trim: Duration::from_millis(idle_trim_ms as u64),
            }),
        }
    }

    pub(crate) fn resolve(&self, path: &str) -> Option<Handle> {
        if path.is_empty() {
            return None;
        }
        let key = PathBuf::from(path);
        let now = Instant::now();

        let mut inner = self.inner.lock().ok()?;
        let cached = inner.lru.get(&key).map(|entry| entry.handle.clone());
        if let Some(handle) = cached {
            inner.last_touch.insert(key, now);
            return Some(handle);
        }
        drop(inner);
        let entry = decode(&key)?;
        let handle = entry.handle.clone();
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_touch.insert(key.clone(), Instant::now());
            inner.lru.put(key, entry);
        }
        Some(handle)
    }

    pub(crate) fn trim_unused(&self) -> usize {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return 0,
        };
        let cutoff = Instant::now()
            .checked_sub(inner.idle_trim)
            .unwrap_or_else(Instant::now);
        let stale: Vec<PathBuf> = inner
            .last_touch
            .iter()
            .filter_map(|(k, t)| (*t < cutoff).then(|| k.clone()))
            .collect();
        for k in &stale {
            inner.lru.pop(k);
            inner.last_touch.remove(k);
        }
        stale.len()
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.lru.clear();
            inner.last_touch.clear();
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.lock().map(|i| i.lru.len()).unwrap_or(0)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn decode(path: &PathBuf) -> Option<Entry> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    Some(Entry {
        handle: Handle::from_rgba(width, height, rgba.into_raw()),
        width,
        height,
    })
}

pub(crate) fn prefetch_rows(cache: &IconCache, rows: &[OverlayRow]) {
    for row in rows {
        if !row.icon_path.is_empty() {
            cache.resolve(&row.icon_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_path_returns_none() {
        let cache = IconCache::default();
        assert!(cache.resolve("").is_none());
    }

    #[test]
    fn missing_file_returns_none() {
        let cache = IconCache::default();
        let path = std::env::temp_dir().join("nex-no-such-icon-99999.png");
        assert!(cache.resolve(path.to_string_lossy().as_ref()).is_none());
    }

    #[test]
    fn clear_resets_cache() {
        let cache = IconCache::new(4, 60_000);
        let _ = cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn trim_unused_returns_count_of_evicted() {
        let cache = IconCache::new(4, 0);
        let evicted = cache.trim_unused();
        assert_eq!(evicted, 0);
    }
}
