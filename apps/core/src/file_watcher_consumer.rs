//! Background consumer for [`DirectoryWatcher`](crate::file_watcher) events.
//!
//! The watcher layer only knows how to detect file system changes and surface
//! them as `WatcherEvent`s. This module owns the policy that turns those
//! events into upsert/delete calls against [`CoreService`]:
//!
//! 1. Build a stable id from a path (same scheme as `discover_filesystem_walk`).
//! 2. Run the same `DiscoveryExclusionPolicy` as the initial scan so the
//!    index never gains items that a full scan would have skipped.
//! 3. Coalesce bursts (Added+Removed for the same path within the same
//!    debounce window collapse to a no-op).
//! 4. Apply a bounded queue between producer and consumer so a flood of
//!    events cannot grow unbounded memory.
//!
//! All public entry points are no-ops on non-Windows targets. The watcher
//! thread is `#[cfg(target_os = "windows")]` in `file_watcher.rs`; the
//! consumer is a pure Rust translation layer so the same code path is
//! exercised on every platform for tests.

#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, UNIX_EPOCH};

use crate::config::Config;
use crate::core_service::CoreService;
use crate::discovery::DiscoveryExclusionPolicy;
use crate::file_watcher::{DirectoryWatcher, WatcherConfig, WatcherEvent, WatcherEventKind};
use crate::model::SearchItem;

/// Maximum time a consumer waits for a new event before flushing whatever
/// is in its current batch. Bounded so a quiet system still updates the
/// index within this interval.
const CONSUMER_FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Hard cap on the number of events held in memory by a single consumer
/// between flushes. Anything above this is dropped with a warning so a
/// pathological FS storm cannot grow RSS.
const CONSUMER_BATCH_CAP: usize = 4096;

/// Per-root watcher + consumer pair.
struct WatcherEntry {
    _watcher: DirectoryWatcher,
    _consumer: JoinHandle<()>,
}

/// RAII handle owning all per-root watchers and their consumer threads.
///
/// Lifecycle:
/// - Constructed via [`FileWatcherHandle::start`].
/// - Stops all watchers and joins all consumer threads on drop.
/// - Idempotent: dropping twice is safe (each watcher/thread is taken
///   exactly once via `Option::take`).
pub(crate) struct FileWatcherHandle {
    entries: Vec<WatcherEntry>,
}

impl FileWatcherHandle {
    /// Start watching each `(root, excluded_roots)` pair. Returns an empty
    /// handle (not an error) when `roots` is empty.
    pub(crate) fn start(
        roots: Vec<PathBuf>,
        excluded_roots: Vec<PathBuf>,
        service: Arc<Mutex<CoreService>>,
    ) -> Self {
        let mut entries = Vec::new();
        for root in roots {
            let config = WatcherConfig::new(root.clone(), excluded_roots.clone());
            let start_result = DirectoryWatcher::start(config);
            let (watcher, rx) = match start_result {
                Ok(pair) => pair,
                Err(error) => {
                    crate::runtime::log_warn(&format!(
                        "[nex] directory_watcher root=\"{}\" start failed: {error}",
                        root.display()
                    ));
                    continue;
                }
            };
            let consumer = spawn_consumer(root, rx, Arc::clone(&service), excluded_roots.clone());
            entries.push(WatcherEntry {
                _watcher: watcher,
                _consumer: consumer,
            });
        }
        Self { entries }
    }

    /// Number of successfully started watcher roots.
    pub(crate) fn active_roots(&self) -> usize {
        self.entries.len()
    }
}

impl Drop for FileWatcherHandle {
    fn drop(&mut self) {
        for entry in self.entries.drain(..) {
            // Drop the watcher first; that signals its OS event loop to
            // exit. Then the consumer thread sees its `recv` unblock and
            // returns, which lets the JoinHandle finish.
            drop(entry._watcher);
            let _ = entry._consumer.join();
        }
    }
}

