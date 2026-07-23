# Changelog

All notable changes to Nex are documented in this file.

## [2.7.0] - 2026-07-23

### Architecture

- **Rust 2024 edition** — migrated from 2021 to 2024. Reserved `gen` keyword renamed to `generation`. Explicit `unsafe {}` blocks with SAFETY comments inside all `unsafe fn` bodies.

### Fixed

- **Scroll-jump on typing** — `scrollToSelected()` removed from `render()`; no more auto-scroll to top on keystroke.
- **Orphaned scroll on new show** — `pendingShow` flag set on Show, consumed by first `apply()` with non-empty rows, resets `scrollTop` to 0 once per show/hide cycle.
- **Smooth scroll conflict** — `scrollToInstant(0)` helper disables CSS smooth scroll for programmatic resets, restores on next RAF. User scrolls stay smooth.
- **`.selected` class lost after render** — re-applied via JS after `replaceChildren`.

## [2.6.0] - 2026-07-22

### Performance

- **Icon pipeline** — 256px `PrivateExtractIconsW` extraction with 128px normalized square icons — crisp rendering at all display sizes.
- **.lnk resolution** — resolve shortcut targets to extract high-res icons from the actual .exe (~95% of Start Menu shortcuts).
- **Progressive icon delivery** — first-open results paint instantly with placeholders, real icons arrive asynchronously.

### Changed

- **Row height** — 52px → 46.22px, 9 clean rows visible (was 7).
- **Icon size** — 30px → 24px, better proportion in tighter rows.
- **Placeholder icon** — generic app icon shown while real icon loads (no more blank holes).
- **Removed row-in animation** — no more 3px slide-up bounce on first paint.
- **Removed inset shadow** — selected rows no longer flicker a border when hovering.
- **Removed fade transitions** — pin/add icons snap instantly, no 120ms delay.
- **Hidden .kind tag** — declutters rows, ready to re-enable later.
- **Input alignment** — line-height: 1, zero padding, translateY(-1px).
- **Background opacity** — dark mode panel 0.80 → 0.70 for better acrylic show-through.

### Fixed

- **Scroll-jump on typing** — removed unconditional `scrollToSelected()` in render; no more auto-scroll to top when typing.

## [2.5.0] - 2026-07-14

### Performance

- **Variable font** — switched from single-weight Geist-Regular to InterVariable (100-900 weight range) — real font weights instead of synthetic.

### Fixed

- **Pinned list** — Quick Launch section hides when all items are unpinned (was auto-filling from recent apps).
- **Footer logo aspect ratio** — fixed stretching by using `height` + `width: auto`.
- **Custom protocol** — switched from `nexasset://` (silently ignored by WebView2 for `<img>`) to base64 served via Rust `serve_asset`.

### Changed

- **Typography** — font sizes reduced by 1px baseline, letter-spacing 0.5px → 0.2px, body weight 300 → 400, placeholder 200 → 300.
- **Footer logo** — embedded via `include_bytes!`, base64-encoded at serve time — no inline base64 in HTML.
- **Color tokens** — `--text-dim` changed to `#818181`, placeholder and search icon use `--border-strong` / `--text-dim`.

## [2.4.2] - 2026-07-10

### Fixed

- **Overlay flash/pulse on open** — quick launch items now load into state before `UiCommand::Show` is posted. Resize fires before painted so window appears at correct height.
- **Opacity flash** — removed opacity 0→1 from row-in animation (translateY only).
- **Body flash on close** — window hides before cleared state is pushed.
- **Divider visible on idle** — `class="idle"` hides list area until rows render.

## [2.4.1] - 2026-07-10

### Changed

- **Warm WebView** — overlay WebView stays resident for process lifetime. Warm-release timer clears decoded icon cache only, keeping page loaded for consistent ~instant re-open timing.
- **Heap reclaim** — cleared icon cache reclaims overlay heap during idle without tearing down the WebView.

## [2.4.0] - 2026-07-09

### Added

- **Quick Launch** — hybrid model: shows pinned items when pins exist, auto-fills from usage when no pins.
- **Pin/unpin** — toggle from search results; input retains focus, overlay updates immediately.
- **Pinned items sorted to top** — in all search results.

### Fixed

- **Icon quality** — `ExtractIconExW` now retrieves highest resolution (32px/48px/256px) instead of fixed 32px.
- **Overlay not opening** — fixed JS regex syntax error in `isItemPinned` that blocked WebView ready signal.
- **Config migration** — fixed nested TOML structure for quick_launch settings with legacy flat-format migration.

## [2.3.1] - 2026-07-04

### Performance

