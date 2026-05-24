# Everything-First File Search Migration Plan

**Date:** 2026-05-24
**Status:** Draft → Active
**Target:** v1.2.0

## Motivation

Nex currently uses a 3-tier hybrid search: SQLite persistence → in-memory cache → Everything live supplement. This causes:

- **Duplicate work**: Everything SDK is queried during index rebuild (up to 200K items) AND at query time (per keystroke)
- **Wasted memory**: File/folder items occupy the in-memory cache even though Everything provides real-time results that are always fresher
- **Slow index rebuild**: Walking Everything's full index (200K items) and persisting to SQLite during every index cycle
- **Complex cache compaction**: The `compact_cached_items` logic exists only to keep file/folder cache bounded

Everything's index is always real-time (it watches NTFS changes). There is no scenario where the SQLite-backed cache provides fresher file results than Everything's live query. If the Everything DLL is loadable, file results should come exclusively from `live_everything_search()` at query time.

## Target Architecture

### When Everything SDK is available (`everything_search_enabled = true` + DLL loads)

```
Index rebuild:   Apps  ──→ SQLite ──→ in-memory cache (apps only)
                 Files ──→ SKIP SQLite ──→ served live from Everything at query time

Search:          In-memory cache (apps only)
                 + Everything live (files only)
                 + Actions / Clipboard / Plugins
                 → Merge & re-rank
```

### When Everything SDK is NOT available (fallback mode)

```
Index rebuild:   Apps  ──→ SQLite ──→ in-memory cache
                 Files ──→ SQLite ──→ in-memory cache (via FileSystemDiscoveryProvider)

Search:          In-memory cache (apps + files)
                 + Actions / Clipboard / Plugins
                 → Search & rank
```

## Changes Required

### Phase 1 — Core Service (`core_service.rs`)

1. **`runtime_providers_from_config`** — When `everything_search_enabled` is true:
   - Register `StartMenuAppDiscoveryProvider` (always)
   - Register `EverythingSearchProvider` (to track `everything_covered_files` flag and handle fallback)
   - **Skip** `FileSystemDiscoveryProvider` entirely
   - When `everything_search_enabled` is false: keep current behavior (include filesystem provider)

2. **`rebuild_index_internal`** — When `everything_covered_files` is true:
   - Call `EverythingSearchProvider::discover()` to set the coverage flag
   - **Do not upsert** file/folder items from the Everything provider into SQLite
   - Only upsert app items from `StartMenuAppDiscoveryProvider`
   - Drop file/folder items from `existing_by_id` without writing to SQLite
   - Set `everything_covered_files = true` for downstream use

3. **`refresh_cache_from_store`** — When Everything is enabled:
   - Only load app-type items from SQLite into `cached_items`
   - `cached_app_items` is unchanged (always apps only)
   - Skip cache compaction for files (no files in cache to compact)

4. **`db_query_candidates`** — When Everything is enabled:
   - Only return app items from SQLite seed query
   - Skip `SearchMode::Files` queries (handled by Everything live)

### Phase 2 — Search Session (`runtime_search_session.rs`)

1. **Everything live search condition** — Remove `!short_query_app_bias` guard:
   - Everything files should be returned even for short queries when Everything is the only file source
   - For 1-2 character queries, cap `max_results` lower (e.g. 5 instead of 20) to keep latency acceptable
   - Full candidate_limit applies for 3+ character queries

2. **`should_use_short_query_app_mode`** — No change needed: this only affects the in-memory indexed cache, not Everything. Apps still come from cache for short queries.

### Phase 3 — Remove Redundant Code

1. **`compact_cached_items`** — When Everything is enabled, file/folder count in cache is 0, so compaction is always a no-op
2. **`cache_compaction_summary` logging** — Will show `retained_file_folders=0` when Everything is enabled
3. **`stale_prune` for files** — When Everything is enabled, file items don't exist in cache, so no stale file pruning needed

### Phase 4 — Pure Fallback Path (No Everything)

When `everything_search_enabled = false` OR the DLL cannot be loaded:
- Keep the EXISTING behavior unchanged: `FileSystemDiscoveryProvider` registers, SQLite persists files, cache holds files, search works from cache
- `live_everything_search` returns `None` → no Everything results appended
- This ensures offline/file-watch-only users are unaffected

### Phase 5 — Config Cleanup

- The `index_max_items_total`, `index_max_items_per_root`, `index_max_items_per_query_seed` config keys become no-ops for files when Everything is enabled
- They still apply to the filesystem fallback provider when Everything is disabled
- Keep them in the schema for backward compatibility; add a note in the config template

## Benefits

| Metric | Before | After |
|--------|--------|-------|
| Index rebuild time | 2-10s (apps + 200K Everything files → SQLite) | ~200ms (apps only → SQLite) |
| In-memory cache size | 10-50 MB (apps + files) | ~2-5 MB (apps only) |
| File result freshness | Index rebuild interval (stale until reindex) | Always real-time (Everything live) |
| Code complexity | Cache compaction, stale pruning, seed limits for files | None for files |
| Startup time | Load all files from SQLite | Load only apps from SQLite |

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Everything not running → no file results | Fallback to filesystem provider when DLL fails to load |
| Everything IPC latency on short queries | Cap `max_results` to 5 for 1-2 char Everything queries |
| Everything index out of date (rare) | Everything watches NTFS changes; it's always in sync |
| User configures no filesystem roots | Everything live respects roots; empty results if no roots configured |
| Backward compatibility with existing SQLite DB | Old DB still has file rows; they're ignored at startup when Everything is enabled; if Everything is later disabled, a reindex restores them |

## Testing

1. Unit: `CoreService` with Everything enabled loads only apps into cache
2. Unit: `runtime_providers_from_config` skips filesystem when Everything is enabled
3. Integration: Search with Everything enabled returns files from live query only
4. Integration: Search with Everything disabled returns files from SQLite cache
5. Smoke: Short query (1-2 chars) returns Everything files with reduced limit
