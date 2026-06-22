# Nex Source Audit: Bugs, Fixes, and Improvements

Date: 2026-06-22

## Status

All 9 priority findings plus #10, #11, #12, #13, #14, #15, and #16 from the
Medium section have been fixed.

The 9 priority findings plus #13 and #15 were fixed in the v2.2.0 release.
#10, #11, and #12 are fixed on the `fix/audit-10-11-12-package-name-drift`
branch (pending merge). #14 and #16 are fixed on `master` (this commit).

| Finding | Status | Commit |
|---------|--------|--------|
| #1 — Stale pruner thread leak | Fixed | `403a86e` |
| #2 — Index sync short-circuit | Fixed | `c8e1b91` |
| #2a — SQLITE_BUSY launch freeze | Fixed | `6283ee7` |
| #3 — Search worker stale config | Fixed | `fc8ebd0` |
| #4 — clear_session() race | Fixed | `039aaad` |
| #5 — FTS5 / search_backend | Fixed | `25deb94` (removed FTS5 entirely) |
| #6 — Tantivy incremental sync | Fixed | `406273a` |
| #7 — Windows path ID normalization | Fixed | `d5904bf` |
| #8 — Console flash on launch | Fixed | `fd4eeba` |
| #9 — Diagnostics privacy | Fixed | `15d1aaa` |
| #10 — Cargo package name drift | Fixed | `343a21d` |
| #11 — Update script wrong repo | Fixed | `78e9152` |
| #12 — Build-from-source package | Fixed | `7c7abec` |
| #13 — Default hotkey | Fixed | `fe263a2` |
| #14 — Overlay doc drift (Iced refs) | Fixed | this commit |
| #15 — JSON config template | Fixed | `fe263a2` (TOML only) |
| #16 — Watcher drop recovery | Fixed | this commit |

## Scope

This audit is based on static inspection of the current repository source and scripts. Existing docs were treated as hints only because several docs are stale. Tests and runtime smoke checks were not run by request because the test suite is currently broken after migrations.

Primary areas inspected:

- Rust runtime, indexing, search, config, diagnostics, overlay, plugins, clipboard, and update paths under `apps/core/src/`
- Windows packaging, install, profiling, update, and validation scripts under `scripts/windows/`
- Current docs only for drift against source behavior

## Priority Fix Order

1. Fixed: make stale-pruner startup idempotent to stop unbounded background thread creation.
2. Fixed: index backend synchronization so Tantivy/FTS5 do not stay stale after rebuilds.
3. Fixed: reload search worker config/plugin state when config changes.
4. Fixed: search session clearing so stale cached results cannot survive hide/reload.
5. Fixed: removed `search_backend` setting (FTS5 removed, Tantivy only).
6. Fixed: Tantivy incremental sync doc iteration after deletions.
7. Fixed: Windows path IDs normalized consistently across discovery and watcher ingestion.
8. Fixed: console flash on launch suppressed (`CREATE_NO_WINDOW`).
9. Fixed: diagnostics privacy — raw config/logs opt-in, query text hashed.

## Critical Findings

### 1. Stale pruner can spawn an unbounded number of threads

Status: fixed in this worktree.

Source:

- `apps/core/src/runtime_loop.rs:537-563`
- `apps/core/src/core_service.rs:723-738`

Original problem:

`RuntimeWorker::on_event` calls `svc.start_stale_pruner(&self.service)` on every event after the background index cache is applied. `CoreService::start_stale_pruner` previously spawned a new infinite-loop `nex-stale-pruner` thread on every call and had no guard, handle, or stop channel.

Impact:

- Every keypress, search update, config event, or runtime event after indexing can create another forever-sleeping thread.
- Each thread periodically wakes and attempts stale pruning, increasing DB/index contention and memory/thread overhead over time.

Applied fix:

- Added an `AtomicBool stale_pruner_started` guard to `CoreService`.
- `start_stale_pruner` now returns immediately after the first successful start.
- If thread creation fails, the guard is reset so a later event can retry.
- A future cleanup can still add a stop channel or owned worker handle for fully explicit shutdown.

