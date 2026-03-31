use rusqlite::{params, Connection};

use crate::action_executor::{launch_path, LaunchError};
use crate::config::{validate, Config, SearchMode};
use crate::contract::{CoreRequest, CoreResponse, LaunchResponse, SearchResponse};
use crate::discovery::{
    DiscoveryProvider, FileSystemDiscoveryProvider, ProviderError, StartMenuAppDiscoveryProvider,
};
use crate::index_store::{self, StoreError};
use crate::model::SearchItem;
use crate::search::SearchFilter;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STALE_PRUNE_INTERVAL: Duration = Duration::from_secs(15);
const PROVIDER_RECONCILE_INTERVAL_SECS: i64 = 30 * 60;
const STALE_PRUNE_BATCH_SIZE: usize = 512;

#[derive(Debug)]
pub enum ServiceError {
    Config(String),
    Store(StoreError),
    Provider(ProviderError),
    Launch(LaunchError),
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
    db: Connection,
    providers: RwLock<Vec<Box<dyn DiscoveryProvider>>>,
    cached_items: RwLock<Vec<SearchItem>>,
    cached_app_items: RwLock<Vec<SearchItem>>,
    last_stale_prune: Mutex<Option<Instant>>,
    stale_prune_cursor: Mutex<usize>,
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
        Ok(Self {
            config: RwLock::new(config),
            db,
            providers: RwLock::new(Vec::new()),
            cached_items: RwLock::new(cached),
            cached_app_items: RwLock::new(cached_apps),
            last_stale_prune: Mutex::new(None),
            stale_prune_cursor: Mutex::new(0),
        })
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

