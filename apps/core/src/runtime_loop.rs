#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};

use crate::clipboard_history;
#[cfg(target_os = "windows")]
use crate::config::Config;
#[cfg(target_os = "windows")]
use crate::core_service::CoreService;
#[cfg(target_os = "windows")]
use crate::hotkey_runtime::default_hotkey_registrar;
#[cfg(target_os = "windows")]
use crate::overlay_state::{HotkeyAction, OverlayState};
#[cfg(target_os = "windows")]
use crate::plugin_sdk::PluginRegistry;
#[cfg(target_os = "windows")]
use crate::query_dsl::ParsedQuery;
#[cfg(target_os = "windows")]
use crate::runtime::{log_info, log_warn, RuntimeError};
#[cfg(target_os = "windows")]
use crate::runtime_actions::{
    execute_action_selection, launch_overlay_selection, should_suppress_failed_uninstall,
    uninstall_confirmation_results,
};
#[cfg(target_os = "windows")]
use crate::runtime_hotkey::{should_suppress_hotkey_for_game_mode, toggle_game_mode_from_tray};
#[cfg(target_os = "windows")]
use crate::runtime_index::{
    config_file_modified_time, maybe_apply_background_index_refresh,
    maybe_apply_runtime_config_reload, maybe_show_background_index_ready_notice,
    should_show_indexing_status, start_background_index_refresh, BackgroundIndexRefresh,
    RuntimeConfigWatcher,
};
#[cfg(target_os = "windows")]
use crate::runtime_overlay_rows::{
    filter_suppressed_uninstall_results, next_selection_index, overlay_rows,
    reconcile_suppressed_uninstall_titles, set_idle_overlay_state, set_status_row_overlay_state,
    track_uninstall_title_suppression, PendingUninstallConfirmation, ACTION_UNINSTALL_CANCEL_ID,
    ACTION_UNINSTALL_CONFIRM_ID, STATUS_ROW_INDEXING, STATUS_ROW_NO_COMMAND_RESULTS,
    STATUS_ROW_NO_RESULTS, STATUS_ROW_TYPE_TO_SEARCH,
};
#[cfg(target_os = "windows")]
use crate::runtime_process::{
    acquire_single_instance_guard, hotkey_registration_recovery_message,
    hotkey_registration_status_text, launch_stable_updater, log_registration,
};
#[cfg(target_os = "windows")]
use crate::runtime_search_session::{
    maybe_expand_uninstall_quick_shortcut, result_limit_for_query, OverlaySearchSession,
};
#[cfg(target_os = "windows")]
use crate::search_worker::SearchWorker;
#[cfg(target_os = "windows")]
use crate::windows_overlay::types::NEX_WM_SEARCH_RESULTS_READY;
#[cfg(target_os = "windows")]
use crate::windows_overlay::{signal_existing_instance_show, NativeOverlayShell, OverlayEvent};
#[cfg(target_os = "windows")]
use std::time::Instant;

