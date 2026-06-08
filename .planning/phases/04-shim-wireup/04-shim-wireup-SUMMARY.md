# Phase 04-shim-wireup — Summary

**Date:** 2026-06-08
**Status:** COMPLETE (all work confirmed by audit)

## Audit Results

All 27 `NativeOverlayShell` methods are implemented and functional:
- `create()`, `is_visible()`, `has_focus()`, `hwnd()`, `show_and_focus()`
- `focus_input_and_select_all()`, `hide()`, `hide_now()`
- `query_text()`, `set_query_text()`, `clear_query_text()`
- `set_status_text()`, `set_hotkey_hint()`, `set_mode_strip_text()`
- `set_help_config_path()`, `show_placeholder_hint()`, `clear_placeholder_hint()`
- `set_hotkey_issue_active()`, `set_game_mode_enabled()`
- `set_performance_tuning()`, `trim_runtime_memory()`
- `set_results()`, `set_selected_index()`, `selected_index()`
- `run_message_loop_with_events()` → `run_message_pump()`
- `shared_model()`, `is_running()` — new methods for Iced boot
- `icon_cache()` — added in Phase 2

## Key Implementation Details

- `runtime_loop.rs` — Fully wired: `NativeOverlayShell::create()` → `Boot` → `boot::run()` → worker thread → `on_event`
- `overlay/mod.rs` — Correct re-exports of `OverlayEvent`, `OverlayRow`, `OverlayRowRole`, `NativeOverlayShell`, platform functions
- `hotkey.rs` — `RegisterHotKey` on dedicated thread, events via crossbeam channel
- `tray.rs` — Full `TrayIcon` implementation (added in Phase 1)
- `platform.rs` — Instance signaling, theme detection

## Outstanding

- `indexing_progress.rs` — Stub only. Full Iced ProgressBar implementation deferred.

## Build

`cargo check -p nex-cli` passes with no errors in the overlay module.