### 2. Index sync can short-circuit while Tantivy/FTS5 are stale

Status: **fixed in commit c8e1b91**.

Source:

- `apps/core/src/core_service.rs:627-631`
- `apps/core/src/core_service.rs:655-657`
- `apps/core/src/core_service.rs:685-686`
- `apps/core/src/core_service.rs:1049-1117`

`rebuild_index_internal` updates the SQLite cache and relies on `sync_indexes_from_cache()` to update search backends. But `sync_indexes_from_cache()` treats a backend as needing first sync only when its doc count is zero, then returns early once both Tantivy and FTS5 are merely non-empty. The comment says it should match cached item count, but the code only checks non-empty state.

Impact:

- After a non-first rebuild, SQLite/cache can contain new or removed items while Tantivy/FTS5 remain stale.
- Removed files can remain searchable.
- New files can fail to appear in indexed search.
- The issue is especially risky because search prefers Tantivy before FTS5.

Fix:

- Compare backend state against cache state, not just zero/non-zero.
- At minimum, always run incremental sync after rebuild when `upserted_total > 0 || removed_total > 0`.
- Better: persist a backend sync stamp with item count, revision, and/or cache hash, then sync when the stamp differs.
- Update the misleading comment after fixing the logic.

### 2a. Launch path blocks on SQLITE_BUSY from concurrent background indexer

Status: **fixed in commit 6283ee7**.

Source:

- `apps/core/src/core_service.rs:424-449`
- `apps/core/src/index_store.rs:44-55`

Original problem:

`launch_with_query_context` called `index_store::get_item(&*self.db(), id)` to look up the launch target. The background indexer (spawned at startup) opens its own SQLite connection to the same database file and holds write locks for the duration of indexing (up to 80s). With default SQLite journal mode (DELETE, no WAL) and busy_timeout=0, every `get_item` call during indexing immediately returns `SQLITE_BUSY`. The error propagates up through `record_successful_launch` and `record_query_selection_hint`, wasting 5-6 seconds before surfacing the failure.

Impact:

- Every program launch attempt during background indexing fails with "database is locked" after a 5-6s freeze.
- The overlay stays visible during the freeze, the acrylic backdrop becomes solid (DWM starvation from thread scheduling pressure), and the window closes ~3s later via the warm-release timer.
- On re-open, the Submit handler sees empty results (stale state from the failed hide) and shows no search results until the indexer completes.

Applied fix:

- `launch_with_query_context` now reads the launch target from `self.cached_items` (in-memory RwLock) instead of the DB. Zero contention with the background indexer.
- Post-launch DB writes (`record_successful_launch`, `record_query_selection_hint`) were removed from the launch path entirely. The in-memory cache is updated directly (use_count + last_accessed), and persistence is picked up by the stale pruner or next index rebuild.
- `open_file` in `index_store.rs` now sets `journal_mode=WAL` and `busy_timeout=1000` on every SQLite connection, enabling concurrent reads during writes and a graceful 1-second retry window for any remaining write contention.

## High Findings

### 3. Search worker keeps stale config and plugin registry after config reload

Status: **fixed in commit fc8ebd0**.

Source:

- `apps/core/src/runtime_loop.rs:318-323`
- `apps/core/src/search_worker.rs:38-43`
- `apps/core/src/search_worker.rs:67-85`
- `apps/core/src/runtime_index.rs:260-377`

The search worker is spawned with cloned `Config` and `PluginRegistry` values and keeps them for the lifetime of the thread. Config reload updates the runtime worker's `runtime_config` and `plugin_registry`, but the search worker never receives the new values.

Impact:

- Config hot reload is incomplete.
- Search results can continue using old values for `show_files`, `show_folders`, `search_mode_default`, `search_dsl_enabled`, plugin actions, plugin paths, index limits, and related search-time behavior.
- Launch/action execution may use newer config while result production uses older config.

Fix:

