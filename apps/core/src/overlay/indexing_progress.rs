//! Stub for the legacy `windows_overlay::indexing_progress` module.
//!
//! The legacy implementation spun up a tiny modal Win32 window that
//! displayed a progress bar while the first-time indexer ran. The
//! Iced shell replaces that with a progress message in the
//! `status_text` field of the model; for Phase 7 we just run the
//! closure directly on a worker thread and ignore the progress
//! `Arc<AtomicU32>` handle, so the runtime can keep calling
//! `run_with_progress_window` and the new path is a no-op UI-wise.
//! A full Iced progress bar lands in Phase 6.

#![cfg(target_os = "windows")]

use std::sync::atomic::AtomicU32;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

pub(crate) fn run_with_progress_window<F, T>(work: F) -> T
where
    F: FnOnce(Arc<AtomicU32>) -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel::<T>();
    thread::Builder::new()
        .name("nex-stub-indexer".into())
        .spawn(move || {
            let _ = tx.send(work(Arc::new(AtomicU32::new(0))));
        })
        .expect("failed to spawn stub indexer thread");
    rx.recv()
        .expect("stub indexer thread finished without sending result")
}
