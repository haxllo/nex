use rusqlite::Connection;

use crate::action_executor::{launch_path, LaunchError};
use crate::config::{validate, Config, SearchMode};
use crate::contract::{CoreRequest, CoreResponse, LaunchResponse, SearchResponse};
use crate::discovery::{
    DiscoveryProvider, FileSystemDiscoveryProvider, ProviderError, StartMenuAppDiscoveryProvider,
};
use crate::fts5_search::Fts5Index;
use crate::index_store::{self, StoreError};
use crate::model::SearchItem;
use crate::search::SearchFilter;
use crate::tantivy_search::TantivyIndex;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STALE_PRUNE_INTERVAL: Duration = Duration::from_secs(15);
const PROVIDER_RECONCILE_INTERVAL_SECS: i64 = 30 * 60;
const STALE_PRUNE_BATCH_SIZE: usize = 16;

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
    tantivy_index: Mutex<Option<TantivyIndex>>,
    fts5_index: Mutex<Option<Fts5Index>>,
    last_stale_prune: Mutex<Option<Instant>>,
    stale_prune_cursor: Mutex<usize>,
    pub(crate) progress: Mutex<Option<Arc<AtomicU32>>>,
    compaction_write_count: Mutex<u32>,
    last_compaction_time: Mutex<Option<Instant>>,
    #[cfg(target_os = "windows")]
    file_watchers: Mutex<Option<crate::file_watcher_consumer::FileWatcherHandle>>,
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

