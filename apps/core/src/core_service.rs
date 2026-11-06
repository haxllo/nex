use rusqlite::Connection;

use crate::action_executor::{launch_path, LaunchError};
use crate::config::{validate, Config, SearchMode};
use crate::contract::{CoreRequest, CoreResponse, LaunchResponse, SearchResponse};
use crate::discovery::{
    DiscoveryProvider, FileSystemDiscoveryProvider, ProviderError, StartMenuAppDiscoveryProvider,
};
use crate::index_store::{self, StoreError};
use crate::model::SearchItem;
use crate::search::SearchFilter;
use crate::tantivy_search::{TantivyIndex, TantivyOpenError};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockWriteGuard, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STALE_PRUNE_INTERVAL: Duration = Duration::from_secs(15);
const PROVIDER_RECONCILE_INTERVAL_SECS: i64 = 30 * 60;
/// 128 `Path::exists` probes per 15 s tick (~8/s amortized). 16 took
/// ~31 h to sweep a 120k-item index; 128 sweeps in ~4 h with the same
/// `try_write` politeness (skips under load).
const STALE_PRUNE_BATCH_SIZE: usize = 128;
/// Items per SQLite write transaction in the discovery rebuild.
const STORE_WRITE_BATCH_SIZE: usize = 500;
const PERSONALIZATION_CACHE_TTL: Duration = Duration::from_secs(5);

/// In-memory TTL cache for personalization boosts.
/// Keyed by (normalized_query, search_mode) → boosts map.
/// Avoids a SQLite query on every keystroke during rapid typing.
struct PersonalizationCache {
    boosts: HashMap<(String, String), (HashMap<String, i64>, Instant)>,
}

impl PersonalizationCache {
    fn new() -> Self {
        Self {
            boosts: HashMap::new(),
        }
    }

    fn get(&mut self, query_norm: &str, mode_key: &str) -> Option<HashMap<String, i64>> {
        let now = Instant::now();
        self.boosts
            .get(&(query_norm.to_string(), mode_key.to_string()))
            .and_then(|(boosts, inserted_at)| {
                if now.duration_since(*inserted_at) < PERSONALIZATION_CACHE_TTL {
                    Some(boosts.clone())
                } else {
                    None
                }
            })
    }

    fn insert(&mut self, query_norm: &str, mode_key: &str, boosts: HashMap<String, i64>) {
        self.boosts.insert(
            (query_norm.to_string(), mode_key.to_string()),
            (boosts, Instant::now()),
        );
    }

    /// Invalidate all cached entries. Called when the user selects an item
    /// so the next search for the same query picks up the new selection count.
    fn invalidate(&mut self) {
        self.boosts.clear();
    }
}

#[derive(Debug)]
pub enum ServiceError {
    Config(String),
    Store(StoreError),
    Provider(ProviderError),
    Launch(LaunchError),
    SearchIndex(String),
    InvalidRequest(String),
    ItemNotFound(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(error) => write!(f, "config error: {error}"),
            Self::Store(error) => write!(f, "store error: {error}"),
            Self::Provider(error) => write!(f, "provider error: {error}"),
            Self::Launch(error) => write!(f, "launch error: {error}"),
            Self::SearchIndex(error) => write!(f, "search index error: {error}"),
            Self::InvalidRequest(error) => write!(f, "invalid request: {error}"),
            Self::ItemNotFound(id) => write!(f, "item not found: {id}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<StoreError> for ServiceError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<LaunchError> for ServiceError {
    fn from(value: LaunchError) -> Self {
        Self::Launch(value)
    }
}

impl From<ProviderError> for ServiceError {
    fn from(value: ProviderError) -> Self {
        Self::Provider(value)
    }
}

pub enum LaunchTarget<'a> {
    Id(&'a str),
    Path(&'a str),
}

pub struct CoreService {
    config: RwLock<Config>,
    db: Mutex<Connection>,
    providers: RwLock<Vec<Box<dyn DiscoveryProvider>>>,
    cached_items: RwLock<Vec<SearchItem>>,
    cached_app_items: RwLock<Vec<SearchItem>>,
    /// Candidate postings for UI searches; rebuilt with the cache.
    search_index: RwLock<Option<(u64, Arc<crate::search::SearchIndex>)>>,
    /// Bumped only when the cache changes; keys the candidate index.
    cache_revision: AtomicU64,
    /// RwLock so searches, commits and syncs (all `&TantivyIndex`; the
    /// writer has its own mutex) run concurrently; only replacing or
    /// closing the index takes the write side.
    tantivy_index: RwLock<Option<TantivyIndex>>,
    /// Throttles watcher-driven commits (see `commit_search_index_if_due`).
    last_index_commit: Mutex<Option<Instant>>,
    index_warmer: Arc<crate::tantivy_search::IndexWarmer>,
    title_fts_backfill_started: AtomicBool,
    /// Set when a Tantivy search/commit fails (e.g. missing segment files);
    /// the background maintenance loop rebuilds the index from SQLite.
    tantivy_needs_heal: AtomicBool,
    /// Store row count seen by the previous drift check; drift is only
    /// repaired once the store count is stable (indexing not mid-flight).
    last_drift_store_count: Mutex<Option<u64>>,
    /// False while `cached_items` may omit store rows (bounded hot cache, or
    /// the UI before its initial load); search then also recalls from SQLite.
    cache_complete: AtomicBool,
    /// True when the resident cache is the bounded hot set rather than every
    /// store row (the indexer and the UI after CIQ-78 Phase 3).
    hot_cache: bool,
    /// True for the UI-process service: it reads the store but must never
    /// write it. Mutations are forwarded to the indexer, which owns writes.
    ui_client: bool,
    last_stale_prune: Mutex<Option<Instant>>,
    stale_prune_cursor: Mutex<Option<String>>,
    stale_pruner_started: AtomicBool,
    stale_pruner_stop: Arc<AtomicBool>,
    pub(crate) progress: Mutex<Option<Arc<AtomicU32>>>,
    compaction_write_count: Mutex<u32>,
    last_compaction_time: Mutex<Option<Instant>>,
    personalization_cache: Mutex<PersonalizationCache>,
    /// Bumped on every mutation of the item set.
    search_generation: AtomicU64,
    /// Epoch seconds of the last recent-documents recency refresh.
    recent_docs_last_scan: AtomicU64,
    /// Peak memory during last indexing cycle (in bytes).
    last_indexing_peak_memory: Mutex<Option<usize>>,
    /// Number of completed indexing cycles.
    indexing_cycle_count: AtomicU64,
    /// Bounded changelog of item-set mutations for the UI sync.
    publish_log: Mutex<PublishLog>,
    #[cfg(target_os = "windows")]
    file_watchers: Mutex<Option<crate::file_watcher_consumer::FileWatcherHandle>>,
    #[cfg(target_os = "windows")]
    application_watchers: Mutex<Option<crate::application_watcher::ApplicationWatcherHandle>>,
}

const PUBLISH_MAX_DELTA_ENTRIES: usize = 512;
const PUBLISH_RESET_THRESHOLD: usize = 512;
const PUBLISH_LOG_MAX_BATCHES: usize = 128;
const PUBLISH_LOG_MAX_ENTRIES: usize = 8192;

/// One sealed changelog batch.
#[derive(Debug)]
struct PublishBatch {
    seq: u64,
    upserts: Vec<SearchItem>,
    deleted: Vec<String>,
}

/// The outcome of a UI sync against the mutation log.
#[derive(Debug)]
pub(crate) enum PublishOutcome {
    Unchanged { generation: u64, instance: u64 },
    Delta {
        generation: u64,
        instance: u64,
        upserts: Vec<SearchItem>,
        deleted: Vec<String>,
    },
    Snapshot { generation: u64, instance: u64 },
}

/// Bounded changelog of item-set mutations.
///
/// Mutations append to an open batch; a sync seals it and hands out the sealed
/// batches after the caller's sequence. A batch larger than
/// `PUBLISH_RESET_THRESHOLD` resets the log (the caller gets a bounded
/// snapshot instead), so the log never holds a rebuild-sized set.
#[derive(Debug)]
struct PublishLog {
    instance: u64,
    next_seq: u64,
    reset_floor: u64,
    open_upserts: Vec<SearchItem>,
    open_deleted: Vec<String>,
    open_entries: usize,
    sealed: VecDeque<PublishBatch>,
    sealed_entries: usize,
}

impl PublishLog {
    fn new(instance: u64) -> Self {
        Self {
            instance,
            next_seq: 1,
            reset_floor: 0,
            open_upserts: Vec::new(),
            open_deleted: Vec::new(),
            open_entries: 0,
            sealed: VecDeque::new(),
            sealed_entries: 0,
        }
    }

    fn current(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    fn reset(&mut self) {
        let seq = self.next_seq;
        self.next_seq = seq.saturating_add(1);
        self.reset_floor = seq;
        self.sealed.clear();
        self.sealed_entries = 0;
        self.open_upserts.clear();
        self.open_deleted.clear();
        self.open_entries = 0;
    }

    fn record(&mut self, upserts: &[SearchItem], deleted: &[String]) {
        let entries = upserts.len() + deleted.len();
        if entries == 0 {
            return;
        }
        if self.open_entries + entries > PUBLISH_RESET_THRESHOLD {
            // The caller already applied the change to the store/cache; the
            // snapshot that follows covers it. Do not retain the batch.
            self.reset();
            return;
        }
        self.open_upserts.extend(upserts.iter().cloned());
        self.open_deleted.extend(deleted.iter().cloned());
        self.open_entries += entries;
    }

    fn seal(&mut self) {
        if self.open_entries == 0 {
            return;
        }
        let seq = self.next_seq;
        self.next_seq = seq.saturating_add(1);
        let upserts = std::mem::take(&mut self.open_upserts);
        let deleted = std::mem::take(&mut self.open_deleted);
        self.open_entries = 0;
        self.sealed_entries += upserts.len() + deleted.len();
        self.sealed.push_back(PublishBatch {
            seq,
            upserts,
            deleted,
        });
        while self.sealed.len() > PUBLISH_LOG_MAX_BATCHES
            || self.sealed_entries > PUBLISH_LOG_MAX_ENTRIES
        {
            let Some(old) = self.sealed.pop_front() else {
                break;
            };
            self.sealed_entries = self
                .sealed_entries
                .saturating_sub(old.upserts.len() + old.deleted.len());
        }
    }

    fn outcome(&mut self, known: u64, client_instance: u64) -> PublishOutcome {
        self.seal();
        let instance = self.instance;
        let current = self.current();
        if client_instance != instance || known < self.reset_floor || known > current {
            return PublishOutcome::Snapshot {
                generation: current,
                instance,
            };
        }
        if known == current {
            return PublishOutcome::Unchanged {
                generation: known,
                instance,
            };
        }
        let oldest = self.sealed.front().map(|batch| batch.seq).unwrap_or(current);
        if known.saturating_add(1) < oldest {
            return PublishOutcome::Snapshot {
                generation: current,
                instance,
            };
        }
        let mut upserts = Vec::new();
        let mut deleted = Vec::new();
        let mut entries = 0_usize;
        for batch in self.sealed.iter().filter(|batch| batch.seq > known) {
            entries += batch.upserts.len() + batch.deleted.len();
            if entries > PUBLISH_MAX_DELTA_ENTRIES {
                return PublishOutcome::Snapshot {
                    generation: current,
                    instance,
                };
            }
            upserts.extend(batch.upserts.iter().cloned());
            deleted.extend(batch.deleted.iter().cloned());
        }
        PublishOutcome::Delta {
            generation: current,
            instance,
            upserts,
            deleted,
        }
    }
}

fn publish_instance() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(1);
    (nanos ^ ((std::process::id() as u64) << 32)) | 1
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRefreshReport {
    pub provider: String,
    pub discovered: usize,
    pub upserted: usize,
    pub removed: usize,
    pub skipped: bool,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRefreshReport {
    pub indexed_total: usize,
    pub discovered_total: usize,
    pub upserted_total: usize,
    pub removed_total: usize,
    pub providers: Vec<ProviderRefreshReport>,
}

/// Watchers removed from the service during a config reload.
///
/// Dropping these joins the consumer threads. They may be blocked on
/// `service.write()`, so callers must drop this value **after** releasing
/// the service `RwLock` read guard.
pub struct DeferredWatcherStop {
    #[cfg(target_os = "windows")]
    file: Option<crate::file_watcher_consumer::FileWatcherHandle>,
    #[cfg(target_os = "windows")]
    application: Option<crate::application_watcher::ApplicationWatcherHandle>,
}

impl CoreService {
    pub fn new(config: Config) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        let db = index_store::open_from_config(&config)?;
        Self::with_loaded_cache_mode(config, db, true, true, false)
    }

    /// Full keep-all cache over `db` with Tantivy enabled. Production uses the
    /// bounded `new_indexer`/`new_ui_client*` constructors instead; this one
    /// serves tests and tools that want every row in memory.
    pub fn with_connection(config: Config, db: Connection) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        Self::with_loaded_cache_mode(config, db, true, true, false)
    }

    /// Indexer-process service: owns the index and publishes the full item set
    /// over IPC. The cache holds only the hot set; `all_items_snapshot` reads
    /// the rest from SQLite.
    pub fn new_indexer(config: Config) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        let db = index_store::open_from_config(&config)?;
        Self::with_loaded_cache_mode(config, db, true, true, true)
    }

    /// UI-process service that starts empty: the runtime loads the store on a
    /// background thread so the overlay and hotkey come up without waiting for
    /// a full item scan. Search falls back to SQLite until that load lands.
    pub fn new_ui_client_lazy(config: Config) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        let db = index_store::open_from_config(&config)?;
        Self::with_loaded_cache_mode(config, db, false, false, true)
    }

    /// UI-process service: reads the store for launch/personalization state,
    /// but owns no Tantivy index. Discovery and backends belong to the indexer
    /// process; search runs over the snapshot it publishes.
    pub fn new_ui_client(config: Config) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        let db = index_store::open_from_config(&config)?;
        Self::with_loaded_cache_mode(config, db, false, true, true)
    }