#[cfg(target_os = "windows")]
pub(crate) fn run_windows_runtime(
    startup_started_at: Instant,
    mut runtime_config: Config,
    service: CoreService,
) -> Result<(), RuntimeError> {
    let service = Arc::new(Mutex::new(service));
    let mut background_index_refresh = {
        let initial_cached_items = {
            let guard = service.lock().unwrap();
            guard.cached_items_len()
        };
        log_info(&format!(
            "[nex] startup cached_items={} (async indexing scheduled)",
            initial_cached_items
        ));
        log_info(&format!(
            "[nex] startup_phase phase=indexing_started elapsed_ms={} initial_cache_empty={} cached_items={}",
            startup_started_at.elapsed().as_millis(),
            initial_cached_items == 0,
            initial_cached_items
        ));
        start_background_index_refresh(
            service.clone(),
            initial_cached_items == 0,
            startup_started_at,
        )
    };

    let mut plugin_registry = PluginRegistry::load_from_config(&runtime_config);
    for warning in &plugin_registry.load_warnings {
        log_warn(&format!("[nex] plugin_warning {warning}"));
    }
    log_info(&format!(
        "[nex] plugins loaded provider_items={} action_items={}",
        plugin_registry.provider_items.len(),
        plugin_registry.action_items.len()
    ));

    unsafe {
        let _ = windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Err(error) = crate::startup::set_enabled(runtime_config.launch_at_startup, &exe) {
            log_warn(&format!("[nex] startup sync warning: {error}"));
        }
    }

    let _single_instance = match acquire_single_instance_guard() {
        Ok(guard) => guard,
        Err(error) => return Err(RuntimeError::Overlay(error)),
    };
    if _single_instance.is_none() {
        let _ = signal_existing_instance_show();
        log_info("[nex] runtime already active; signaled existing instance");
        return Ok(());
    }

    let mut overlay_state = OverlayState::default();
    let overlay = NativeOverlayShell::create().map_err(RuntimeError::Overlay)?;
    overlay.set_help_config_path(runtime_config.config_path.to_string_lossy().as_ref());
    overlay.set_hotkey_hint(&runtime_config.hotkey);
    overlay.set_performance_tuning(
        runtime_config.idle_cache_trim_ms,
        runtime_config.active_memory_target_mb,
    );
    overlay.set_game_mode_enabled(runtime_config.game_mode_enabled);
    log_info("[nex] native overlay shell initialized (hidden)");
    log_info(&format!(
        "[nex] startup_phase phase=overlay_ready elapsed_ms={}",
        startup_started_at.elapsed().as_millis()
    ));

    let search_worker = SearchWorker::new(
        service.clone(),
        runtime_config.clone(),
        Arc::new(plugin_registry.clone()),
        overlay.hwnd as isize,
        NEX_WM_SEARCH_RESULTS_READY,
    );

    let mut registrar = default_hotkey_registrar();
    let hotkey_issue_status = match registrar.register_hotkey(&runtime_config.hotkey) {
        Ok(registration) => {
            log_registration(&registration);
            overlay.set_hotkey_issue_active(false);
            None
        }
        Err(error) => {
            let recovery_message = hotkey_registration_recovery_message(
                &runtime_config.hotkey,
                &runtime_config.config_path,
            );
            let suggested =
                crate::settings::suggested_hotkey_presets(&runtime_config.hotkey, 3).join("|");
            log_warn(&format!(
                "[nex] hotkey_registration_issue hotkey={} suggestions={} error={:?}",
                runtime_config.hotkey, suggested, error
            ));
            log_warn(&format!("[nex] {recovery_message}"));
            overlay.set_hotkey_issue_active(true);
            Some(hotkey_registration_status_text(&runtime_config.hotkey))
        }
    };
    log_info(&format!(
        "[nex] startup_phase phase=hotkey_ready elapsed_ms={} hotkey={}",
        startup_started_at.elapsed().as_millis(),
        runtime_config.hotkey
    ));
    log_info("[nex] event loop running (native overlay)");

    let mut max_results = runtime_config.max_results as usize;
    let mut config_watcher = RuntimeConfigWatcher {
        path: runtime_config.config_path.clone(),
        last_checked: Instant::now(),
        last_modified: config_file_modified_time(runtime_config.config_path.as_path()),
    };
    let mut current_results: Vec<crate::model::SearchItem> = Vec::new();
    let mut suppressed_uninstall_titles: Vec<String> = Vec::new();
    let mut pending_uninstall_confirmation: Option<PendingUninstallConfirmation> = None;
    let mut selected_index = 0_usize;
    let mut last_query = String::new();
    let mut last_sent_generation: u64 = 0;
    let mut search_session = OverlaySearchSession::default();

    overlay
        .run_message_loop_with_events(|event| {
            maybe_apply_runtime_config_reload(
                &overlay,
                &service,
                &mut runtime_config,
                &mut plugin_registry,
                &mut search_session,
                &mut pending_uninstall_confirmation,
                &mut max_results,
                &mut config_watcher,
                &mut background_index_refresh,
            );
            maybe_apply_background_index_refresh(
                &service,
                &mut background_index_refresh,
                &runtime_config,
            );
            maybe_show_background_index_ready_notice(&overlay, &mut background_index_refresh);
            match event {
                OverlayEvent::Hotkey(_) => {
                    log_info("[nex] hotkey_event received");
                    let overlay_visible = overlay.is_visible();
                    overlay_state.set_visible(overlay_visible);
                    if !overlay_visible && should_suppress_hotkey_for_game_mode(&runtime_config) {
                        log_info(
                            "[nex] hotkey ignored because game mode is active for the foreground app",
                        );
                        return;
                    }
                    let action = overlay_state.on_hotkey(overlay.has_focus());
                    match action {
                        HotkeyAction::ShowAndFocus | HotkeyAction::FocusExisting => {
                            reconcile_suppressed_uninstall_titles(&mut suppressed_uninstall_titles);
                            overlay.show_and_focus();
                            if runtime_config.clipboard_enabled {
                                let _ = clipboard_history::maybe_capture_latest(&runtime_config);
                            }
                            if overlay.query_text().trim().is_empty() {
                                set_idle_overlay_state(&overlay);
                                if let Some(issue) = hotkey_issue_status.as_deref() {
                                    overlay.set_status_text(issue);
                                }
                                maybe_show_background_index_ready_notice(
                                    &overlay,
                                    &mut background_index_refresh,
                                );
                            }
                        }
                        HotkeyAction::Hide => {
                            overlay.hide();
                            reset_overlay_session(
                                &overlay,
                                &mut current_results,
                                &mut selected_index,
                            );
                            pending_uninstall_confirmation = None;
                            last_query.clear();
                            last_sent_generation = 0;
                            search_session.clear();
                            search_worker.clear_session();
                            maybe_apply_background_index_refresh(
                                &service,
                                &mut background_index_refresh,
                                &runtime_config,
                            );
                        }
                    }
                }
                OverlayEvent::ExternalShow => {
                    overlay_state.set_visible(overlay.is_visible());
                    reconcile_suppressed_uninstall_titles(&mut suppressed_uninstall_titles);
                    overlay.show_and_focus();
                    if runtime_config.clipboard_enabled {
                        let _ = clipboard_history::maybe_capture_latest(&runtime_config);
                    }
                    if overlay.query_text().trim().is_empty() {
                        set_idle_overlay_state(&overlay);
                        if let Some(issue) = hotkey_issue_status.as_deref() {
                            overlay.set_status_text(issue);
                        }
                        maybe_show_background_index_ready_notice(
                            &overlay,
                            &mut background_index_refresh,
                        );
                    }
                }
                OverlayEvent::ExternalQuit => {
                    overlay.hide_now();
                    last_query.clear();
                    last_sent_generation = 0;
                    search_session.clear();
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                    }
                }
                OverlayEvent::TrayToggleGameMode => {
                    toggle_game_mode_from_tray(&overlay, &mut runtime_config);
                }
                OverlayEvent::TrayCheckForUpdates => {
                    match launch_stable_updater() {
                        Ok(_) => overlay.set_status_text("Updater launched"),
                        Err(error) => {
                            log_warn(&format!("[nex] updater launch failed from tray: {error}"));
                            overlay.set_status_text("Could not launch updater");
                        }
                    }
                }
                OverlayEvent::Escape => {
                    if overlay_state.on_escape() {
                        overlay.hide_now();
                        reset_overlay_session(
                            &overlay,
                            &mut current_results,
                            &mut selected_index,
                        );
                        pending_uninstall_confirmation = None;
                        last_query.clear();
                        last_sent_generation = 0;
                        search_session.clear();
                    }
                }
                OverlayEvent::QueryChanged(query) => {
                    apply_query_change(
                        query,
                        &overlay,
                        &search_worker,
                        &runtime_config,
                        max_results,
                        &mut pending_uninstall_confirmation,
                        &mut current_results,
                        &mut selected_index,
                        &mut last_query,
                        &mut last_sent_generation,
                    );
                }
                OverlayEvent::SearchResultsReady => {
                    apply_search_results(
                        &search_worker,
                        &overlay,
                        &runtime_config,
                        &background_index_refresh,
                        &suppressed_uninstall_titles,
                        &mut current_results,
                        &mut selected_index,
                        last_sent_generation,
                    );
                }
                OverlayEvent::MoveSelection(direction) => {
                    if current_results.is_empty() {
                        return;
                    }

                    selected_index =
                        next_selection_index(selected_index, current_results.len(), direction);
                    overlay.set_selected_index(selected_index);
                }
                OverlayEvent::Submit => {
                    if current_results.is_empty() {
                        if overlay.query_text().trim().is_empty() {
                            set_idle_overlay_state(&overlay);
                            overlay.show_placeholder_hint(STATUS_ROW_TYPE_TO_SEARCH);
                        } else if should_show_indexing_status(&background_index_refresh) {
                            set_status_row_overlay_state(&overlay, STATUS_ROW_INDEXING);
                        } else {
                            let parsed_query = ParsedQuery::parse(
                                overlay.query_text().trim(),
                                runtime_config.search_dsl_enabled,
                            );
                            set_status_row_overlay_state(
                                &overlay,
                                if parsed_query.command_mode {
                                    STATUS_ROW_NO_COMMAND_RESULTS
                                } else {
                                    STATUS_ROW_NO_RESULTS
                                },
                            );
                        }
                        return;
                    }

                    if let Some(list_selection) = overlay.selected_index() {
                        selected_index = list_selection.min(current_results.len() - 1);
                    }

                    let selected = &current_results[selected_index];
                    if pending_uninstall_confirmation.is_some() {
                        let selected_id = selected.id.clone();
                        if selected_id == ACTION_UNINSTALL_CONFIRM_ID {
                            let Some(pending) = pending_uninstall_confirmation.take() else {
                                return;
                            };
                            overlay.hide_now();
                            overlay_state.on_escape();
                            match execute_action_selection(
                                &*service.lock().unwrap(),
                                &runtime_config,
                                &plugin_registry,
                                &pending.uninstall_action,
                            ) {
                                Ok(()) => {
                                    track_uninstall_title_suppression(
                                        &mut suppressed_uninstall_titles,
                                        pending.uninstall_action.title.as_str(),
                                    );
                                    overlay.set_status_text("");
                                    reset_overlay_session(
                                        &overlay,
                                        &mut current_results,
                                        &mut selected_index,
                                    );
                                    last_query.clear();
                                    last_sent_generation = 0;
                                    search_session.clear();
                            search_worker.clear_session();
                                }
                                Err(error) => {
                                    if should_suppress_failed_uninstall(error.as_str()) {
                                        track_uninstall_title_suppression(
                                            &mut suppressed_uninstall_titles,
                                            pending.uninstall_action.title.as_str(),
                                        );
                                        current_results = pending.previous_results;
                                        filter_suppressed_uninstall_results(
                                            &mut current_results,
                                            &suppressed_uninstall_titles,
                                        );
                                        selected_index = pending
                                            .previous_selected_index
                                            .min(current_results.len().saturating_sub(1));
                                        if current_results.is_empty() {
                                            set_status_row_overlay_state(
                                                &overlay,
                                                if pending.previous_command_mode {
                                                    STATUS_ROW_NO_COMMAND_RESULTS
                                                } else {
                                                    STATUS_ROW_NO_RESULTS
                                                },
                                            );
                                        } else {
                                            let rows = overlay_rows(
                                                &current_results,
                                                pending.previous_command_mode,
                                            );
                                            overlay.set_results(&rows, selected_index);
                                        }
                                        overlay.set_status_text(
                                            "Uninstall entry is stale and was hidden",
                                        );
                                    } else {
                                        pending_uninstall_confirmation = Some(pending);
                                        overlay.show_and_focus();
                                        overlay
                                            .set_status_text(&format!("Launch error: {error}"));
                                    }
                                }
                            }
                            return;
                        }

                        if selected_id == ACTION_UNINSTALL_CANCEL_ID {
                            let Some(pending) = pending_uninstall_confirmation.take() else {
                                return;
                            };
                            current_results = pending.previous_results;
                            selected_index = pending
                                .previous_selected_index
                                .min(current_results.len().saturating_sub(1));
                            if current_results.is_empty() {
                                set_status_row_overlay_state(
                                    &overlay,
                                    if pending.previous_command_mode {
                                        STATUS_ROW_NO_COMMAND_RESULTS
                                    } else {
                                        STATUS_ROW_NO_RESULTS
                                    },
                                );
                            } else {
                                let rows =
                                    overlay_rows(&current_results, pending.previous_command_mode);
                                overlay.set_results(&rows, selected_index);
                            }
                            overlay.set_status_text("");
                            return;
                        }

                        pending_uninstall_confirmation = None;
                    }

                    let selected_is_uninstall = selected
                        .id
                        .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX);

                    if selected_is_uninstall {
                        let parsed_query = ParsedQuery::parse(
                            overlay.query_text().trim(),
                            runtime_config.search_dsl_enabled,
                        );
                        pending_uninstall_confirmation = Some(PendingUninstallConfirmation {
                            uninstall_action: selected.clone(),
                            previous_results: current_results.clone(),
                            previous_selected_index: selected_index,
                            previous_command_mode: parsed_query.command_mode,
                        });
                        current_results = uninstall_confirmation_results(selected);
                        selected_index = 0;
                        let rows = overlay_rows(&current_results, true);
                        overlay.set_results(&rows, selected_index);
                        overlay.set_status_text("");
                        return;
                    }

                    if selected.id == crate::action_registry::ACTION_TRIM_MEMORY_ID {
                        search_session.clear();
                        overlay.trim_runtime_memory();
                        overlay.set_status_text("Memory caches trimmed");
                        return;
                    }

                    match launch_overlay_selection(
                        &*service.lock().unwrap(),
                        &runtime_config,
                        &plugin_registry,
                        &current_results,
                        selected_index,
                        last_query.as_str(),
                    ) {
                        Ok(()) => {
                            overlay.set_status_text("");
                            overlay.hide_now();
                            overlay_state.on_escape();
                            reset_overlay_session(
                                &overlay,
                                &mut current_results,
                                &mut selected_index,
                            );
                            pending_uninstall_confirmation = None;
                            last_query.clear();
                            last_sent_generation = 0;
                            search_session.clear();
                            search_worker.clear_session();
                        }
                        Err(error) => {
                            overlay.set_status_text(&format!("Launch error: {error}"));
                        }
                    }
                }
            }
        })
        .map_err(RuntimeError::Overlay)?;
    registrar.unregister_all()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn reset_overlay_session(
    overlay: &NativeOverlayShell,
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
) {
    overlay.clear_query_text();
    current_results.clear();
    *selected_index = 0;
    set_idle_overlay_state(overlay);
}