- **Arrow key navigation** — `SelectChanged` sends only `{"selected": idx}` (~20 bytes) instead of full state snapshot (~134KB). Arrow keys now ~1ms instead of ~20ms.
- **Icon delivery** — dual `PostWebMessageAsJson`: lightweight state JSON first, icon data JSON second. State lock hold reduced from ~5ms to ~0.1ms.
- **Lock contention** — `push_state()` clones `ShimState` first, drops lock, then serializes outside lock.
- **CSS compositor** — removed redundant `backdrop-filter: saturate(140%)` that doubled compositor work.
- **Row animation** — `@keyframes row-in` only applied on initial render, preventing 20 concurrent animations per keystroke.
- **JS debounce** — reduced from 80ms to 40ms for faster first keystroke response.
- **Painted IPC** — `post("painted")` only sent after `show_pending = true`.
- **Personalization cache** — 5-second TTL for SQLite personalization queries, eliminating ~1-2ms per search.
- **Tantivy incremental sync** — writer lock held only during write phase (~1ms) instead of full scan (~100ms).

### Fixed

- **Shutdown hang** — 3-part root cause: COM STA deadlock resolved by switching to MTA, `FileWatcherHandle::drop` made non-blocking with `mem::forget`, `std::process::exit(0)` added as safety net.
- **COM init per invocation** — moved `CoInitializeEx` to `thread_local!` for persistent prefetch thread.
- **Startup unwrap hardened** — replaced 5 bare `.unwrap()` calls with `unwrap_or_else` for poisoned lock recovery.
- **Escape handler duplicate** — fixed duplicated clear logic in search session.
- **Indexer thread panic** — handled gracefully instead of crashing main thread.
- **Stale pruner shutdown** — added `AtomicBool` stop signal for clean exit.
- **File watcher shutdown** — `stop_file_watchers()` called before worker thread join.
- **Runtime worker panic guard** — wrapped `worker.run()` in `catch_unwind`.
- **Blocking search reads** — replaced blocking `cached_items.read()` with `try_read()`.
- **Icon prefetch thread accumulation** — single persistent `nex-icon-prefetch` thread instead of per-call spawning.
- **`last_touch` unbounded growth** — added `clean_orphaned_touches()` to cap LRU HashMap growth.

### Architecture

- **Persistent prefetch thread** — single thread with shared work slot replaces per-call thread spawning.
- **COM MTA** — icon prefetch uses `COINIT_MULTITHREADED` instead of `COINIT_APARTMENTTHREADED` to prevent `ExitProcess` deadlock.
- **`catch_unwind` on search worker** — prevents search panic from killing the worker thread.

## [2.3.0] - 2026-06-22

### Added

- **Launch at startup default** — `launch_at_startup` now defaults to `true`.

### Fixed