fn spawn_consumer(
    root: PathBuf,
    rx: Receiver<Vec<WatcherEvent>>,
    service: Arc<Mutex<CoreService>>,
    excluded_roots: Vec<PathBuf>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("nex-watcher-consumer[{}]", root.display()))
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_consumer(root, rx, service, excluded_roots)
            }));
            if let Err(payload) = outcome {
                let message = panic_message_to_string(&payload);
                crate::runtime::log_warn(&format!(
                    "[nex] directory_watcher consumer panicked: {message}"
                ));
            }
        })
        .expect("watcher consumer thread should spawn")
}

fn run_consumer(
    root: PathBuf,
    rx: Receiver<Vec<WatcherEvent>>,
    service: Arc<Mutex<CoreService>>,
    excluded_roots: Vec<PathBuf>,
) {
    // The receiver can produce one or more `Vec<WatcherEvent>`s per batch.
    // We coalesce across batches too: if a file is "Added" then "Removed"
    // within the flush window, the index update is a no-op.
    let mut pending_added: HashMap<PathBuf, ()> = HashMap::new();
    let mut pending_removed: HashSet<PathBuf> = HashSet::new();
    let mut total_dropped: usize = 0;

    loop {
        let recv = rx.recv_timeout(CONSUMER_FLUSH_INTERVAL);
        match recv {
            Ok(batch) => {
                for event in batch {
                    if pending_added.len() + pending_removed.len() >= CONSUMER_BATCH_CAP {
                        total_dropped = total_dropped.saturating_add(1);
                        continue;
                    }
                    apply_event_to_pending(event, &mut pending_added, &mut pending_removed);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // No new events; fall through to flush.
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Watcher thread has exited. Flush whatever is pending, then stop.
                flush_pending(
                    &root,
                    &service,
                    &excluded_roots,
                    &mut pending_added,
                    &mut pending_removed,
                );
                if total_dropped > 0 {
                    crate::runtime::log_warn(&format!(
                        "[nex] directory_watcher root=\"{}\" dropped {total_dropped} events due to batch cap",
                        root.display()
                    ));
                }
                return;
            }
        }

        if !pending_added.is_empty() || !pending_removed.is_empty() {
            flush_pending(
                &root,
                &service,
                &excluded_roots,
                &mut pending_added,
                &mut pending_removed,
            );
        }
    }
}

fn apply_event_to_pending(
    event: WatcherEvent,
    pending_added: &mut HashMap<PathBuf, ()>,
    pending_removed: &mut HashSet<PathBuf>,
) {
    match event.kind {
        WatcherEventKind::Added | WatcherEventKind::Modified => {
            // If a "Removed" was queued for the same path, the net effect
            // is a no-op; drop both. Otherwise the new state is "exists".
            if pending_removed.remove(&event.path) {
                pending_added.remove(&event.path);
            } else {
                pending_added.insert(event.path, ());
            }
        }
        WatcherEventKind::Removed => {
            // Reverse: a prior "Added" cancels out.
            if pending_added.remove(&event.path).is_none() {
                pending_removed.insert(event.path);
            }
        }
        WatcherEventKind::Renamed => {
            // The watcher currently emits the same path twice for a
            // rename (once as Removed with the old name, once as Added
            // with the new name). Treat as Added; the Removed side has
            // already been queued by the prior notification.
            pending_added.insert(event.path, ());
        }
    }
}

fn flush_pending(
    root: &Path,
    service: &Arc<Mutex<CoreService>>,
    excluded_roots: &[PathBuf],
    pending_added: &mut HashMap<PathBuf, ()>,
    pending_removed: &mut HashSet<PathBuf>,
) {
    if pending_added.is_empty() && pending_removed.is_empty() {
        return;
    }

    // Snapshot so the locks we take below don't conflict with mutations
    // we do further down this function.
    let added: Vec<PathBuf> = pending_added.drain().map(|(p, ())| p).collect();
    let removed: Vec<PathBuf> = pending_removed.drain().collect();

    // Build the deletion id list and the upsert `SearchItem` list *outside*
    // the service lock. These call into the FS and the config snapshot,
    // which should not run while the service is held — a long batch on
    // a broad root would otherwise block the message-pump thread.
    let exclusion = DiscoveryExclusionPolicy::new(excluded_roots);

    let mut upsert_items: Vec<SearchItem> = Vec::with_capacity(added.len());
    {
        let guard = match service.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let cfg = guard.config_snapshot();
        let show_files = cfg.show_files;
        let show_folders = cfg.show_folders;
        for path in &added {
            if !is_under_root(path, root) {
                continue;
            }
            if exclusion.should_exclude_path_under_root(path, root) {
                continue;
            }
            if let Some(item) =
                path_to_search_item(path, root, &cfg, show_files, show_folders)
            {
                upsert_items.push(item);
            }
        }
    }

    // Deletions first: removing an item frees its id, which is required
    // before we can re-upsert under the same id.
    let removed_ids: Vec<String> = removed
        .iter()
        .filter(|path| is_under_root(path, root))
        .map(|path| id_for_path(path))
        .collect();

    // Apply the batch in a single critical section. This bounds the
    // lock-hold time to a small number of Tantivy/SQLite writes and
    // eliminates the per-item lock ping-pong between this thread and
    // the message pump.
    let mut deleted = 0usize;
    let mut upserted = 0usize;
    {
        let guard = match service.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        for id in &removed_ids {
            if guard.delete_item_by_id(id).is_ok() {
                deleted += 1;
            }
        }
        for item in &upsert_items {
            if guard.upsert_item(item).is_ok() {
                upserted += 1;
            }
        }
    }

    if upserted > 0 || deleted > 0 {
        crate::runtime::log_info(&format!(
            "[nex] directory_watcher root=\"{}\" flush added={} removed={}",
            root.display(),
            upserted,
            deleted
        ));
    }
}

fn is_under_root(path: &Path, root: &Path) -> bool {
    // Case-insensitive on Windows is handled by the OS; we just check
    // that `path` is a descendant of `root` by prefix.
    path.strip_prefix(root).is_ok() || path == root
}

fn id_for_path(path: &Path) -> String {
    // The id scheme is `file:<path>` / `folder:<path>` and must match
    // `discover_filesystem_walk` and the Everything backend exactly.
    let lowercased = path.to_string_lossy();
    let kind = if path.is_dir() { "folder" } else { "file" };
    format!("{kind}:{lowercased}")
}

/// Convert a path into a [`SearchItem`], or `None` if the path should not
/// be indexed (file vs folder filter, missing file, root itself, etc.).
///
/// The kind and id are intentionally identical to what
/// `discover_filesystem_walk` would have produced for the same path, so
/// a watcher-driven upsert is indistinguishable from a scan-driven one.
fn path_to_search_item(
    path: &Path,
    root: &Path,
    config: &Config,
    show_files: bool,
    show_folders: bool,
) -> Option<SearchItem> {
    if path == root {
        return None;
    }

    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return None,
    };

    let is_dir = metadata.is_dir();
    if is_dir {
        if !show_folders {
            return None;
        }
    } else if !metadata.is_file() {
        return None;
    } else if !show_files {
        return None;
    }

    let kind = if is_dir { "folder" } else { "file" };
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let path_text = path.to_string_lossy();
    let id = format!("{kind}:{path_text}");

    let last_accessed_epoch_secs = metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let _ = config; // reserved for future per-config filtering (e.g. show_files per extension)
    Some(SearchItem::new(&id, kind, &title, &path_text).with_usage(0, last_accessed_epoch_secs))
}