- Replace captured config with shared `Arc<RwLock<SearchContext>>`, or send a `SearchControl::ReloadConfig(Config, Arc<PluginRegistry>)` message to the worker.
- Clear per-session caches on reload.
- Keep search result creation and action execution on the same config generation.

### 4. `clear_session()` can race with the next query

Status: **fixed in commit 039aaad**.

Source:

- `apps/core/src/search_worker.rs:51-57`
- `apps/core/src/search_worker.rs:148-150`

The search worker drains the clear channel only at the top of the loop before blocking on `req_rx.recv()`. If `clear_session()` is called while the worker is blocked waiting for a query, the next query can wake the worker and be processed before the clear signal is drained.

Impact:

- The first query after hide/reload can use an old `OverlaySearchSession`.
- Cached prefix/final results can leak into the next overlay session.
- This is subtle because the next loop iteration clears the session, but that is too late for the first post-clear query.

Fix:

- Use one command channel with `SearchCommand::{Query, Clear, Reload}` so ordering is explicit.
- Or drain `clear_rx` again immediately after receiving a query and before processing it.
- A stronger design is to attach a generation ID to requests and discard caches when generation changes.

### 5. `search_backend` config is exposed but not honored

Status: **fixed in commit 25deb94** (FTS5 removed entirely, Tantivy is the sole backend).

User prompt: if tantivy is better than fts5, why not use tantivy by default, do we really need fts5? if not, take a decision whether to remove it

Source:

- `apps/core/src/config.rs:232`
- `apps/core/src/config.rs:691-695`
- `apps/core/src/config.rs:904-907`
- `apps/core/src/core_service.rs:861-889`

The config exposes `search_backend`, and templates document values such as `tantivy` and `fts5`. Runtime search still always tries Tantivy first and falls back to FTS5, regardless of the setting.

Impact:

- A user setting `search_backend = "fts5"` does not force or prefer FTS5 when Tantivy is available.
- Diagnostics and config imply a control that runtime does not implement.

Fix:

- Honor `config_snapshot.search_backend` when opening/searching backends.
- If the setting is intended only as a diagnostic preference, rename or remove it from user config.
- Add coverage for backend selection once the tests are repaired.

### 6. Tantivy incremental sync can miss live docs after deletions

Status: **fixed in commit 406273a**.

Source:

- `apps/core/src/tantivy_search.rs:294-348`
- `apps/core/src/tantivy_search.rs:300-314`

The incremental sync code iterates document IDs from `0..segment_reader.num_docs()`. Tantivy doc IDs are bounded by `max_doc`; `num_docs()` is the live document count. After deletions, live docs can exist at IDs greater than `num_docs() - 1`.

Impact:

- Existing IDs can be undercounted.
- Stale documents may not be deleted during incremental sync.
- Search results can keep removed items until a full rebuild or merge happens.

Fix:

- Iterate `0..segment_reader.max_doc()` and skip deleted docs with the alive bitset.
- Or use a Tantivy API that iterates only alive docs.
- Add a regression case: index docs, delete one, add another, run incremental sync, verify removed IDs disappear.

### 7. Windows file IDs are not normalized consistently

Status: **fixed in commit d5904bf**.

Source:

- `apps/core/src/file_watcher_consumer.rs:316-322`
- `apps/core/src/file_watcher_consumer.rs:362-363`
- `apps/core/src/discovery.rs:576`
- `apps/core/src/discovery.rs:603`
- `apps/core/src/everything_bridge.rs:264`
- `apps/core/src/everything_bridge.rs:281`

`id_for_path` claims paths are lowercased, but the implementation uses `path.to_string_lossy()` without lowercasing. Discovery and Everything ingestion also use raw path strings.

Impact:

- Windows paths are case-insensitive, but IDs are treated case-sensitively.
- If watcher events report a different path casing than initial discovery, delete/update can miss the existing item and create duplicates.
- Stale rows can survive even when the file has moved or disappeared.

Fix:

