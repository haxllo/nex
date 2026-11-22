# Changelog

All notable changes to Nex are documented in this file.

**Format note.** Entries up to 2.7.0 below were written at the time with full
detail. From 2.8.0 onward the long form lives in `docs/releases/v<ver>-notes.md`,
one page per version, which is what the GitHub release page shows. This file keeps
a one-line summary per version so that a reader can find the release that changed
something without opening sixty files.

## Versions after 2.7.0

| Version | Date | Summary |
|---|---|---|
| 3.3.0 | 2026-11-22 | One WebView2 environment for both windows, an honest indexer probe, and a helper that unhooks before it is terminated |
| 3.2.0 | 2026-11-20 | Win-menu mask held from the hook and while idle, panel revealed before post-idle paging, glass rim eased into the middle |
| 3.1.1 | 2026-11-19 | Rim light read from the pane's own backdrop; a moving USN journal is retried instead of forcing a full rescan |
| 3.1.0 | 2026-11-19 | First-keystroke latency cut, rim bend smoothed into the band so the edge has no seam |
| 3.0.2 | 2026-11-18 | App rows the startup load asks for are indexed; the search worker wakes when the store load lands |
| 3.0.1 | 2026-11-18 | Release page drafted from the previous release, with the notes users had not seen |
| 3.0.0 | 2026-11-18 | 27 new capabilities and 70 fixes since 2.40.0 |
| 2.40.0 | 2026-11-07 | Indexing moved fully out of the UI process; search rebuilt for scale with USN deltas and a disk icon cache |
| 2.39.0 | 2026-11-05 | NTFS MFT discovery replaces the directory walk on NTFS volumes |
| 2.38.1 | 2026-11-05 | The indexer no longer starts with a visible console window |
| 2.38.0 | 2026-11-05 | Indexing runs in its own process |
| 2.37.0 | 2026-11-03 | Files and folders under discovery roots are reliably searchable again |
| 2.36.3 | 2026-11-03 | Expanded-view action flyout stops ghosting open |
| 2.36.2 | 2026-11-03 | Compact bar stops clipping pins when both bars are visible |
| 2.36.1 | 2026-11-03 | Pinned row stops jumping to a wrong position |
| 2.36.0 | 2026-11-03 | Smarter compact bar, cleaner test gate |
| 2.35.0 | 2026-11-03 | Pin limits removed; compact view shows pins as launch icons beside search |
| 2.34.2 | 2026-11-03 | What's New shows the full release notes in their own scroll area |
| 2.34.1 | 2026-11-02 | Liquid glass stops flashing a dark panel on open |
| 2.34.0 | 2026-11-02 | Rounded edges for every panel, not only the glass one |
| 2.33.1 | 2026-11-01 | Switching between glass and solid backgrounds no longer leaves a stale surface |
| 2.33.0 | 2026-10-27 | Glass rendering fixed on laptops; cleaner first show |
| 2.32.1 | 2026-10-27 | The update notice clears itself |
| 2.32.0 | 2026-10-27 | Results use the full width; pins persist |
| 2.31.0 | 2026-10-23 | The idle panel becomes a usable surface rather than a short app list |
| 2.30.0 | 2026-10-21 | Faster, steadier glass panel |
| 2.29.1 | 2026-10-21 | New application icon |
| 2.29.0 | 2026-10-20 | The Win key behaves like a system key at every display scale |
| 2.28.0 | 2026-10-20 | The panel is drawn natively as glass by default |
| 2.27.1 | 2026-10-18 | Glass is live from the first frame; lighter surface, quieter grain |
| 2.27.0 | 2026-10-18 | Liquid Glass becomes an option in the display settings |
| 2.26.3 | 2026-10-15 | Post-update entry stays and names the detected release |
| 2.26.2 | 2026-10-15 | Update entry names the detected version and opens its notes |
| 2.26.1 | 2026-10-15 | Chat UI pass: composer, dropdowns, hints, and message chrome harmonised |
| 2.26.0 | 2026-10-14 | Text rendering fixed; the helper grows up |
| 2.25.3 | 2026-10-12 | Opaque modes start opaque, glass parks when it should, resizes stop flashing |
| 2.25.2 | 2026-10-12 | Update pill stops narrating its own progress |
| 2.25.1 | 2026-10-12 | Update pill clears itself after an up-to-date check |
| 2.25.0 | 2026-10-12 | Panel background becomes a setting; chat keeps up |
| 2.24.2 | 2026-10-10 | Updater could deadlock holding the install folder; fixed |
| 2.24.1 | 2026-10-10 | Follow-up chat turns work again; failed updates name the culprit |
| 2.24.0 | 2026-10-10 | File picker in chat, honest PC specs, tidier footer |
| 2.23.3 | 2026-10-08 | No flash-close or half-painted opens |
| 2.23.2 | 2026-10-08 | Chat reliability fixes |
| 2.23.1 | 2026-10-07 | What's New polish and a cleaner footer |
| 2.23.0 | 2026-10-07 | In-overlay chat arrives, no CLI install needed |
| 2.22.12 | 2026-10-06 | Liquid glass enabled by default with an automatic fallback |
| 2.22.11 | 2026-10-06 | Keyboard focus handed to the search field as the overlay opens |
| 2.22.10 | 2026-10-05 | Media view rebuilt; the window became glass |
| 2.22.9 | 2026-10-02 | The update button returns for per-user installs |
| 2.22.8 | 2026-10-02 | What's New view tells you what just changed |
| 2.22.7 | 2026-10-01 | Update failed with a wrong "installed for all users" error |
| 2.22.6 | 2026-10-01 | Orphan cleanup that actually fires on hybrid installs |
| 2.22.5 | 2026-10-01 | Updates that cannot brick the install; every step logged |
| 2.22.4 | 2026-10-01 | Faster downloads; stale machine-wide registration cleaned up |
| 2.22.3 | 2026-10-01 | Correct elevated orphan cleanup without flipping the install into admin mode |
| 2.22.2 | 2026-10-01 | Per-user updates elevate once to remove a legacy copy |
| 2.22.1 | 2026-10-01 | Orphaned-install detection and cleanup |
| 2.22.0 | 2026-10-01 | Now-playing media view with album art and transport controls |
| 2.21.4 | 2026-09-28 | Automatic silent updates |
| 2.21.3 | 2026-09-28 | Search, icon, and watcher reliability |
| 2.21.2 | 2026-09-26 | Update notifications and safer updating |
| 2.21.1 | 2026-09-26 | Icon cache invalidation fixed so 2.21.0's icons appear |
| 2.21.0 | 2026-09-25 | Faster search, robust indexing, better diagnostics |
| 2.20.0 | 2026-09-19 | Web bookmarks with favicons, and a Quick Launch overhaul |
| 2.19.0 | 2026-09-15 | Windows lifecycle, search, persistence, accessibility, and overlay interaction fixes |
| 2.18.0 | 2026-09-08 | Discoverable search modes, visual clipboard history, safer settings |
| 2.17.1 | 2026-09-03 | Hotkey flash fixed; visible keyboard selection; caption drag cleaned up |
| 2.17.0 | 2026-08-29 | Settings window redesigned with native hotkey capture; MIT licence in the installer |
| 2.16.0 | 2026-08-22 | Inline power menu replaces the popup; "Show all apps" expansion |
| 2.15.1 | 2026-08-18 | Hotkey opens immediately while indexing; two first-run WebView2 crashes fixed |
| 2.15.0 | 2026-08-18 | Startup made instant, icons for settings pages, short queries stop hiding files |
| 2.14.2 | 2026-08-15 | Automatic accent colour decoded correctly; accent fills brightened |
| 2.14.1 | 2026-08-15 | Folders return to results; typing stops causing visible re-renders |
| 2.14.0 | 2026-08-10 | Selection mapped through the row index; sparse glyph icons fixed |
| 2.13.0 | 2026-08-05 | Create a folder or file from an unmatched query |
| 2.12.0 | 2026-08-04 | New icon extraction pipeline; sharper icons in grid view |
| 2.11.0 | 2026-08-02 | Power menu and idle power popup in the overlay footer |
| 2.10.0 | 2026-07-31 | Hotkey can be changed while running; Win-key chord detection fixed |
| 2.9.1 | 2026-07-29 | Release metadata only |
| 2.9.0 | 2026-07-29 | Release metadata only |
| 2.8.1 | 2026-07-24 | Release metadata only |
| 2.8.0 | 2026-07-24 | Overlay flash fixes, retained GPU layers, themed icons, web-search row on no results |

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
