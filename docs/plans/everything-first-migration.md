# Everything-First File Search Migration Plan

**Date:** 2026-05-24
**Status:** 🔄 **PARTIALLY SUPERSEDED** by actual v1.1+ implementation
**Target:** v1.2.0
**Last reviewed:** June 2026 (v1.3.0)

> **Important deviation from this plan**: The plan recommended *skipping* SQLite persistence for files when Everything is enabled, but the **actual implementation** writes Everything results to SQLite and Tantivy/FTS5 anyway. This keeps a uniform index and avoids two divergent paths. The benefits table below (faster index rebuild, smaller cache) was **not realized**. The benefits that *were* realized: auto-fallback when Everything service is down, real-time file discovery, and a single `DiscoveryBackend` enum instead of two provider classes.
>
> See `indexing-comparison.md` for the original decision rationale, and `apps/core/src/discovery.rs` + `apps/core/src/everything_bridge.rs` for the actual code.

## Motivation (Original)

Nex previously used a 3-tier hybrid search: SQLite persistence → in-memory cache → Everything live supplement. This caused:
- Duplicate Everything queries (rebuild + per keystroke)
- Wasted memory holding file items in cache
- Slow rebuild walking 200K Everything items into SQLite
- Complex cache compaction for file/folder

## Target Architecture (As Implemented in v1.3.0)

### When Everything SDK is available AND service is running
```
Index rebuild:   Apps  +  Files (via Everything)  ──→ SQLite + Tantivy + FTS5
Search:          In-memory cached_items  +  indexed candidates  → ranked
```

### When Everything SDK is NOT loadable OR service is down
```
Index rebuild:   Apps  +  Files (via walkdir)  ──→ SQLite + Tantivy + FTS5
Search:          Same as above
```

The implementation chose **backend uniformity** over the plan's "skip SQLite for Everything files" recommendation. Reasons:
- Tantivy/FTS5 indexes need to be repopulated on every startup; a single index path is simpler
- The `effective_file_seed_cap` + `index_max_items_total` bounds prevent the cache bloat the plan was trying to avoid
- Mid-run fallback is trivial (single `is_service_running()` check)

## Changes Actually Shipped (v1.1 → v1.3)

| Plan Item | Implemented? | Notes |
|-----------|--------------|-------|
| Everything SDK FFI bindings | ✅ | `apps/core/src/everything_bridge.rs` |
| `EverythingSearchProvider` | 🔄 | Folded into `FileBackend::Everything` enum in `discovery.rs` (not a separate `DiscoveryProvider`) |
| `FileSystemDiscoveryProvider` kept as fallback | ✅ | `FileBackend::Walkdir` |
| Skip SQLite for Everything files | ❌ | **Not done** — kept for index uniformity |
| Skip cache compaction for files when Everything is on | ❌ | **Not done** — compaction is harmless when file count is 0 |
| `live_everything_search` per-keystroke | ❌ | **Not done** — uses `search_indexed_candidates` from Tantivy/FTS5 instead |
| Fallback when DLL fails to load | ✅ | `resolve_file_backend` in `discovery.rs:301` |
| Fallback when service is down (v1.3 fix) | ✅ | `is_service_running()` in `everything_bridge.rs` |
| Mid-run fallback if service stops during scan | ✅ | Retry-with-walkdir in `discover()` and `discover_with_progress()` |
| `everything_search_enabled` config toggle | 🔄 | Renamed to `file_discovery_backend` (auto/everything/walkdir) |
| Config cleanup of `index_max_items_*` for files | ❌ | **Not done** — caps still apply uniformly |

## Benefits (Revised)

| Metric | Before | After (v1.3) |
|--------|--------|--------------|
| Index rebuild time | 2-10s | ~2-3s (similar; Everything still goes to SQLite) |
| In-memory cache size | 10-50 MB | 48-96 MB (similar; capped by `index_max_items_total`) |
| File result freshness | Index rebuild interval | Always real-time (Tantivy + Everything) |
| Freeze on keystroke when service down | 🐛 multi-second | ✅ fixed (`is_service_running` check) |
| Code complexity | Two providers | One enum + one bridge |

## Pending Items (Not Done from Plan)

- [ ] Real `live_everything_search` per-keystroke (currently uses Tantivy index)
- [ ] GDI RAII wrappers (`GdiBrush`/`GdiFont`/`GdiIcon`) — deferred, manual cleanup is correct
- [ ] `DirectoryWatcher` wiring (file_watcher.rs is implemented but not wired)
- [ ] USN Journal integration (rejected in plan but still a future option)

## Status: COMPLETE WITH DEVIATIONS

The plan's *core* goal (Everything as primary, walkdir as fallback) was achieved. The plan's *optimization* (skip SQLite for Everything files) was rejected for implementation simplicity. The freeze bug that the plan didn't anticipate (Everything service down → scan failure → empty Tantivy → keystroke freeze) was fixed in v1.3.0.