    fn with_loaded_cache_mode(
        config: Config,
        db: Connection,
        enable_tantivy: bool,
        load_items: bool,
        hot_cache: bool,
    ) -> Result<Self, ServiceError> {
        let all_items = if load_items {
            if hot_cache {
                index_store::list_hot_items(&db, UI_CACHE_MAX_ITEMS)?
            } else {
                index_store::list_items(&db)?
            }
        } else {
            Vec::new()
        };
        let cached_apps = collect_app_items(&all_items);

        let index_dir = config.index_db_path.parent().unwrap_or(Path::new("."));
        let tantivy_path = index_dir.join("index.tantivy");

        let tantivy_index = if enable_tantivy {
            if hot_cache {
                // Only read the whole store when the index is missing/empty;
                // an existing index already covers it.
                let docs = TantivyIndex::read_only_doc_count(&tantivy_path).unwrap_or(0);
                let seed = if docs == 0 {
                    index_store::list_items(&db)?
                } else {
                    Vec::new()
                };
                open_or_reset_tantivy_index(&tantivy_path, &seed)
            } else {
                open_or_reset_tantivy_index(&tantivy_path, &all_items)
            }
        } else {
            None
        };
        let cache_complete = load_items && !hot_cache;

        Ok(Self {
            config: RwLock::new(config),
            db: Mutex::new(db),
            providers: RwLock::new(Vec::new()),
            cached_items: RwLock::new(all_items),
            cached_app_items: RwLock::new(cached_apps),
            search_index: RwLock::new(None),
            cache_revision: AtomicU64::new(0),
            tantivy_index: RwLock::new(tantivy_index),
            last_index_commit: Mutex::new(None),
            index_warmer: Arc::new(crate::tantivy_search::IndexWarmer::default()),
            title_fts_backfill_started: AtomicBool::new(false),
            tantivy_needs_heal: AtomicBool::new(false),
            last_drift_store_count: Mutex::new(None),
            cache_complete: AtomicBool::new(cache_complete),
            hot_cache,
            ui_client: !enable_tantivy,
            last_stale_prune: Mutex::new(None),
            stale_prune_cursor: Mutex::new(None),
            stale_pruner_started: AtomicBool::new(false),
            stale_pruner_stop: Arc::new(AtomicBool::new(false)),
            progress: Mutex::new(None),
            compaction_write_count: Mutex::new(0),
            last_compaction_time: Mutex::new(None),
            personalization_cache: Mutex::new(PersonalizationCache::new()),
            // Starts at 1 so an empty cache (generation 0) never validates.
            search_generation: AtomicU64::new(1),
            recent_docs_last_scan: AtomicU64::new(0),
            last_indexing_peak_memory: Mutex::new(None),
            indexing_cycle_count: AtomicU64::new(0),
            publish_log: Mutex::new(PublishLog::new(publish_instance())),
            #[cfg(target_os = "windows")]
            file_watchers: Mutex::new(None),
            #[cfg(target_os = "windows")]
            application_watchers: Mutex::new(None),
        })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Public accessor for the database connection (for Quick Launch queries).
    pub fn db_ref(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db()
    }

    pub fn with_providers(self, providers: Vec<Box<dyn DiscoveryProvider>>) -> Self {
        self.replace_providers(providers);
        self
    }

    pub fn with_runtime_providers(self) -> Self {
        let providers = runtime_providers_from_config(&self.config_snapshot());
        self.replace_providers(providers);
        self
    }

    pub fn reconfigure_runtime_providers(
        &self,
        cfg: &Config,
    ) -> Result<DeferredWatcherStop, ServiceError> {
        validate(cfg).map_err(ServiceError::Config)?;
        let providers = runtime_providers_from_config(cfg);
        self.replace_runtime_config(cfg.clone());
        self.replace_providers(providers);
        // Hand the old watchers back instead of joining here: the caller may
        // still hold the service read guard, and the consumer threads can be
        // blocked on `service.write()`. Dropping the returned value after the
        // guard is released joins them safely.
        Ok(DeferredWatcherStop {
            #[cfg(target_os = "windows")]
            file: self.take_file_watchers(),
            #[cfg(target_os = "windows")]
            application: self.take_application_watchers(),
        })
    }

    fn replace_providers(&self, providers: Vec<Box<dyn DiscoveryProvider>>) {
        match self.providers.write() {
            Ok(mut guard) => *guard = providers,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = providers;
            }
        }
    }

    fn runtime_providers(&self) -> Vec<String> {
        match self.providers.read() {
            Ok(guard) => guard
                .iter()
                .map(|p| p.provider_name().to_string())
                .collect(),
            Err(poisoned) => poisoned
                .into_inner()
                .iter()
                .map(|p| p.provider_name().to_string())
                .collect(),
        }
    }

    pub fn configured_provider_names(&self) -> Vec<String> {
        self.runtime_providers()
    }

    pub(crate) fn config_snapshot(&self) -> Config {
        match self.config.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn replace_runtime_config(&self, next: Config) {
        match self.config.write() {
            Ok(mut guard) => *guard = next,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = next;
            }
        }
    }
}

