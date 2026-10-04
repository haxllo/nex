pub mod action_executor;
pub mod action_registry;
pub(crate) mod bookmarks;
pub(crate) mod calculator;
pub mod clipboard_history;
pub mod config;
pub mod contract;
pub mod core_service;
#[cfg(target_os = "windows")]
pub(crate) mod console_signal;
pub mod discovery;
#[cfg(target_os = "windows")]
pub(crate) mod everything_bridge;
#[cfg(target_os = "windows")]
pub(crate) mod file_watcher;
#[cfg(target_os = "windows")]
pub(crate) mod file_watcher_consumer;
pub mod hotkey;
pub mod hotkey_runtime;
pub(crate) mod hot_prefix;
pub mod index_store;
pub mod logging;
#[cfg(target_os = "windows")]
pub(crate) mod media;
pub(crate) mod media_position;
pub mod model;
pub mod overlay_state;
pub mod plugin_sdk;
#[cfg(target_os = "windows")]
pub(crate) mod power_actions;
pub mod query_dsl;
pub(crate) mod recent_files;
pub mod runtime;
pub(crate) mod runtime_actions;
pub(crate) mod runtime_commands;
pub(crate) mod runtime_diagnostics;
pub(crate) mod runtime_hotkey;
pub(crate) mod runtime_index;
#[cfg(target_os = "windows")]
pub(crate) mod runtime_loop;
#[cfg(target_os = "windows")]
pub(crate) mod runtime_overlay_rows;
pub(crate) mod runtime_process;
pub(crate) mod runtime_search_session;
pub mod search;
pub(crate) mod search_worker;
pub mod settings;
pub mod settings_catalog;
pub mod startup;
pub(crate) mod tantivy_search;
pub mod transport;
pub mod uninstall_registry;
pub mod updater;
pub mod whats_new;
#[cfg(target_os = "windows")]
pub(crate) mod overlay;
pub(crate) mod settings_snapshot;

/// Max settings-save IPC body (bytes). Mirrors the overlay IPC cap so the
/// settings path enforces the same bound on non-Windows test builds.
#[cfg(not(target_os = "windows"))]
pub(crate) fn overlay_ipc_max_bytes() -> usize {
    64 * 1024
}

/// Max settings-save IPC body (bytes) on Windows: the overlay IPC cap.
#[cfg(target_os = "windows")]
pub(crate) fn overlay_ipc_max_bytes() -> usize {
    overlay::ipc::MAX_IPC_BYTES
}
