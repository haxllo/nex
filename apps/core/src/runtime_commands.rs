use crate::config::{self};
use crate::runtime::{
    load_query_profile_status_report, load_status_diagnostics_snapshot, log_info, log_warn,
    run_with_options, RuntimeError, RuntimeOptions,
};
#[cfg(target_os = "windows")]
use crate::runtime_process::{inspect_runtime_process_state, stop_runtime_instance, StopRuntimeOutcome};

pub(crate) fn command_ensure_config() -> Result<(), RuntimeError> {
    let cfg = config::load(None)?;
    if !cfg.config_path.exists() {
        config::write_user_template(&cfg, &cfg.config_path)?;
        log_info(&format!(
            "[nex] wrote user config template to {}",
            cfg.config_path.display()
        ));
    }
    log_info(&format!("[nex] config ready at {}", cfg.config_path.display()));
    Ok(())
}

pub(crate) fn command_sync_startup() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let cfg = config::load(None)?;
        let exe = std::env::current_exe()?;
        crate::startup::set_enabled(cfg.launch_at_startup, &exe)?;
        log_info(&format!(
            "[nex] startup registration synced: enabled={}",
            cfg.launch_at_startup
        ));
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] startup sync is unsupported on this platform");
        Ok(())
    }
}

pub(crate) fn command_set_launch_at_startup(enabled: bool) -> Result<(), RuntimeError> {
    let mut cfg = config::load(None)?;
    cfg.launch_at_startup = enabled;
    config::save(&cfg)?;

    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe()?;
        crate::startup::set_enabled(enabled, &exe)?;
    }

    log_info(&format!(
        "[nex] launch_at_startup updated: enabled={} (can be changed in config)",
        enabled
    ));
    Ok(())
}

pub(crate) fn command_status() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let state = inspect_runtime_process_state();
        let running = state.has_overlay_window;
        log_info(&format!(
            "[nex] status: {}",
            if running {
                "running"
            } else if !state.other_runtime_pids.is_empty() {
                "degraded (process without overlay window)"
            } else {
                "stopped"
            }
        ));
        if !state.other_runtime_pids.is_empty() {
            log_warn(&format!(
                "[nex] status detected runtime_pids_without_window={:?} recommendation=run --restart",
                state.other_runtime_pids
            ));
        }
        if let Some(snapshot) = load_status_diagnostics_snapshot() {
            if let Some(line) = snapshot.hotkey_registration_issue_line {
                log_warn(&format!("[nex] status last_hotkey_issue {line}"));
            }
            if let Some(line) = snapshot.overlay_ready_line {
                log_info(&format!("[nex] status last_overlay_ready {line}"));
            }
            if let Some(line) = snapshot.hotkey_ready_line {
                log_info(&format!("[nex] status last_hotkey_ready {line}"));
            }
            if let Some(line) = snapshot.indexing_started_line {
                log_info(&format!("[nex] status last_indexing_started {line}"));
            }
            if let Some(line) = snapshot.indexing_completed_line {
                log_info(&format!("[nex] status last_indexing_completed {line}"));
            }
            if let Some(line) = snapshot.cache_applied_line {
                log_info(&format!("[nex] status last_cache_applied {line}"));
            }
            if let Some(line) = snapshot.startup_index_line {
                log_info(&format!("[nex] status last_indexing {line}"));
            }
            if let Some(line) = snapshot.last_provider_line {
                log_info(&format!("[nex] status last_provider {line}"));
            }
            if let Some(line) = snapshot.last_provider_freshness_line {
                log_info(&format!("[nex] status last_provider_freshness {line}"));
            }
            if let Some(line) = snapshot.last_stale_prune_line {
                log_info(&format!("[nex] status last_stale_prune {line}"));
            }
            if let Some(line) = snapshot.last_cache_compaction_line {
                log_info(&format!("[nex] status last_cache_compaction {line}"));
            }
            if let Some(line) = snapshot.last_icon_cache_line {
                log_info(&format!("[nex] status last_icon_cache {line}"));
            }
            if let Some(line) = snapshot.last_overlay_tuning_line {
                log_info(&format!("[nex] status last_overlay_tuning {line}"));
            }
            if let Some(line) = snapshot.last_memory_snapshot_line {
                log_info(&format!("[nex] status last_memory_snapshot {line}"));
            }
            if let Some(line) = snapshot.last_config_reload_line {
                log_info(&format!("[nex] status last_config_reload {line}"));
            }
        }
        if let Some(report) = load_query_profile_status_report() {
            if let Some(recent) = report.recent {
                log_info(&format!(
                    "[nex] status query_latency_recent samples={} p50_ms={} p95_ms={} p99_ms={} max_ms={} avg_ms={} indexed_p95_ms={} short_q_samples={} short_q_p95_ms={} short_q_app_bias_rate={}%",
                    recent.samples,
                    recent.p50_total_ms,
                    recent.p95_total_ms,
                    recent.p99_total_ms,
                    recent.max_total_ms,
                    recent.avg_total_ms,
                    recent.p95_indexed_ms,
                    recent.short_query_samples,
                    recent.short_query_p95_total_ms,
                    recent.short_query_app_bias_rate_pct
                ));
            }
            if let Some(historical) = report.historical {
                log_info(&format!(
                    "[nex] status query_latency_historical samples={} p50_ms={} p95_ms={} p99_ms={} max_ms={} avg_ms={} indexed_p95_ms={} short_q_samples={} short_q_p95_ms={} short_q_app_bias_rate={}%",
                    historical.samples,
                    historical.p50_total_ms,
                    historical.p95_total_ms,
                    historical.p99_total_ms,
                    historical.max_total_ms,
                    historical.avg_total_ms,
                    historical.p95_indexed_ms,
                    historical.short_query_samples,
                    historical.short_query_p95_total_ms,
                    historical.short_query_app_bias_rate_pct
                ));
            }
            log_info(&format!(
                "[nex] status query_guard recent_skipped_symbol_queries={} historical_skipped_symbol_queries={}",
                report.recent_skipped_symbol_queries, report.historical_skipped_symbol_queries
            ));
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] status: unsupported on this platform");
        Ok(())
    }
}

pub(crate) fn command_quit() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        match stop_runtime_instance(std::time::Duration::from_secs(3))? {
            StopRuntimeOutcome::AlreadyStopped => {
                log_info("[nex] quit skipped (not running)");
                Ok(())
            }
            StopRuntimeOutcome::Graceful => {
                log_info("[nex] quit completed (graceful)");
                Ok(())
            }
            StopRuntimeOutcome::Forced => {
                log_warn("[nex] quit required forced process termination");
                Ok(())
            }
            StopRuntimeOutcome::Failed => Err(RuntimeError::Overlay(
                "quit failed: runtime is still active after graceful and forced attempts"
                    .to_string(),
            )),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] quit is unsupported on this platform");
        Ok(())
    }
}

pub(crate) fn command_restart() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        match stop_runtime_instance(std::time::Duration::from_secs(3))? {
            StopRuntimeOutcome::Failed => {
                return Err(RuntimeError::Overlay(
                    "restart failed: existing runtime could not be stopped".to_string(),
                ));
            }
            StopRuntimeOutcome::Forced => {
                log_warn("[nex] restart required forced process termination");
            }
            StopRuntimeOutcome::Graceful | StopRuntimeOutcome::AlreadyStopped => {}
        }
        run_with_options(RuntimeOptions::default())
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_with_options(RuntimeOptions::default())
    }
}