    fn config_snapshot(&self) -> Config {
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
    // Always register filesystem provider so toggling file/folder discovery off
    // can actively prune stale file/folder records from the index.
    providers.push(Box::new(
        FileSystemDiscoveryProvider::with_options(
            config.discovery_roots.clone(),
            5,
            config.discovery_exclude_roots.clone(),
            config.windows_search_enabled,
            config.windows_search_fallback_filesystem,
            config.show_files,
            config.show_folders,
        )
        .with_index_limits(
            config.index_max_items_total as usize,
            config.index_max_items_per_root as usize,
        ),
    ));
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
        self.prune_stale_items_if_due()?;
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

        let mut seed_items = {
            let guard = match self.cached_items.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.clone()
        };

        if should_use_db_query_seed(filter, query) {
            let db_seed_limit = (config_snapshot.index_max_items_per_query_seed as usize).max(250);
            let db_candidates = self.db_query_candidates(query, filter.mode, db_seed_limit)?;
            merge_seed_candidates(&mut seed_items, db_candidates);
        }

        let query_boosts = self.query_personalization_boosts(query, filter.mode)?;
        Ok(crate::search::search_with_filter_with_boosts(
            &seed_items,
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
                let item = index_store::get_item(&self.db, id)?
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
                        index_store::delete_item(&self.db, &item.id)?;
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
            &self.db,
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

        let mut existing_items = index_store::list_items(&self.db)?;
        let mut existing_by_id: HashMap<String, SearchItem> = existing_items
            .drain(..)
            .map(|item| (item.id.clone(), item))
            .collect();

        let mut discovered_total = 0_usize;
        let mut upserted_total = 0_usize;
        let mut removed_total = 0_usize;
        let mut provider_reports = Vec::with_capacity(providers_guard.len());
        let now_epoch_secs = now_epoch_secs();

        for provider in providers_guard.iter() {
            let started = Instant::now();
            let provider_name = provider.provider_name().to_string();
            let provider_stamp = if incremental_mode {
                provider.change_stamp()
            } else {
                None
            };
            if incremental_mode
                && should_skip_provider_discovery(
                    &self.db,
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
                log_provider_freshness_status(
                    &self.db,
                    &provider_name,
                    now_epoch_secs,
                    true,
                )?;
                continue;
            }

            let discovered = provider.discover()?;
            let discovered_count = discovered.len();
            discovered_total += discovered_count;

            let mut upserted = 0_usize;
            let mut discovered_ids = HashSet::with_capacity(discovered_count);

            for mut item in discovered {
                if let Some(previous) = existing_by_id.get(&item.id) {
                    // Discovery providers do not carry usage metrics; preserve learned
                    // launch signals across incremental/full refreshes.
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
                    index_store::upsert_item(&self.db, &item)?;
                    upserted += 1;
                    upserted_total += 1;
                }
                existing_by_id.insert(item.id.clone(), item);
            }

            // Kind-based ownership is safe for current runtime provider composition:
            // start-menu apps own kind=app, filesystem owns kind=file/folder.
            let removable_ids: Vec<String> = existing_by_id
                .values()
                .filter(|item| provider_manages_kind(provider.provider_name(), &item.kind))
                .filter(|item| !discovered_ids.contains(&item.id))
                .map(|item| item.id.clone())
                .collect();

            for id in &removable_ids {
                index_store::delete_item(&self.db, id)?;
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
                    &self.db,
                    &provider_name,
                    provider_stamp.as_deref(),
                    now_epoch_secs,
                )?;
                log_provider_freshness_status(
                    &self.db,
                    &provider_name,
                    now_epoch_secs,
                    false,
                )?;
            }
        }

        self.refresh_cache_from_store()?;
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
        index_store::upsert_item(&self.db, item)?;
        self.upsert_cached_item(item.clone());
        Ok(())
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

impl CoreService {
    fn cached_len(&self) -> usize {
        match self.cached_items.read() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    fn db_query_candidates(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
    ) -> Result<Vec<SearchItem>, ServiceError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        if matches!(mode, SearchMode::Actions | SearchMode::Clipboard) {
            return Ok(Vec::new());
        }

        let sql = match mode {
            SearchMode::Files => {
                "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs
                 FROM item
                 WHERE (title LIKE ?1 COLLATE NOCASE OR path LIKE ?1 COLLATE NOCASE)
                   AND kind IN ('file', 'folder')
                 ORDER BY use_count DESC, last_accessed_epoch_secs DESC, id
                 LIMIT ?2"
            }
            SearchMode::Apps => {
                "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs
                 FROM item
                 WHERE (title LIKE ?1 COLLATE NOCASE OR path LIKE ?1 COLLATE NOCASE)
                   AND kind = 'app'
                 ORDER BY use_count DESC, last_accessed_epoch_secs DESC, id
                 LIMIT ?2"
            }
            SearchMode::All => {
                "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs
                 FROM item
                 WHERE title LIKE ?1 COLLATE NOCASE OR path LIKE ?1 COLLATE NOCASE
                 ORDER BY use_count DESC, last_accessed_epoch_secs DESC, id
                 LIMIT ?2"
            }
            SearchMode::Actions | SearchMode::Clipboard => unreachable!(),
        };

        let pattern = format!("%{trimmed}%");
        let mut stmt = self
            .db
            .prepare(sql)
            .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
        let mut rows = stmt
            .query(params![pattern, limit as i64])
            .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|error| ServiceError::Store(StoreError::Db(error)))?
        {
            let id: String = row
                .get(0)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            let kind: String = row
                .get(1)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            let title: String = row
                .get(2)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            let path: String = row
                .get(3)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            let subtitle: String = row
                .get(4)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            let use_count: u32 = row
                .get(5)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            let last_accessed_epoch_secs: i64 = row
                .get(6)
                .map_err(|error| ServiceError::Store(StoreError::Db(error)))?;
            out.push(SearchItem::from_owned_with_subtitle(
                id,
                kind,
                title,
                path,
                subtitle,
                use_count,
                last_accessed_epoch_secs,
            ));
        }
        Ok(out)
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
            index_store::list_query_selections(&self.db, &query_norm, search_mode_key(mode), 64)?;
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
        let latest_full = index_store::list_items(&self.db)?;
        let latest_apps = collect_app_items(&latest_full);
        let summary = cache_compaction_summary(&latest_full, &config_snapshot);
        let latest = compact_cached_items(&latest_full, &config_snapshot);
        if summary.dropped_total > 0 {
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

    fn record_successful_launch(&self, item: &SearchItem) -> Result<(), ServiceError> {
        let now = now_epoch_secs();
        let mut updated = item.clone();
        updated.use_count = updated.use_count.saturating_add(1);
        updated.last_accessed_epoch_secs = now.max(updated.last_accessed_epoch_secs);

        index_store::upsert_item(&self.db, &updated)?;
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
            index_store::delete_item(&self.db, stale_id)?;
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
    cache_compaction_summary(items, cfg)
        .retain_items(items)
        .0
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

    let memory_scaled_cap = ((cfg.active_memory_target_mb as usize).saturating_mul(8)).clamp(250, 1500);
    base_cap.min(memory_scaled_cap)
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
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() && (bytes[2] == b'\\' || bytes[2] == b'/') {
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

fn merge_seed_candidates(seed_items: &mut Vec<SearchItem>, extra: Vec<SearchItem>) {
    if extra.is_empty() {
        return;
    }
    let mut seen: HashSet<String> = seed_items.iter().map(|item| item.id.clone()).collect();
    for item in extra {
        if seen.insert(item.id.clone()) {
            seed_items.push(item);
        }
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
    fn broad_root_mode_reduces_file_folder_cache_cap() {
        let mut cfg = Config::default();
        cfg.show_files = true;
        cfg.show_folders = true;
        cfg.discovery_roots = vec![PathBuf::from(r"C:\")];
        cfg.index_max_items_per_query_seed = 5_000;
        cfg.active_memory_target_mb = 72;

        assert_eq!(effective_file_folder_cache_cap(&cfg), 576);
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
        assert_eq!(summary.effective_file_seed_cap, 576);
        assert_eq!(summary.retained_file_folders, 576);
        assert_eq!(summary.retained_total, 596);
        assert_eq!(retained.len(), 596);
        assert_eq!(
            retained
                .iter()
                .filter(|item| item.kind.eq_ignore_ascii_case("app"))
                .count(),
            20
        );
    }
}