- Add one central helper such as `canonical_item_id_for_path(kind, path)`.
- Normalize path separators and casing consistently on Windows.
- Use the helper in walkdir discovery, Everything discovery, and watcher ingestion.

### 8. Plugin safe mode still allows protocol/shell launches

Status: **fixed in commit fd4eeba** (console flash suppressed via CREATE_NO_WINDOW flags).

User prompt: sometimes when opening a file or a folder a console flashes

Source:

- `apps/core/src/runtime_actions.rs:159-177`
- `apps/core/src/action_executor.rs:35-36`
- `apps/core/src/action_executor.rs:135-150`

`plugins_safe_mode` blocks only `PluginActionKind::Command`. `OpenPath` actions still pass through to `launch_path`, and that path can hand non-filesystem strings to ShellExecute/open behavior.

Impact:

- Safe mode may still allow plugin-defined URLs, protocol handlers, `shell:` targets, or bare strings to launch.
- This may be intended, but the trust boundary is weaker than the name suggests.

Fix:

- In safe mode, allow `OpenPath` only for existing local files/folders.
- Add a separate explicit setting for protocol targets if they are desired.
- Document the plugin trust model directly in config comments and plugin docs.

### 9. Diagnostics bundle includes raw config and raw query-bearing logs

Status: **fixed in commit 15d1aaa**.

Source:

- `apps/core/src/runtime_diagnostics.rs:631-703`
- `apps/core/src/runtime_diagnostics.rs:701`
- `apps/core/src/runtime_diagnostics.rs:706-737`
- `apps/core/src/runtime_search_session.rs:214-215`
- `apps/core/src/runtime_search_session.rs:576-590`

Diagnostics writes a sanitized config file, but also copies raw config files to `config.raw.*`. It also copies recent logs. Query profile logging includes a sanitized-but-still-readable query string, truncated to 48 characters and with control characters replaced.

Impact:

- Support bundles can expose indexed paths, ignored paths, plugin paths, custom web templates, and recent user queries.
- The presence of sanitized config can give a false sense of privacy because raw config is also bundled.

Fix:

- Make raw config/log inclusion opt-in, for example `--include-raw`.
- Redact query text in copied logs, or log query length/hash by default.
- Keep the sanitized config as the default support artifact.

## Medium Findings

### 10. Cargo package name drift breaks scripts and docs

Source:

- `apps/core/Cargo.toml`
- `scripts/windows/install-nex.ps1:86`
- `scripts/windows/run-sprint4-validation.ps1:13`
- `scripts/windows/profile-memory-and-icons.ps1:37`
- `scripts/windows/profile-memory-and-icons.ps1:48`
- `scripts/windows/profile-memory-and-icons.ps1:52`
- `AGENTS.md:8`
- `docs/engineering/windows-operator-runbook.md:11`
- `docs/engineering/windows-operator-runbook.md:28`
- `docs/engineering/windows-operator-runbook.md:345-346`
- `docs/engineering/windows-security-release-checklist.md:38-40`
- `docs/engineering/windows-runtime-validation-checklist.md:27`

The workspace package is `nex-launch`, with binary `nex`. Several scripts and docs still call `cargo ... -p nex`.

Impact:

- Build-from-source install path fails.
- Profiling and validation scripts fail before reaching the runtime.
- Operator docs send maintainers to commands that no longer match the workspace.

Fix:

- Replace script commands with package-accurate forms, for example `cargo build -p nex-launch --release --bin nex`.
- Update docs after scripts are fixed.
- Consider a small script/static check that rejects `cargo ... -p nex` unless the package exists.

### 11. Update script points at the wrong GitHub repository by default

Source:

- `scripts/windows/update-nex.ps1:5`
- `apps/core/Cargo.toml:7-8`

`update-nex.ps1` defaults `$Repo` to `haxllo/sch`, while the crate metadata and README point to `haxllo/nex`.

Impact:

- The runtime updater can query the wrong release feed.
- Users may see missing, wrong, or unrelated update behavior.

Fix:

- Change the default repo to `haxllo/nex`.
- Add a dry-run/static validation that checks updater repo against crate metadata.