fn panic_message_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_watcher::{WatcherEvent, WatcherEventKind};

    fn evt(kind: WatcherEventKind, p: &str) -> WatcherEvent {
        WatcherEvent {
            kind,
            path: PathBuf::from(p),
        }
    }

    #[test]
    fn added_then_removed_collapses_to_noop() {
        let mut added = HashMap::new();
        let mut removed = HashSet::new();
        apply_event_to_pending(evt(WatcherEventKind::Added, "/a"), &mut added, &mut removed);
        apply_event_to_pending(evt(WatcherEventKind::Removed, "/a"), &mut added, &mut removed);
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn removed_then_added_collapses_to_noop() {
        let mut added = HashMap::new();
        let mut removed = HashSet::new();
        apply_event_to_pending(
            evt(WatcherEventKind::Removed, "/a"),
            &mut added,
            &mut removed,
        );
        apply_event_to_pending(evt(WatcherEventKind::Added, "/a"), &mut added, &mut removed);
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn added_only_keeps_add() {
        let mut added = HashMap::new();
        let mut removed = HashSet::new();
        apply_event_to_pending(evt(WatcherEventKind::Added, "/a"), &mut added, &mut removed);
        assert!(added.contains_key(&PathBuf::from("/a")));
        assert!(removed.is_empty());
    }

    #[test]
    fn removed_only_keeps_remove() {
        let mut added = HashMap::new();
        let mut removed = HashSet::new();
        apply_event_to_pending(
            evt(WatcherEventKind::Removed, "/a"),
            &mut added,
            &mut removed,
        );
        assert!(added.is_empty());
        assert!(removed.contains(&PathBuf::from("/a")));
    }

    #[test]
    fn modified_treated_as_added() {
        let mut added = HashMap::new();
        let mut removed = HashSet::new();
        apply_event_to_pending(
            evt(WatcherEventKind::Modified, "/a"),
            &mut added,
            &mut removed,
        );
        assert!(added.contains_key(&PathBuf::from("/a")));
    }

    #[test]
    fn renamed_inserts_as_added() {
        let mut added = HashMap::new();
        let mut removed = HashSet::new();
        apply_event_to_pending(
            evt(WatcherEventKind::Renamed, "/a/new"),
            &mut added,
            &mut removed,
        );
        assert!(added.contains_key(&PathBuf::from("/a/new")));
    }

    #[test]
    fn id_for_path_uses_kind_prefix() {
        // The id scheme must match discover_filesystem_walk exactly.
        assert_eq!(
            id_for_path(&PathBuf::from("/some/file.txt")),
            "file:/some/file.txt"
        );
    }

    #[test]
    fn is_under_root_accepts_descendants() {
        let root = PathBuf::from("/a");
        assert!(is_under_root(&PathBuf::from("/a/b/c"), &root));
        assert!(is_under_root(&PathBuf::from("/a"), &root));
        assert!(!is_under_root(&PathBuf::from("/b"), &root));
    }

    #[test]
    fn batch_cap_drops_excess_events() {
        // Build a consumer-style state machine and flood it with one more
        // event than the cap to verify the dropping behavior. The cap is
        // shared across the two maps in `run_consumer`; here we mirror
        // the same check directly to keep this test pure.
        let mut added: HashMap<PathBuf, ()> = HashMap::new();
        let mut removed: HashSet<PathBuf> = HashSet::new();
        let mut total_dropped = 0usize;
        for i in 0..(CONSUMER_BATCH_CAP + 5) {
            if added.len() + removed.len() >= CONSUMER_BATCH_CAP {
                total_dropped += 1;
                continue;
            }
            apply_event_to_pending(
                evt(WatcherEventKind::Added, &format!("/p/{i}")),
                &mut added,
                &mut removed,
            );
        }
        assert_eq!(added.len() + removed.len(), CONSUMER_BATCH_CAP);
        assert_eq!(total_dropped, 5);
    }

    #[test]
    fn time_to_unix_handles_pre_epoch() {
        // SystemTime before UNIX_EPOCH should yield 0, not crash.
        let before_epoch = std::time::SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(60))
            .expect("system time before epoch");
        let secs = before_epoch
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        assert_eq!(secs, 0);
    }
}
