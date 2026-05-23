use crate::config;
#[cfg(target_os = "windows")]
use crate::runtime_process::inspect_runtime_process_state;
use crate::runtime::{log_info, RuntimeError};
use std::path::Path;

pub(crate) const QUERY_PROFILE_LOG_THRESHOLD_MS: u128 = 35;
pub(crate) const SHORT_QUERY_APP_BIAS_MAX_LEN: usize = 2;
const QUERY_PROFILE_STATUS_SAMPLE_WINDOW: usize = 400;
const CURRENT_LOG_PREFIX: &str = "[nex]";
const LEGACY_LOG_PREFIXES: &[&str] = &["[nex-core]", "[swiftfind-core]"];

pub(crate) fn env_var_with_legacy(
    current: &str,
    legacy: &str,
) -> Result<String, std::env::VarError> {
    std::env::var(current).or_else(|_| std::env::var(legacy))
}

fn runtime_log_prefixes() -> impl Iterator<Item = &'static str> {
    std::iter::once(CURRENT_LOG_PREFIX).chain(LEGACY_LOG_PREFIXES.iter().copied())
}

fn runtime_log_marker(prefix: &str, marker: &str) -> String {
    format!("{prefix} {marker}")
}

fn rfind_runtime_log_marker(content: &str, marker: &str) -> Option<usize> {
    runtime_log_prefixes()
        .filter_map(|prefix| content.rfind(&runtime_log_marker(prefix, marker)))
        .max()
}