impl CoreService {
    pub fn new(config: Config) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        let db = index_store::open_from_config(&config)?;
        Self::with_loaded_cache(config, db)
    }

    pub fn with_connection(config: Config, db: Connection) -> Result<Self, ServiceError> {
        validate(&config).map_err(ServiceError::Config)?;
        Self::with_loaded_cache(config, db)
    }

    fn with_loaded_cache(config: Config, db: Connection) -> Result<Self, ServiceError> {
        let cached = index_store::list_items(&db)?;
        let cached_apps = collect_app_items(&cached);

        let index_dir = config.index_db_path.parent().unwrap_or(Path::new("."));
        let tantivy_path = index_dir.join("index.tantivy");

        let tantivy_index = match open_tantivy_index(&tantivy_path) {
            Some(idx) => Some(idx),
            None => {
                // Schema mismatch or corrupt — delete dir and recreate
                crate::logging::info("[nex] Tantivy index schema changed or corrupt, resetting");
                let _ = std::fs::remove_dir_all(&tantivy_path);
                match TantivyIndex::open(&tantivy_path) {
                    Ok(idx) => Some(idx),
                    Err(e) => {
                        crate::logging::info(&format!(
                            "[nex] Tantivy index init after reset: {e}, falling back to FTS5"
                        ));
                        None
                    }
                }
            }
        };

        let fts5_index = if tantivy_index.is_none() {
            // FTS5 is the sole backend — clear so sync_indexes_from_cache
            // takes the full-rebuild path.
            match Fts5Index::open(&config.index_db_path) {
                Ok(idx) => {
                    let _ = idx.clear();
                    Some(idx)
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "[nex] FTS5 index init: {e}, running without FTS index"
                    ));
                    None
                }
            }
        } else {
            // Tantivy is primary. Don't clear FTS5 here — let
            // sync_indexes_from_cache decide first vs incremental
            // based on the existing doc count.
            match Fts5Index::open(&config.index_db_path) {
                Ok(idx) => Some(idx),
                Err(e) => {
                    crate::logging::info(&format!("[nex] FTS5 fallback index init: {e}"));
                    None
                }
            }
        };

        Ok(Self {
            config: RwLock::new(config),
            db: Mutex::new(db),
            providers: RwLock::new(Vec::new()),
            cached_items: RwLock::new(cached),
            cached_app_items: RwLock::new(cached_apps),
            tantivy_index: Mutex::new(tantivy_index),
            fts5_index: Mutex::new(fts5_index),
            last_stale_prune: Mutex::new(None),
            stale_prune_cursor: Mutex::new(0),
            progress: Mutex::new(None),
            compaction_write_count: Mutex::new(0),
            last_compaction_time: Mutex::new(None),
            #[cfg(target_os = "windows")]
            file_watchers: Mutex::new(None),
        })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
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

    pub fn reconfigure_runtime_providers(&self, cfg: &Config) -> Result<(), ServiceError> {
        validate(cfg).map_err(ServiceError::Config)?;
        let providers = runtime_providers_from_config(cfg);
        self.replace_runtime_config(cfg.clone());
        self.replace_providers(providers);
        // Discovery roots or excludes changed: stop the existing watchers
        // so the next indexing pass can restart them on the new paths.
        // They will be re-spawned by the runtime once the next reindex
        // completes.
        #[cfg(target_os = "windows")]
        {
            self.stop_file_watchers();
        }
        Ok(())
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

fn runtime_providers_from_config(config: &Config) -> Vec<Box<dyn DiscoveryProvider>> {
    let mut providers: Vec<Box<dyn DiscoveryProvider>> = Vec::new();
    providers.push(Box::new(StartMenuAppDiscoveryProvider::default()));

    // Register filesystem provider for roots-based file/folder discovery.
    let filesystem_provider = FileSystemDiscoveryProvider::from_config(config, 5);
    crate::runtime::log_info(&format!(
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
            let guard = match self.cached_app_items.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let query_boosts = self.query_personalization_boosts(query, filter.mode)?;
            return Ok(crate::search::search_with_filter_with_boosts(
                &guard,
                query,
                effective_limit,
                filter,
                Some(&query_boosts),
            ));
        }

        // Path 1: indexed search (Tantivy/FTS5) returns pre-ranked candidates
        // directly. The index is only populated when background indexing has
        // finished successfully. When the index is empty (e.g., Everything
        // service was down during indexing) we fall back to scanning the
        // in-memory cache, holding the read lock only for the duration of
        // the ranking pass — no full Vec clone per keystroke.
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
                    // Augment with in-memory cache items the index missed.
                    if let Ok(guard) = self.cached_items.read() {
                        let cache_ranked = crate::search::search_with_filter_with_boosts(
                            &guard,
                            query,
                            effective_limit.saturating_sub(ranked.len()),
                            filter,
                            Some(&query_boosts),
                        );
                        for item in cache_ranked {
                            if !ranked.iter().any(|r| r.id == item.id) {
                                ranked.push(item);
                                if ranked.len() >= effective_limit {
                                    break;
                                }
                            }
                        }
                    }
                }
                return Ok(ranked);
            }
        }

        // Path 2: no index results — rank the in-memory cache directly,
        // holding the read lock only while we score and select.
        let query_boosts = self.query_personalization_boosts(query, filter.mode)?;
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Ok(crate::search::search_with_filter_with_boosts(
            &guard,
            query,
            effective_limit,
            filter,
            Some(&query_boosts),
        ))
    }

    pub fn cached_items_snapshot(&self) -> Vec<SearchItem> {
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }

    pub fn cached_items_len(&self) -> usize {
        self.cached_len()
    }

    pub fn reload_cache_from_store(&self) -> Result<usize, ServiceError> {
        self.refresh_cache_from_store()?;
        Ok(self.cached_len())
    }

    pub fn launch(&self, target: LaunchTarget<'_>) -> Result<(), ServiceError> {
        self.launch_with_query_context(target, None, None)
    }

    pub fn launch_with_query_context(
        &self,
        target: LaunchTarget<'_>,
        query: Option<&str>,
        mode: Option<SearchMode>,
    ) -> Result<(), ServiceError> {
        match target {
            LaunchTarget::Path(path) => launch_path(path).map_err(ServiceError::from),
            LaunchTarget::Id(id) => {
                let item = index_store::get_item(&*self.db(), id)?
                    .ok_or_else(|| ServiceError::ItemNotFound(id.to_string()))?;
                match launch_path(&item.path) {
                    Ok(()) => {
                        self.record_successful_launch(&item)?;
                        if let (Some(query), Some(mode)) = (query, mode) {
                            self.record_query_selection_hint(query, mode, &item.id)?;
                        }
                        Ok(())
                    }
                    Err(error) if should_prune_after_launch_error(&item, &error) => {
                        index_store::delete_item(&*self.db(), &item.id)?;
                        self.remove_cached_item_by_id(&item.id);
                        Err(ServiceError::from(error))
                    }
                    Err(error) => Err(ServiceError::from(error)),
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
        index_store::record_query_selection(
            &*self.db(),
            &query_norm,
            search_mode_key(mode),
            item_id,
            now_epoch_secs(),
        )?;
        Ok(())
    }

    pub fn rebuild_index(&self) -> Result<usize, ServiceError> {
        let report = self.rebuild_index_incremental_with_report()?;
        Ok(report.indexed_total)
    }

    pub fn rebuild_index_with_report(&self) -> Result<IndexRefreshReport, ServiceError> {
        self.rebuild_index_internal(false)
    }

    pub fn rebuild_index_incremental_with_report(
        &self,
    ) -> Result<IndexRefreshReport, ServiceError> {
        self.rebuild_index_internal(true)
    }

    fn rebuild_index_internal(
        &self,
        incremental_mode: bool,
    ) -> Result<IndexRefreshReport, ServiceError> {
        let providers_guard = match self.providers.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if providers_guard.is_empty() {
            self.refresh_cache_from_store()?;
            return Ok(IndexRefreshReport {
                indexed_total: self.cached_len(),
                discovered_total: 0,
                upserted_total: 0,
                removed_total: 0,
                providers: Vec::new(),
            });
        }

        let mut existing_items = index_store::list_items(&*self.db())?;
        let mut existing_by_id: HashMap<String, SearchItem> = existing_items
            .drain(..)
            .map(|item| (item.id.clone(), item))
            .collect();

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
            if incremental_mode
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
            let discovery_cap = if is_filesystem {
                (config_snapshot.index_max_items_total as usize).max(1)
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
            let discovered = if progress_pct.is_some() {
                provider.discover_with_progress(Some(&mut discovery_progress))?
            } else {
                provider.discover()?
            };
            if let Some(ref pct) = progress_pct {
                pct.store(
                    cumulative_weight.saturating_add(discovery_weight).min(95),
                    Ordering::Relaxed,
                );
            }

            let discovered_count = discovered.len();
            discovered_total += discovered_count;

            let mut upserted = 0_usize;
            let mut discovered_ids = HashSet::with_capacity(discovered_count);

            for (item_idx, mut item) in discovered.into_iter().enumerate() {
                if let Some(previous) = existing_by_id.get(&item.id) {
                    if item.use_count == 0 {
                        item.use_count = previous.use_count;
                    }
                    if item.last_accessed_epoch_secs <= 0 {
                        item.last_accessed_epoch_secs = previous.last_accessed_epoch_secs;
                    }
                }

                discovered_ids.insert(item.id.clone());

                let changed = existing_by_id
                    .get(&item.id)
                    .map(|previous| previous != &item)
                    .unwrap_or(true);
                if changed {
                    index_store::upsert_item(&*self.db(), &item)?;
                    upserted += 1;
                    upserted_total += 1;
                }
                existing_by_id.insert(item.id.clone(), item);

                if let Some(ref pct) = progress_pct {
                    if discovered_count > 0 {
                        let pct_val = cumulative_weight
                            + discovery_weight
                            + (write_weight * (item_idx as u32 + 1)) / discovered_count as u32;
                        pct.store(pct_val.min(95), Ordering::Relaxed);
                    }
                }
            }

            cumulative_weight += provider_weight;

            // Kind-based ownership is safe for current runtime provider composition:
            // start-menu apps own kind=app, filesystem owns kind=file/folder.
            let removable_ids: Vec<String> = existing_by_id
                .values()
                .filter(|item| provider_manages_kind(provider.provider_name(), &item.kind))
                .filter(|item| !discovered_ids.contains(&item.id))
                .map(|item| item.id.clone())
                .collect();

            for id in &removable_ids {
                index_store::delete_item(&*self.db(), id)?;
                existing_by_id.remove(id);
            }

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
            self.sync_indexes_from_cache()?;
        }
        if let Some(ref pct) = progress_pct {
            pct.store(100, Ordering::Relaxed);
        }
        let indexed_total = self.cached_len();
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

    pub fn upsert_item(&self, item: &SearchItem) -> Result<(), ServiceError> {
        index_store::upsert_item(&*self.db(), item)?;
        self.upsert_cached_item(item.clone());
        self.index_item_on_backends(item);
        Ok(())
    }

    /// Delete an item by id from the SQLite store, the in-memory caches,
    /// and the Tantivy/FTS5 search indexes. Idempotent: deleting a
    /// missing id is not an error.
    pub fn delete_item_by_id(&self, id: &str) -> Result<(), ServiceError> {
        index_store::delete_item(&*self.db(), id)?;
        self.remove_cached_item_by_id(id);
        self.remove_item_from_backends(id);
        Ok(())
    }

    /// Spawn a background stale-pruner thread that runs
    /// `prune_stale_items_if_due` on a 15 s cadence so the search
    /// critical path never blocks on a write lock. Uses
    /// `try_write` so a concurrent search is never delayed.
    pub fn start_stale_pruner(&self, service_arc: &Arc<RwLock<CoreService>>) {
        let svc = Arc::clone(service_arc);
        std::thread::Builder::new()
            .name("nex-stale-pruner".into())
            .spawn(move || loop {
                std::thread::sleep(STALE_PRUNE_INTERVAL);
                if let Ok(guard) = svc.try_write() {
                    let _ = guard.prune_stale_items_if_due();
                }
            })
            .ok();
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
            crate::runtime::log_info(
                "[nex] directory_watcher: no discovery_roots configured; skipping start",
            );
            return Ok(());
        }

        let handle =
            crate::file_watcher_consumer::FileWatcherHandle::start(roots, excluded_roots, Arc::clone(service_arc));
        crate::runtime::log_info(&format!(
            "[nex] directory_watcher: started on {} root(s)",
            handle.active_roots()
        ));
        *slot = Some(handle);
        Ok(())
    }

    /// Stop and join the per-root file watcher consumers. Safe to call
    /// even if watchers were never started.
    #[cfg(target_os = "windows")]
    pub fn stop_file_watchers(&self) {
        let mut slot = match self.file_watchers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(handle) = slot.take() {
            drop(handle); // RAII joins threads
            crate::runtime::log_info("[nex] directory_watcher: stopped");
        }
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

fn open_tantivy_index(tantivy_path: &std::path::Path) -> Option<TantivyIndex> {
    match TantivyIndex::open(tantivy_path) {
        Ok(idx) => {
            match idx.num_docs() {
                Ok(n) if n > 0 => {
                    // Valid index with documents — keep it
                    Some(idx)
                }
                _ => {
                    // Empty or corrupt — clear and rebuild on next sync
                    let _ = idx.clear();
                    Some(idx)
                }
            }
        }
        Err(_e) => None, // Schema mismatch or other error — caller will reset
    }
}

impl CoreService {
    fn cached_len(&self) -> usize {
        match self.cached_items.read() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    fn search_indexed_candidates(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        if limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Try Tantivy first
        let tantivy_guard = match self.tantivy_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            match idx.search(query, limit) {
                Ok(results) => return Ok(results),
                Err(e) => {
                    crate::logging::info(&format!(
                        "[nex] Tantivy search failed: {e}, falling back to FTS5"
                    ));
                }
            }
        }
        drop(tantivy_guard);

        // Fall back to FTS5
        let fts5_guard = match self.fts5_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *fts5_guard {
            match idx.search(query, limit) {
                Ok(results) => return Ok(results),
                Err(e) => {
                    crate::logging::info(&format!("[nex] FTS5 search failed: {e}"));
                }
            }
        }

        Ok(Vec::new())
    }

    /// Warm search indexes + SQLite page cache so the user's first
    /// keystroke doesn't pay cold-page-fault latency.  Must be called
    /// with the outer service lock held (read or write).
    pub fn warm_search_cache(&self) {
        // Warm Tantivy reader + page cache
        if let Ok(guard) = self.tantivy_index.lock() {
            if let Some(ref idx) = *guard {
                idx.warmup();
            }
        }
        // Warm FTS5 reader + page cache
        if let Ok(guard) = self.fts5_index.lock() {
            if let Some(ref idx) = *guard {
                idx.warmup();
            }
        }
        // Touch the SQLite database: issue a no-op query so the OS
        // pages in the hot portions of the file (personalization
        // boosts table, use_count tracking, etc.).
        let _ = self.db().query_row("SELECT 1", [], |_| Ok(()));
        // Touch the cached items inner lock to warm the cache line.
        drop(self.cached_items.read());
        drop(self.cached_app_items.read());
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

        let rows =
            index_store::list_query_selections(&*self.db(), &query_norm, search_mode_key(mode), 64)?;
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
        Ok(boosts)
    }

    fn refresh_cache_from_store(&self) -> Result<(), ServiceError> {
        let config_snapshot = self.config_snapshot();
        let latest_full = index_store::list_items(&*self.db())?;
        let latest_apps = collect_app_items(&latest_full);
        let latest = {
            let compact_started = std::time::Instant::now();
            let summary = cache_compaction_summary(&latest_full, &config_snapshot);
            let compacted = compact_cached_items(&latest_full, &config_snapshot);
            let compact_elapsed_ms = compact_started.elapsed().as_millis();
            if summary.dropped_total > 0 || compact_elapsed_ms > 100 {
                if compact_elapsed_ms > 100 {
                    crate::logging::info(&format!(
                        "[nex] cache_compaction input_total={} retained={} dropped={} elapsed_ms={}",
                        summary.input_total,
                        summary.retained_total,
                        summary.dropped_total,
                        compact_elapsed_ms,
                    ));
                } else {
                    crate::logging::info(&format!(
                        "[nex] cache_compaction input_total={} retained={} dropped={} retained_apps={} retained_file_folders={} retained_other={} effective_file_seed_cap={} broad_root_mode={} active_memory_target_mb={}",
                        summary.input_total,
                        summary.retained_total,
                        summary.dropped_total,
                        summary.retained_apps,
                        summary.retained_file_folders,
                        summary.retained_other,
                        summary.effective_file_seed_cap,
                        summary.broad_root_mode,
                        summary.active_memory_target_mb
                    ));
                }
            }
            compacted
        };
        match self.cached_items.write() {
            Ok(mut guard) => {
                *guard = latest;
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = latest;
            }
        }
        match self.cached_app_items.write() {
            Ok(mut guard) => {
                *guard = latest_apps;
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = latest_apps;
            }
        }
        Ok(())
    }

    fn upsert_cached_item(&self, item: SearchItem) {
        let item_for_apps = item.clone();
        let item_id = item.id.clone();
        let is_app = item.kind.eq_ignore_ascii_case("app");
        match self.cached_items.write() {
            Ok(mut guard) => upsert_cached_item_inner(&mut guard, item),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                upsert_cached_item_inner(&mut guard, item);
            }
        }
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
    }

    fn remove_cached_item_by_id(&self, id: &str) {
        match self.cached_items.write() {
            Ok(mut guard) => guard.retain(|entry| entry.id != id),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.retain(|entry| entry.id != id);
            }
        }
        match self.cached_app_items.write() {
            Ok(mut guard) => guard.retain(|entry| entry.id != id),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.retain(|entry| entry.id != id);
            }
        }
    }

    pub(crate) fn sync_indexes_from_cache(&self) -> Result<(), ServiceError> {
        let items = index_store::list_items(&*self.db())?;

        // Determine if this is the first sync (both backends have 0 docs)
        let tantivy_is_first = self
            .tantivy_index
            .lock()
            .map(|g| g.as_ref().map_or(true, |idx| idx.num_docs().unwrap_or(0) == 0))
            .unwrap_or(true);
        let fts5_is_first = self
            .fts5_index
            .lock()
            .map(|g| g.as_ref().map_or(true, |idx| idx.num_docs().unwrap_or(0) == 0))
            .unwrap_or(true);

        crate::logging::info(&format!(
            "[nex] sync_indexes items={} tantivy_available={} fts5_available={} tantivy_first={} fts5_first={}",
            items.len(),
            self.tantivy_index
                .lock()
                .map(|g| g.is_some())
                .unwrap_or(false),
            self.fts5_index.lock().map(|g| g.is_some()).unwrap_or(false),
            tantivy_is_first,
            fts5_is_first,
        ));

        let tantivy_guard = match self.tantivy_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            let result = if tantivy_is_first {
                idx.index_items(&items)
            } else {
                idx.incremental_sync_items(&items)
            };
            if let Err(e) = result {
                crate::logging::info(&format!("[nex] Tantivy index sync error: {e}"));
            }
        }
        drop(tantivy_guard);

        let fts5_guard = match self.fts5_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *fts5_guard {
            let result = if fts5_is_first {
                idx.index_items(&items)
            } else {
                idx.incremental_sync_items(&items)
            };
            if let Err(e) = result {
                crate::logging::info(&format!("[nex] FTS5 index sync error: {e}"));
            }
        }

        self.maybe_compact_backends();

        Ok(())
    }

    fn index_item_on_backends(&self, item: &SearchItem) {
        let tantivy_guard = match self.tantivy_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            let _ = idx.upsert_item(item);
        }
        drop(tantivy_guard);

        let fts5_guard = match self.fts5_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *fts5_guard {
            let _ = idx.upsert_item(item);
        }

        self.bump_compaction_counter();
    }

    fn remove_item_from_backends(&self, id: &str) {
        let tantivy_guard = match self.tantivy_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *tantivy_guard {
            let _ = idx.delete_item(id);
        }
        drop(tantivy_guard);

        let fts5_guard = match self.fts5_index.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ref idx) = *fts5_guard {
            let _ = idx.delete_item(id);
        }

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
            let tantivy_guard = match self.tantivy_index.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(ref idx) = *tantivy_guard {
                let _ = idx.flush();
            }
            drop(tantivy_guard);

            let fts5_guard = match self.fts5_index.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(ref idx) = *fts5_guard {
                let _ = idx.optimize();
            }

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

            let tantivy_mb = match self.tantivy_index.lock() {
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

    fn record_successful_launch(&self, item: &SearchItem) -> Result<(), ServiceError> {
        let now = now_epoch_secs();
        let mut updated = item.clone();
        updated.use_count = updated.use_count.saturating_add(1);
        updated.last_accessed_epoch_secs = now.max(updated.last_accessed_epoch_secs);

        index_store::upsert_item(&*self.db(), &updated)?;
        self.upsert_cached_item(updated);
        Ok(())
    }

    fn prune_stale_items_if_due(&self) -> Result<(), ServiceError> {
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
            return Ok(());
        }

        let candidates = self.stale_prune_candidates(STALE_PRUNE_BATCH_SIZE);
        let stale_ids: Vec<String> = candidates
            .iter()
            .filter(|item| is_stale_index_entry(item))
            .map(|item| item.id.clone())
            .collect();

        if stale_ids.is_empty() {
            return Ok(());
        }

        for stale_id in &stale_ids {
            index_store::delete_item(&*self.db(), stale_id)?;
        }

        match self.cached_items.write() {
            Ok(mut guard) => {
                let stale_lookup: HashSet<&str> = stale_ids.iter().map(String::as_str).collect();
                guard.retain(|entry| !stale_lookup.contains(entry.id.as_str()));
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                let stale_lookup: HashSet<&str> = stale_ids.iter().map(String::as_str).collect();
                guard.retain(|entry| !stale_lookup.contains(entry.id.as_str()));
            }
        }
        match self.cached_app_items.write() {
            Ok(mut guard) => {
                let stale_lookup: HashSet<&str> = stale_ids.iter().map(String::as_str).collect();
                guard.retain(|entry| !stale_lookup.contains(entry.id.as_str()));
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                let stale_lookup: HashSet<&str> = stale_ids.iter().map(String::as_str).collect();
                guard.retain(|entry| !stale_lookup.contains(entry.id.as_str()));
            }
        }

        // Remove stale entries from Tantivy and FTS5 backends so
        // deleted-on-disk files stop appearing in search results.
        for stale_id in &stale_ids {
            self.remove_item_from_backends(stale_id);
        }

        crate::logging::info(&format!(
            "[nex] stale_prune scanned={} removed={} cached_items_remaining={}",
            candidates.len(),
            stale_ids.len(),
            self.cached_len()
        ));

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
        let guard = match self.cached_items.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if guard.is_empty() {
            *cursor = 0;
            return Vec::new();
        }

        let len = guard.len();
        let start = (*cursor).min(len - 1);
        let take = batch_size.min(len);
        let mut out = Vec::with_capacity(take);
        for offset in 0..take {
            let idx = (start + offset) % len;
            out.push(guard[idx].clone());
        }
        *cursor = (start + take) % len;
        out
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

#[allow(dead_code)]
fn filter_file_items(items: &[SearchItem]) -> Vec<SearchItem> {
    items
        .iter()
        .filter(|item| {
            !item.kind.eq_ignore_ascii_case("file") && !item.kind.eq_ignore_ascii_case("folder")
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheCompactionSummary {
    input_total: usize,
    retained_total: usize,
    dropped_total: usize,
    retained_apps: usize,
    retained_file_folders: usize,
    retained_other: usize,
    effective_file_seed_cap: usize,
    broad_root_mode: bool,
    active_memory_target_mb: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderFreshnessStatus {
    last_scan_age_secs: i64,
    reconcile_interval_secs: i64,
    has_stamp: bool,
}

fn compact_cached_items(items: &[SearchItem], cfg: &Config) -> Vec<SearchItem> {
    cache_compaction_summary(items, cfg).retain_items(items).0
}

fn cache_compaction_summary(items: &[SearchItem], cfg: &Config) -> CacheCompactionSummary {
    let effective_file_seed_cap = effective_file_folder_cache_cap(cfg);
    let broad_root_mode = broad_root_discovery_enabled(cfg);
    let (retained_total, retained_apps, retained_file_folders, retained_other) =
        retained_cache_counts(items, effective_file_seed_cap);

    CacheCompactionSummary {
        input_total: items.len(),
        retained_total,
        dropped_total: items.len().saturating_sub(retained_total),
        retained_apps,
        retained_file_folders,
        retained_other,
        effective_file_seed_cap,
        broad_root_mode,
        active_memory_target_mb: cfg.active_memory_target_mb,
    }
}

impl CacheCompactionSummary {
    fn retain_items(&self, items: &[SearchItem]) -> (Vec<SearchItem>, usize) {
        let mut out = Vec::with_capacity(items.len().min(self.effective_file_seed_cap + 2048));
        let mut file_or_folder_count = 0_usize;

        for item in items {
            if is_file_or_folder_kind(item.kind.as_str()) {
                if file_or_folder_count >= self.effective_file_seed_cap {
                    continue;
                }
                file_or_folder_count += 1;
            }
            out.push(item.clone());
        }

        (out, file_or_folder_count)
    }
}

fn retained_cache_counts(
    items: &[SearchItem],
    effective_file_seed_cap: usize,
) -> (usize, usize, usize, usize) {
    let mut retained_total = 0_usize;
    let mut retained_apps = 0_usize;
    let mut retained_file_folders = 0_usize;
    let mut retained_other = 0_usize;
    let mut file_or_folder_count = 0_usize;

    for item in items {
        if is_file_or_folder_kind(item.kind.as_str()) {
            if file_or_folder_count >= effective_file_seed_cap {
                continue;
            }
            file_or_folder_count += 1;
            retained_file_folders += 1;
        } else if item.kind.eq_ignore_ascii_case("app") {
            retained_apps += 1;
        } else {
            retained_other += 1;
        }
        retained_total += 1;
    }

    (
        retained_total,
        retained_apps,
        retained_file_folders,
        retained_other,
    )
}

fn effective_file_folder_cache_cap(cfg: &Config) -> usize {
    let base_cap = (cfg.index_max_items_per_query_seed as usize).max(250);
    if !broad_root_discovery_enabled(cfg) {
        return base_cap;
    }

    // SearchItem is roughly 200–500 bytes on disk; size the seed cap to fit
    // ~25% of the active memory target (leaving headroom for Tantivy/FTS5,
    // the icon cache, and rank buffers). The previous formula used 8 items
    // per MB, which silently capped a 72MB target to 576 items even when
    // the user explicitly raised index_max_items_per_query_seed to 5000+.
    let memory_target_bytes =
        (cfg.active_memory_target_mb as usize).saturating_mul(1024 * 1024);
    let budget_bytes = memory_target_bytes / 4;
    let approx_items = budget_bytes / 400;
    let memory_scaled_cap = approx_items.clamp(250, base_cap);
    base_cap.min(memory_scaled_cap.max(250))
}

fn broad_root_discovery_enabled(cfg: &Config) -> bool {
    if !(cfg.show_files || cfg.show_folders) {
        return false;
    }
    cfg.discovery_roots
        .iter()
        .any(|root| is_broad_discovery_root(root))
}

fn is_broad_discovery_root(path: &Path) -> bool {
    let raw = path.to_string_lossy().trim().replace('/', "\\");
    if raw.is_empty() {
        return false;
    }
    if raw == "\\" || raw == "/" {
        return true;
    }
    if raw.len() == 2 {
        let bytes = raw.as_bytes();
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return true;
        }
    }
    if raw.len() == 3 {
        let bytes = raw.as_bytes();
        if bytes[1] == b':'
            && bytes[0].is_ascii_alphabetic()
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
    }
    false
}

fn is_file_or_folder_kind(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("file") || kind.eq_ignore_ascii_case("folder")
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

fn provider_manages_kind(provider_name: &str, kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    match provider_name {
        "start-menu-apps" | "app" => kind == "app",
        "filesystem" | "file" => kind == "file" || kind == "folder",
        _ => false,
    }
}

fn should_prune_after_launch_error(item: &SearchItem, error: &LaunchError) -> bool {
    let is_filesystem_target = looks_like_filesystem_path(item.path.trim());
    match error {
        LaunchError::MissingPath(_) => {
            is_filesystem_target
                && (item.kind.eq_ignore_ascii_case("app")
                    || item.kind.eq_ignore_ascii_case("file")
                    || item.kind.eq_ignore_ascii_case("folder"))
        }
        LaunchError::LaunchFailed {
            code: Some(code), ..
        } => {
            // ShellExecute missing-file/path errors: remove stale entries immediately.
            (*code == 2 || *code == 3)
                && is_filesystem_target
                && (item.kind.eq_ignore_ascii_case("app")
                    || item.kind.eq_ignore_ascii_case("file")
                    || item.kind.eq_ignore_ascii_case("folder"))
        }
        LaunchError::LaunchFailed { .. } | LaunchError::EmptyPath => false,
    }
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

#[cfg(test)]
mod tests {
    use super::{
        broad_root_discovery_enabled, cache_compaction_summary, compact_cached_items,
        effective_file_folder_cache_cap, CoreService,
    };
    use crate::config::{Config, SearchMode};
    use crate::index_store::open_memory;
    use crate::model::SearchItem;
    use crate::search::SearchFilter;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn broad_root_mode_detects_drive_roots() {
        let mut cfg = Config::default();
        cfg.show_files = true;
        cfg.show_folders = true;
        cfg.discovery_roots = vec![PathBuf::from(r"C:\")];
        assert!(broad_root_discovery_enabled(&cfg));
    }

    #[test]
    fn broad_root_mode_ignores_default_profile_roots() {
        let cfg = Config::default();
        assert!(!broad_root_discovery_enabled(&cfg));
    }

    #[test]
    fn broad_root_mode_honors_explicit_seed_cap() {
        let mut cfg = Config::default();
        cfg.show_files = true;
        cfg.show_folders = true;
        cfg.discovery_roots = vec![PathBuf::from(r"C:\")];
        cfg.index_max_items_per_query_seed = 5_000;
        cfg.active_memory_target_mb = 72;

        // 72MB target with 5000 seed cap: the user explicitly raised the cap
        // and 72MB can comfortably hold thousands of SearchItem rows, so the
        // cap should track the user's setting rather than the old 8-items/MB
        // heuristic (which silently clamped this exact configuration to 576).
        assert_eq!(effective_file_folder_cache_cap(&cfg), 5_000);
    }

    #[test]
    fn broad_root_mode_scales_down_for_tight_memory_target() {
        let mut cfg = Config::default();
        cfg.show_files = true;
        cfg.show_folders = true;
        cfg.discovery_roots = vec![PathBuf::from(r"C:\")];
        cfg.index_max_items_per_query_seed = 50_000;
        cfg.active_memory_target_mb = 20;

        // 20MB / 4 = 5MB budget / 400 bytes per item ≈ 13,107, clamped to
        // [250, 50_000], so the cap is the floor for the user-set seed cap.
        let cap = effective_file_folder_cache_cap(&cfg);
        assert!(cap >= 250, "cap should never drop below 250: {cap}");
        assert!(cap <= 50_000, "cap should not exceed user setting: {cap}");
    }

    #[test]
    fn broad_root_mode_never_drops_below_minimum_floor() {
        let mut cfg = Config::default();
        cfg.show_files = true;
        cfg.show_folders = true;
        cfg.discovery_roots = vec![PathBuf::from(r"C:\")];
        cfg.index_max_items_per_query_seed = 250;
        cfg.active_memory_target_mb = 20;

        assert_eq!(effective_file_folder_cache_cap(&cfg), 250);
    }

    #[test]
    fn cache_compaction_keeps_apps_but_tightens_files_for_broad_roots() {
        let mut cfg = Config::default();
        cfg.show_files = true;
        cfg.show_folders = true;
        cfg.discovery_roots = vec![PathBuf::from(r"C:\")];
        cfg.index_max_items_per_query_seed = 5_000;
        cfg.active_memory_target_mb = 72;

        let mut items = Vec::new();
        for idx in 0..20 {
            items.push(SearchItem::new(
                &format!("app-{idx}"),
                "app",
                &format!("App {idx}"),
                &format!(r"C:\Apps\App{idx}.lnk"),
            ));
        }
        for idx in 0..700 {
            items.push(SearchItem::new(
                &format!("file-{idx}"),
                "file",
                &format!("File {idx}"),
                &format!(r"C:\Data\File{idx}.txt"),
            ));
        }

        let summary = cache_compaction_summary(&items, &cfg);
        let retained = compact_cached_items(&items, &cfg);

        assert!(summary.broad_root_mode);
        assert_eq!(summary.retained_apps, 20);
        assert_eq!(summary.effective_file_seed_cap, 5_000);
        assert_eq!(summary.retained_file_folders, 700);
        assert_eq!(summary.retained_total, 720);
        assert_eq!(retained.len(), 720);
        assert_eq!(
            retained
                .iter()
                .filter(|item| item.kind.eq_ignore_ascii_case("app"))
                .count(),
            20
        );
    }
}
