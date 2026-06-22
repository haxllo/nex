use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::thread::JoinHandle;

use crossbeam_channel::Sender;

use crate::config::Config;
use crate::core_service::CoreService;
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
}

pub(crate) struct SearchResult {
    pub(crate) generation: u64,
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
                            while let Ok(next) = req_rx.try_recv() {
                                latest = next;
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

                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let service_guard = match service.try_read() {
                                        Ok(g) => g,
                                        Err(_) => {
                                            return Err(
                                                "search engine temporarily locked"
                                                    .to_string(),
                                            )
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

                            let _ = res_tx.send(SearchResult {
                                generation: latest.generation,
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
        let gen = self.next_gen.fetch_add(1, Ordering::SeqCst);
        let _ = self.request_tx.send(SearchRequest {
            generation: gen,
            config_generation,
            parsed_query,
            max_results,
        });
        gen
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
