use crate::action_registry::search_actions_with_mode;
use crate::clipboard_history;
use crate::config::Config;
use crate::core_service::CoreService;
use crate::model::SearchItem;
use crate::plugin_sdk::PluginRegistry;
use crate::query_dsl::ParsedQuery;
use crate::runtime::log_info;
use crate::runtime::UNINSTALL_QUERY_RESULT_LIMIT;
use crate::runtime_diagnostics::{
    percentile_u128, QUERY_PROFILE_LOG_THRESHOLD_MS, SHORT_QUERY_APP_BIAS_MAX_LEN,
};
use crate::search::{search_with_filter, SearchFilter};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub(crate) const INDEXED_PREFIX_CACHE_MIN_QUERY_LEN: usize = 1;
pub(crate) const INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT: usize = 120;
pub(crate) const INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT: usize = 480;
pub(crate) const FINAL_QUERY_CACHE_MAX_ENTRIES: usize = 32;
pub(crate) const ADAPTIVE_INDEXED_LATENCY_WINDOW: usize = 24;

#[derive(Debug, Clone)]
pub(crate) struct IndexedPrefixCache {
    pub(crate) normalized_query: String,
    pub(crate) indexed_filter: SearchFilter,
    pub(crate) seed_items: Vec<SearchItem>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OverlaySearchSession {
    pub(crate) indexed_prefix_cache: Option<IndexedPrefixCache>,
    pub(crate) final_query_cache: HashMap<String, Vec<SearchItem>>,
    pub(crate) final_query_cache_lru: VecDeque<String>,
    pub(crate) indexed_latency_ms: VecDeque<u128>,
}

impl OverlaySearchSession {
    pub(crate) fn clear(&mut self) {
        self.indexed_prefix_cache = None;
        self.final_query_cache.clear();
        self.final_query_cache_lru.clear();
        self.indexed_latency_ms.clear();
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn search_overlay_results(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    parsed_query: &ParsedQuery,
    result_limit: usize,
) -> Result<Vec<SearchItem>, String> {
    let mut session = OverlaySearchSession::default();
    search_overlay_results_with_session(
        service,
        cfg,
        plugins,
        parsed_query,
        result_limit,
        &mut session,
    )
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn search_overlay_results_with_session(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    parsed_query: &ParsedQuery,
    result_limit: usize,
    session: &mut OverlaySearchSession,
) -> Result<Vec<SearchItem>, String> {
    if result_limit == 0 {
        return Ok(Vec::new());
    }

    let filter = build_search_filter(cfg, parsed_query);
    let text_query = parsed_query.free_text.trim();
    let normalized_query = crate::model::normalize_for_search(text_query);
    if should_skip_non_searchable_query(parsed_query, &normalized_query) {
        log_info(&format!(
            "[nex] query_guard skip=non_searchable_symbol_only q=\"{}\"",
            sanitize_query_for_profile_log(parsed_query.raw.as_str())
        ));
        session.clear();
        return Ok(Vec::new());
    }
    let cache_key = final_query_cache_key(parsed_query, &filter, &normalized_query, result_limit);
    if let Some(cached) = cached_final_query_results(session, &cache_key) {
        return Ok(cached);
    }
    let candidate_limit = candidate_limit_for_query(
        result_limit,
        &filter,
        &normalized_query,
        parsed_query.command_mode,
    );
    let base_indexed_seed_limit = indexed_seed_limit(candidate_limit, normalized_query.len());
    let seed_cap = (cfg.index_max_items_per_query_seed as usize).max(candidate_limit);
    let indexed_seed_limit = adaptive_indexed_seed_limit(
        session,
        candidate_limit,
        normalized_query.len(),
        base_indexed_seed_limit,
    )
    .min(seed_cap);
    let short_query_app_bias =
        should_use_short_query_app_mode(parsed_query, &filter, &normalized_query);
    let mut indexed_filter = filter.clone();
    if short_query_app_bias {
        indexed_filter.mode = crate::config::SearchMode::Apps;
    }

    let search_started = Instant::now();
    let mut merged = Vec::new();
    let indexed_started = Instant::now();
    let mut indexed_cache_hit = false;
    let prefix_cache_eligible = is_prefix_cache_eligible_query(parsed_query, short_query_app_bias);
    let indexed_seed_items = if let Some(cache) =
        session.indexed_prefix_cache.as_ref().filter(|cache| {
            can_use_indexed_prefix_cache(
                cache,
                prefix_cache_eligible,
                &normalized_query,
                &indexed_filter,
            )
        }) {
        indexed_cache_hit = true;
        search_with_filter(
            &cache.seed_items,
            text_query,
            indexed_seed_limit,
            &indexed_filter,
        )
    } else {
        service
            .search_with_filter_uncapped(text_query, indexed_seed_limit, &indexed_filter)
            .map_err(|error| format!("indexed search failed: {error}"))?
    };
    let indexed_ms = indexed_started.elapsed().as_millis();
    if !indexed_cache_hit {
        record_indexed_latency_sample(session, indexed_ms);
    }
    let indexed_count = indexed_seed_items.len();
    merged.extend(indexed_seed_items.iter().take(candidate_limit).cloned());
    if prefix_cache_eligible && normalized_query.len() >= INDEXED_PREFIX_CACHE_MIN_QUERY_LEN {
        session.indexed_prefix_cache = Some(IndexedPrefixCache {
            normalized_query: normalized_query.clone(),
            indexed_filter: indexed_filter.clone(),
            seed_items: indexed_seed_items,
        });
    } else {
        session.clear();
    }

    let mut provider_ms = 0_u128;
    let mut provider_count = 0_usize;
    if !short_query_app_bias {
        let provider_started = Instant::now();
        let provider_results = search_with_filter(
            &plugins.provider_items,
            text_query,
            candidate_limit,
            &filter,
        );
        provider_ms = provider_started.elapsed().as_millis();
        provider_count = provider_results.len();
        merged.extend(provider_results);
    }

    let actions_started = Instant::now();
    let mut action_items =
        search_actions_with_mode(text_query, candidate_limit, parsed_query.command_mode, cfg);
    let built_in_actions_count = action_items.len();
    let mut plugin_action_count = 0_usize;
    if !plugins.action_items.is_empty() {
        let plugin_actions = search_with_filter(
            &plugins.action_items,
            text_query,
            candidate_limit,
            &SearchFilter {
                mode: crate::config::SearchMode::Actions,
                ..SearchFilter::default()
            },
        );
        plugin_action_count = plugin_actions.len();
        action_items.extend(plugin_actions);
    }
    let action_results = search_with_filter(&action_items, text_query, candidate_limit, &filter);
    let actions_ms = actions_started.elapsed().as_millis();
    let action_count = action_results.len();
    merged.extend(action_results);

    let mut clipboard_ms = 0_u128;
    let mut clipboard_count = 0_usize;
    if !short_query_app_bias {
        let clipboard_started = Instant::now();
        let clipboard_results =
            clipboard_history::search_history(cfg, text_query, &filter, candidate_limit.min(120));
        clipboard_ms = clipboard_started.elapsed().as_millis();
        clipboard_count = clipboard_results.len();
        merged.extend(clipboard_results);
    }

    let rank_started = Instant::now();
    let ranked = search_with_filter(&merged, text_query, result_limit, &filter);
    let rank_ms = rank_started.elapsed().as_millis();
    let total_ms = search_started.elapsed().as_millis();
    if total_ms >= QUERY_PROFILE_LOG_THRESHOLD_MS {
        log_info(&format!(
            "[nex] query_profile q=\"{}\" mode={} candidate_limit={} indexed_seed_limit={} short_app_bias={} indexed_cache_hit={} indexed_count={} indexed_ms={} provider_count={} provider_ms={} action_count={} action_ms={} built_in_actions={} plugin_actions={} clipboard_count={} clipboard_ms={} rank_ms={} total_ms={}",
            sanitize_query_for_profile_log(text_query),
            format!("{:?}", filter.mode).to_ascii_lowercase(),
            candidate_limit,
            indexed_seed_limit,
            short_query_app_bias,
            indexed_cache_hit,
            indexed_count,
            indexed_ms,
            provider_count,
            provider_ms,
            action_count,
            actions_ms,
            built_in_actions_count,
            plugin_action_count,
            clipboard_count,
            clipboard_ms,
            rank_ms,
            total_ms
        ));
    }
    store_final_query_results(session, cache_key, ranked.as_slice());
    Ok(ranked)
}

pub(crate) fn build_search_filter(cfg: &Config, parsed_query: &ParsedQuery) -> SearchFilter {
    let mode = resolved_mode_for_query(cfg, parsed_query);
    SearchFilter {
        mode,
        kind_filter: parsed_query.kind_filter.clone(),
        extension_filter: parsed_query.extension_filter.clone(),
        include_files: cfg.show_files,
        include_folders: cfg.show_folders,
        include_groups: parsed_query.include_groups.clone(),
        exclude_terms: parsed_query.exclude_terms.clone(),
        modified_within: parsed_query.modified_within,
        created_within: parsed_query.created_within,
    }
}

pub(crate) fn resolved_mode_for_query(
    cfg: &Config,
    parsed_query: &ParsedQuery,
) -> crate::config::SearchMode {
    let mut mode = parsed_query
        .mode_override
        .unwrap_or(cfg.search_mode_default);
    if parsed_query.command_mode {
        mode = crate::config::SearchMode::Actions;
    }
    mode
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn should_use_short_query_app_mode(
    parsed_query: &ParsedQuery,
    filter: &SearchFilter,
    normalized_query: &str,
) -> bool {
    if normalized_query.is_empty() || normalized_query.len() > SHORT_QUERY_APP_BIAS_MAX_LEN {
        return false;
    }
    if parsed_query.command_mode {
        return false;
    }
    if filter.mode != crate::config::SearchMode::All {
        return false;
    }
    parsed_query.kind_filter.is_none()
        && parsed_query.extension_filter.is_none()
        && parsed_query.exclude_terms.is_empty()
        && parsed_query.modified_within.is_none()
        && parsed_query.created_within.is_none()
}

pub(crate) fn should_skip_non_searchable_query(
    parsed_query: &ParsedQuery,
    normalized_query: &str,
) -> bool {
    if !normalized_query.is_empty() {
        return false;
    }
    if parsed_query.command_mode {
        return false;
    }
    if parsed_query.mode_override.is_some() {
        return false;
    }
    parsed_query.kind_filter.is_none()
        && parsed_query.extension_filter.is_none()
        && parsed_query.include_groups.is_empty()
        && parsed_query.exclude_terms.is_empty()
        && parsed_query.modified_within.is_none()
        && parsed_query.created_within.is_none()
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(crate) fn result_limit_for_query(base_limit: usize, parsed_query: &ParsedQuery) -> usize {
    if base_limit == 0 {
        return 0;
    }
    if parsed_query.command_mode
        && crate::uninstall_registry::has_uninstall_intent(parsed_query.free_text.as_str())
    {
        return base_limit.max(UNINSTALL_QUERY_RESULT_LIMIT);
    }
    base_limit
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn maybe_expand_uninstall_quick_shortcut(
    query: &str,
    last_query: &str,
) -> Option<String> {
    let raw = query.trim_start();
    let remainder = raw.strip_prefix('>')?;
    if remainder.eq_ignore_ascii_case("u") {
        let last_trimmed = last_query.trim();
        if last_trimmed.is_empty() || last_trimmed == ">" {
            return Some(">u ".to_string());
        }
    }
    None
}

pub(crate) fn candidate_limit_for_query(
    result_limit: usize,
    filter: &SearchFilter,
    normalized_query: &str,
    command_mode: bool,
) -> usize {
    if result_limit == 0 {
        return 0;
    }

    let base = result_limit.saturating_mul(6).max(60);
    if command_mode || filter.mode == crate::config::SearchMode::Actions {
        return result_limit
            .saturating_mul(4)
            .max(48)
            .min(160)
            .max(result_limit);
    }

    match normalized_query.len() {
        0 => result_limit
            .saturating_mul(2)
            .max(24)
            .min(64)
            .max(result_limit),
        1 => match filter.mode {
            crate::config::SearchMode::All => result_limit
                .saturating_mul(3)
                .max(45)
                .min(96)
                .max(result_limit),
            crate::config::SearchMode::Files => result_limit
                .saturating_mul(5)
                .max(70)
                .min(200)
                .max(result_limit),
            _ => result_limit
                .saturating_mul(4)
                .max(56)
                .min(180)
                .max(result_limit),
        },
        2 => match filter.mode {
            crate::config::SearchMode::All => result_limit
                .saturating_mul(4)
                .max(56)
                .min(140)
                .max(result_limit),
            crate::config::SearchMode::Files => result_limit
                .saturating_mul(5)
                .max(70)
                .min(200)
                .max(result_limit),
            _ => result_limit
                .saturating_mul(4)
                .max(56)
                .min(180)
                .max(result_limit),
        },
        _ => base.min(280).max(result_limit),
    }
}

pub(crate) fn indexed_seed_limit(candidate_limit: usize, normalized_query_len: usize) -> usize {
    let multiplier = match normalized_query_len {
        0 | 1 => 4,
        2 => 2,
        _ => 2,
    };
    candidate_limit.saturating_mul(multiplier).clamp(
        INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT,
        INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT,
    )
}

pub(crate) fn adaptive_indexed_seed_limit(
    session: &OverlaySearchSession,
    candidate_limit: usize,
    normalized_query_len: usize,
    base_seed_limit: usize,
) -> usize {
    let mut samples: Vec<u128> = session.indexed_latency_ms.iter().copied().collect();
    if samples.len() < 6 {
        return base_seed_limit;
    }

    let p95 = percentile_u128(&mut samples, 0.95);
    let scaled = if p95 >= 160 {
        (base_seed_limit.saturating_mul(60)) / 100
    } else if p95 >= 120 {
        (base_seed_limit.saturating_mul(72)) / 100
    } else if p95 >= 95 {
        (base_seed_limit.saturating_mul(84)) / 100
    } else if p95 <= 50 && normalized_query_len >= 3 {
        (base_seed_limit.saturating_mul(108)) / 100
    } else {
        base_seed_limit
    };

    let minimum = candidate_limit.max(INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT / 2);
    scaled.clamp(minimum, INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT)
}

pub(crate) fn record_indexed_latency_sample(session: &mut OverlaySearchSession, indexed_ms: u128) {
    session.indexed_latency_ms.push_back(indexed_ms);
    while session.indexed_latency_ms.len() > ADAPTIVE_INDEXED_LATENCY_WINDOW {
        session.indexed_latency_ms.pop_front();
    }
}

pub(crate) fn final_query_cache_key(
    parsed_query: &ParsedQuery,
    filter: &SearchFilter,
    normalized_query: &str,
    result_limit: usize,
) -> String {
    format!(
        "q={};mode={:?};kind={};ext={};include={};exclude={};modified={:?};created={:?};cmd={};limit={}",
        normalized_query,
        filter.mode,
        filter.kind_filter.as_deref().unwrap_or("-"),
        filter.extension_filter.as_deref().unwrap_or("-"),
        encode_term_groups(&filter.include_groups),
        filter.exclude_terms.join(","),
        filter.modified_within,
        filter.created_within,
        parsed_query.command_mode,
        result_limit
    )
}

pub(crate) fn encode_term_groups(groups: &[Vec<String>]) -> String {
    if groups.is_empty() {
        return "-".to_string();
    }

    groups
        .iter()
        .map(|group| group.join("+"))
        .collect::<Vec<String>>()
        .join("|")
}

pub(crate) fn cached_final_query_results(
    session: &mut OverlaySearchSession,
    key: &str,
) -> Option<Vec<SearchItem>> {
    let cached = session.final_query_cache.get(key).cloned()?;
    if let Some(position) = session
        .final_query_cache_lru
        .iter()
        .position(|entry| entry == key)
    {
        session.final_query_cache_lru.remove(position);
    }
    session.final_query_cache_lru.push_back(key.to_string());
    Some(cached)
}

pub(crate) fn store_final_query_results(
    session: &mut OverlaySearchSession,
    key: String,
    results: &[SearchItem],
) {
    if results.is_empty() {
        return;
    }

    session
        .final_query_cache
        .insert(key.clone(), results.to_vec());
    if let Some(position) = session
        .final_query_cache_lru
        .iter()
        .position(|entry| entry == &key)
    {
        session.final_query_cache_lru.remove(position);
    }
    session.final_query_cache_lru.push_back(key);

    while session.final_query_cache.len() > FINAL_QUERY_CACHE_MAX_ENTRIES {
        let Some(oldest) = session.final_query_cache_lru.pop_front() else {
            break;
        };
        session.final_query_cache.remove(&oldest);
    }
}

pub(crate) fn can_use_indexed_prefix_cache(
    cache: &IndexedPrefixCache,
    prefix_cache_eligible: bool,
    normalized_query: &str,
    indexed_filter: &SearchFilter,
) -> bool {
    if !prefix_cache_eligible {
        return false;
    }
    if cache.seed_items.is_empty() || cache.normalized_query.is_empty() {
        return false;
    }
    if !indexed_filter_matches_for_prefix_cache(&cache.indexed_filter, indexed_filter) {
        return false;
    }
    normalized_query.len() > cache.normalized_query.len()
        && normalized_query.starts_with(&cache.normalized_query)
}

pub(crate) fn indexed_filter_matches_for_prefix_cache(a: &SearchFilter, b: &SearchFilter) -> bool {
    a.mode == b.mode
        && a.kind_filter == b.kind_filter
        && a.extension_filter == b.extension_filter
        && a.modified_within == b.modified_within
        && a.created_within == b.created_within
}

pub(crate) fn is_prefix_cache_eligible_query(
    parsed_query: &ParsedQuery,
    short_query_app_bias: bool,
) -> bool {
    if short_query_app_bias || parsed_query.command_mode {
        return false;
    }
    if parsed_query.mode_override.is_some()
        || parsed_query.kind_filter.is_some()
        || parsed_query.extension_filter.is_some()
        || !parsed_query.exclude_terms.is_empty()
        || parsed_query.modified_within.is_some()
        || parsed_query.created_within.is_some()
    {
        return false;
    }
    if parsed_query.free_text.trim().is_empty() {
        return false;
    }
    parsed_query.raw.trim() == parsed_query.free_text.trim()
}

pub(crate) fn sanitize_query_for_profile_log(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return "-".to_string();
    }
    // Debug mode: show raw truncated query
    if std::env::var("NEX_DEBUG_QUERY_TEXT").as_deref() == Ok("1") {
        let mut cleaned = String::new();
        for ch in trimmed.chars().take(48) {
            if ch.is_control() {
                cleaned.push(' ');
            } else {
                cleaned.push(ch);
            }
        }
        return cleaned.trim().to_string();
    }
    // Default: hash the query so support bundles never expose typed text
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    trimmed.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:016x}", hash)
}