fn line_contains_runtime_log_marker(line: &str, marker: &str) -> bool {
    runtime_log_prefixes().any(|prefix| line.contains(&runtime_log_marker(prefix, marker)))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StatusDiagnosticsSnapshot {
    pub(crate) hotkey_registration_issue_line: Option<String>,
    pub(crate) overlay_ready_line: Option<String>,
    pub(crate) hotkey_ready_line: Option<String>,
    pub(crate) indexing_started_line: Option<String>,
    pub(crate) indexing_completed_line: Option<String>,
    pub(crate) cache_applied_line: Option<String>,
    pub(crate) startup_index_line: Option<String>,
    pub(crate) last_provider_line: Option<String>,
    pub(crate) last_provider_freshness_line: Option<String>,
    pub(crate) last_stale_prune_line: Option<String>,
    pub(crate) last_cache_compaction_line: Option<String>,
    pub(crate) last_icon_cache_line: Option<String>,
    pub(crate) last_overlay_tuning_line: Option<String>,
    pub(crate) last_memory_snapshot_line: Option<String>,
    pub(crate) last_config_reload_line: Option<String>,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueryProfileSample {
    total_ms: u128,
    indexed_ms: u128,
    query_len: usize,
    short_app_bias: bool,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueryProfileSummary {
    pub(crate) samples: usize,
    pub(crate) p50_total_ms: u128,
    pub(crate) p95_total_ms: u128,
    pub(crate) p99_total_ms: u128,
    pub(crate) max_total_ms: u128,
    pub(crate) avg_total_ms: u128,
    pub(crate) p95_indexed_ms: u128,
    pub(crate) short_query_samples: usize,
    pub(crate) short_query_p95_total_ms: u128,
    pub(crate) short_query_app_bias_rate_pct: u8,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryProfileStatusReport {
    pub(crate) recent: Option<QueryProfileSummary>,
    pub(crate) historical: Option<QueryProfileSummary>,
    pub(crate) recent_skipped_symbol_queries: usize,
    pub(crate) historical_skipped_symbol_queries: usize,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn command_status_json() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let state = inspect_runtime_process_state();
        let lifecycle = if state.has_overlay_window {
            "running"
        } else if !state.other_runtime_pids.is_empty() {
            "degraded"
        } else {
            "stopped"
        };

        let snapshot = load_status_diagnostics_snapshot();
        let report = load_query_profile_status_report();
        let diagnostics = snapshot
            .as_ref()
            .map(build_status_diagnostics_json)
            .unwrap_or_else(|| serde_json::json!({}));

        let payload = serde_json::json!({
            "runtime_state": lifecycle,
            "has_overlay_window": state.has_overlay_window,
            "other_runtime_pids": state.other_runtime_pids,
            "diagnostics": diagnostics,
            "query_latency": report.map(query_profile_report_json),
        });
        let encoded = serde_json::to_string_pretty(&payload)
            .map_err(|error| RuntimeError::Args(format!("status-json encode error: {error}")))?;
        println!("{encoded}");
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let payload = serde_json::json!({
            "runtime_state": "unsupported_platform",
            "has_overlay_window": false,
            "other_runtime_pids": Vec::<u32>::new(),
            "diagnostics": serde_json::json!({}),
            "query_latency": serde_json::Value::Null,
        });
        let encoded = serde_json::to_string_pretty(&payload)
            .map_err(|error| RuntimeError::Args(format!("status-json encode error: {error}")))?;
        println!("{encoded}");
        Ok(())
    }
}

pub(crate) fn command_diagnostics_bundle() -> Result<(), RuntimeError> {
    let cfg = config::load(None)?;
    let output_dir = write_diagnostics_bundle(&cfg)?;
    log_info(&format!(
        "[nex] diagnostics bundle written to {}",
        output_dir.display()
    ));
    Ok(())
}

pub(crate) fn command_probe_everything() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let cfg = config::load(None)?;
        let enabled = cfg.everything_search_enabled;

        if !enabled {
            log_info("[nex] everything_probe enabled=disabled_in_config");
            log_info(
                "[nex] everything_probe hint=Set everything_search_enabled: true in your config",
            );
            return Ok(());
        }

        match crate::everything::probe_everything_sdk() {
            Ok(true) => {
                log_info("[nex] everything_probe status=ok sdk_loaded=true");
                log_info("[nex] everything_probe hint=Everything SDK loaded successfully. Nex will use Everything for instant search.");
            }
            Ok(_) => {
                log_info(
                    "[nex] everything_probe status=unavailable sdk_loaded=false reason=unknown",
                );
                log_info("[nex] everything_probe hint=Nex will fall back to filesystem provider (Windows Search / walkdir).");
            }
            Err(msg) => {
                log_info(&format!(
                    "[nex] everything_probe status=unavailable sdk_loaded=false reason={msg}"
                ));
                log_info("[nex] everything_probe hint=Nex will fall back to filesystem provider (Windows Search / walkdir).");
            }
        }

        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] everything_probe status=unsupported_platform");
        log_info("[nex] everything_probe hint=Everything SDK is only available on Windows.");
        Ok(())
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn load_status_diagnostics_snapshot() -> Option<StatusDiagnosticsSnapshot> {
    let content = crate::logging::candidate_log_paths()
        .into_iter()
        .find_map(|log_path| std::fs::read_to_string(log_path).ok())?;
    parse_status_diagnostics_snapshot(&content)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn load_query_profile_status_report() -> Option<QueryProfileStatusReport> {
    let content = crate::logging::candidate_log_paths()
        .into_iter()
        .find_map(|log_path| std::fs::read_to_string(log_path).ok())?;
    summarize_query_profile_status_report(&content)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_status_diagnostics_snapshot(
    content: &str,
) -> Option<StatusDiagnosticsSnapshot> {
    let hotkey_registration_issue_line =
        latest_line_with_token(content, "hotkey_registration_issue ");
    let overlay_ready_line = latest_line_with_token(content, "startup_phase phase=overlay_ready ");
    let hotkey_ready_line = latest_line_with_token(content, "startup_phase phase=hotkey_ready ");
    let indexing_started_line =
        latest_line_with_token(content, "startup_phase phase=indexing_started ");
    let indexing_completed_line =
        latest_line_with_token(content, "startup_phase phase=indexing_completed ");
    let cache_applied_line = latest_line_with_token(content, "startup_phase phase=cache_applied ");
    let startup_index_line = latest_line_with_token(content, "startup indexed_items=");
    let last_provider_line = latest_line_with_token(content, "index_provider name=");
    let last_provider_freshness_line = latest_line_with_token(content, "provider_freshness ");
    let last_stale_prune_line = latest_line_with_token(content, "stale_prune ");
    let last_cache_compaction_line = latest_line_with_token(content, "cache_compaction ");
    let last_icon_cache_line = latest_line_with_token(content, "overlay_icon_cache reason=");
    let last_overlay_tuning_line = latest_line_with_token(content, "overlay_tuning ");
    let last_memory_snapshot_line = latest_line_with_token(content, "memory_snapshot reason=");
    let last_config_reload_line = latest_line_with_token(content, "config reloaded ");

    if hotkey_registration_issue_line.is_none()
        && overlay_ready_line.is_none()
        && hotkey_ready_line.is_none()
        && indexing_started_line.is_none()
        && indexing_completed_line.is_none()
        && cache_applied_line.is_none()
        && startup_index_line.is_none()
        && last_provider_line.is_none()
        && last_provider_freshness_line.is_none()
        && last_stale_prune_line.is_none()
        && last_cache_compaction_line.is_none()
        && last_icon_cache_line.is_none()
        && last_overlay_tuning_line.is_none()
        && last_memory_snapshot_line.is_none()
        && last_config_reload_line.is_none()
    {
        return None;
    }

    Some(StatusDiagnosticsSnapshot {
        hotkey_registration_issue_line,
        overlay_ready_line,
        hotkey_ready_line,
        indexing_started_line,
        indexing_completed_line,
        cache_applied_line,
        startup_index_line,
        last_provider_line,
        last_provider_freshness_line,
        last_stale_prune_line,
        last_cache_compaction_line,
        last_icon_cache_line,
        last_overlay_tuning_line,
        last_memory_snapshot_line,
        last_config_reload_line,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn latest_line_with_token(content: &str, token: &str) -> Option<String> {
    content
        .lines()
        .rev()
        .find(|line| line.contains(token))
        .map(str::to_string)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn summarize_query_profile_status_report(content: &str) -> Option<QueryProfileStatusReport> {
    let recent_samples = parse_recent_query_profile_samples(content);
    let historical_samples = parse_query_profile_samples(content);
    let recent = summarize_query_profile_samples(&recent_samples);
    let historical = summarize_query_profile_samples(&historical_samples);
    if recent.is_none() && historical.is_none() {
        return None;
    }

    let recent_lines = recent_runtime_log_slice(content);
    let recent_skipped_symbol_queries = count_skipped_symbol_query_guards(recent_lines);
    let historical_skipped_symbol_queries = count_skipped_symbol_query_guards(content);

    Some(QueryProfileStatusReport {
        recent,
        historical,
        recent_skipped_symbol_queries,
        historical_skipped_symbol_queries,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn build_status_diagnostics_json(
    snapshot: &StatusDiagnosticsSnapshot,
) -> serde_json::Value {
    let hotkey_issue = build_phase_status_json(snapshot.hotkey_registration_issue_line.as_ref());
    let overlay_ready = build_phase_status_json(snapshot.overlay_ready_line.as_ref());
    let hotkey_ready = build_phase_status_json(snapshot.hotkey_ready_line.as_ref());
    let indexing_started = build_phase_status_json(snapshot.indexing_started_line.as_ref());
    let indexing_completed = build_phase_status_json(snapshot.indexing_completed_line.as_ref());
    let cache_applied = build_phase_status_json(snapshot.cache_applied_line.as_ref());
    let startup_indexing = snapshot
        .startup_index_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let provider = snapshot
        .last_provider_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let provider_freshness = snapshot
        .last_provider_freshness_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let stale_prune = snapshot
        .last_stale_prune_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let cache_compaction = snapshot
        .last_cache_compaction_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let icon_cache = snapshot
        .last_icon_cache_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let overlay_tuning = snapshot
        .last_overlay_tuning_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let memory_snapshot = snapshot
        .last_memory_snapshot_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let config_reload = snapshot
        .last_config_reload_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let config_reload_epoch_secs = snapshot
        .last_config_reload_line
        .as_ref()
        .and_then(|line| parse_log_line_epoch_secs(line));

    serde_json::json!({
        "startup_lifecycle": {
            "overlay_ready": overlay_ready,
            "hotkey_ready": hotkey_ready,
            "indexing_started": indexing_started,
            "indexing_completed": indexing_completed,
            "cache_applied": cache_applied,
        },
        "hotkey_issue": hotkey_issue,
        "startup_indexing": startup_indexing,
        "provider": provider,
        "provider_freshness": provider_freshness,
        "stale_prune": stale_prune,
        "cache_compaction": cache_compaction,
        "icon_cache": icon_cache,
        "overlay_tuning": overlay_tuning,
        "memory_snapshot": memory_snapshot,
        "config_reload": config_reload,
        "config_reload_epoch_secs": config_reload_epoch_secs,
        "raw": {
            "hotkey_issue_line": snapshot.hotkey_registration_issue_line,
            "overlay_ready_line": snapshot.overlay_ready_line,
            "hotkey_ready_line": snapshot.hotkey_ready_line,
            "indexing_started_line": snapshot.indexing_started_line,
            "indexing_completed_line": snapshot.indexing_completed_line,
            "cache_applied_line": snapshot.cache_applied_line,
            "startup_indexing_line": snapshot.startup_index_line,
            "provider_line": snapshot.last_provider_line,
            "provider_freshness_line": snapshot.last_provider_freshness_line,
            "stale_prune_line": snapshot.last_stale_prune_line,
            "cache_compaction_line": snapshot.last_cache_compaction_line,
            "icon_cache_line": snapshot.last_icon_cache_line,
            "overlay_tuning_line": snapshot.last_overlay_tuning_line,
            "memory_snapshot_line": snapshot.last_memory_snapshot_line,
            "config_reload_line": snapshot.last_config_reload_line,
        }
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_phase_status_json(line: Option<&String>) -> serde_json::Value {
    let tokens = line.and_then(|value| parse_key_value_tokens(value));
    let epoch_secs = line.and_then(|value| parse_log_line_epoch_secs(value));
    serde_json::json!({
        "tokens": tokens,
        "epoch_secs": epoch_secs,
        "line": line.cloned(),
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn query_profile_report_json(report: QueryProfileStatusReport) -> serde_json::Value {
    serde_json::json!({
        "recent": report.recent.map(query_profile_summary_json),
        "historical": report.historical.map(query_profile_summary_json),
        "recent_skipped_symbol_queries": report.recent_skipped_symbol_queries,
        "historical_skipped_symbol_queries": report.historical_skipped_symbol_queries,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn query_profile_summary_json(summary: QueryProfileSummary) -> serde_json::Value {
    serde_json::json!({
        "samples": summary.samples,
        "p50_total_ms": summary.p50_total_ms,
        "p95_total_ms": summary.p95_total_ms,
        "p99_total_ms": summary.p99_total_ms,
        "max_total_ms": summary.max_total_ms,
        "avg_total_ms": summary.avg_total_ms,
        "p95_indexed_ms": summary.p95_indexed_ms,
        "short_query_samples": summary.short_query_samples,
        "short_query_p95_total_ms": summary.short_query_p95_total_ms,
        "short_query_app_bias_rate_pct": summary.short_query_app_bias_rate_pct,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_log_line_epoch_secs(line: &str) -> Option<u64> {
    let trimmed = line.trim();
    let start = trimmed.find('[')? + 1;
    let end = trimmed[start..].find(']')? + start;
    trimmed[start..end].parse::<u64>().ok()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_key_value_tokens(line: &str) -> Option<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for token in line.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_end_matches(':');
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_end_matches(',');
        if value.is_empty() {
            continue;
        }
        if let Ok(number) = value.parse::<u64>() {
            map.insert(key.to_string(), serde_json::json!(number));
            continue;
        }
        if let Ok(number) = value.parse::<f64>() {
            map.insert(key.to_string(), serde_json::json!(number));
            continue;
        }
        if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
            map.insert(
                key.to_string(),
                serde_json::json!(value.eq_ignore_ascii_case("true")),
            );
            continue;
        }
        map.insert(
            key.to_string(),
            serde_json::json!(value.trim_matches('"').to_string()),
        );
    }
    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

#[cfg(test)]
pub(crate) fn summarize_query_profiles(content: &str) -> Option<QueryProfileSummary> {
    let samples = parse_recent_query_profile_samples(content);
    summarize_query_profile_samples(&samples)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn summarize_query_profile_samples(samples: &[QueryProfileSample]) -> Option<QueryProfileSummary> {
    let mut samples = samples.to_vec();
    if samples.is_empty() {
        return None;
    }

    if samples.len() > QUERY_PROFILE_STATUS_SAMPLE_WINDOW {
        samples.drain(0..(samples.len() - QUERY_PROFILE_STATUS_SAMPLE_WINDOW));
    }
    if samples.is_empty() {
        return None;
    }

    let mut total_ms: Vec<u128> = samples.iter().map(|sample| sample.total_ms).collect();
    let mut indexed_ms: Vec<u128> = samples.iter().map(|sample| sample.indexed_ms).collect();
    let max_total_ms = total_ms.iter().copied().max().unwrap_or(0);
    let avg_total_ms = total_ms.iter().sum::<u128>() / (total_ms.len() as u128);
    let p50_total_ms = percentile_u128(&mut total_ms, 0.50);
    let p95_total_ms = percentile_u128(&mut total_ms, 0.95);
    let p99_total_ms = percentile_u128(&mut total_ms, 0.99);
    let p95_indexed_ms = percentile_u128(&mut indexed_ms, 0.95);

    let short_query_samples: Vec<QueryProfileSample> = samples
        .iter()
        .copied()
        .filter(|sample| sample.query_len <= SHORT_QUERY_APP_BIAS_MAX_LEN)
        .collect();
    let short_query_samples_count = short_query_samples.len();
    let mut short_total_ms: Vec<u128> = short_query_samples
        .iter()
        .map(|sample| sample.total_ms)
        .collect();
    let short_query_p95_total_ms = percentile_u128(&mut short_total_ms, 0.95);
    let short_query_app_bias_count = short_query_samples
        .iter()
        .filter(|sample| sample.short_app_bias)
        .count();
    let short_query_app_bias_rate_pct = if short_query_samples_count == 0 {
        0
    } else {
        ((short_query_app_bias_count * 100) / short_query_samples_count) as u8
    };

    Some(QueryProfileSummary {
        samples: samples.len(),
        p50_total_ms,
        p95_total_ms,
        p99_total_ms,
        max_total_ms,
        avg_total_ms,
        p95_indexed_ms,
        short_query_samples: short_query_samples_count,
        short_query_p95_total_ms,
        short_query_app_bias_rate_pct,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn recent_runtime_log_slice(content: &str) -> &str {
    let Some(pos) = rfind_runtime_log_marker(content, "startup mode=") else {
        return content;
    };
    let line_start = content[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(pos);
    &content[line_start..]
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn count_skipped_symbol_query_guards(content: &str) -> usize {
    content
        .lines()
        .filter(|line| line.contains("query_guard skip=non_searchable_symbol_only"))
        .count()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_recent_query_profile_samples(content: &str) -> Vec<QueryProfileSample> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let start_index = lines
        .iter()
        .rposition(|line| line_contains_runtime_log_marker(line, "startup mode="))
        .unwrap_or(0);
    parse_query_profile_samples(&lines[start_index..].join("\n"))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_query_profile_samples(content: &str) -> Vec<QueryProfileSample> {
    content
        .lines()
        .filter(|line| line_contains_runtime_log_marker(line, "query_profile "))
        .filter_map(|line| {
            let total_ms = parse_u128_field(line, "total_ms=")?;
            let indexed_ms = parse_u128_field(line, "indexed_ms=").unwrap_or(0);
            let query = parse_quoted_field(line, "q=").unwrap_or_default();
            let query_len = query.chars().count();
            let short_app_bias = parse_bool_field(line, "short_app_bias=").unwrap_or(false);
            Some(QueryProfileSample {
                total_ms,
                indexed_ms,
                query_len,
                short_app_bias,
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_u128_field(line: &str, key: &str) -> Option<u128> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let value = tail
        .split_whitespace()
        .next()
        .map(|part| part.trim_end_matches(','))
        .unwrap_or_default();
    value.parse::<u128>().ok()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_bool_field(line: &str, key: &str) -> Option<bool> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let value = tail
        .split_whitespace()
        .next()
        .map(|part| part.trim_end_matches(','))
        .unwrap_or_default();
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_quoted_field(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    if !tail.starts_with('"') {
        return None;
    }
    let end = tail[1..].find('"')?;
    Some(tail[1..(1 + end)].to_string())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn percentile_u128(values: &mut [u128], percentile: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let last = values.len().saturating_sub(1);
    let idx = ((last as f64) * percentile.clamp(0.0, 1.0)).round() as usize;
    values[idx.min(last)]
}

pub(crate) fn write_diagnostics_bundle(
    cfg: &config::Config,
) -> Result<std::path::PathBuf, RuntimeError> {
    let support_dir = config::stable_app_data_dir().join("support");
    std::fs::create_dir_all(&support_dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bundle_dir = support_dir.join(format!("diagnostics-{stamp}"));
    std::fs::create_dir_all(&bundle_dir)?;

    let running_state = runtime_state_summary();
    let summary = format!(
        "nex diagnostics bundle\ngenerated_epoch_secs={stamp}\nruntime_state={running_state}\nconfig_path={}\nindex_db_path={}\nlogs_dir={}\n",
        cfg.config_path.display(),
        cfg.index_db_path.display(),
        crate::logging::logs_dir().display()
    );
    std::fs::write(bundle_dir.join("summary.txt"), summary)?;

    if cfg.config_path.exists() {
        let raw_ext = cfg
            .config_path
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|ext| !ext.trim().is_empty())
            .unwrap_or("txt");
        let _ = std::fs::copy(
            &cfg.config_path,
            bundle_dir.join(format!("config.raw.{raw_ext}")),
        );
    }

    let sanitized_cfg = serde_json::json!({
        "version": cfg.version,
        "max_results": cfg.max_results,
        "hotkey": cfg.hotkey,
        "launch_at_startup": cfg.launch_at_startup,
        "search_mode_default": cfg.search_mode_default,
        "search_dsl_enabled": cfg.search_dsl_enabled,
        "uninstall_actions_enabled": cfg.uninstall_actions_enabled,
        "web_search_provider": cfg.web_search_provider,
        "clipboard_enabled": cfg.clipboard_enabled,
        "clipboard_retention_minutes": cfg.clipboard_retention_minutes,
        "clipboard_exclude_sensitive_patterns_count": cfg.clipboard_exclude_sensitive_patterns.len(),
        "plugins_enabled": cfg.plugins_enabled,
        "plugin_paths_count": cfg.plugin_paths.len(),
        "plugins_safe_mode": cfg.plugins_safe_mode,
        "game_mode_enabled": cfg.game_mode_enabled,
        "idle_cache_trim_ms": cfg.idle_cache_trim_ms,
        "active_memory_target_mb": cfg.active_memory_target_mb,
        "index_max_items_total": cfg.index_max_items_total,
        "index_max_items_per_root": cfg.index_max_items_per_root,
        "index_max_items_per_query_seed": cfg.index_max_items_per_query_seed,
        "discovery_roots_count": cfg.discovery_roots.len(),
        "discovery_exclude_roots_count": cfg.discovery_exclude_roots.len(),
        "show_files": cfg.show_files,
        "show_folders": cfg.show_folders
    });
    let encoded = serde_json::to_string_pretty(&sanitized_cfg)
        .map_err(|e| RuntimeError::Args(format!("failed to encode sanitized config: {e}")))?;
    std::fs::write(bundle_dir.join("config.sanitized.json"), encoded)?;

    copy_recent_logs_to_bundle(&crate::logging::logs_dir(), &bundle_dir.join("logs"))?;

    Ok(bundle_dir)
}

fn copy_recent_logs_to_bundle(
    source_logs_dir: &Path,
    target_logs_dir: &Path,
) -> Result<(), RuntimeError> {
    std::fs::create_dir_all(target_logs_dir)?;
    if !source_logs_dir.exists() {
        return Ok(());
    }

    let mut entries = std::fs::read_dir(source_logs_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    entries.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    entries.reverse();

    for path in entries.into_iter().take(5) {
        if let Some(name) = path.file_name() {
            let _ = std::fs::copy(&path, target_logs_dir.join(name));
        }
    }

    Ok(())
}

fn runtime_state_summary() -> String {
    #[cfg(target_os = "windows")]
    {
        let state = inspect_runtime_process_state();
        if state.has_overlay_window {
            return "running".to_string();
        }
        if !state.other_runtime_pids.is_empty() {
            return format!(
                "degraded(process_without_overlay_window pids={:?})",
                state.other_runtime_pids
            );
        }
        "stopped".to_string()
    }

    #[cfg(not(target_os = "windows"))]
    {
        "unsupported_platform".to_string()
    }
}