- **Cargo package name drift** (#10) — workspace package renamed from `nex-launch` to `nex` so `cargo -p nex` works everywhere.
- **Update script wrong repo** (#11) — `update-nex.ps1` defaults to `haxllo/nex`.
- **Build-from-source installer path** (#12) — `install-nex.ps1` uses correct `cargo build -p nex --release --bin nex`.
- **File watcher drop recovery** (#16) — consumer triggers `rebuild_index_incremental_with_report` after batch overflow.
- **Clipboard history privacy** (#17) — clipboard history is off by default and encrypted with DPAPI.

### Architecture

- **Config reload semantics** (#19) — `RuntimeWorker` bumps `config_generation` counter on reload and clears search worker session immediately.
- **Stale Iced references** (#14) — removed from overlay doc comments and architecture docs (WebView2/tao+wry is the single UI shell).

## [2.2.2] - 2026-06-22

### Fixed

- **Nex not showing in Task Manager startup apps** — `options.background` defaulted to `true`, causing argument parser to reject CLI commands (`--set-launch-at-startup`, `--ensure-config`, `--quit`) with an invisible failure. Fixed by auto-setting `background = false` for all non-`Run` commands.

## [2.2.1] - 2026-06-22

### Fixed

- **First hotkey after warm-release teardown now works** — when WebView rebuilds during `build_webview()`, spurious `Focused(true)`/`Focused(false)` events triggered the click-outside-to-dismiss handler. Added `!show_pending` guard to the escape condition.

## [2.2.0] - 2026-06-22

### Fixed

- **SQLITE_BUSY launch freeze** — launch path reads from in-memory cache instead of the DB, avoiding 5-6s blocks when the background indexer holds the SQLite lock. Added WAL journal mode and busy timeout.
- **Warm-release hotkey miss** — `show_pending` set before `build_webview` blocks spurious `Escape` from Tao's `Focused` events during WebView rebuild.
- **Session clearing race** — drains clear channel after receiving search request so first post-hide query doesn't use stale results.
- **Index sync staleness** — `sync_indexes_from_cache` compares backend doc counts against actual cache item count.
- **Tantivy incremental sync misses** — iterates `max_doc` (not `num_docs`) and skips deleted docs via alive bitset.
- **Config hot-reload stale in search worker** — config and plugin registry shared via `Arc<RwLock<>>` for immediate effect.
- **Console flash on launch** — added `CREATE_NO_WINDOW` to plugin command and explorer.exe spawns.
- **Diagnostics privacy** — raw config and logs are opt-in (`NEX_INCLUDE_RAW_DIAGNOSTICS=1`). Query profile logs show hash instead of readable text.

### Architecture

- **Removed FTS5 search backend** — Tantivy is the sole indexed search backend. Removed `fts5_search` module, `SearchBackend` config enum, and all dual-backend sync paths.
- **Removed JSON config template** — only TOML templates are written going forward (JSON loading preserved for backward compatibility).
- **Default hotkey changed** — from `Ctrl+Shift+Space` to `Ctrl+Space`.

## [2.1.1] - 2026-06-21

### Fixed

- **Shutdown hang** — `OnceLock::clone()` deep copy hid thread ID from `Drop`; listener thread never posted `WM_QUIT`, blocking `handle.join()` forever. Wrapped in `Arc` to share state.

## [2.1.0] - 2026-06-20

### Performance

- **Warm Tantivy cache on show** — pre-reads ALL Tantivy segment files + SQLite DB into OS page cache before showing overlay, cutting first-keystroke latency.
- **Adaptive debounce** — first char instant, rapid chars coalesced at 80ms.
- **O(1) selection via rowMap** — `scrollIntoView` only on selection change.
- **Stale prune off critical path** — moved to background thread.
- **`recv_timeout` jitter** — stop channel + `select!` instead of 50ms poll.
- **Fire-and-forget state push** — replaced blocking `evaluate_script` with `PostWebMessageAsString`.
- **`RwLock` over `Mutex`** — for `CoreService` enabling concurrent read access.

### Fixed

- **Everything SDK race** — fixed bridge race, DLL planting, `is_service_running` state clobber.
- **First-show blank** — WebView now renders state before becoming visible.
- **FTS5/Tantivy re-index** — fixed redundant re-index and progress window acrylic.
- **FTS5 mutex deadlock** — dropped `fts5_guard` before `maybe_compact_backends`.
- **Scrollbar flash** — fixed overlay scrollbar visibility on open.

### Changed

- **GUI subsystem** — no console flash at startup. Removed dead SVG icon deps.
- **DWM drop shadow** — replaced CSS box-shadow with native `DwmExtendFrameIntoClientArea` (-1 margins).

## [2.0.0] - 2026-06-18

### Added

- **WebView2 overlay** — replaced Iced rendering with tao + wry (WebView2). All UI is now HTML/CSS/JS.
- **DWM drop shadow** — native window shadow via `DwmExtendFrameIntoClientArea`.
- **FTS5 incremental sync** — with search relevance scoring.
- **Tantivy foundation** — full-text search engine with BM25 ranking, fuzzy, prefix, phrase matching.
- **Everything SDK** — bundled Everything64.dll with auto-detection and graceful fallback.
- **File watcher consumer** — `delete_item_by_id` for real-time index updates.

### Fixed

- **Screen-tear on hover** — eliminated D2D/GDI desync by moving to GDI+-only rendering path.
- **Winit main-thread panic** — thread-swap fix for overlay thread safety.

### Architecture

- **Complete Iced migration** — removed all Iced 0.14 code. Tray, icons, view, shim, legacy removal completed.
- **Config migration** — to TOML with JSON backward compatibility.
- **Planning docs** — stability, indexing perf, search quality phase plans added.

## [1.3.0] - 2026-05-28

### Changed

- **GDI+-only rendering** — removed D2D+GDI hybrid; all rendering now uses GDI+. Eliminates screen-tear and panel flash.
- **Inter font** — replaced SpaceMono with Inter across the UI.
- **GDI+ text rendering** — ClearType hinting for blurry text fix. Pre-created font handles, `SelectObject` eliminated from draw path.
- **Help tip** — converted from GDI to GDI+ rendering. Fixed border clipping with float-based rounded rect path (0.5px inset).

### Fixed

- **Footer hint rendering** — fixed degenerate-paint flash.
- **CI test hangs** — gated web/shell launch tests behind env var. Fixed scaffold test path, package name (`nex` → `nex-cli`).
- **CI matrix** — dropped ubuntu-latest (Nex is Windows-only).

## [1.2.0] - 2026-05-26

### Added

- **Channel updater** — with manifest integrity verification and rollback support.
- **Async icon painting** — icon cache invalidation and background refresh.

### Performance

- **Optimized icon caching** — reduced icon decode overhead on overlay show.

## [1.1.1] - 2026-05-24

### Changed

- **Search is now fully async** — query processing moved to a dedicated background thread. The UI thread no longer blocks during search, keeping the overlay responsive even during heavy queries.

### Fixed

- **Overlay no longer freezes during search** — every keystroke previously triggered a synchronous search on the UI thread. Now handled by `SearchWorker` with stale-request draining and `PostMessageW` result notification.
- **Thread safety for CoreService** — `Arc<Mutex<CoreService>>` ensures the SQLite connection is never accessed concurrently.
- **Worker thread lifecycle** — `SearchWorker::Drop` signals thread exit via channel closure and joins the thread cleanly.

## [1.1.0] - 2026-05-24

### Added

- **Everything SDK integration** — instant file search via Everything64.dll bundled with the installer. Auto-detection with graceful fallback.
- **Plugin SDK foundations** — trait-based plugin interface with WASM distribution path prepared.
- **Window management system** — 8 preset tile layouts (left/right half, top/bottom half, four quadrants, center, maximize, restore).
- **Calculator (basic)** — inline arithmetic evaluation supporting `+`, `-`, `*`, `/`, `%`, and parenthesized expressions.
- **Emoji picker** — type `:` followed by a keyword to search and insert emoji.

### Changed

- **Packaging bundles Everything64.dll** — `package-windows-artifact.ps1` downloads and includes the Everything SDK DLL automatically.
- **Stale docs updated** — system-architecture.md, project-charter.md, requirements.md, and others corrected to match TOML config, Ctrl+Space default.
- **Version bumped to 1.1.0**.

### Fixed

- **PowerShell packaging compatibility** — TLS 1.2 enforced for `Invoke-WebRequest` on older Windows.
- **Everything SDK extraction** — three fallback methods (.NET ZipFile, Expand-Archive, Shell.Application COM) for all PowerShell versions.
- **All stale documentation references** corrected.

## [1.0.0] - 2026-05-17

### Added

- **Global hotkey** (Alt+Space by default) to summon launcher from anywhere.
- **Fuzzy search** for apps, files, and folders.
- **Custom actions and web search**.
- **Clipboard history** (optional).
- **Plugin support**.
- **Game mode**.

---

## Legacy (pre-v1.0.0 codebase)

The following entries are from the SwiftFind/Nex codebase before the v1.0.0 restart. Kept for historical reference only.

### [6.4.0] - 2026-03-31

### Added

- On-demand updater entry points: `Check for Updates` command action and tray menu item.
- Hotkey conflict recovery — Nex stays alive when hotkey cannot be registered.
- Structured `hotkey_registration_issue` diagnostics with tray-based recovery flow.
- Freshness diagnostics via `provider_freshness` and `stale_prune`.

### Changed

- New installs default to app-first search behavior (`show_files = false`, `show_folders = false`).
- Discovery scope hardened — excludes low-value system, cache, and noise paths.
- Startup diagnostics expose lifecycle markers for overlay, hotkey, and indexing readiness.
- Broad-root indexing tightens file/folder cache retention.

### [6.3] - 2026-03-13

### Added

- Windows hybrid discovery — system app and Windows Search-backed discovery.
- `show_files` / `show_folders` config toggles.
- Command-palette uninstall workflow with quick mode.
- Live config reload for safe settings.
- Delayed query execution smoothing.
- Tray-backed Game Mode toggle.
- Installer startup-choice prompt and current-user vs all-users scope.
- Rebrand from `SwiftFind` to `Nex`.

### Changed

- Config format migrated to TOML with legacy JSON fallback.
- Default hotkey changed to `Ctrl+Space`.
- Default web search provider changed to Google.
- App result rows enriched with publisher/subtitle metadata.

### Fixed

- Excluded documentation, samples, FAQ, and web shortcuts from app discovery.
- Suppressed stale uninstall entries and broken app hits.
- Fixed duplicate config-file opening from overlay.
- Fixed hotkey behavior on desktop shell surfaces and fullscreen edge cases.
- Fixed installer scope duplication and shutdown behavior.
- Fixed shell app launching for `shell:` targets.
- Fixed overlay polish issues (footer spacing, alignment, symbol rendering).