#[cfg(target_os = "windows")]
fn apply_query_change(
    query: String,
    overlay: &NativeOverlayShell,
    search_worker: &SearchWorker,
    runtime_config: &Config,
    max_results: usize,
    pending_uninstall_confirmation: &mut Option<PendingUninstallConfirmation>,
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
    last_query: &mut String,
    last_sent_generation: &mut u64,
) {
    *pending_uninstall_confirmation = None;
    let mut query = query;
    if let Some(expanded) = maybe_expand_uninstall_quick_shortcut(&query, last_query.as_str()) {
        overlay.set_query_text(&expanded);
        query = expanded;
    }

    let trimmed = query.trim();
    if trimmed.is_empty() {
        current_results.clear();
        *selected_index = 0;
        last_query.clear();
        *last_sent_generation = 0;
        *pending_uninstall_confirmation = None;
        set_idle_overlay_state(overlay);
        return;
    }
    if trimmed == last_query {
        return;
    }
    *last_query = trimmed.to_string();
    let parsed_query = ParsedQuery::parse(trimmed, runtime_config.search_dsl_enabled);
    let query_result_limit = result_limit_for_query(max_results, &parsed_query);

    let gen = search_worker.send_request(parsed_query, query_result_limit);
    *last_sent_generation = gen;
}