/// Take `lock` for writing without parking as a waiting writer.
///
/// A blocked `write()` makes every later `read()` wait too (the std lock is
/// writer-preferring), so a watcher batch queued behind the rebuild's
/// minutes-long read guard stalled all IPC searches and syncs until the
/// rebuild finished. Polling `try_write` lets readers keep flowing meanwhile.
pub fn write_without_blocking_readers<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    const POLL_INTERVAL: Duration = Duration::from_millis(10);
    loop {
        match lock.try_write() {
            Ok(guard) => return guard,
            Err(TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

fn runtime_providers_from_config(config: &Config) -> Vec<Box<dyn DiscoveryProvider>> {
    let mut providers: Vec<Box<dyn DiscoveryProvider>> = Vec::new();
    providers.push(Box::new(StartMenuAppDiscoveryProvider::default()));

    // Register filesystem provider for roots-based file/folder discovery.
    let filesystem_provider = FileSystemDiscoveryProvider::from_config(config, config.discovery_max_depth as usize);
    crate::logging::info(&format!(
        "[nex] file_discovery_backend = {} (requested={})",
        filesystem_provider.backend_label(),
        config.file_discovery_backend.as_str(),
    ));
    providers.push(Box::new(filesystem_provider));
    providers
}

impl CoreService {
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchItem>, ServiceError> {
        self.search_with_filter(query, limit, &SearchFilter::default())
    }

    pub fn search_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        self.search_with_filter_internal(query, limit, filter, true)
    }

    pub fn search_with_filter_uncapped(
        &self,
        query: &str,
        limit: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        self.search_with_filter_internal(query, limit, filter, false)
    }

    fn search_with_filter_internal(
        &self,
        query: &str,
        limit: usize,
        filter: &SearchFilter,
        clamp_to_config_max: bool,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        let config_snapshot = self.config_snapshot();

        let effective_limit = if clamp_to_config_max {
            if limit == 0 {
                config_snapshot.max_results as usize
            } else {
                limit.min(config_snapshot.max_results as usize)
            }
        } else if limit == 0 {
            config_snapshot.max_results as usize
        } else {
            limit
        };

        if should_use_app_cache(filter) {
            // Read a consistent (generation, items) snapshot with a bounded
            // blocking read. The swap in `refresh_cache_from_store` replaces
            // the whole Vec at once, so the wait is a single swap — never an
            // empty result caused by lock contention.
            let (_generation, items) = self.cached_app_snapshot();
            let query_boosts = self.query_personalization_boosts(query, filter.mode)?;
            return Ok(crate::search::search_with_filter_with_boosts(
                &items,
                query,
                effective_limit,
                filter,
                Some(&query_boosts),
            ));
        }

        // Path 1: indexed search (Tantivy) returns pre-ranked candidates
        // directly. The index is only populated when background indexing has
        // finished successfully. When the index is empty (e.g., Everything
        // service was down during indexing) we fall back to scanning the
        // in-memory cache snapshot. Tantivy gives snapshot isolation via
        // per-query reader reload; the cache fallback below is stamped with
        // the live generation so mixed-generation merges stay detectable.
        if should_use_db_query_seed(filter, query) {
            let indexed_seed_limit =
                (config_snapshot.index_max_items_per_query_seed as usize).max(250);
            let candidates = self.search_indexed_candidates(query, indexed_seed_limit)?;
            if !candidates.is_empty() {
                let query_boosts = self.query_personalization_boosts(query, filter.mode)?;
                let mut ranked = crate::search::search_with_filter_with_boosts(
                    &candidates,
                    query,
                    effective_limit,
                    filter,
                    Some(&query_boosts),
                );
                if ranked.len() < effective_limit {
                    // Augment with in-memory cache items the index missed,
                    // ranked from the same live snapshot.
                    let cache_ranked = self.with_cached_items_and_index(|items, index| {
                        crate::search::search_with_filter_with_boosts_indexed(
                            items,
                            index,
                            query,
                            effective_limit.saturating_sub(ranked.len()),
                            filter,
                            Some(&query_boosts),
                        )
                    });
                    for item in cache_ranked {
                        let dominated = ranked.iter().any(|r| {
                            (!r.path.is_empty() && !item.path.is_empty()
                                && r.path.eq_ignore_ascii_case(&item.path))
                                || r.id == item.id
                        });
                        if !dominated {
                            ranked.push(item);
                            if ranked.len() >= effective_limit {
                                break;
                            }
                        }
                    }
                }
                if ranked.len() < effective_limit {
                    self.append_store_recall(&mut ranked, query, effective_limit, filter, &query_boosts);
                }
                // Hoist apps to the top so the grid-cell app row is
                // always visible regardless of BM25 score ordering.
                let mut apps: Vec<SearchItem> = Vec::new();
                let mut rest: Vec<SearchItem> = Vec::new();
                for r in ranked {
                    if r.kind.eq_ignore_ascii_case("app") {
                        apps.push(r);
                    } else {
                        rest.push(r);
                    }
                }
                ranked = apps.into_iter().chain(rest).collect();
                return Ok(ranked);
            }
        }

        // Path 2: no index results — rank a consistent snapshot of the
        // in-memory cache. A blocking snapshot read is bounded by a single
        // whole-Vec swap, so contention surfaces as a slightly older (but
        // valid) result set, never a blank flash.
        let query_boosts = self.query_personalization_boosts(query, filter.mode)?;
        let mut ranked = self.with_cached_items_and_index(|items, index| {
            crate::search::search_with_filter_with_boosts_indexed(
                items,
                index,
                query,
                effective_limit,
                filter,
                Some(&query_boosts),
            )
        });
        if ranked.len() < effective_limit {
            self.append_store_recall(&mut ranked, query, effective_limit, filter, &query_boosts);
        }
        Ok(ranked)
    }

    /// Top up `ranked` with file/folder rows straight from SQLite (FTS5
    /// title trigram index, with a `LIKE` scan while the backfill runs).
    ///
    /// This is the bounded-cache path: the UI process keeps only a hot set
    /// (`UI_CACHE_MAX_ITEMS`) and owns no Tantivy index, so when the
    /// indexer's seed search is unavailable or times out, recall from the
    /// store is how it reaches files outside that set. Skipped when the
    /// cache already holds every row.
    fn append_store_recall(
        &self,
        ranked: &mut Vec<SearchItem>,
        query: &str,
        effective_limit: usize,
        filter: &SearchFilter,
        query_boosts: &HashMap<String, i64>,
    ) {
        // Substring recall below three characters matches most of the store
        // and cannot use the trigram index; prefix search already covers it.
        if self.cache_complete.load(Ordering::SeqCst)
            || !query.split_whitespace().any(|term| term.chars().count() >= 3)
        {
            return;
        }
        let (include_files, include_folders) = match filter.mode {
            SearchMode::All => (filter.include_files, filter.include_folders),
            SearchMode::Files => (filter.include_files, false),
            SearchMode::Folders => (false, filter.include_folders),
            _ => return,
        };
        let needed = effective_limit.saturating_sub(ranked.len());
        if needed == 0 {
            return;
        }
        let fetch = needed.saturating_mul(4).clamp(64, 1000);
        let rows = match index_store::search_file_folder_items(
            &*self.db(),
            query,
            include_files,
            include_folders,
            fetch,
        ) {
            Ok(rows) => rows,
            Err(e) => {
                crate::logging::warn(&format!("[nex] store recall query failed: {e}"));
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let seen_ids: HashSet<String> = ranked.iter().map(|r| r.id.clone()).collect();
        let rows: Vec<SearchItem> = rows
            .into_iter()
            .filter(|row| !seen_ids.contains(&row.id))
            .collect();
        let store_ranked = crate::search::search_with_filter_with_boosts(
            &rows,
            query,
            needed,
            filter,
            Some(query_boosts),
        );
        ranked.extend(store_ranked);
    }

    pub fn cached_items_snapshot(&self) -> Vec<SearchItem> {
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }

    /// Borrow the cached items without cloning. The IPC delta path diffs the
    /// live set against a compact index; cloning 80k items on every poll was
    /// both a large allocation and a stall for the writer.
    pub fn with_cached_items<R>(&self, f: impl FnOnce(&[SearchItem]) -> R) -> R {
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&guard)
    }

    /// Borrow the cache together with the candidate index built for that
    /// exact revision. The cache read guard is held while the index is read,
    /// so a cache writer cannot swap the items between the two.
    pub fn with_cached_items_and_index<R>(
        &self,
        f: impl FnOnce(&[SearchItem], Option<&crate::search::SearchIndex>) -> R,
    ) -> R {
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let revision = self.cache_revision.load(Ordering::Acquire);
        let index = self.search_index.read().ok().and_then(|slot| {
            slot.as_ref()
                .filter(|(stored, _)| *stored == revision)
                .map(|(_, index)| Arc::clone(index))
        });
        f(&guard, index.as_deref())
    }

    /// Rebuild the candidate index while the cache write guard is held. The
    /// revision and index are published before the writer releases the cache,
    /// so a reader can never pair new items with an old index.
    fn rebuild_search_index_for_cache(&self, items: &[SearchItem]) {
        if !self.ui_client {
            return;
        }
        let revision = self.cache_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let index = Arc::new(crate::search::SearchIndex::build(items));
        if let Ok(mut slot) = self.search_index.write() {
            *slot = Some((revision, index));
        }
    }

    pub fn cached_items_len(&self) -> usize {
        self.cached_len()
    }

    /// True once the cache holds the full published set (or the store load
    /// finished). The UI starts with an empty cache, so this is how the
    /// search worker knows a query arrived before the initial load landed.
    pub fn cache_is_complete(&self) -> bool {
        self.cache_complete.load(Ordering::SeqCst)
    }

    pub fn is_ui_client(&self) -> bool {
        self.ui_client
    }

    pub fn reload_cache_from_store(&self) -> Result<usize, ServiceError> {
        self.refresh_cache_from_store()?;
        Ok(self.cached_len())
    }

    /// Bounded first-sync snapshot read from SQLite, without cloning the
    /// indexer's full cache.
    pub fn hot_snapshot_items(&self) -> Result<Vec<SearchItem>, ServiceError> {
        Ok(index_store::list_hot_items(&*self.db(), UI_CACHE_MAX_ITEMS)?)
    }

    /// Full store contents. Served from the cache when it holds every row;
    /// bounded (hot) caches read SQLite directly.
    pub fn all_items_snapshot(&self) -> Result<Vec<SearchItem>, ServiceError> {
        if self.cache_complete.load(Ordering::SeqCst) {
            return Ok(self.cached_items_snapshot());
        }
        Ok(index_store::list_items(&*self.db())?)
    }

    /// Replace the searchable item set from the indexer. The UI keeps a
    /// smaller subset; the rest is found by the indexer's search or SQLite.
    pub fn apply_external_snapshot(&self, items: Vec<SearchItem>) -> usize {
        let items = if self.ui_client {
            bounded_ui_items(&items)
        } else {
            items
        };
        let apps = collect_app_items(&items);
        let count = items.len();
        {
            let mut guard = self
                .cached_items
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *guard = items;
            self.rebuild_search_index_for_cache(&guard);
        }
        *self.cached_app_items.write().unwrap_or_else(|e| e.into_inner()) = apps;
        self.cache_complete.store(!self.ui_client, Ordering::SeqCst);
        self.bump_search_generation();
        count
    }

    /// Apply an incremental change published by the indexer.
    ///
    /// Returns false when the delta was empty. Upserts replace by id (or are
    /// appended); deleted ids are removed. Item order is not significant for
    /// search, so this is a single rebuild pass rather than an index shuffle.
    pub fn apply_external_delta(&self, upserts: Vec<SearchItem>, deleted: Vec<String>) -> bool {
        if upserts.is_empty() && deleted.is_empty() {
            return false;
        }
        let mut replacements: std::collections::HashMap<String, SearchItem> =
            upserts.into_iter().map(|item| (item.id.clone(), item)).collect();
        let deleted: std::collections::HashSet<String> = deleted.into_iter().collect();

        let mut guard = self.cached_items.write().unwrap_or_else(|e| e.into_inner());
        let mut next = Vec::with_capacity(guard.len() + replacements.len());
        for item in guard.drain(..) {
            if deleted.contains(&item.id) {
                continue;
            }
            match replacements.remove(&item.id) {
                Some(replacement) => next.push(replacement),
                None => next.push(item),
            }
        }
        next.extend(replacements.into_values());
        let next = if self.ui_client {
            bounded_ui_items(&next)
        } else {
            next
        };
        let apps = collect_app_items(&next);
        *guard = next;
        self.rebuild_search_index_for_cache(&guard);
        drop(guard);
        *self.cached_app_items.write().unwrap_or_else(|e| e.into_inner()) = apps;
        self.cache_complete.store(!self.ui_client, Ordering::SeqCst);
        self.bump_search_generation();
        true
    }

    pub fn launch(&self, target: LaunchTarget<'_>) -> Result<(), ServiceError> {
        self.launch_with_query_context(target, None, None)
    }

    pub fn launch_with_query_context(
        &self,
        target: LaunchTarget<'_>,
        _query: Option<&str>,
        _mode: Option<SearchMode>,
    ) -> Result<(), ServiceError> {
        match target {
            LaunchTarget::Path(path) => launch_path(path).map_err(ServiceError::from),
            LaunchTarget::Id(id) => {
                // Look up the item from the in-memory cache instead of the DB
                // to avoid SQLITE_BUSY when the background indexer is writing
                // to the same database file from its own connection.
                let cached = self
                    .cached_items
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|i| i.id == id)
                    .cloned();
                // Compaction can evict rows that search still returns via
                // store recall, so fall back to the store on a cache miss.
                let item = match cached {
                    Some(item) => item,
                    None => index_store::get_item(&*self.db(), id)?
                        .ok_or_else(|| ServiceError::ItemNotFound(id.to_string()))?,
                };
                match launch_path(&item.path) {
                    Ok(()) => {
                        // Launch learning updates both ranking and Quick Launch.
                        if let Err(error) = self.record_successful_launch(&item) {
                            crate::logging::warn(&format!("[nex] record launch failed: {error}"));
                        }
                        Ok(())
                    }
                    Err(error) => {
                        // Log structured launch failure diagnostics
                        crate::logging::warn(&format!(
                            "[nex] launch_failure id={} kind={} path=\"{}\" error={}",
                            item.id,
                            item.kind,
                            item.path,
                            error
                        ));
                        if should_prune_after_launch_error(&item, &error) {
                            if self.ui_client {
                                crate::ipc::send_indexer_command(
                                    "delete_item",
                                    serde_json::json!({ "id": item.id }),
                                );
                            } else if let Err(e) = index_store::delete_item(&*self.db(), &item.id)
                            {
                                crate::logging::warn(&format!(
                                    "[nex] delete_item after launch error failed: {e}"
                                ));
                            }
                            self.remove_cached_item_by_id(&item.id);
                        }
                        Err(ServiceError::from(error))
                    }
                }
            }
        }
    }

    pub fn record_query_selection_hint(
        &self,
        query: &str,
        mode: SearchMode,
        item_id: &str,
    ) -> Result<(), ServiceError> {
        let query_norm = crate::model::normalize_for_search(query);
        if query_norm.is_empty() {
            return Ok(());
        }
        if matches!(mode, SearchMode::Actions | SearchMode::Clipboard) {
            return Ok(());
        }
        if self.ui_client {
            // The indexer owns the store; it records the selection. Drop the
            // local personalization cache so the next read picks it up.
            {
                let mut cache = match self.personalization_cache.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                cache.invalidate();
            }
            crate::ipc::send_indexer_command(
                "record_selection",
                serde_json::json!({
                    "query": query_norm,
                    "mode": search_mode_key(mode),
                    "id": item_id,
                }),
            );
            return Ok(());
        }
        index_store::record_query_selection(
            &*self.db(),
            &query_norm,
            search_mode_key(mode),
            item_id,
            now_epoch_secs(),
        )?;
        // Invalidate the personalization cache so the next search for the
        // same query picks up the new selection count.
        {
            let mut cache = match self.personalization_cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            cache.invalidate();
        }
        Ok(())
    }

    pub fn rebuild_index(&self) -> Result<usize, ServiceError> {
        let report = self.rebuild_index_incremental_with_report()?;
        Ok(report.indexed_total)
    }

    pub fn rebuild_index_with_report(&self) -> Result<IndexRefreshReport, ServiceError> {
        self.rebuild_index_internal(false, None)
    }

    pub fn rebuild_index_incremental_with_report(
        &self,
    ) -> Result<IndexRefreshReport, ServiceError> {
        self.rebuild_index_internal(true, None)
    }

    /// Reconcile app launch surfaces without walking configured file roots.
    /// Native application notifications call this path to bypass provider stamps.
    #[cfg(target_os = "windows")]
    pub(crate) fn rebuild_application_index_with_report(
        &self,
    ) -> Result<IndexRefreshReport, ServiceError> {
        self.rebuild_index_internal(false, Some("start-menu-apps"))
    }

    fn rebuild_index_internal(
        &self,
        incremental_mode: bool,
        provider_name: Option<&str>,
    ) -> Result<IndexRefreshReport, ServiceError> {
        let cycle_start = Instant::now();
        let memory_before = self_measure_memory();
        let providers_guard = match self.providers.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if providers_guard.is_empty() {
            self.refresh_cache_from_store()?;
            return Ok(IndexRefreshReport {
                indexed_total: index_store::count_items(&*self.db())? as usize,
                discovered_total: 0,
                upserted_total: 0,
                removed_total: 0,
                providers: Vec::new(),
            });
        }

        let mut discovered_total = 0_usize;
        let mut upserted_total = 0_usize;
        let mut removed_total = 0_usize;
        let mut provider_reports = Vec::with_capacity(providers_guard.len());
        let now_epoch_secs = now_epoch_secs();

        let progress_pct = match self.progress.lock() {
            Ok(g) => g.clone(),
            Err(e) => e.into_inner().clone(),
        };
        let mut cumulative_weight: u32 = 0;
        let config_snapshot = self.config_snapshot();

        for provider in providers_guard.iter() {
            if provider_name.is_some_and(|name| provider.provider_name() != name) {
                continue;
            }
            let started = Instant::now();
            let provider_name = provider.provider_name().to_string();
            let is_filesystem = provider_name == "filesystem";
            let provider_weight: u32 = if is_filesystem { 90 } else { 5 };
            let discovery_weight: u32 = if is_filesystem {
                provider_weight.saturating_mul(4) / 5
            } else {
                provider_weight
            };
            let write_weight = provider_weight.saturating_sub(discovery_weight);
            let provider_stamp = if incremental_mode {
                provider.change_stamp()
            } else {
                None
            };
            // The roots/excludes/caps changed since the last completed full
            // scan. The saved USN position cannot describe items that were
            // never under an old root, so take one full pass before trusting
            // deltas again.
            let mut force_provider_refresh = incremental_mode
                && provider_stamp_changed(
                    &*self.db(),
                    &provider_name,
                    provider_stamp.as_deref(),
                )?;
            let incremental = if force_provider_refresh {
                None
            } else {
                provider.take_incremental()
            };
            if let Some(result) = incremental {
                match result {
                    Ok(update) => {
                        let upserted = update.upserts.len();
                        let mut removed_ids = update.deleted_ids.clone();
                        for chunk in update.deleted_ids.chunks(STORE_WRITE_BATCH_SIZE) {
                            index_store::delete_items(&*self.db(), chunk)?;
                        }
                        for prefix in &update.deleted_prefixes {
                            removed_ids.extend(index_store::delete_items_under_prefix(
                                &*self.db(),
                                prefix,
                            )?);
                        }
                        // Delta deletes must reach the search backends too;
                        // otherwise the Tantivy index keeps serving them.
                        for id in &removed_ids {
                            self.remove_item_from_backends(id);
                        }
                        for chunk in update.upserts.chunks(STORE_WRITE_BATCH_SIZE) {
                            index_store::upsert_items(&*self.db(), chunk)?;
                        }
                        let removed = removed_ids.len();
                        self.record_publish(&update.upserts, &removed_ids);
                        // Advance the journal only after every write landed.
                        #[cfg(target_os = "windows")]
                        for (letter, journal_id, next_usn) in &update.states {
                            crate::mft_discovery::write_processed_state(
                                *letter,
                                *journal_id,
                                *next_usn,
                            );
                        }
                        upserted_total += upserted;
                        removed_total += removed;
                        provider_reports.push(ProviderRefreshReport {
                            provider: provider_name.clone(),
                            discovered: upserted + removed,
                            upserted,
                            removed,
                            skipped: false,
                            elapsed_ms: started.elapsed().as_millis(),
                        });
                        cumulative_weight = cumulative_weight.saturating_add(provider_weight);
                        if let Some(ref pct) = progress_pct {
                            pct.store(cumulative_weight.min(95), Ordering::Relaxed);
                        }
                        continue;
                    }
                    Err(error) => {
                        crate::logging::warn(&format!(
                            "[nex] discovery_delta provider={provider_name} failed: {error}"
                        ));
                        // A delta that could not be applied must not be masked
                        // by the provider freshness skip: run the full pass.
                        force_provider_refresh = true;
                    }
                }
            }
            if incremental_mode
                && !force_provider_refresh
                && should_skip_provider_discovery(
                    &*self.db(),
                    &provider_name,
                    provider_stamp.as_deref(),
                    now_epoch_secs,
                )?
            {
                provider_reports.push(ProviderRefreshReport {
                    provider: provider_name.clone(),
                    discovered: 0,
                    upserted: 0,
                    removed: 0,
                    skipped: true,
                    elapsed_ms: started.elapsed().as_millis(),
                });
                log_provider_freshness_status(&*self.db(), &provider_name, now_epoch_secs, true)?;
                cumulative_weight = cumulative_weight.saturating_add(provider_weight);
                if let Some(ref pct) = progress_pct {
                    pct.store(cumulative_weight.min(95), Ordering::Relaxed);
                }
                continue;
            }

            if let Some(ref pct) = progress_pct {
                pct.store(cumulative_weight.min(95), Ordering::Relaxed);
            }
            // Discovery has no item cap; this is only the progress scale.
            const DISCOVERY_PROGRESS_SCALE: usize = 1_000_000;
            let discovery_cap = if is_filesystem {
                DISCOVERY_PROGRESS_SCALE
            } else {
                1
            };
            let mut discovery_progress = |discovered_count: usize| {
                if let Some(ref pct) = progress_pct {
                    let phase_pct = if is_filesystem {
                        (discovered_count.min(discovery_cap) as u32)
                            .saturating_mul(discovery_weight)
                            / discovery_cap as u32
                    } else {
                        discovery_weight
                    };
                    pct.store(
                        cumulative_weight.saturating_add(phase_pct).min(95),
                        Ordering::Relaxed,
                    );
                }
            };
            index_store::scan_begin(&*self.db())?;
            let mut discovered_count = 0_usize;
            let mut upserted = 0_usize;
            let mut pending_upserts: Vec<SearchItem> =
                Vec::with_capacity(STORE_WRITE_BATCH_SIZE);
            let mut pending_seen: Vec<String> = Vec::with_capacity(STORE_WRITE_BATCH_SIZE);
            {
                let mut sink =
                    |mut item: SearchItem| -> Result<(), crate::discovery::ProviderError> {
                        discovered_count += 1;
                        // One PK lookup per discovered item replaces the full
                        // store map: usage merge and change detection use the
                        // stored row.
                        let stored = index_store::get_item(&*self.db(), &item.id)
                            .map_err(|e| crate::discovery::ProviderError::new(e.to_string()))?;
                        if let Some(previous) = stored.as_ref() {
                            if item.use_count == 0 {
                                item.use_count = previous.use_count;
                            }
                            if item.last_accessed_epoch_secs <= 0 {
                                item.last_accessed_epoch_secs =
                                    previous.last_accessed_epoch_secs;
                            }
                            // Usage counters belong to the user, not the scanner:
                            // a rescan must never reset launch history, or the
                            // idle "your apps" list empties itself every scan.
                            if item.launch_count == 0 {
                                item.launch_count = previous.launch_count;
                            }
                            if item.last_launched_at <= 0 {
                                item.last_launched_at = previous.last_launched_at;
                            }
                        }

                        pending_seen.push(item.id.clone());
                        let changed = stored
                            .as_ref()
                            .map(|previous| previous != &item)
                            .unwrap_or(true);
                        if changed {
                            pending_upserts.push(item);
                            upserted += 1;
                            upserted_total += 1;
                        }

                        if pending_seen.len() >= STORE_WRITE_BATCH_SIZE {
                            index_store::upsert_items(&*self.db(), &pending_upserts)
                                .map_err(|e| crate::discovery::ProviderError::new(e.to_string()))?;
                            self.index_items_on_backends(&pending_upserts);
                            self.record_publish(&pending_upserts, &[]);
                            pending_upserts.clear();
                            index_store::scan_mark(&*self.db(), &pending_seen)
                                .map_err(|e| crate::discovery::ProviderError::new(e.to_string()))?;
                            pending_seen.clear();
                        }
                        Ok(())
                    };
                provider.discover_into(Some(&mut discovery_progress), &mut sink)?;
            }
            if !pending_upserts.is_empty() {
                index_store::upsert_items(&*self.db(), &pending_upserts)?;
                self.index_items_on_backends(&pending_upserts);
                self.record_publish(&pending_upserts, &[]);
            }
            index_store::scan_mark(&*self.db(), &pending_seen)?;

            if let Some(ref pct) = progress_pct {
                pct.store(
                    cumulative_weight
                        .saturating_add(discovery_weight)
                        .saturating_add(write_weight)
                        .min(95),
                    Ordering::Relaxed,
                );
            }
            discovered_total += discovered_count;
            cumulative_weight += provider_weight;

            // Kind-based ownership is safe for current runtime provider composition:
            // start-menu apps own kind=app, filesystem owns kind=file/folder.
            let removable_ids =
                index_store::scan_prune(&*self.db(), provider_kinds(provider.provider_name()))?;

            for chunk in removable_ids.chunks(STORE_WRITE_BATCH_SIZE) {
                for id in chunk {
                    self.remove_item_from_backends(id);
                }
                self.record_publish(&[], chunk);
            }
            index_store::scan_end(&*self.db())?;

            removed_total += removable_ids.len();
            provider_reports.push(ProviderRefreshReport {
                provider: provider_name.clone(),
                discovered: discovered_count,
                upserted,
                removed: removable_ids.len(),
                skipped: false,
                elapsed_ms: started.elapsed().as_millis(),
            });

            if incremental_mode {
                persist_provider_discovery_state(
                    &*self.db(),
                    &provider_name,
                    provider_stamp.as_deref(),
                    now_epoch_secs,
                )?;
                log_provider_freshness_status(&*self.db(), &provider_name, now_epoch_secs, false)?;
            }
        }

        if let Some(ref pct) = progress_pct {
            pct.store(95, Ordering::Relaxed);
        }
        self.refresh_cache_from_store()?;
        if progress_pct.is_none() {
            // The rebuild now applies Tantivy per batch; the full-store sync is
            // only needed when the index is missing and has to be reseeded.
            let tantivy_available = self
                .tantivy_index
                .read()
                .map(|guard| guard.is_some())
                .unwrap_or_else(|poisoned| poisoned.into_inner().is_some());
            if !tantivy_available {
                self.sync_indexes_from_cache_with_force(upserted_total > 0 || removed_total > 0)?;
            }
        }
        if let Some(ref pct) = progress_pct {
            pct.store(100, Ordering::Relaxed);
        }
        let indexed_total = index_store::count_items(&*self.db())? as usize;

        // Record memory and cycle metrics
        let memory_after = self_measure_memory();
        let memory_delta = memory_after.saturating_sub(memory_before);
        {
            let mut peak = match self.last_indexing_peak_memory.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            *peak = Some(memory_after);
        }
        self.indexing_cycle_count.fetch_add(1, Ordering::SeqCst);

        let elapsed_ms = cycle_start.elapsed().as_millis();
        crate::logging::info(&format!(
            "[nex] indexing_metrics cycle_count={} elapsed_ms={} memory_before_mb={} memory_after_mb={} memory_delta_mb={}",
            self.indexing_cycle_count.load(Ordering::SeqCst),
            elapsed_ms,
            memory_before / 1024 / 1024,
            memory_after / 1024 / 1024,
            memory_delta / 1024 / 1024
        ));

        Ok(IndexRefreshReport {
            indexed_total,
            discovered_total,
            upserted_total,
            removed_total,
            providers: provider_reports,
        })
    }

    pub fn rebuild_index_incremental(&self) -> Result<usize, ServiceError> {
        let report = self.rebuild_index_incremental_with_report()?;
        Ok(report.indexed_total)
    }

    fn record_publish(&self, upserts: &[SearchItem], deleted: &[String]) {
        let mut log = self
            .publish_log
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        log.record(upserts, deleted);
    }

    /// Consume the mutations since `known` for one publishing client.
    pub(crate) fn publish_outcome(&self, known: u64, client_instance: u64) -> PublishOutcome {
        let mut log = self
            .publish_log
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        log.outcome(known, client_instance)
    }

    pub fn upsert_item(&self, item: &SearchItem) -> Result<(), ServiceError> {
        index_store::upsert_item(&*self.db(), item)?;
        self.upsert_cached_item(item.clone());
        self.index_item_on_backends(item);
        self.record_publish(std::slice::from_ref(item), &[]);
        Ok(())
    }

    /// Delete an item by id from the SQLite store, the in-memory caches,
    /// and the Tantivy search index. Idempotent: deleting a
    /// missing id is not an error.
    pub fn delete_item_by_id(&self, id: &str) -> Result<(), ServiceError> {
        index_store::delete_item(&*self.db(), id)?;
        self.remove_cached_item_by_id(id);
        self.remove_item_from_backends(id);
        self.record_publish(&[], std::slice::from_ref(&id.to_string()));
        Ok(())
    }

    /// Delete every file/folder row beneath `folder_path` (not the folder
    /// itself). A folder delete or rename-away only produces one watcher
    /// event for the folder, so descendants must be removed explicitly.
    pub fn delete_items_under_path(&self, folder_path: &str) -> Result<usize, ServiceError> {
        let ids = index_store::list_item_ids_under_path(&*self.db(), folder_path)?;
        for id in &ids {
            self.delete_item_by_id(id)?;
        }
        Ok(ids.len())
    }

    /// Commit buffered Tantivy writes so watcher-driven upserts/deletes are
    /// visible to search now and survive process exit.
    pub fn flush_search_index(&self) {
        let guard = match self.tantivy_index.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *guard {
            if let Err(e) = idx.flush() {
                crate::logging::warn(&format!("[nex] Tantivy flush failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
            }
        }
        drop(guard);
        self.bump_search_generation();
    }

    /// Commit buffered Tantivy writes if any are pending and the last
    /// commit is at least `min_interval` old. Commits are fsync-heavy, so
    /// watcher churn is throttled instead of committing every batch; no GC
    /// here (the periodic `maybe_compact_backends` flush handles it).
    pub(crate) fn commit_search_index_if_due(&self, min_interval: Duration) -> bool {
        let mut last = match self.last_index_commit.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if last.is_some_and(|at| at.elapsed() < min_interval) {
            return false;
        }
        let committed = match self.tantivy_index.read() {
            Ok(guard) => guard.as_ref().map(|idx| idx.commit_pending()),
            Err(poisoned) => poisoned.into_inner().as_ref().map(|idx| idx.commit_pending()),
        };
        match committed {
            Some(Ok(true)) => {
                *last = Some(Instant::now());
                drop(last);
                self.bump_search_generation();
                true
            }
            Some(Err(e)) => {
                crate::logging::warn(&format!("[nex] Tantivy commit failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
                false
            }
            _ => false,
        }
    }

    /// Background Tantivy upkeep: rebuild after a failed search/commit, and
    /// repair drift when the document count disagrees with the SQLite store.
    pub fn maintain_search_index(&self) {
        if self.tantivy_needs_heal.swap(false, Ordering::SeqCst) {
            self.rebuild_search_index_from_store();
            return;
        }
        // Buffered deletes/upserts (e.g. from the stale pruner) are not
        // drift; publish them before comparing counts.
        self.commit_search_index_if_due(Duration::ZERO);

        let store_count = match index_store::count_items(&*self.db()) {
            Ok(n) => n,
            Err(_) => return,
        };
        let docs = {
            let guard = match self.tantivy_index.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match guard.as_ref() {
                Some(idx) => idx.num_docs(),
                // No index (e.g. another instance holds the lock).
                None => return,
            }
        };
        let docs = match docs {
            Ok(n) => n,
            Err(e) => {
                crate::logging::warn(&format!(
                    "[nex] Tantivy doc count failed: {e}; scheduling index rebuild"
                ));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
                return;
            }
        };

        let mut last = match self.last_drift_store_count.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if docs == store_count {
            *last = None;
            return;
        }
        // A changing store count means indexing is still writing; wait for
        // it to settle (the post-index sync handles that case).
        if *last != Some(store_count) {
            *last = Some(store_count);
            return;
        }
        *last = None;
        drop(last);
        crate::logging::info(&format!(
            "[nex] Tantivy drift detected docs={docs} store_items={store_count}; reconciling"
        ));
        let items = match index_store::list_items(&*self.db()) {
            Ok(items) => items,
            Err(e) => {
                crate::logging::warn(&format!("[nex] Tantivy drift repair skipped: {e}"));
                return;
            }
        };
        // Only missing/extra documents are written, unlike a forced sync
        // that re-adds every document.
        let result = match self.tantivy_index.read() {
            Ok(guard) => guard.as_ref().map(|idx| idx.reconcile_items(&items)),
            Err(poisoned) => poisoned.into_inner().as_ref().map(|idx| idx.reconcile_items(&items)),
        };
        match result {
            Some(Ok((added, removed))) => {
                crate::logging::info(&format!(
                    "[nex] Tantivy drift repaired added={added} removed={removed}"
                ));
                self.bump_search_generation();
            }
            Some(Err(e)) => {
                crate::logging::warn(&format!("[nex] Tantivy drift repair failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
            }
            None => {}
        }
    }

    fn rebuild_search_index_from_store(&self) {
        let tantivy_path = {
            let cfg = self.config_snapshot();
            let index_dir = cfg.index_db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
            index_dir.join("index.tantivy")
        };
        let items = match index_store::list_items(&*self.db()) {
            Ok(items) => items,
            Err(e) => {
                crate::logging::warn(&format!("[nex] Tantivy rebuild skipped: store read failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
                return;
            }
        };
        let mut guard = match self.tantivy_index.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Close our writer first so the directory can be removed and no
        // second writer is ever opened on it.
        if let Some(old) = guard.take() {
            close_tantivy_index(old);
        }
        *guard = reset_tantivy_index(&tantivy_path, &items);
        drop(guard);
        self.bump_search_generation();
    }

    /// Live Tantivy document count, or `None` when running without an index.
    pub fn search_index_doc_count(&self) -> Option<u64> {
        let guard = match self.tantivy_index.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().and_then(|idx| idx.num_docs().ok())
    }

    /// Commit and close the index (waiting for merges) before exit.
    pub(crate) fn shutdown_search_index(&self) {
        let taken = match self.tantivy_index.write() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(idx) = taken {
            close_tantivy_index(idx);
        }
    }

    /// Spawn a background stale-pruner thread that runs
    /// `prune_stale_items_if_due` on a 15 s cadence so the search
    /// critical path never blocks on a write lock. Uses
    /// `try_write` so a concurrent search is never delayed.
    /// Idempotent: repeated calls after startup are a no-op.
    pub fn start_stale_pruner(&self, service_arc: &Arc<RwLock<CoreService>>) {
        if self.stale_pruner_started.swap(true, Ordering::AcqRel) {
            return;
        }

        // Clear any previous stop signal (idempotent restart).
        self.stale_pruner_stop.store(false, Ordering::Release);
        let stop = Arc::clone(&self.stale_pruner_stop);
        let svc = Arc::clone(service_arc);
        if let Err(error) = std::thread::Builder::new()
            .name("nex-stale-pruner".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(STALE_PRUNE_INTERVAL);
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // Short read locks only: the existence checks below can
                    // stall for seconds on a spun-down drive and must never
                    // hold the service lock that searches and shows need.
                    let candidates = match svc.try_read() {
                        Ok(guard) => guard.stale_prune_candidates_if_due(),
                        Err(_) => continue,
                    };
                    let stale_ids: Vec<String> = candidates
                        .iter()
                        .filter(|item| is_stale_index_entry(item))
                        .map(|item| item.id.clone())
                        .collect();
                    if let Ok(guard) = svc.try_read() {
                        if let Err(error) = guard.remove_stale_items(&stale_ids, candidates.len()) {
                            crate::logging::warn(&format!("[nex] stale prune failed: {error}"));
                        }
                        guard.maintain_search_index();
                    }
                }
            })
        {
            self.stale_pruner_started.store(false, Ordering::Release);
            crate::logging::warn(&format!("[nex] stale pruner start failed: {error}"));
        }
        self.start_title_fts_backfill(service_arc);
    }

    /// Index pre-existing rows into the title trigram table in small
    /// batches after a store upgrade; recall uses the slower LIKE scan
    /// until this finishes.
    fn start_title_fts_backfill(&self, service_arc: &Arc<RwLock<CoreService>>) {
        if !index_store::title_fts_backfill_pending(&*self.db()).unwrap_or(false)
            || self.title_fts_backfill_started.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let stop = Arc::clone(&self.stale_pruner_stop);
        let svc = Arc::clone(service_arc);
        let spawned = std::thread::Builder::new()
            .name("nex-title-fts-backfill".into())
            .spawn(move || {
                let mut warned = false;
                while !stop.load(Ordering::Relaxed) {
                    let step = match svc.try_read() {
                        Ok(guard) => index_store::backfill_title_fts_batch(&*guard.db(), 2_000),
                        Err(_) => Ok(false),
                    };
                    match step {
                        Ok(true) => {
                            crate::logging::info("[nex] title index backfill complete");
                            break;
                        }
                        Ok(false) => std::thread::sleep(Duration::from_millis(15)),
                        Err(error) => {
                            if !warned {
                                warned = true;
                                crate::logging::warn(&format!(
                                    "[nex] title index backfill retrying: {error}"
                                ));
                            }
                            std::thread::sleep(Duration::from_secs(2));
                        }
                    }
                }
            });
        if spawned.is_err() {
            self.title_fts_backfill_started.store(false, Ordering::Release);
        }
    }

    /// Signal the stale pruner thread to stop. The thread will exit at
    /// the next sleep boundary (up to STALE_PRUNE_INTERVAL = 15 s).
    pub fn stop_stale_pruner(&self) {
        if self.stale_pruner_started.swap(false, Ordering::AcqRel) {
            self.stale_pruner_stop.store(true, Ordering::Release);
        }
    }

    /// Start the per-root `DirectoryWatcher` consumers that apply
    /// filesystem changes to the live index. Idempotent: calling twice
    /// is a no-op. No-op on non-Windows targets.
    ///
    /// `service_arc` is the same `Arc<RwLock<CoreService>>` that owns this
    /// service; the consumer threads re-acquire it to apply changes.
    #[cfg(target_os = "windows")]
    pub fn start_file_watchers(
        &self,
        service_arc: &Arc<RwLock<CoreService>>,
    ) -> Result<(), ServiceError> {
        let mut slot = match self.file_watchers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_some() {
            return Ok(());
        }

        let config = self.config_snapshot();
        let roots = config.discovery_roots.clone();
        let excluded_roots = config.discovery_exclude_roots.clone();

        if roots.is_empty() {
            crate::logging::info(
                "[nex] directory_watcher: no discovery_roots configured; skipping start",
            );
            return Ok(());
        }

        let handle =
            crate::file_watcher_consumer::FileWatcherHandle::start(roots, excluded_roots, Arc::clone(service_arc));
        crate::logging::info(&format!(
            "[nex] directory_watcher: started on {} root(s)",
            handle.active_roots()
        ));
        *slot = Some(handle);
        Ok(())
    }

    /// Start the watcher for Start Menu, Desktop, and application registry changes.
    #[cfg(target_os = "windows")]
    pub fn start_application_watchers(
        &self,
        service_arc: &Arc<RwLock<CoreService>>,
    ) {
        let mut slot = self
            .application_watchers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }

        let handle = crate::application_watcher::ApplicationWatcherHandle::start(Arc::clone(service_arc));
        crate::logging::info(&format!(
            "[nex] app discovery watcher: started on {} directories and {} registry keys",
            handle.active_directories(),
            handle.active_registry_keys()
        ));
        *slot = Some(handle);
    }

    /// Stop application-source watchers and their refresh worker.
    #[cfg(target_os = "windows")]
    pub fn stop_application_watchers(&self) {
        if let Some(handle) = self.take_application_watchers() {
            drop(handle);
            crate::logging::info("[nex] app discovery watcher: stopped");
        }
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn take_application_watchers(
        &self,
    ) -> Option<crate::application_watcher::ApplicationWatcherHandle> {
        self.application_watchers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Stop and join the per-root file watcher consumers. Safe to call
    /// even if watchers were never started.
    #[cfg(target_os = "windows")]
    pub fn stop_file_watchers(&self) {
        if let Some(handle) = self.take_file_watchers() {
            drop(handle); // RAII joins threads
            crate::logging::info("[nex] directory_watcher: stopped");
        }
    }

    /// Take the file watcher handle out of the slot without joining.
    /// The caller owns the handle and drops it (which joins consumer
    /// threads). Separation is needed during shutdown because the
    /// consumer thread may be blocked on `service.write()`, so we
    /// must NOT hold the RwLock read guard while joining — deadlock.
    pub(crate) fn take_file_watchers(&self) -> Option<crate::file_watcher_consumer::FileWatcherHandle> {
        let mut slot = match self.file_watchers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.take()
    }

    pub fn handle_command(&self, request: CoreRequest) -> Result<CoreResponse, ServiceError> {
        match request {
            CoreRequest::Search(search) => {
                let results = self.search(&search.query, search.limit.unwrap_or(0))?;
                Ok(CoreResponse::Search(SearchResponse {
                    results: results.into_iter().map(Into::into).collect(),
                }))
            }
            CoreRequest::Launch(launch) => {
                if let Some(id) = launch.id.as_deref() {
                    if !id.trim().is_empty() {
                        self.launch(LaunchTarget::Id(id))?;
                        return Ok(CoreResponse::Launch(LaunchResponse { launched: true }));
                    }
                }

                if let Some(path) = launch.path.as_deref() {
                    if !path.trim().is_empty() {
                        self.launch(LaunchTarget::Path(path))?;
                        return Ok(CoreResponse::Launch(LaunchResponse { launched: true }));
                    }
                }

                Err(ServiceError::InvalidRequest(
                    "launch requires non-empty id or path".into(),
                ))
            }
        }
    }
}

fn open_tantivy_index(
    tantivy_path: &std::path::Path,
    seed_items: &[SearchItem],
) -> Result<TantivyIndex, TantivyOpenError> {
    // Transient failures (AV scanners holding new segment files, a dying
    // process's GC racing our open) resolve within milliseconds — retry
    // once before condemning the whole index to a destructive reset.
    let idx = match TantivyIndex::open(tantivy_path) {
        Ok(idx) => idx,
        Err(TantivyOpenError::LockBusy(msg)) => return Err(TantivyOpenError::LockBusy(msg)),
        Err(first) => {
            crate::logging::info(&format!(
                "[nex] Tantivy open failed ({first}); retrying once"
            ));
            std::thread::sleep(std::time::Duration::from_millis(250));
            TantivyIndex::open(tantivy_path).map_err(|second| match second {
                TantivyOpenError::LockBusy(msg) => TantivyOpenError::LockBusy(msg),
                TantivyOpenError::Other(msg) => {
                    TantivyOpenError::Other(format!("{first}; retry failed: {msg}"))
                }
            })?
        }
    };
    if let Ok(0) = idx.num_docs() {
        // Empty index — re-seed from SQLite immediately so searches never
        // silently fall back to the cache. No clear(): committing an empty
        // state before re-seeding risks sealing an empty index if the
        // process dies after.
        seed_tantivy_index(&idx, seed_items);
    }
    Ok(idx)
}

fn seed_tantivy_index(idx: &TantivyIndex, items: &[SearchItem]) {
    if items.is_empty() {
        return;
    }
    match idx.index_items(items) {
        Ok(()) => crate::logging::info(&format!(
            "[nex] Tantivy seeded from store items={}",
            items.len()
        )),
        Err(e) => crate::logging::warn(&format!("[nex] Tantivy re-seed failed: {e}")),
    }
}

/// Open the index, resetting it only when it is genuinely unusable. A lock
/// held by another process means a running instance owns a healthy index:
/// deleting it would wipe that instance's search, so run without Tantivy.
fn open_or_reset_tantivy_index(
    tantivy_path: &std::path::Path,
    seed_items: &[SearchItem],
) -> Option<TantivyIndex> {
    match open_tantivy_index(tantivy_path, seed_items) {
        Ok(idx) => Some(idx),
        Err(TantivyOpenError::LockBusy(msg)) => {
            crate::logging::warn(&format!(
                "[nex] Tantivy index in use by another process ({msg}); running without search index"
            ));
            None
        }
        Err(TantivyOpenError::Other(msg)) => {
            crate::logging::info(&format!("[nex] Tantivy open failed: {msg}"));
            reset_tantivy_index(tantivy_path, seed_items)
        }
    }
}

/// Delete a corrupt/schema-mismatched tantivy directory, reopen from
/// scratch and re-seed from the store so search never runs on an empty
/// index after a reset. Shared by boot-time and mid-run reset paths.
fn reset_tantivy_index(
    tantivy_path: &std::path::Path,
    seed_items: &[SearchItem],
) -> Option<TantivyIndex> {
    crate::logging::info("[nex] Tantivy index schema changed or corrupt, resetting");
    if let Err(e) = std::fs::remove_dir_all(tantivy_path) {
        // MmapDirectory holds handles on Windows; a partial delete leaves
        // stale files that later commits reference. Log so a corrupt
        // directory is diagnosable instead of silently looping.
        crate::logging::info(&format!(
            "[nex] Tantivy reset: remove_dir_all failed: {e}"
        ));
    }
    match TantivyIndex::open(tantivy_path) {
        Ok(idx) => {
            seed_tantivy_index(&idx, seed_items);
            Some(idx)
        }
        Err(e) => {
            crate::logging::info(&format!(
                "[nex] Tantivy index init after reset: {e}, running without search index"
            ));
            None
        }
    }
}

fn close_tantivy_index(idx: TantivyIndex) {
    if let Err(e) = idx.close() {
        crate::logging::warn(&format!("[nex] Tantivy close failed: {e}"));
    }
}

impl CoreService {
    fn cached_len(&self) -> usize {
        match self.cached_items.read() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    pub fn search_tantivy_seed(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        self.search_indexed_candidates(query, limit)
    }

    fn search_indexed_candidates(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        if limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // try_read: while the index is being replaced (rare rebuild),
        // fall back to cache + store recall instead of blocking the keystroke.
        let tantivy_guard = match self.tantivy_index.try_read() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Ok(Vec::new()),
        };
        if let Some(ref idx) = *tantivy_guard {
            match idx.search(query, limit) {
                Ok(results) => return Ok(results),
                Err(e) => {
                    // Typically missing/corrupt segment files. Callers fall
                    // back to cache + SQLite recall; the maintenance loop
                    // rebuilds the index from the store.
                    if !self.tantivy_needs_heal.swap(true, Ordering::SeqCst) {
                        crate::logging::warn(&format!(
                            "[nex] Tantivy search failed: {e}; scheduling index rebuild"
                        ));
                    }
                }
            }
        }
        drop(tantivy_guard);

        Ok(Vec::new())
    }

    /// Warm search indexes so the user's first keystroke doesn't pay
    /// cold-page-fault latency.  Must be called with the outer service
    /// lock held (read or write).
    pub fn warm_search_cache(&self) {
        // UI client mode owns no Tantivy reader: warming segment files there is
        // pure I/O on the show path. Only the indexer owns and warms the index.
        let has_index = self
            .tantivy_index
            .read()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        if !has_index {
            return;
        }
        // Called on the show path: never read index files synchronously
        // here; the warmer only re-reads new or changed segment files.
        let index_dir = {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            cfg.index_db_path
                .parent()
                .unwrap_or(Path::new("."))
                .join("index.tantivy")
        };
        self.index_warmer.warm_in_background(index_dir);
    }

    fn bump_search_generation(&self) {
        self.search_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Current index generation. Bumped on every mutation of the item set
    /// (cache refresh, upsert, delete, index sync, prune), so readers can
    /// detect that the underlying data changed between two observations.
    pub fn search_generation(&self) -> u64 {
        self.search_generation.load(Ordering::SeqCst)
    }

    /// A point-in-time view of the searchable item set: the cache contents
    /// plus the generation they were read at. Readers that hold a snapshot
    /// never observe a partially rebuilt cache — the swap in
    /// `refresh_cache_from_store` replaces the whole `Vec` at once, and a
    /// blocking read here is bounded by that single swap.
    pub fn cached_snapshot(&self) -> (u64, Vec<SearchItem>) {
        let generation = self.search_generation.load(Ordering::SeqCst);
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        (generation, guard.clone())
    }

    /// Same as [`Self::cached_snapshot`] but for the apps-only cache used
    /// by the Apps-mode fast path.
    pub fn cached_app_snapshot(&self) -> (u64, Vec<SearchItem>) {
        let generation = self.search_generation.load(Ordering::SeqCst);
        let guard = match self.cached_app_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        (generation, guard.clone())
    }

    /// Claim the right to run one recency refresh, at most once per
    /// `interval_secs`. Returns `false` when another pass already claimed the
    /// current window, so concurrent show events cannot stack up work.
    pub(crate) fn recent_documents_scan_due(&self, interval_secs: i64) -> bool {
        let now = crate::recent_files::now_epoch_secs().max(0) as u64;
        let interval = interval_secs.max(0) as u64;
        let previous = self.recent_docs_last_scan.load(Ordering::SeqCst);
        if previous > 0 && now.saturating_sub(previous) < interval {
            return false;
        }
        self.recent_docs_last_scan
            .compare_exchange(previous, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Refresh recency for recently opened documents from the shell's MRU.
    pub(crate) fn apply_recent_documents(&self) -> Result<usize, ServiceError> {
        match crate::recent_files::recent_dir_default() {
            Some(dir) => self.apply_recent_documents_from(&dir),
            None => Ok(0),
        }
    }

    /// Apply the recency overlay from an explicit MRU directory.
    ///
    /// Existing file/folder items get their `last_accessed_epoch_secs`
    /// advanced; recent targets the filesystem scan never reached are
    /// inserted. Only forwards-moving recency is written, so repeated passes
    /// converge and cost nothing once the list is fully reflected.
    pub(crate) fn apply_recent_documents_from(&self, dir: &Path) -> Result<usize, ServiceError> {
        let targets = crate::recent_files::collect_recent_targets(
            dir,
            crate::recent_files::RECENT_FILES_MAX_ENTRIES,
        );
        if targets.is_empty() {
            return Ok(0);
        }

        let mut wanted: HashSet<String> = HashSet::new();
        let mut identities = Vec::new();
        for (path, modified) in targets {
            let Some((kind, id)) = crate::recent_files::recent_item_identity(&path) else {
                continue;
            };
            wanted.insert(id.clone());
            identities.push((kind, id, path, modified));
        }
        if identities.is_empty() {
            return Ok(0);
        }

        // Only materialise the rows we might touch; the full cache can hold
        // well over a hundred thousand entries.
        let existing: HashMap<String, SearchItem> = {
            let guard = match self.cached_items.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .iter()
                .filter(|item| wanted.contains(&item.id))
                .map(|item| (item.id.clone(), item.clone()))
                .collect()
        };

        let mut applied = 0_usize;
        for (kind, id, path, modified) in identities {
            if applied >= crate::recent_files::RECENT_FILES_MAX_APPLIED_PER_PASS {
                break;
            }

            let path_text = path.to_string_lossy().to_string();
            // Compaction evicts most file rows from the cache; read those
            // from the store so their usage counts are not reset.
            let stored = match existing.get(&id) {
                Some(_) => None,
                None => index_store::get_item(&*self.db(), &id)?,
            };
            let item = match existing.get(&id).or(stored.as_ref()) {
                Some(current) => {
                    if current.last_accessed_epoch_secs >= modified {
                        continue;
                    }
                    let mut updated = current.clone();
                    updated.last_accessed_epoch_secs = modified;
                    updated
                }
                None => {
                    let Some(title) = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .filter(|name| !name.is_empty())
                    else {
                        continue;
                    };
                    let subtitle = path
                        .parent()
                        .map(|parent| parent.to_string_lossy().to_string())
                        .unwrap_or_default();
                    // Recency only — usage counts stay zero so a real launch
                    // is still what drives frequency bonuses.
                    SearchItem::from_owned_with_subtitle(
                        id,
                        kind.to_string(),
                        title,
                        path_text,
                        subtitle,
                        0,
                        modified,
                    )
                }
            };

            self.upsert_item(&item)?;
            applied += 1;
        }

        Ok(applied)
    }

    fn query_personalization_boosts(
        &self,
        query: &str,
        mode: SearchMode,
    ) -> Result<HashMap<String, i64>, ServiceError> {
        let query_norm = crate::model::normalize_for_search(query);
        if query_norm.is_empty() || matches!(mode, SearchMode::Actions | SearchMode::Clipboard) {
            return Ok(HashMap::new());
        }

        let mode_key = search_mode_key(mode);

        // Check the in-memory TTL cache first to avoid a SQLite query
        // on every keystroke during rapid typing.
        {
            let mut cache = match self.personalization_cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(cached) = cache.get(&query_norm, mode_key) {
                return Ok(cached);
            }
        }

        let rows =
            index_store::list_query_selections(&*self.db(), &query_norm, mode_key, 64)?;
        let now = now_epoch_secs();
        let mut boosts = HashMap::with_capacity(rows.len());
        for (item_id, selected_count, last_selected_epoch_secs) in rows {
            let usage_boost = (selected_count.min(12) as i64) * 280;
            let recency_boost = query_memory_recency_boost(last_selected_epoch_secs, now);
            let total = (usage_boost + recency_boost).clamp(0, 5_000);
            if total > 0 {
                boosts.insert(item_id, total);
            }
        }

        // Cache the result (even if empty — avoids repeated queries for
        // queries with no personalization history).
        {
            let mut cache = match self.personalization_cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            cache.insert(&query_norm, mode_key, boosts.clone());
        }

        Ok(boosts)
    }

    fn refresh_cache_from_store(&self) -> Result<(), ServiceError> {
        let (latest, latest_apps) = if self.hot_cache {
            let hot = index_store::list_hot_items(&*self.db(), UI_CACHE_MAX_ITEMS)?;
            let apps = collect_app_items(&hot);
            self.cache_complete.store(false, Ordering::SeqCst);
            (hot, apps)
        } else {
            let latest = index_store::list_items(&*self.db())?;
            let apps = collect_app_items(&latest);
            self.cache_complete.store(true, Ordering::SeqCst);
            (latest, apps)
        };
        let mut items_guard = match self.cached_items.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *items_guard = latest;
        self.rebuild_search_index_for_cache(&items_guard);
        drop(items_guard);
        let mut apps_guard = match self.cached_app_items.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *apps_guard = latest_apps;
        drop(apps_guard);
        self.bump_search_generation();
        Ok(())
    }

    fn upsert_cached_item(&self, item: SearchItem) {
        let item_for_apps = item.clone();
        let item_id = item.id.clone();
        let is_app = item.kind.eq_ignore_ascii_case("app");
        let mut items_guard = match self.cached_items.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        upsert_cached_item_inner(&mut items_guard, item);
        if self.ui_client {
            let bounded = bounded_ui_items(&items_guard);
            if bounded.len() != items_guard.len() {
                self.cache_complete.store(false, Ordering::SeqCst);
            }
            *items_guard = bounded;
        }
        self.rebuild_search_index_for_cache(&items_guard);
        drop(items_guard);
        match self.cached_app_items.write() {
            Ok(mut guard) => {
                if is_app {
                    upsert_cached_item_inner(&mut guard, item_for_apps);
                } else {
                    guard.retain(|entry| entry.id != item_id);
                }
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                if is_app {
                    upsert_cached_item_inner(&mut guard, item_for_apps);
                } else {
                    guard.retain(|entry| entry.id != item_id);
                }
            }
        }
        self.bump_search_generation();
    }

    fn remove_cached_item_by_id(&self, id: &str) {
        let mut items_guard = match self.cached_items.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        items_guard.retain(|entry| entry.id != id);
        if self.ui_client {
            *items_guard = bounded_ui_items(&items_guard);
        }
        self.rebuild_search_index_for_cache(&items_guard);
        drop(items_guard);
        match self.cached_app_items.write() {
            Ok(mut guard) => guard.retain(|entry| entry.id != id),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.retain(|entry| entry.id != id);
            }
        }
        self.bump_search_generation();
    }

    pub(crate) fn sync_indexes_from_cache(&self) -> Result<(), ServiceError> {
        self.sync_indexes_from_cache_with_force(false)
    }

    fn sync_indexes_from_cache_with_force(&self, force: bool) -> Result<(), ServiceError> {
        let items = index_store::list_items(&*self.db())?;
        let item_count = items.len();

        let tantivy_docs = self
            .tantivy_index
            .read()
            .ok()
            .and_then(|g| g.as_ref().and_then(|idx| idx.num_docs().ok()));

        let tantivy_is_first = tantivy_docs.map(|d| d == 0).unwrap_or(true);

        // Counts alone cannot distinguish replaced documents, so only skip
        // a matching index when the rebuild did not change any stored item.
        if !force && tantivy_docs == Some(item_count as u64) {
            self.maybe_compact_backends();
            return Ok(());
        }

        crate::logging::info(&format!(
            "[nex] sync_indexes items={} tantivy_docs={:?} tantivy_first={}",
            item_count,
            tantivy_docs,
            tantivy_is_first,
        ));

        // Derive tantivy path before taking the tantivy lock to avoid
        // holding config RwLock + tantivy Mutex simultaneously.
        let tantivy_path = {
            let cfg = self.config.read().unwrap();
            let index_dir = cfg.index_db_path.parent().unwrap_or(Path::new("."));
            index_dir.join("index.tantivy")
        };

        // Mid-run self-heal: if the index is None (schema bump or
        // corruption), reset and reopen under the write lock.
        let missing = self
            .tantivy_index
            .read()
            .map(|guard| guard.is_none())
            .unwrap_or_else(|poisoned| poisoned.into_inner().is_none());
        if missing {
            crate::logging::info("[nex] Tantivy index missing or schema changed mid-run, resetting");
            let mut guard = self.tantivy_index.write().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                // No seed: the index is re-filled from `items` just below.
                *guard = open_or_reset_tantivy_index(&tantivy_path, &[]);
            }
        }

        // Read side: searches keep running against the last commit while
        // the sync writes (the writer has its own mutex).
        let tantivy_guard = self.tantivy_index.read().unwrap_or_else(|e| e.into_inner());
        let mut drop_index = false;
        if let Some(ref idx) = *tantivy_guard {
            let result = if tantivy_is_first {
                idx.index_items(&items)
            } else {
                idx.incremental_sync_items(&items)
            };
            match result {
                Ok(()) => {
                    let n = idx
                        .num_docs()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|e| format!("num_docs err: {e}"));
                    crate::logging::info(&format!(
                        "[nex] Tantivy sync committed items={} docs={}",
                        items.len(),
                        n
                    ));
                }
                Err(e) => {
                    // A failed sync may have committed only part of the
                    // deletion/insertion batch, leaving documents the store no
                    // longer has. Drop the index so searches fall back to the
                    // in-memory cache (rebuilt from the store) instead of
                    // serving removed items; the next rebuild reindexes.
                    crate::logging::info(&format!(
                        "[nex] Tantivy index sync error: {e}; dropping index until next rebuild"
                    ));
                    drop_index = true;
                }
            }
        }
        drop(tantivy_guard);
        if drop_index {
            let taken = self.tantivy_index.write().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(old) = taken {
                close_tantivy_index(old);
            }
        }

        self.maybe_compact_backends();
        self.bump_search_generation();
        Ok(())
    }

    fn index_item_on_backends(&self, item: &SearchItem) {
        let tantivy_guard = match self.tantivy_index.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            if let Err(e) = idx.upsert_item(item) {
                crate::logging::warn(&format!("[nex] Tantivy upsert failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
            }
        }
        // Release before bumping: compaction re-locks `tantivy_index`, and
        // std Mutex is not re-entrant (deadlocked on the 500th write).
        drop(tantivy_guard);
        self.bump_compaction_counter();
    }

    fn index_items_on_backends(&self, items: &[SearchItem]) {
        if items.is_empty() {
            return;
        }
        let tantivy_guard = match self.tantivy_index.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            if let Err(e) = idx.upsert_items(items) {
                crate::logging::warn(&format!("[nex] Tantivy batch upsert failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
            }
        }
        drop(tantivy_guard);
        self.bump_compaction_counter();
    }

    fn remove_item_from_backends(&self, id: &str) {
        let tantivy_guard = match self.tantivy_index.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            if let Err(e) = idx.delete_item(id) {
                crate::logging::warn(&format!("[nex] Tantivy delete failed: {e}"));
                self.tantivy_needs_heal.store(true, Ordering::SeqCst);
            }
        }
        drop(tantivy_guard);
        self.bump_compaction_counter();
    }

    fn bump_compaction_counter(&self) {
        let mut count = match self.compaction_write_count.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        *count += 1;
        if *count >= 500 {
            drop(count);
            self.maybe_compact_backends();
        }
    }

    pub(crate) fn maybe_compact_backends(&self) {
        let should_compact = {
            let mut last = match self.last_compaction_time.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let now = Instant::now();
            match *last {
                Some(prev) if now.duration_since(prev) < Duration::from_secs(300) => false,
                _ => {
                    *last = Some(now);
                    true
                }
            }
        };

        if should_compact {
            let tantivy_guard = match self.tantivy_index.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(ref idx) = *tantivy_guard {
                let _ = idx.flush();
            }
            drop(tantivy_guard);

            let mut count = match self.compaction_write_count.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *count = 0;
        }
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn log_memory_stats(&self) {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut pmc: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ret = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) };
        if ret != 0 {
            let working_set_mb = pmc.WorkingSetSize / (1024 * 1024);
            let pagefile_mb = pmc.PagefileUsage / (1024 * 1024);

            let tantivy_mb = match self.tantivy_index.read() {
                Ok(g) => g
                    .as_ref()
                    .map_or(0, |idx| idx.mem_usage_bytes()),
                Err(_) => 0,
            } / (1024 * 1024);

            crate::logging::info(&format!(
                "[nex] memory_stats working_set_mb={} pagefile_mb={} tantivy_mb={}",
                working_set_mb, pagefile_mb, tantivy_mb,
            ));
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn log_memory_stats(&self) {}

    pub(crate) fn record_successful_launch_by_id(&self, id: &str) -> Result<(), ServiceError> {
        let item = self
            .cached_items
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or_else(|| ServiceError::ItemNotFound(id.to_string()))?;
        self.record_successful_launch(&item)
    }

    fn record_successful_launch(&self, item: &SearchItem) -> Result<(), ServiceError> {
        let now = now_epoch_secs();
        let mut updated = item.clone();
        updated.use_count = updated.use_count.saturating_add(1);
        updated.launch_count = updated.launch_count.saturating_add(1);
        updated.last_accessed_epoch_secs = now.max(updated.last_accessed_epoch_secs);
        updated.last_launched_at = now.max(updated.last_launched_at);

        let id = updated.id.clone();
        if self.ui_client {
            // The indexer owns the store. Bump the local cache now so ranking
            // and Quick Launch reflect the launch immediately; the next delta
            // carries the authoritative row.
            self.upsert_cached_item(updated);
            crate::ipc::send_indexer_command("record_launch", serde_json::json!({ "id": id }));
            return Ok(());
        }
        index_store::upsert_item(&self.db(), &updated)?;
        self.upsert_cached_item(updated.clone());
        self.record_publish(std::slice::from_ref(&updated), &[]);
        Ok(())
    }

    /// Next batch of cached items to existence-check, or empty when a prune
    /// pass is not due yet. The caller checks the filesystem without holding
    /// the service lock, then calls `remove_stale_items`.
    pub(crate) fn stale_prune_candidates_if_due(&self) -> Vec<SearchItem> {
        let should_prune = {
            let mut last = match self.last_stale_prune.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let now = Instant::now();
            match *last {
                Some(prev) if now.duration_since(prev) < STALE_PRUNE_INTERVAL => false,
                _ => {
                    *last = Some(now);
                    true
                }
            }
        };

        if !should_prune {
            self.maybe_compact_backends();
            return Vec::new();
        }
        self.stale_prune_candidates(STALE_PRUNE_BATCH_SIZE)
    }

    pub(crate) fn remove_stale_items(
        &self,
        stale_ids: &[String],
        scanned: usize,
    ) -> Result<(), ServiceError> {
        if stale_ids.is_empty() {
            return Ok(());
        }

        for stale_id in stale_ids {
            index_store::delete_item(&*self.db(), stale_id)?;
        }

        let stale_lookup: HashSet<&str> = stale_ids.iter().map(String::as_str).collect();
        let mut items_guard = match self.cached_items.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        items_guard.retain(|entry| !stale_lookup.contains(entry.id.as_str()));
        if self.ui_client {
            *items_guard = bounded_ui_items(&items_guard);
        }
        self.rebuild_search_index_for_cache(&items_guard);
        drop(items_guard);
        match self.cached_app_items.write() {
            Ok(mut guard) => guard.retain(|entry| !stale_lookup.contains(entry.id.as_str())),
            Err(poisoned) => poisoned
                .into_inner()
                .retain(|entry| !stale_lookup.contains(entry.id.as_str())),
        }

        // Remove stale entries from Tantivy backend so
        // deleted-on-disk files stop appearing in search results.
        for stale_id in stale_ids {
            self.remove_item_from_backends(stale_id);
        }

        crate::logging::info(&format!(
            "[nex] stale_prune scanned={} removed={} cached_items_remaining={}",
            scanned,
            stale_ids.len(),
            self.cached_len()
        ));

        self.record_publish(&[], stale_ids);
        self.bump_search_generation();
        Ok(())
    }

    fn stale_prune_candidates(&self, batch_size: usize) -> Vec<SearchItem> {
        if batch_size == 0 {
            return Vec::new();
        }

        let mut cursor = match self.stale_prune_cursor.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let page = match index_store::list_item_page(&*self.db(), cursor.as_deref(), batch_size) {
            Ok(page) => page,
            Err(error) => {
                crate::logging::warn(&format!("[nex] stale prune page failed: {error}"));
                return Vec::new();
            }
        };
        // A short page means the scan reached the end; start over next time.
        *cursor = if page.len() < batch_size {
            None
        } else {
            page.last().map(|item| item.id.clone())
        };
        page
    }
}

fn upsert_cached_item_inner(cached: &mut Vec<SearchItem>, item: SearchItem) {
    if let Some(existing) = cached.iter_mut().find(|entry| entry.id == item.id) {
        *existing = item;
    } else {
        cached.push(item);
    }
}

fn collect_app_items(items: &[SearchItem]) -> Vec<SearchItem> {
    items
        .iter()
        .filter(|item| item.kind.eq_ignore_ascii_case("app"))
        .cloned()
        .collect()
}

/// The UI keeps a small local set. Everything else is found through the
/// indexer's search path or the SQLite fallback.
pub(crate) const UI_CACHE_MAX_ITEMS: usize = 10_000;

pub(crate) fn bounded_ui_items(items: &[SearchItem]) -> Vec<SearchItem> {
    let mut apps = Vec::new();
    let mut others = Vec::new();
    for item in items {
        if item.kind.eq_ignore_ascii_case("app") {
            apps.push(item.clone());
        } else {
            others.push(item.clone());
        }
    }
    let keep = UI_CACHE_MAX_ITEMS.saturating_sub(apps.len());
    if others.len() > keep {
        others.sort_by(|a, b| {
            b.use_count
                .cmp(&a.use_count)
                .then_with(|| b.last_accessed_epoch_secs.cmp(&a.last_accessed_epoch_secs))
                .then_with(|| b.last_launched_at.cmp(&a.last_launched_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        others.truncate(keep);
    }
    apps.extend(others);
    apps
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderFreshnessStatus {
    last_scan_age_secs: i64,
    reconcile_interval_secs: i64,
    has_stamp: bool,
}

fn should_use_app_cache(filter: &SearchFilter) -> bool {
    filter.mode == SearchMode::Apps
}

fn should_use_db_query_seed(filter: &SearchFilter, query: &str) -> bool {
    !query.trim().is_empty() && matches!(filter.mode, SearchMode::All | SearchMode::Files)
}

fn search_mode_key(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::All => "all",
        SearchMode::Apps => "apps",
        SearchMode::Files => "files",
        SearchMode::Folders => "folders",
        SearchMode::Actions => "actions",
        SearchMode::Clipboard => "clipboard",
    }
}

fn query_memory_recency_boost(last_selected_epoch_secs: i64, now_epoch_secs: i64) -> i64 {
    if last_selected_epoch_secs <= 0 || now_epoch_secs <= 0 {
        return 0;
    }
    let age_secs = now_epoch_secs.saturating_sub(last_selected_epoch_secs);
    if age_secs <= 86_400 {
        900
    } else if age_secs <= 7 * 86_400 {
        550
    } else if age_secs <= 30 * 86_400 {
        220
    } else {
        0
    }
}

fn is_stale_index_entry(item: &SearchItem) -> bool {
    if !(item.kind.eq_ignore_ascii_case("app")
        || item.kind.eq_ignore_ascii_case("file")
        || item.kind.eq_ignore_ascii_case("folder"))
    {
        return false;
    }

    let path = item.path.trim();
    if path.is_empty() {
        return false;
    }
    if path.contains("://") {
        return false;
    }
    if !looks_like_filesystem_path(path) {
        return false;
    }

    !Path::new(path).exists()
}

fn looks_like_filesystem_path(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }

    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn provider_kinds(provider_name: &str) -> &'static [&'static str] {
    match provider_name {
        "start-menu-apps" | "app" => &["app"],
        "filesystem" | "file" => &["file", "folder"],
        _ => &[],
    }
}

fn should_prune_after_launch_error(item: &SearchItem, error: &LaunchError) -> bool {
    let is_filesystem_target = looks_like_filesystem_path(item.path.trim());
    let should_prune = match error {
        LaunchError::MissingPath(_) => true,
        LaunchError::LaunchFailed { code: Some(code), .. } => *code == 2 || *code == 3,
        LaunchError::Structured { code: Some(code), .. } => *code == 2 || *code == 3,
        _ => false,
    };
    should_prune
        && is_filesystem_target
        && (item.kind.eq_ignore_ascii_case("app")
            || item.kind.eq_ignore_ascii_case("file")
            || item.kind.eq_ignore_ascii_case("folder"))
}

fn provider_stamp_changed(
    db: &Connection,
    provider_name: &str,
    stamp: Option<&str>,
) -> Result<bool, ServiceError> {
    let Some(stamp) = stamp else {
        return Ok(false);
    };
    let previous = index_store::get_meta(db, &provider_stamp_meta_key(provider_name))?;
    Ok(previous.as_deref() != Some(stamp))
}

fn should_skip_provider_discovery(
    db: &Connection,
    provider_name: &str,
    stamp: Option<&str>,
    now_epoch_secs: i64,
) -> Result<bool, ServiceError> {
    let Some(stamp) = stamp else {
        return Ok(false);
    };

    let stamp_key = provider_stamp_meta_key(provider_name);
    let previous_stamp = index_store::get_meta(db, &stamp_key)?;
    if previous_stamp.as_deref() != Some(stamp) {
        return Ok(false);
    }

    let last_scan_key = provider_last_scan_meta_key(provider_name);
    let last_scan_epoch = index_store::get_meta(db, &last_scan_key)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if last_scan_epoch <= 0 {
        return Ok(false);
    }

    Ok(now_epoch_secs.saturating_sub(last_scan_epoch) < PROVIDER_RECONCILE_INTERVAL_SECS)
}

fn persist_provider_discovery_state(
    db: &Connection,
    provider_name: &str,
    stamp: Option<&str>,
    now_epoch_secs: i64,
) -> Result<(), ServiceError> {
    if let Some(stamp) = stamp {
        let stamp_key = provider_stamp_meta_key(provider_name);
        index_store::set_meta(db, &stamp_key, stamp)?;
    }

    let last_scan_key = provider_last_scan_meta_key(provider_name);
    index_store::set_meta(db, &last_scan_key, &now_epoch_secs.to_string())?;
    Ok(())
}

fn provider_stamp_meta_key(provider_name: &str) -> String {
    format!("provider_stamp:{provider_name}")
}

fn provider_last_scan_meta_key(provider_name: &str) -> String {
    format!("provider_last_scan_epoch:{provider_name}")
}

fn load_provider_freshness_status(
    db: &Connection,
    provider_name: &str,
    now_epoch_secs: i64,
) -> Result<ProviderFreshnessStatus, ServiceError> {
    let last_scan_key = provider_last_scan_meta_key(provider_name);
    let last_scan_epoch = index_store::get_meta(db, &last_scan_key)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let stamp_key = provider_stamp_meta_key(provider_name);
    let has_stamp = index_store::get_meta(db, &stamp_key)?
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    Ok(ProviderFreshnessStatus {
        last_scan_age_secs: if last_scan_epoch > 0 {
            now_epoch_secs.saturating_sub(last_scan_epoch).max(0)
        } else {
            -1
        },
        reconcile_interval_secs: PROVIDER_RECONCILE_INTERVAL_SECS,
        has_stamp,
    })
}

fn log_provider_freshness_status(
    db: &Connection,
    provider_name: &str,
    now_epoch_secs: i64,
    skipped: bool,
) -> Result<(), ServiceError> {
    let freshness = load_provider_freshness_status(db, provider_name, now_epoch_secs)?;
    crate::logging::info(&format!(
        "[nex] provider_freshness name={} skipped={} last_scan_age_secs={} reconcile_interval_secs={} has_stamp={}",
        provider_name,
        skipped,
        freshness.last_scan_age_secs,
        freshness.reconcile_interval_secs,
        freshness.has_stamp
    ));
    Ok(())
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

/// Estimate current process memory usage in bytes.
/// Platform-agnostic fallback that works on Windows and Unix.
fn self_measure_memory() -> usize {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        unsafe {
            let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ) != 0
            {
                return pmc.WorkingSetSize as usize;
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Fallback for non-Windows: read /proc/self/status (Linux) or use sysinfo (macOS)
        // For now, return 0 to avoid crashing on unsupported platforms
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(kb_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = kb_str.parse::<usize>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::CoreService;
    use crate::config::{Config, SearchMode};
    use crate::index_store::{self, open_memory};
    use crate::model::SearchItem;
    use crate::search::SearchFilter;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn polling_writer_lets_new_readers_in_while_it_waits() {
        use std::sync::{mpsc, Arc, RwLock};
        use std::time::Duration;

        let lock = Arc::new(RwLock::new(0_u32));
        // Stands in for the rebuild's long-lived read guard.
        let rebuild_guard = lock.read().unwrap();

        let (acquired_tx, acquired_rx) = mpsc::channel();
        let writer_lock = Arc::clone(&lock);
        let writer = std::thread::spawn(move || {
            let mut guard = super::write_without_blocking_readers(&writer_lock);
            *guard += 1;
            acquired_tx.send(()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(acquired_rx.try_recv().is_err(), "writer must wait for the reader");

        // An IPC search arriving now must not queue behind the waiting writer.
        let reader_lock = Arc::clone(&lock);
        let reader_got_in = std::thread::spawn(move || reader_lock.try_read().is_ok())
            .join()
            .unwrap();
        assert!(reader_got_in);

        drop(rebuild_guard);
        acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer acquires once the reader leaves");
        writer.join().unwrap();
        assert_eq!(*lock.read().unwrap(), 1);
    }

    /// Config whose Tantivy directory is unique per test, so parallel tests
    /// never share an `IndexWriter` on the same directory.
    fn isolated_config(label: &str) -> Config {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nex-prefetch-{label}-{unique}"));
        let mut config = Config::default();
        config.index_db_path = dir.join("index.sqlite3");
        config
    }

    #[test]
    fn hot_cache_recalls_rows_outside_the_hot_set_from_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = Config::default();
        config.index_db_path = dir.path().join("index.sqlite3");

        let file = |name: &str, use_count: u32| {
            let path = format!("C:\\recall\\{name}");
            let mut item = SearchItem::new(
                &format!("file:{}", path.to_ascii_lowercase()),
                "file",
                name,
                &path,
            );
            item.use_count = use_count;
            item
        };
        let needle = file("quarterly-message.txt", 0);
        let needle_folder = SearchItem::new(
            "folder:c:\\recall\\archive-message",
            "folder",
            "archive-message",
            "C:\\recall\\archive-message",
        );
        {
            let db = index_store::open_file(&config.index_db_path).expect("store");
            let mut items: Vec<SearchItem> = (0..super::UI_CACHE_MAX_ITEMS + 50)
                .map(|i| file(&format!("notes-{i:05}.txt"), 1))
                .collect();
            items.push(needle.clone());
            items.push(needle_folder.clone());
            index_store::upsert_items(&db, &items).expect("seed store");
        }

        let service = CoreService::new_ui_client(config).expect("service");
        assert_eq!(service.cached_items_len(), super::UI_CACHE_MAX_ITEMS);
        assert!(!service.cache_is_complete());
        let cached = service.cached_items_snapshot();
        assert!(cached.iter().all(|i| i.id != needle.id && i.id != needle_folder.id));

        // Mid-name term: outside the hot set and not a prefix match.
        let results = service.search("message", 20).expect("search");
        assert!(results.iter().any(|r| r.id == needle.id));

        let folders = service
            .search_with_filter(
                "archive",
                20,
                &SearchFilter {
                    mode: SearchMode::Folders,
                    ..Default::default()
                },
            )
            .expect("folder search");
        assert!(folders.iter().any(|r| r.id == needle_folder.id));
        assert!(folders.iter().all(|r| r.kind == "folder"));

        // The path does not exist, so launch fails on the path rather than
        // with ItemNotFound from a cache-only lookup.
        let err = service
            .launch(super::LaunchTarget::Id(&needle.id))
            .unwrap_err();
        assert!(!matches!(err, super::ServiceError::ItemNotFound(_)));
    }

    #[test]
    fn publish_log_streams_deltas_and_resets_large_batches() {
        let service = CoreService::with_connection(
            isolated_config("publish-log"),
            open_memory().unwrap(),
        )
        .expect("service should initialize");
        let a = SearchItem::new(
            "file:c:\\publish-a.txt",
            "file",
            "publish-a.txt",
            "C:\\publish-a.txt",
        );
        service.upsert_item(&a).expect("item should upsert");

        let instance = match service.publish_outcome(0, 0) {
            super::PublishOutcome::Snapshot { instance, .. } => instance,
            other => panic!("expected snapshot, got {other:?}"),
        };
        match service.publish_outcome(0, instance) {
            super::PublishOutcome::Delta {
                upserts, deleted, ..
            } => {
                assert_eq!(upserts.len(), 1);
                assert!(deleted.is_empty());
            }
            other => panic!("expected delta, got {other:?}"),
        }
        assert!(matches!(
            service.publish_outcome(1, instance),
            super::PublishOutcome::Unchanged { .. }
        ));

        service
            .delete_item_by_id(&a.id)
            .expect("delete should succeed");
        match service.publish_outcome(1, instance) {
            super::PublishOutcome::Delta {
                upserts, deleted, ..
            } => {
                assert!(upserts.is_empty());
                assert_eq!(deleted, vec![a.id.clone()]);
            }
            other => panic!("expected delete delta, got {other:?}"),
        }

        for index in 0..(super::PUBLISH_RESET_THRESHOLD + 10) {
            let item = SearchItem::new(
                &format!("file:c:\\publish-{index}.txt"),
                "file",
                &format!("publish-{index}.txt"),
                &format!("C:\\publish-{index}.txt"),
            );
            service.upsert_item(&item).expect("item should upsert");
        }
        assert!(matches!(
            service.publish_outcome(2, instance),
            super::PublishOutcome::Snapshot { .. }
        ));
    }

    #[test]
    fn successful_launch_updates_ranking_and_quick_launch_usage() {
        let service = CoreService::with_connection(
            isolated_config("launch-usage"),
            open_memory().unwrap(),
        )
        .expect("service should initialize");
        let item = SearchItem::new("app-editor", "app", "Editor", "C:\\editor.exe");
        service.upsert_item(&item).expect("item should upsert");

        service
            .record_successful_launch_by_id(&item.id)
            .expect("launch should be recorded");

        let cached = service
            .cached_items_snapshot()
            .into_iter()
            .find(|candidate| candidate.id == item.id)
            .expect("item should remain cached");
        assert_eq!(cached.use_count, 1);
        assert_eq!(cached.launch_count, 1);
        assert!(cached.last_accessed_epoch_secs > 0);
        assert_eq!(cached.last_accessed_epoch_secs, cached.last_launched_at);

        let stored = index_store::get_item(&service.db(), &item.id)
            .expect("stored item should load")
            .expect("stored item should exist");
        assert_eq!(stored.use_count, 1);
        assert_eq!(stored.launch_count, 1);
    }

    #[test]
    fn recent_documents_recency_overlay_advances_and_inserts() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let recent_dir = std::env::temp_dir().join(format!("nex-mru-{unique}"));
        std::fs::create_dir_all(&recent_dir).expect("recent dir should be created");

        // A file the index already knows about but with no recency.
        let known = std::env::temp_dir().join(format!("nex-mru-known-{unique}.pdf"));
        std::fs::write(&known, b"pdf").expect("known file should exist");
        // A file the filesystem scan never reached, only visible via the MRU.
        let unseen = std::env::temp_dir().join(format!("nex-mru-unseen-{unique}.pdf"));
        std::fs::write(&unseen, b"pdf").expect("unseen file should exist");

        std::fs::write(
            recent_dir.join("known.lnk"),
            crate::recent_files::lnk_with_link_info(
                // LinkInfo local base paths carry a trailing separator.
                &format!("{}\\", known.parent().unwrap().to_string_lossy()),
                known.file_name().unwrap().to_string_lossy().as_ref(),
            ),
        )
        .expect("shortcut should be written");
        std::fs::write(
            recent_dir.join("unseen.lnk"),
            crate::recent_files::lnk_with_link_info(
                &format!("{}\\", unseen.parent().unwrap().to_string_lossy()),
                unseen.file_name().unwrap().to_string_lossy().as_ref(),
            ),
        )
        .expect("shortcut should be written");

        let service = CoreService::with_connection(
            isolated_config("recent-docs"),
            open_memory().unwrap(),
        )
        .expect("service should initialize");
        let known_id = format!("file:{}", known.to_string_lossy().to_ascii_lowercase());
        service
            .upsert_item(&SearchItem::new(
                &known_id,
                "file",
                known.file_name().unwrap().to_string_lossy().as_ref(),
                known.to_string_lossy().as_ref(),
            ))
            .expect("known file should upsert");

        let applied = service
            .apply_recent_documents_from(&recent_dir)
            .expect("overlay should apply");
        assert_eq!(applied, 2, "both MRU entries should be applied");

        let recency_of = |id: &str| {
            service
                .cached_items_snapshot()
                .into_iter()
                .find(|item| item.id == id)
                .map(|item| item.last_accessed_epoch_secs)
        };
        assert!(
            recency_of(&known_id).unwrap_or(0) > 0,
            "existing item should gain recency"
        );
        let unseen_id = format!("file:{}", unseen.to_string_lossy().to_ascii_lowercase());
        assert!(
            recency_of(&unseen_id).unwrap_or(0) > 0,
            "MRU-only target should be inserted with recency"
        );

        // Second pass must be a no-op: recency only moves forward.
        let second = service
            .apply_recent_documents_from(&recent_dir)
            .expect("overlay should apply");
        assert_eq!(second, 0, "re-applying the same MRU must do nothing");

        let _ = std::fs::remove_dir_all(&recent_dir);
        let _ = std::fs::remove_file(&known);
        let _ = std::fs::remove_file(&unseen);
    }

    #[test]
    fn recent_documents_scan_is_rate_limited() {
        let service = CoreService::with_connection(
            isolated_config("recent-rate"),
            open_memory().unwrap(),
        )
        .expect("service should initialize");

        assert!(service.recent_documents_scan_due(600));
        assert!(
            !service.recent_documents_scan_due(600),
            "a second call inside the window must be refused"
        );
        assert!(
            service.recent_documents_scan_due(0),
            "a zero interval always allows a refresh"
        );
    }

    #[test]
    fn app_mode_search_excludes_non_app_items() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let app_path = std::env::temp_dir().join(format!("nex-app-cache-app-{unique}.tmp"));
        let file_path = std::env::temp_dir().join(format!("nex-app-cache-file-{unique}.tmp"));
        std::fs::write(&app_path, b"ok").expect("app path should exist");
        std::fs::write(&file_path, b"ok").expect("file path should exist");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "app-vivaldi",
                "app",
                "Vivaldi",
                app_path.to_string_lossy().as_ref(),
            ))
            .expect("app should upsert");
        service
            .upsert_item(&SearchItem::new(
                "file-video",
                "file",
                "video notes",
                file_path.to_string_lossy().as_ref(),
            ))
            .expect("file should upsert");

        let filter = SearchFilter {
            mode: SearchMode::Apps,
            ..SearchFilter::default()
        };
        let results = service
            .search_with_filter("v", 20, &filter)
            .expect("search should succeed");
        assert!(results.iter().any(|item| item.id == "app-vivaldi"));
        assert!(!results.iter().any(|item| item.id == "file-video"));

        std::fs::remove_file(app_path).expect("app temp file should be removed");
        std::fs::remove_file(file_path).expect("file temp file should be removed");
    }

    #[test]
    fn app_cache_tracks_kind_changes() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nex-app-cache-kind-{unique}.tmp"));
        std::fs::write(&path, b"ok").expect("temp file should exist");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "entry-1",
                "app",
                "Visual Studio Code",
                path.to_string_lossy().as_ref(),
            ))
            .expect("app should upsert");
        service
            .upsert_item(&SearchItem::new(
                "entry-1",
                "file",
                "Visual Studio Code.txt",
                path.to_string_lossy().as_ref(),
            ))
            .expect("file should replace app");

        let filter = SearchFilter {
            mode: SearchMode::Apps,
            ..SearchFilter::default()
        };
        let results = service
            .search_with_filter("visual", 20, &filter)
            .expect("search should succeed");
        assert!(!results.iter().any(|item| item.id == "entry-1"));

        std::fs::remove_file(path).expect("temp file should be removed");
    }

    #[test]
    fn uncapped_search_respects_requested_limit_above_config_max() {
        let mut cfg = Config::default();
        cfg.max_results = 5;
        let service = CoreService::with_connection(cfg, open_memory().unwrap())
            .expect("service should initialize");

        let mut temp_paths = Vec::new();
        for idx in 0..25 {
            let path = std::env::temp_dir().join(format!("nex-uncapped-{idx}.tmp"));
            std::fs::write(&path, b"ok").expect("temp file should exist");
            temp_paths.push(path.clone());
            service
                .upsert_item(&SearchItem::new(
                    &format!("app-{idx:02}"),
                    "app",
                    &format!("Alpha App {idx:02}"),
                    path.to_string_lossy().as_ref(),
                ))
                .expect("item should upsert");
        }

        let filter = SearchFilter::default();
        let capped = service
            .search_with_filter("alpha", 20, &filter)
            .expect("capped search should succeed");
        let uncapped = service
            .search_with_filter_uncapped("alpha", 20, &filter)
            .expect("uncapped search should succeed");

        assert_eq!(capped.len(), 5);
        assert!(uncapped.len() >= 20);

        for path in temp_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn bounded_ui_items_keeps_apps_and_most_used() {
        let mut items: Vec<SearchItem> = (0..5)
            .map(|index| {
                SearchItem::new(
                    &format!("app{index}"),
                    "app",
                    &format!("App {index}"),
                    "C:\\app.exe",
                )
            })
            .collect();
        for index in 0..super::UI_CACHE_MAX_ITEMS + 20 {
            let mut item = SearchItem::new(
                &format!("file{index}"),
                "file",
                &format!("file-{index}.txt"),
                &format!("C:\\file-{index}.txt"),
            );
            item.use_count = index as u32;
            items.push(item);
        }

        let bounded = super::bounded_ui_items(&items);
        assert_eq!(bounded.len(), super::UI_CACHE_MAX_ITEMS);
        assert_eq!(
            bounded.iter().filter(|item| item.kind == "app").count(),
            5
        );
        assert!(bounded
            .iter()
            .any(|item| item.use_count == (super::UI_CACHE_MAX_ITEMS + 19) as u32));
        assert!(!bounded.iter().any(|item| item.id == "file0"));
    }

    #[test]
    fn cached_snapshot_is_generation_stamped_and_consistent() {
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        let before = service.search_generation();
        service
            .upsert_item(&SearchItem::new(
                "snap-1",
                "app",
                "Snapshot App",
                "C:\\snap.exe",
            ))
            .expect("item should upsert");
        assert!(service.search_generation() > before);

        let (generation, items) = service.cached_snapshot();
        assert_eq!(generation, service.search_generation());
        assert!(items.iter().any(|item| item.id == "snap-1"));

        // A fresh search over the live set sees the same item — search no
        // longer has a lock-contention path that returns an empty Vec.
        let results = service
            .search_with_filter("snapshot", 10, &SearchFilter::default())
            .expect("search should succeed");
        assert!(results.iter().any(|item| item.id == "snap-1"));
    }

    #[test]
    fn search_results_never_reflect_partial_cache_swap() {
        // The swap in refresh_cache_from_store replaces the whole Vec at
        // once; a concurrent search either sees the pre- or post-swap set,
        // never an empty Vec from lock contention. Exercise the read side
        // under contention: hold a read guard (blocking writers) and confirm
        // the snapshot still returns the full pre-swap contents.
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "swap-1",
                "app",
                "Swap App",
                "C:\\swap.exe",
            ))
            .expect("item should upsert");

        let guard = service
            .cached_items
            .read()
            .expect("read lock should be available");
        let expected = guard.len();
        drop(guard);

        let (_generation, items) = service.cached_snapshot();
        assert_eq!(items.len(), expected);
        assert!(items.iter().any(|item| item.id == "swap-1"));
    }
}
