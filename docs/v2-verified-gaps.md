# Nex v2 — Reliability gaps (resolved)

Date: 2026-06-19 | Updated: 2026-07-03

All 10 items in this document were fixed during/after the WebView2 migration
but the doc was never updated. Live code verification confirms every bug is
resolved. See individual commits or the source locations below.

| # | Severity | Bug | Fix commit/verification |
|---|----------|-----|------------------------|
| 1 | Correctness | Stale prune skipped Tantivy backends | `core_service.rs:1243-1245` — calls `remove_item_from_backends` per stale id |
| 2 | Reliability | FTS5 cleared on every startup | **N/A** — FTS5 backend removed entirely; only Tantivy remains |
| 3 | Hang | Progress window hung on indexing error | `indexing_progress.rs:156-163,192-198` — `work_done_rx` + `Cmd::WorkDone` closes independently |
| 4 | Unsound | `EverythingBridge` unsafe `Send+Sync` | `everything_bridge.rs:34` — `SDK_LOCK` mutex serializes all SDK access |
| 5 | Medium | DLL loaded by bare name | `everything_bridge.rs:82-115` — probes absolute paths first; `LoadLibraryExW` with safe search dirs |
| 6 | Medium | `is_service_running` clobbered query state | `everything_bridge.rs:136` — holds `SDK_LOCK` for entire probe |
| 7 | Low | Icon idle-trim dead code | `icons.rs:66,73,88` — `touch()` called on cache hits |
| 8 | Low | Unnamed threads per `set_results` | `shim.rs:270-286` — named `"nex-icon-prefetch"`; first 8 icons sync |
| 9 | Low | Warm-release timer stacking | `host.rs:141-176` — single thread with channel-based re-arming |
| 10 | Cosmetic | `serve_asset` icon param unused | `host.rs:344-346` — parameter removed from signature |