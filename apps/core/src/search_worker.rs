use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::thread::JoinHandle;

use crossbeam_channel::Sender;

use crate::config::Config;
use crate::core_service::{CacheReady, CoreService};
use crate::model::SearchItem;
use crate::overlay::model::OverlayEvent;
use crate::plugin_sdk::PluginRegistry;
use crate::query_dsl::ParsedQuery;
use crate::runtime_search_session::{search_overlay_results_with_session, OverlaySearchSession};

pub(crate) struct SearchRequest {
    pub(crate) generation: u64,
    pub(crate) config_generation: u64,
    pub(crate) parsed_query: ParsedQuery,
    pub(crate) max_results: usize,
    /// For logging: number of requests coalesced into this one
    pub(crate) coalesced_count: usize,
}

pub(crate) struct SearchResult {
    pub(crate) generation: u64,
    pub(crate) config_generation: u64,
    pub(crate) results: Vec<SearchItem>,
    pub(crate) error: Option<String>,
    pub(crate) command_mode: bool,
}

pub(crate) struct SearchWorker {
    request_tx: std::sync::mpsc::Sender<SearchRequest>,
    clear_tx: std::sync::mpsc::Sender<()>,
    result_rx: Mutex<std::sync::mpsc::Receiver<SearchResult>>,
    next_gen: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl SearchWorker {
    pub(crate) fn new(
        service: Arc<RwLock<CoreService>>,
        cache_ready: Arc<CacheReady>,
        shared_config: Arc<RwLock<Config>>,
        shared_plugin_registry: Arc<RwLock<PluginRegistry>>,
        event_tx: Sender<OverlayEvent>,
    ) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<SearchRequest>();
        let (res_tx, res_rx) = std::sync::mpsc::channel::<SearchResult>();
        let (clear_tx, clear_rx) = std::sync::mpsc::channel::<()>();

        let thread = thread::Builder::new()
            .name("nex-search-worker".into())
            .spawn(move || {
                let mut session = OverlaySearchSession::default();
                let mut last_config_generation: u64 = 0;
                loop {
                    while clear_rx.try_recv().is_ok() {
                        session.clear();
                    }

                    match req_rx.recv() {
                        Ok(mut latest) => {
                            let mut coalesced = 0_usize;
                            while let Ok(next) = req_rx.try_recv() {
                                coalesced += 1;
                                latest = next;
                            }
                            latest.coalesced_count = coalesced;

                            if coalesced > 0 {
                                crate::logging::info(&format!(
                                    "[nex] search_coalesced query_len={} coalesced={} generation={}",
                                    latest.parsed_query.raw.len(),
                                    coalesced,
                                    latest.generation
                                ));
                            }

                            // Drain the clear channel again after recv.
                            // A clear_session() signal that arrived while
                            // we were blocked on recv() would have been
                            // missed by the top-of-loop drain — without
                            // this, the first post-clear query can use a
                            // stale OverlaySearchSession with cached results
                            // from before the hide/reload.
                            while clear_rx.try_recv().is_ok() {
                                session.clear();
                            }

                            // Defense-in-depth: even if every clear_session()
                            // signal was drained correctly above, the config
                            // generation on the request lets us detect a config
                            // reload that happened between when the session was
                            // cleared and when this request was sent.  This is
                            // redundant with the clear-channel drains today but
                            // makes the design race-free by construction.
                            if latest.config_generation != last_config_generation {
                                session.clear();
                                last_config_generation = latest.config_generation;
                            }

                            if latest.max_results == 0 {
                                continue;
                            }

                            // The UI starts with an empty cache while the
                            // store/snapshot loads off-thread. Searching that
                            // set returns nothing, and nothing re-runs the
                            // query, so the first results only appeared on a
                            // later keystroke (1-2 s at login).
                            let query_started = std::time::Instant::now();
                            let waited = wait_for_initial_cache(&service, &cache_ready);
                            let wait_ms = waited.as_millis();

                            // Bounded retry on lock contention: a refresh or
                            // watcher batch can hold the service write lock
                            // for a few milliseconds. Retrying a handful of
                            // times (with a short sleep) turns what used to
                            // be an empty "temporarily locked" flash into a
                            // slightly delayed — but valid — result. Only
                            // after the retries are exhausted do we report
                            // the error for this generation; the next
                            // keystroke sends a fresh generation anyway.
                            const SERVICE_LOCK_RETRIES: usize = 8;
                            const SERVICE_LOCK_RETRY_WAIT: std::time::Duration =
                                std::time::Duration::from_millis(5);
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let mut attempt = 0_usize;
                                    let service_guard = loop {
                                        match service.try_read() {
                                            Ok(g) => break g,
                                            Err(_) => {
                                                attempt += 1;
                                                if attempt > SERVICE_LOCK_RETRIES {
                                                    return Err(
                                                        "search engine temporarily locked"
                                                            .to_string(),
                                                    );
                                                }
                                                std::thread::sleep(SERVICE_LOCK_RETRY_WAIT);
                                            }
                                        }
                                    };
                                    search_overlay_results_with_session(
                                        &*service_guard,
                                        &*shared_config.read().map_err(|e| format!("config lock: {e}"))?,
                                        &*shared_plugin_registry.read().map_err(|e| format!("plugin lock: {e}"))?,
                                        &latest.parsed_query,
                                        latest.max_results,
                                        &mut session,
                                    )
                                }));

                            let (results, error) = match outcome {
                                Ok(Ok(items)) => (items, None),
                                Ok(Err(e)) => (Vec::new(), Some(e)),
                                Err(panic_payload) => {
                                    let msg = panic_payload
                                        .downcast_ref::<&str>()
                                        .map(|s| s.to_string())
                                        .or_else(|| {
                                            panic_payload.downcast_ref::<String>().cloned()
                                        })
                                        .unwrap_or_else(|| {
                                            "unknown internal error".to_string()
                                        });
                                    (
                                        Vec::new(),
                                        Some(format!(
                                            "search engine encountered an internal error: {msg}"
                                        )),
                                    )
                                }
                            };

                            // The first keystroke is the only query that pays
                            // for a cold reader, a cold icon cache and the
                            // store load at once, so the split is logged for
                            // every query that waited or ran long.
                            let total_ms = query_started.elapsed().as_millis();
                            if wait_ms > 0 || total_ms >= 50 {
                                crate::logging::info(&format!(
                                    "[nex] search_waited wait_ms={wait_ms} total_ms={total_ms} rows={} cache_ready={}",
                                    results.len(),
                                    cache_ready.is_finished()
                                ));
                            }

                            let _ = res_tx.send(SearchResult {
                                generation: latest.generation,
                                config_generation: latest.config_generation,
                                results,
                                error,
                                command_mode: latest.parsed_query.command_mode,
                            });

                            let _ = event_tx.send(OverlayEvent::SearchResultsReady);
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("search worker thread should spawn");

        Self {
            request_tx: req_tx,
            clear_tx,
            result_rx: Mutex::new(res_rx),
            next_gen: AtomicU64::new(1),
            thread: Some(thread),
        }
    }

    pub(crate) fn send_request(
        &self,
        config_generation: u64,
        parsed_query: ParsedQuery,
        max_results: usize,
    ) -> u64 {
        let generation = self.next_gen.fetch_add(1, Ordering::SeqCst);
        let _ = self.request_tx.send(SearchRequest {
            generation,
            config_generation,
            parsed_query,
            max_results,
            coalesced_count: 0,
        });
        generation
    }

    pub(crate) fn try_recv(&self) -> Option<SearchResult> {
        self.result_rx.lock().ok()?.try_recv().ok()
    }

    pub(crate) fn clear_session(&self) {
        let _ = self.clear_tx.send(());
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        let (dead_tx, _) = std::sync::mpsc::channel::<SearchRequest>();
        let _ = std::mem::replace(&mut self.request_tx, dead_tx);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Hold a query until the UI's initial cache load lands.
///
/// The loader holds a shared read guard for the duration of the load, so the
/// lock is free between polls; waiting instead of searching an empty set is
/// what makes the first keystroke show results without a second keystroke.
/// Bounded so a failed load (or a genuinely empty store) cannot wedge the
/// worker.
fn wait_for_initial_cache(
    service: &Arc<RwLock<CoreService>>,
    ready: &CacheReady,
) -> std::time::Duration {
    const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_millis(2_500);

    // Cheap gate first: most queries arrive long after the load and must not
    // touch the signal at all.
    let still_loading = match service.try_read() {
        Ok(guard) => {
            guard.is_ui_client()
                && guard.cached_items_len() == 0
                && !guard.cache_is_complete()
        }
        // The loader holds the write lock, so it is mid-load by definition.
        Err(_) => true,
    };
    if !still_loading {
        return std::time::Duration::ZERO;
    }

    // The load already finished and the cache is still empty: the store is
    // genuinely empty. Waiting cannot change that answer.
    if ready.is_finished() {
        return std::time::Duration::ZERO;
    }

    // Block on the load instead of sampling it every 10 ms. The poll used to
    // quantise the first keystroke: a load that landed 1 ms after a sample
    // still cost the rest of that sample.
    let waited = ready.wait_timeout(WAIT_LIMIT);
    if waited >= WAIT_LIMIT {
        crate::logging::info("[nex] search_waited_for_cache timed_out=true elapsed_ms=2500");
    }
    waited
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // The wait must be a wake-up, not a poll: the whole point of the change is
    // that a load landing between samples is visible immediately.
    #[test]
    fn cache_ready_wakes_a_waiter_instead_of_timing_out() {
        let ready = Arc::new(CacheReady::default());
        let waiter = Arc::clone(&ready);
        let handle = std::thread::spawn(move || waiter.wait_timeout(Duration::from_secs(2)));

        std::thread::sleep(Duration::from_millis(20));
        ready.mark_finished();

        let waited = handle.join().expect("waiter thread");
        assert!(
            waited < Duration::from_millis(500),
            "waiter should be woken by the signal, waited {waited:?}"
        );
        assert!(ready.is_finished());
    }

    #[test]
    fn cache_ready_does_not_block_when_the_load_already_finished() {
        let ready = CacheReady::default();
        ready.mark_finished();
        assert!(ready.wait_timeout(Duration::from_secs(5)) < Duration::from_millis(50));
    }

    #[test]
    fn cache_ready_is_bounded_when_nothing_signals() {
        let ready = CacheReady::default();
        let waited = ready.wait_timeout(Duration::from_millis(60));
        assert!(waited >= Duration::from_millis(50));
        assert!(waited < Duration::from_millis(400));
        assert!(!ready.is_finished());
    }
}
