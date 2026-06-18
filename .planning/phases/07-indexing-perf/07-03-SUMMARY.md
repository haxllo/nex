# 07-03-SUMMARY — Memory Profiling & Icon Cache Tuning

**Date:** 2026-06-08
**Status:** Complete

## Summary

Added periodic memory profiling (working set, pagefile, Tantivy arena), made the icon cache config-driven (derived from `active_memory_target_mb`), and confirmed the indexing path is free of `.unwrap()` calls on Mutex locks in production code.

## Changes

### `apps/core/src/overlay/icons.rs`
- Added `pub(crate) fn reconfigure(&self, max_entries: usize, trim_ms: u32)` to `IconCache`:
  - Updates `inner.max_entries` and `inner.idle_trim` in-place
  - Resizes LRU if capacity changed
- Added `pub(crate) fn icon_cache_capacity_from_memory_target(target_mb: u16) -> usize`:
  - Computes `(target_mb * 1MB / 10) / 4KB`, clamped 32..512

### `apps/core/src/overlay/shim.rs`
- Modified `set_performance_tuning()`:
  - Now computes `max_entries` from `active_memory_target_mb` via the new helper
  - Calls `icon_cache.reconfigure(max_entries, idle_cache_trim_ms)` instead of `icon_cache.clear()`

### `apps/core/src/tantivy_search.rs`
- Added `pub fn mem_usage_bytes(&self) -> usize`:
  - Returns `16_000_000` (writer arena) + `segment_readers().len() * 2_000_000` (rough segment estimate)

### `apps/core/src/core_service.rs`
- Added `pub(crate) fn log_memory_stats(&self)`:
  - Windows: Uses `GetProcessMemoryInfo` for working set + pagefile bytes
  - Queries Tantivy `mem_usage_bytes()` for internal index memory
  - Logs as `[nex] memory_stats working_set_mb={} pagefile_mb={} tantivy_mb={}`
  - Non-Windows: no-op (using `#[cfg(not(target_os = "windows"))]`)

### `apps/core/src/runtime_loop.rs`
- Added `last_memory_log: Instant` to `RuntimeWorker` struct
- Added periodic memory log trigger in `on_event()`:
  - Every 30 seconds, calls `service.log_memory_stats()`
  - Resets `last_memory_log` timestamp after logging
- Added `use std::time::Duration` import

### `.unwrap()` Audit
- `tantivy_search.rs`: All `.unwrap()` calls are in `#[cfg(test)]` blocks — zero in production code
- `core_service.rs`: All `.unwrap()` calls are in test code. Production indexing methods (`index_item_on_backends`, `remove_item_from_backends`, `sync_indexes_from_cache`) use `match { Ok(g) => g, Err(p) => p.into_inner() }` pattern — already hardened

## Test Results
- `cargo check -p nex-cli`: passes with zero new errors
- All existing tests pass
