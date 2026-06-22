# Nex v2 — Verified reliability gaps after WebView2 migration
 
Date: 2026-06-19
 
This document records verified findings after re-checking the actual source code and a fresh `cargo build`. Several subagent claims were overstated or based on stale docs (AGENTS.md / the v2 plan). Each item is tagged as **real bug**, **design choice**, or **stale doc / cleanup**.
 
---
 
## Real bugs (worth fixing)
 
### 1. Stale prune does not update search backends
- **Location:** `apps/core/src/core_service.rs:1235-1260`
- **Issue:** `prune_stale_items_if_due` deletes from SQLite and the two in-memory caches, but never calls `remove_item_from_backends` (defined at `core_service.rs:1086-1105`). Meanwhile `delete_item_by_id` at `core_service.rs:712` correctly deletes from store + cache + both backends. So prune leaves Tantivy/FTS5 full of dead entries.
- **Impact:** Users keep seeing deleted files in results until the next full sync. Index/cache drift.
- **Severity:** real correctness bug.
 
### 2. FTS5 is cleared on every startup
- **Location:** `apps/core/src/core_service.rs:147-171`
- **Issue:** Both branches of `if tantivy_index.is_none()` open FTS5 and immediately call `idx.clear()`. Then `sync_indexes_from_cache` at `core_service.rs:1049` calls `index_items`, which calls `clear()` again at `fts5_search.rs:83`. So on a normal startup, FTS5 is wiped at least once, then fully rebuilt, even though Tantivy is the primary backend and search only falls back to FTS5 when Tantivy is unavailable.
- **Impact:** Wasteful startup work. FTS5 fallback sits empty until the sync succeeds, so if the first sync fails, there is no usable fallback.
- **Severity:** wasteful / reliability risk.
 
### 3. Progress window hangs if indexing errors before 100%
- **Location:** `apps/core/src/overlay/indexing_progress.rs:150-183`
- **Issue:** `run_return` exits only on `Cmd::Close`. `Cmd::Close` is sent only after `Cmd::Update(v >= 100)`. The work closure writes `100` to the progress Arc at `core_service.rs:685`, but only on the success path. If `rebuild_index_incremental_with_report` returns `Err` early, the work thread returns the error but `100` is never written, the poll thread never exits, and the window never closes. The `recv_timeout(300s)` at `indexing_progress.rs:182` is unreachable because `run_return` blocks first.
- **Impact:** Indefinite main-thread hang on first-time indexing failure, with a stuck progress bar.
- **Severity:** real hang bug.
 
### 4. EverythingBridge is `Send + Sync` over global mutable state
- **Location:** `apps/core/src/everything_bridge.rs:26-27`
- **Issue:** The SDK function pointers (`set_search_w`, `query_w`, `get_num_results`, etc.) all operate on process-global Everything state. The bridge has `unsafe impl Send + Sync` and stores `HMODULE`/function table, but nothing serializes access to that global state. `discover()` and `is_service_running()` mutate it, so concurrent calls can corrupt the query/results.
- **Impact:** Unsound concurrency; corrupted result lists under concurrent access.
- **Severity:** unsound concurrency. Should be a mutex or single-threaded access.
 
### 5. EverythingBridge loads DLL by bare name
- **Location:** `apps/core/src/everything_bridge.rs:55`
- **Issue:** `LoadLibraryW("Everything64.dll")` relies on the DLL search order, which is a DLL-planting risk and can be order-dependent. The code does fall back to probing candidate paths, but the first attempt is unsafe.
- **Impact:** Security/reliability issue on some systems.
- **Severity:** medium.
 
### 6. `is_service_running` mutates global state as a side effect
- **Location:** `apps/core/src/everything_bridge.rs:109`
- **Issue:** The function calls `set_search_w("")` and then `query_w(0)` to test whether the IPC works. That clobbers any in-progress query state from another thread. A service check should not be stateful.
- **Impact:** Concurrency hazard, part of the same unsoundness issue as #4.
- **Severity:** medium.
 
### 7. Icon idle-trim is dead code
- **Location:** `apps/core/src/overlay/icons.rs:81-99`
- **Issue:** `trim_unused` filters `inner.last_touch` by age, but `png_bytes` and `png_bytes_cached` never write to `last_touch`. The field is initialized in `new` at `icons.rs:41` but stays empty forever. So `trim_unused` always returns 0. The LRU still self-trims by capacity, so the cache does not leak, but the idle-timeout trim is inert.
- **Impact:** No automatic idle-time based icon cache trim. Capacity trim works.
- **Severity:** low-impact real bug.
 
### 8. Unnamed threads spawned per `set_results`
- **Location:** `apps/core/src/overlay/shim.rs:250`
- **Issue:** `set_results` calls `std::thread::spawn` (no builder, no name) for icon prefetch. Rapid typing spawns many short-lived threads. Each call path calls `CoInitializeEx` in `icons.rs:320` but never `CoUninitialize`. Threads exit naturally, so it is churn rather than a true leak, but it is unbounded and not instrumented.
- **Impact:** Thread churn under rapid typing; mild COM apartment hygiene issue.
- **Severity:** churn / hygiene.
 
