# 07-01-SUMMARY — Incremental Search Indexing

**Date:** 2026-06-08
**Status:** Complete

## Summary

Replaced full-index rebuilds (`delete_all_documents` + reindex all) with incremental sync in both Tantivy and FTS5 search backends. The `sync_indexes_from_cache()` method now uses incremental operations for non-first syncs, reducing index sync latency from O(n) to O(delta).

## Changes

### `apps/core/src/tantivy_search.rs`
- Added `incremental_sync_items(&self, items: &[SearchItem])` method
  - Collects existing document IDs from all segments via `Searcher::segment_readers()`
  - Deletes items absent from the incoming set
  - Adds/updates incoming items via delete-then-add pattern
  - Single commit + garbage collect at end of batch
- Added 3 new unit tests: `test_tantivy_incremental_sync_basic`, `test_tantivy_incremental_sync_empty_index`, `test_tantivy_incremental_sync_empty_list`
- Added `use std::collections::HashSet` import

### `apps/core/src/fts5_search.rs`
- Added `incremental_sync_items(&self, items: &[SearchItem])` method
  - Queries existing IDs via `SELECT id FROM item_fts5`
  - Wraps deletes + inserts in `BEGIN IMMEDIATE` / `COMMIT` transaction
  - Uses delete-then-insert (matching `upsert_item` pattern) since `id` is UNINDEXED not UNIQUE
- Added 3 new unit tests: `test_fts5_incremental_sync_basic`, `test_fts5_incremental_sync_empty_index`, `test_fts5_incremental_sync_empty_list`
- Added `use std::collections::HashSet` import

### `apps/core/src/core_service.rs`
- Modified `sync_indexes_from_cache()`:
  - Added first-sync detection: checks `num_docs() == 0` on both backends
  - First sync uses bulk `index_items()` (efficient for initial load)
  - Subsequent syncs use `incremental_sync_items()`
  - Added `tantivy_first` and `fts5_first` fields to the `[nex] sync_indexes` log line

## Test Results
- `cargo test --lib -p nex-cli -- tantivy_search`: 9 passed (3 new + 6 existing)
- `cargo test --lib -p nex-cli -- fts5_search`: 8 passed (3 new + 5 existing)
- `cargo check -p nex-cli`: passes with zero new errors
