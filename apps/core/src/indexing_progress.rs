//! Progress of a large discovery pass, from the indexer's counters to the
//! UI's decision to show the indexing progress window.
//!
//! The indexer bumps [`FullPassProgress`] while a full (non-delta) provider
//! pass runs and reports it in its `sync` response. The UI keeps the latest
//! report in [`publish_indexer_progress`] and drives the window through
//! [`ProgressWindowGate`], which only opens it for large passes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};

/// Wait this long before showing anything, so short passes never flash.
pub const SHOW_AFTER_MS: u64 = 2_000;
/// A pass that has found this many items is large enough for the window.
pub const LARGE_PASS_ITEMS: u64 = 5_000;
/// A pass running this long is large enough for the window regardless of count.
pub const LARGE_PASS_ELAPSED_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullPassSnapshot {
    /// Items the discovery providers emitted so far.
    pub discovered: u64,
    /// Items written to the store so far.
    pub indexed: u64,
    pub elapsed_ms: u64,
}

impl FullPassSnapshot {
    pub fn to_json(self) -> Value {
        json!({
            "discovered": self.discovered,
            "indexed": self.indexed,
            "elapsed_ms": self.elapsed_ms,
        })
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            discovered: value.get("discovered")?.as_u64()?,
            indexed: value.get("indexed")?.as_u64()?,
            elapsed_ms: value.get("elapsed_ms")?.as_u64()?,
        })
    }

    fn is_large(&self) -> bool {
        self.elapsed_ms >= SHOW_AFTER_MS
            && (self.discovered >= LARGE_PASS_ITEMS || self.elapsed_ms >= LARGE_PASS_ELAPSED_MS)
    }
}

/// Live counters for the indexer's running full pass. Watcher batches and
/// USN deltas never touch it, so they can never open the window.
#[derive(Default)]
pub struct FullPassProgress {
    active: AtomicBool,
    discovered: AtomicU64,
    indexed: AtomicU64,
    started_at: Mutex<Option<Instant>>,
}

impl FullPassProgress {
    fn begin(&self) {
        self.discovered.store(0, Ordering::Relaxed);
        self.indexed.store(0, Ordering::Relaxed);
        *self.started_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        self.active.store(true, Ordering::Release);
    }

    fn end(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn add_discovered(&self, count: u64) {
        self.discovered.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_indexed(&self, count: u64) {
        self.indexed.fetch_add(count, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Option<FullPassSnapshot> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let elapsed_ms = self
            .started_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|at| at.elapsed().as_millis() as u64)
            .unwrap_or(0);
        Some(FullPassSnapshot {
            discovered: self.discovered.load(Ordering::Relaxed),
            indexed: self.indexed.load(Ordering::Relaxed),
            elapsed_ms,
        })
    }
}

/// Marks one index cycle's full pass. The first provider that takes the
/// full path starts it; dropping the scope ends it, including on error.
pub struct FullPassScope<'a> {
    progress: &'a FullPassProgress,
    started: bool,
}

impl<'a> FullPassScope<'a> {
    pub fn new(progress: &'a FullPassProgress) -> Self {
        Self {
            progress,
            started: false,
        }
    }

    pub fn start(&mut self) {
        if !self.started {
            self.started = true;
            self.progress.begin();
        }
    }
}

impl Drop for FullPassScope<'_> {
    fn drop(&mut self) {
        if self.started {
            self.progress.end();
        }
    }
}

/// What the UI should do with the progress window after one observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressWindowAction {
    None,
    Show(FullPassSnapshot),
    Update(FullPassSnapshot),
    /// The pass finished: show the completed state briefly, then close.
    Finish(FullPassSnapshot),
    /// The indexer went away mid-pass: close without claiming completion.
    Close,
}

#[derive(Debug, Default)]
pub struct ProgressWindowGate {
    shown: Option<FullPassSnapshot>,
}

impl ProgressWindowGate {
    pub fn observe(&mut self, report: IndexerProgressReport) -> ProgressWindowAction {
        match (self.shown, report.full_pass) {
            (None, Some(progress)) if progress.is_large() => {
                self.shown = Some(progress);
                ProgressWindowAction::Show(progress)
            }
            (None, _) => ProgressWindowAction::None,
            (Some(previous), Some(progress)) => {
                if progress == previous {
                    ProgressWindowAction::None
                } else {
                    self.shown = Some(progress);
                    ProgressWindowAction::Update(progress)
                }
            }
            (Some(previous), None) => {
                self.shown = None;
                if report.reachable {
                    ProgressWindowAction::Finish(previous)
                } else {
                    ProgressWindowAction::Close
                }
            }
        }
    }
}