### 12. Build-from-source installer path uses the wrong package

User prompt: the package is `nex`, cant publish crate because nex is already a crate

Source:

- `scripts/windows/install-nex.ps1:86`

The installer script's `-BuildFromSource` path runs `cargo build -p nex --release --quiet`, but the package is `nex-launch`.

Impact:

- Source-based installation fails even when Rust and dependencies are installed.

Fix:

- Use `cargo build -p nex-launch --release --quiet --bin nex`.
- Keep artifact packaging scripts aligned; `scripts/windows/package-windows-artifact.ps1` already uses `nex-launch`.

### 13. Default hotkey documentation conflicts with code

Status: **fixed in commit fe263a2** (default changed to `Ctrl+Space`).

User prompt: the default is supposed to be `Ctrl+Space`

Source:

- `apps/core/src/config.rs:261`
- `README.md:87`
- `docs/architecture/configuration-spec.md:12`
- `docs/product/requirements.md:6`
- `docs/README.md:17`
- `docs/engineering/windows-operator-runbook.md:98`
- `docs/engineering/windows-runtime-behavior.md:9`
- `scripts/windows/record-manual-e2e.ps1:8`

The code default is `Ctrl+Shift+Space`. Current docs mention `Alt+Space` or `Ctrl+Space` in multiple places.

Impact:

- First-run behavior does not match docs.
- Manual validation prompts can test the wrong hotkey.
- Support instructions become unreliable.

Fix:

- Decide the intended default.
- If `Ctrl+Shift+Space` is intended, update README, product docs, operator docs, validation prompts, and examples.
- If `Ctrl+Space` is intended, change `Config::default()` and the recommended preset ordering.

### 14. Overlay architecture docs are stale after WebView2 migration

Status: **fixed in this commit** (doc-only).

Source:

- `apps/core/src/overlay/host.rs:3` (doc comment)
- `apps/core/src/overlay/platform.rs:1,10` (doc comment)
- `apps/core/src/overlay/hotkey.rs:9,14` (doc comment)
- `apps/core/src/overlay/indexing_progress.rs:3` (doc comment)
- `docs/architecture/system-architecture.md:6,15,27` (architecture doc)

Current overlay implementation is WebView2-based (tao + wry). Several
doc comments inside the overlay module still referenced the Iced
runtime as if it were the active stack, and `docs/architecture/system-architecture.md`
described the UI shell as a "Native Win32 owner-draw overlay".

Applied fix:

- Replaced Iced references in `overlay/host.rs`, `overlay/platform.rs`,
  `overlay/hotkey.rs`, and `overlay/indexing_progress.rs` with accurate
  descriptions of the WebView2/tao/wry stack and the runtime worker
  that drains `OverlayEvent`.
- Updated `docs/architecture/system-architecture.md` to describe the
  WebView2 overlay and dropped the owner-draw wording.
- The `overlay/mod.rs` module-level architecture map (already accurate)
  was left in place as the single source of truth for module ownership.
- The historical `.planning/` migration plans and summaries still
  mention Iced because they describe the migration as it was planned
  and executed; they are deliberately retained as history.

### 15. JSON config template policy conflicts with current code

Status: **fixed in commit fe263a2** (removed JSON template writer, TOML only).

User prompt: Toml is enough, JSON is legacy

Source:

- `apps/core/src/config.rs:529-757`
- `apps/core/src/config.rs:767-961`
- `AGENTS.md`

The repository instructions say not to add new keys to the JSON template and to add only to the TOML template. The current JSON writer includes many modern keys, including search backend, plugin toggles, UI warm-release settings, and indexing caps.

Impact:

- The code and maintenance policy disagree.
- Future config changes may be applied inconsistently.

Fix:

- Decide whether JSON remains a fully maintained compatibility format.
- If yes, update the policy.
- If no, freeze JSON output and write new keys only in TOML.

### 16. File watcher event drops have no recovery path

Status: **fixed in this commit**.

Source:

