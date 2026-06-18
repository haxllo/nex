# Phase 06-02 Summary: Config Error UX + Multi-Monitor Positioning

## Description
Improved config parse error messages with field-level context and implemented cursor-aware multi-monitor window positioning.

## Changes

### config.rs — `parse_text` (line ~1297)
- Replaced the old blob-style error message (concatenating all three parser errors) with format-aware messages that detect whether the config looks like TOML, JSON, or unknown format, providing actionable hints.

### config.rs — `validate` (line ~930)
- Updated all validation error messages to include the specific field name and the invalid value:
  - `"max_results must be between 5 and 100, got {value}"`
  - `"clipboard_retention_minutes must be between 5 and 43200, got {value}"`
  - `"idle_cache_trim_ms must be between 100 and 10000, got {value}"`
  - `"active_memory_target_mb must be between 20 and 512, got {value}"`
  - `"index_max_items_total must be between 10000 and 2000000, got {value}"`
  - `"index_max_items_per_root must be between 1000 and 1000000, got {value}"`
  - `"index_max_items_per_query_seed must be between 250 and 200000, got {value}"`
  - `"index_max_items_per_root ({per_root}) must be <= index_max_items_total ({total})"`
  - `"Invalid hotkey '{hotkey}': {error}"`

### config.rs — `load` (lines ~406, ~439)
- Wrapped `parse_text` calls with `map_err` to include file path context for both legacy path and primary config path.

### boot.rs — cursor-aware positioning
- Added `cursor_monitor_work_area()` (Windows only) to detect the monitor work area at the cursor position using `GetCursorPos`, `MonitorFromPoint`, and `GetMonitorInfoW`.
- Added `overlay_position()` to center the overlay on the cursor's monitor, falling back to `Position::Centered`.
- Replaced hardcoded `Position::Centered` with `overlay_position()`.

## Status: COMPLETE
## Verdict: PASS