### 9. Warm-release timer thread stacks on rapid toggles
- **Location:** `apps/core/src/overlay/host.rs:205-211`
- **Issue:** Every `Hide` spawns a `nex-ui-warm-release` thread that sleeps the full delay then sends `Teardown`. If the user hides/shows repeatedly, stale timers still sleep until their delay expires. The generation guard makes their `Teardown` no-op, but the threads still consume resources. With `ui_warm_release_ms` maxed to 600000, this could be meaningful.
- **Impact:** Thread stacking under rapid overlay toggles; mostly harmless at default 5s delay.
- **Severity:** low under normal use.
 
### 10. `serve_asset` icon parameter is unused
- **Location:** `apps/core/src/overlay/host.rs:295-312`
- **Issue:** The function receives `_icons: &Arc<IconCache>` but only serves `/index.html`, `/style.css`, `/app.js`. The plan's `nexasset://icon/<path>` route was dropped because icons are embedded as base64 data URIs in `snapshot_json` at `host.rs:431-443`. The `_icons` parameter is vestigial and slightly misleading.
- **Impact:** Harmless cleanup.
- **Severity:** cosmetic.
 
---
 
## Design choices / not bugs
 
- **Base64 icon embedding instead of custom protocol.** This is intentional. The code comment at `host.rs:432` says "custom protocols don't work for subresource requests in WebView2." The `serve_asset` icon protocol is dead, but the data-URI approach is the working design.
- **Dual-index writes (Tantivy + FTS5).** `sync_indexes_from_cache` writes both. Search uses Tantivy primary and FTS5 fallback. This is a design trade-off (hot fallback) rather than a bug. It could be optimized, but it is not unreliable unless the FTS5 fallback is cold because of bug #2.
- **SwiftFind env-var/constant names.** These are intentionally retained per `AGENTS.md:77` ("don't rename unless explicitly asked"). Not a bug.
- **Icon cache not re-warmed on Show after teardown.** Idle show has no rows (`set_idle_overlay_state` at `runtime_overlay_rows.rs:484` calls `set_results(&[], 0)`), so there are no icons to prefetch. The cache re-warms on the first typed search. This is correct, not a gap.
- **MoveSelection variant.** `model.rs:35` defines it, but JS handles navigation locally and the IPC only posts `query/submit/escape/select`. The match arm at `runtime_loop.rs:914` is dead but harmless. The plan explicitly said this becomes "dead but harmless."
- **Memory trim cadence.** `trim_runtime_memory` is exposed only as the manual `ACTION_TRIM_MEMORY_ID` action. Combined with bug #7, automatic idle-time icon trimming is inert. This is a design choice (manual trigger) plus the dead idle-trim logic.
 
---
 
## Stale documentation / stale plan references
 
- **`AGENTS.md:51-77` "Overlay Rendering Stack" is wrong.** It describes `gdiplus_rendering.rs`, `painting.rs`, `d2d_renderer.rs`, and a listbox/Win32 GDI+ overlay. None of those files exist. The overlay is WebView2 (tao + wry). The dead-code warning count "~22" is also stale; the current build produces **11 warnings** from the new WebView2 overlay.
- **`AGENTS.md:61-67` "Key Overlay Architecture"** describes `WS_EX_LAYERED`, `LB_GETCURSEL`, hover_index, etc. — all from the old architecture.
- **`platform.rs:1-10` module doc** still says "Platform glue for the Iced overlay." Iced is gone.
- **Plan Phase E "keep `NEX_WM_SEARCH_RESULTS_READY`"** — the constant was removed. Search results now use `OverlayEvent::SearchResultsReady` over the crossbeam channel, so the constant was no longer needed.
- **Plan Phase E "remove `resvg`, `tiny-skia`, `palette`"** — `palette` is gone. `resvg`/`tiny-skia`/`usvg` are still present as `[dev-dependencies]` only for `examples/gen_icon.rs`. If the example is meant to live, the deps are intentional; if not, remove all four together.
- **Mica backdrop** — `AGENTS.md` correctly notes this is planned but unimplemented. Only acrylic is applied. This is a real parity gap, but cosmetic.
 
---
 
## Current build warnings (actual, not AGENTS.md "~22")
 
From `cargo build --bin nex` on 2026-06-19:
 
- `field tray_hi_tx is never read`
- `function next_selection_index is never used`
- `function log_registration is never used`
- `field id is never read`
- `method id is never used`
- `methods png_bytes_cached, len, and is_empty are never used`
- `variant MoveSelection is never constructed`
- `variant Light is never constructed`
- `function detect_system_theme is never used`
- `function hotkey_id_for is never used`
- `methods hwnd, focus_input_and_select_all, and set_selected_index are never used`
 
Total: 11 warnings from the WebView2 overlay and related runtime code.
 
---
 
## Recommended fix order
 
1. Stale prune → backends (#1) — small diff, real correctness bug.
2. FTS5 startup clear (#2) — only clear when Tantivy is dead or schema mismatch, not unconditionally.
3. Progress window hang (#3) — couple the work-thread result to `Cmd::Close`, not just 100% progress.
4. EverythingBridge soundness (#4, #5, #6) — serialize access, use safe DLL loading, and make `is_service_running` non-destructive.
5. Icon `last_touch` / trim cadence (#7 + #8) — write timestamps, or remove `trim_unused` and rely on LRU; also use a named thread / threadpool for prefetch.
6. Warm-release timer stacking (#9) — replace per-hide thread with one scheduler.
7. Docs cleanup — rewrite `AGENTS.md` overlay section, update `platform.rs` doc, decide on `gen_icon.rs` + SVG deps.
8. Mica — nice-to-have, not reliability.