/// The UI's latest view of the indexer, written by the snapshot-sync thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexerProgressReport {
    pub reachable: bool,
    pub full_pass: Option<FullPassSnapshot>,
}

static LATEST_REPORT: Mutex<IndexerProgressReport> = Mutex::new(IndexerProgressReport {
    reachable: true,
    full_pass: None,
});

pub fn publish_indexer_progress(report: IndexerProgressReport) {
    *LATEST_REPORT.lock().unwrap_or_else(|e| e.into_inner()) = report;
}

pub fn latest_indexer_progress() -> IndexerProgressReport {
    *LATEST_REPORT.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(discovered: u64, elapsed_ms: u64) -> Option<FullPassSnapshot> {
        Some(FullPassSnapshot {
            discovered,
            indexed: discovered / 2,
            elapsed_ms,
        })
    }

    fn report(full_pass: Option<FullPassSnapshot>) -> IndexerProgressReport {
        IndexerProgressReport {
            reachable: true,
            full_pass,
        }
    }

    #[test]
    fn small_or_young_passes_never_open_the_window() {
        let mut gate = ProgressWindowGate::default();
        // Many items, but inside the show delay.
        assert_eq!(gate.observe(report(pass(50_000, 1_500))), ProgressWindowAction::None);
        // Past the delay but small and quick.
        assert_eq!(gate.observe(report(pass(300, 4_000))), ProgressWindowAction::None);
        // A finished small pass does not report a completion either.
        assert_eq!(gate.observe(report(None)), ProgressWindowAction::None);
    }

    #[test]
    fn large_pass_shows_updates_and_finishes() {
        let mut gate = ProgressWindowGate::default();
        let first = pass(6_000, 2_500);
        assert_eq!(gate.observe(report(first)), ProgressWindowAction::Show(first.unwrap()));
        assert_eq!(gate.observe(report(first)), ProgressWindowAction::None);
        let next = pass(9_000, 3_500);
        assert_eq!(gate.observe(report(next)), ProgressWindowAction::Update(next.unwrap()));
        assert_eq!(gate.observe(report(None)), ProgressWindowAction::Finish(next.unwrap()));
        assert_eq!(gate.observe(report(None)), ProgressWindowAction::None);
    }

    #[test]
    fn long_pass_shows_even_with_few_items() {
        let mut gate = ProgressWindowGate::default();
        let slow = pass(800, 10_000);
        assert_eq!(gate.observe(report(slow)), ProgressWindowAction::Show(slow.unwrap()));
    }

    #[test]
    fn lost_indexer_closes_without_completion() {
        let mut gate = ProgressWindowGate::default();
        gate.observe(report(pass(6_000, 3_000)));
        let lost = IndexerProgressReport {
            reachable: false,
            full_pass: None,
        };
        assert_eq!(gate.observe(lost), ProgressWindowAction::Close);
    }

    #[test]
    fn scope_reports_only_while_a_full_pass_runs() {
        let progress = FullPassProgress::default();
        assert!(progress.snapshot().is_none());
        {
            let mut scope = FullPassScope::new(&progress);
            assert!(progress.snapshot().is_none(), "unstarted scope reports nothing");
            scope.start();
            progress.add_discovered(3);
            progress.add_indexed(2);
            scope.start(); // second provider in the same cycle keeps counting
            progress.add_discovered(1);
            let snap = progress.snapshot().expect("active pass");
            assert_eq!((snap.discovered, snap.indexed), (4, 2));
        }
        assert!(progress.snapshot().is_none(), "dropping the scope ends the pass");
    }

    #[test]
    fn snapshot_json_roundtrips() {
        let snap = FullPassSnapshot {
            discovered: 12,
            indexed: 7,
            elapsed_ms: 3_400,
        };
        assert_eq!(FullPassSnapshot::from_json(&snap.to_json()), Some(snap));
        assert_eq!(FullPassSnapshot::from_json(&json!({ "discovered": 1 })), None);
    }
}