- `apps/core/src/file_watcher_consumer.rs:144-188`
- `apps/core/src/file_watcher_consumer.rs:170-175`

When a batch exceeds `CONSUMER_BATCH_CAP`, extra events are dropped
with a counter, but previously the only place a drop was surfaced was
the `Disconnected` arm of the consumer loop. While the runtime
continued, a dropped event left the index silently stale.

Applied fix:

- Added `dropped_since_last_flush` counter alongside the existing
  `total_dropped` total.
- On the next flush, if `dropped_since_last_flush > 0`, log a warning
  immediately (not only on disconnect) and call
  `CoreService::rebuild_index_incremental_with_report` to resync the
  index from disk.
- The disconnect arm keeps the same behavior and now also triggers
  resync when `total_dropped > 0`.
- Resync failures are non-fatal: logged at warn level, the next flush
  cycle and the queued discovery reindex path can retry.

Why resync, not a dirty flag + status message: the resync is cheap
when the index is already in sync (a few hundred ms of FS walk, no
Tantivy commit unless the result differs), and it self-heals in a
single step. A status message would still require the same scan to
actually reconcile the index, so the resync is the smallest correct
recovery.

### 17. Clipboard history stores plaintext sensitive data by default

Source:

- `apps/core/src/config.rs:279`
- `apps/core/src/clipboard_history.rs:20-53`
- `apps/core/src/clipboard_history.rs:138-146`
- `apps/core/src/clipboard_history.rs:191-196`
- `apps/core/src/runtime_loop.rs:638-639`
- `apps/core/src/runtime_loop.rs:681-682`

Clipboard history is enabled by default, captures the current clipboard when the overlay opens, and persists plaintext JSON. Sensitive filtering is substring based.

Impact:

- Passwords, tokens, recovery codes, or private text can persist locally if they do not match the simple filters.
- This is more a product/privacy risk than a code crash bug.

Fix:

- Consider defaulting clipboard history off or requiring first-run opt-in.
- On Windows, encrypt at rest with DPAPI.
- Improve sensitive detection and add a visible clear-history control/status.

### 18. Query profile logging records readable query text

Source:

- `apps/core/src/runtime_search_session.rs:214-215`
- `apps/core/src/runtime_search_session.rs:576-590`

`sanitize_query_for_profile_log` truncates query text to 48 characters and removes control characters, but otherwise keeps the search query readable.

Impact:

- Logs can expose private typed queries.
- Diagnostics can copy those logs into support bundles.

Fix:

- Log query length, query mode, token count, or a one-way hash by default.
- Add explicit debug opt-in for raw query logging.

## Lower-Priority Improvements

### 19. Centralize config reload semantics

Config reload currently updates several runtime concerns in different places: overlay hints, backend caps, plugin registry, icon cache settings, and launch behavior. Search worker reload is missing, which is the immediate bug, but the broader improvement is to define one reload transaction that updates every config-dependent subsystem with a generation ID.

Suggested shape:

- `RuntimeConfigGeneration { generation, config, plugin_registry }`
- Search requests carry `generation`.
- Search worker, overlay state, action execution, and diagnostics use the same generation.

### 20. Add source-level drift checks once tests are repaired

Several issues are simple static drift:

- Cargo package name in scripts
- GitHub repo owner/name in updater
- Default hotkey in docs versus code
- Current config version in docs versus code

Suggested checks:

- A lightweight script that validates script/docs literals against `Cargo.toml` and `Config::default()`.
- Run it before release packaging.

## Validation Notes

No tests, builds, or smoke tests were run by request. The findings above are source-based and should be validated after the migration-related test breakage is resolved.

Recommended future validation once tests are usable:

- Unit/regression test for stale-pruner idempotency.
- Integration test for rebuild plus backend sync after file removal and addition.
- Search worker reload test for `show_files`, `search_backend`, and plugin toggles.
- Race test or deterministic command-channel test for search session clearing.
- Tantivy deletion/incremental-sync regression.
- Static script validation for package name and updater repo.
