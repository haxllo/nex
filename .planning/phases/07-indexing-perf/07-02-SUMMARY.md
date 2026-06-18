# 07-02-SUMMARY — Index Compaction

**Date:** 2026-06-08
**Status:** Complete

## Summary

Replaced per-item Tantivy commits with batched commits (threshold: 500 writes). Configured Tantivy `LogMergePolicy` at open time to prevent index fragmentation. Added FTS5 `optimize()` method and a compaction trigger in CoreService (500-write counter + 5-minute timer).

## Changes

### `apps/core/src/tantivy_search.rs`
- Added `LogMergePolicy` configuration at `open()` time:
  - `set_min_num_segments(3)`, `set_level_log_size(5.0)`, `set_del_docs_ratio_before_merge(0.5)`
  - Applied via `writer.set_merge_policy(Box::new(merge_policy))`
- Added `write_count: Mutex<u32>` and `commit_threshold: u32` (default: 500) to `TantivyIndex`
- Modified `upsert_item()` and `delete_item()` to defer commits:
  - Removed inline `writer.commit()` + `garbage_collect_files()` calls
  - Now call `maybe_commit_and_gc()` which increments write counter and commits only at threshold
- Added `pub fn flush(&self)` — explicit commit + GC + counter reset
- Added `fn maybe_commit_and_gc(&self)` — internal write counter check
- Updated `test_tantivy_delete_item` to call `flush()` after deferred-commit delete
- Added `test_tantivy_deferred_commit_and_flush` — verifies search doesn't see uncommitted writes
- Fixed `mut` warnings on writer locks (Tantivy API takes `&self`)

### `apps/core/src/fts5_search.rs`
- Added `pub fn optimize(&self)` — calls FTS5 `INSERT INTO item_fts5(item_fts5) VALUES('optimize')`

### `apps/core/src/core_service.rs`
- Added `compaction_write_count: Mutex<u32>` and `last_compaction_time: Mutex<Option<Instant>>` fields
- Added `fn bump_compaction_counter(&self)` — increments counter, triggers compaction at 500
- Added `pub(crate) fn maybe_compact_backends(&self)`:
  - Time-gated: 5-minute cooldown between compactions
  - Calls `TantivyIndex::flush()` (commit + GC pending writes)
  - Calls `Fts5Index::optimize()`
  - Resets write counter to 0
- Wired compaction into:
  - `index_item_on_backends()` → calls `bump_compaction_counter()`
  - `remove_item_from_backends()` → calls `bump_compaction_counter()`
  - `sync_indexes_from_cache()` → calls `maybe_compact_backends()` after sync
  - `prune_stale_items_if_due()` → calls `maybe_compact_backends()` on timer path

## Test Results
- `cargo check -p nex-cli`: passes with zero new errors
- All Tantivy unit tests pass (including new deferred-commit test)
- All FTS5 unit tests pass