#[cfg(target_os = "windows")]
fn apply_search_results(
    search_worker: &SearchWorker,
    overlay: &NativeOverlayShell,
    _runtime_config: &Config,
    background_index_refresh: &BackgroundIndexRefresh,
    suppressed_uninstall_titles: &[String],
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
    last_sent_generation: u64,
) {
    let Some(result) = search_worker.try_recv() else {
        return;
    };

    if result.generation < last_sent_generation {
        return;
    }

    let command_mode = result.command_mode;

    if let Some(error) = result.error {
        current_results.clear();
        *selected_index = 0;
        overlay.set_results(&[], 0);
        overlay.set_status_text(&format!("Search error: {error}"));
        return;
    }

    let mut results = result.results;
    crate::runtime_overlay_rows::dedupe_overlay_results(&mut results);
    if !suppressed_uninstall_titles.is_empty() {
        filter_suppressed_uninstall_results(&mut results, suppressed_uninstall_titles);
    }
    *current_results = results;
    *selected_index = 0;

    if current_results.is_empty() {
        if should_show_indexing_status(background_index_refresh) {
            set_status_row_overlay_state(overlay, STATUS_ROW_INDEXING);
        } else {
            set_status_row_overlay_state(
                overlay,
                if command_mode {
                    STATUS_ROW_NO_COMMAND_RESULTS
                } else {
                    STATUS_ROW_NO_RESULTS
                },
            );
        }
    } else {
        let rows = overlay_rows(current_results, command_mode);
        overlay.set_results(&rows, *selected_index);
    }
}
