# Windows Operator Runbook (Current)

This runbook covers how to run and validate the Nex native Windows overlay runtime.
Nex is the product name; the current internal Windows runtime binary remains `nex.exe`.

## Start Runtime

From repo root:

```powershell
cargo run -p nex
```

Installed release mode (recommended for users):

```powershell
.\scripts\install-nex.ps1
```

Notes:

- When run from the packaged release zip, installer uses prebuilt `bin\nex.exe`.
- Rust/Cargo is not required for end users.

Background mode (detached, no terminal dependency):

```powershell
cargo run -p nex -- --background
```

Expected startup logs include:

- runtime mode
- configured hotkey
- config path
- index db path
- indexed item count
- indexing totals (`discovered`, `upserted`, `removed`)
- per-provider indexing diagnostics (`name`, `discovered`, `upserted`, `removed`, `skipped`, `elapsed_ms`)

## Process Visibility

When running on Windows:

- `cargo run` starts child process `nex.exe`.
- In Task Manager, look for `nex.exe` (and `cargo.exe` while running from Cargo).

## Lifecycle Commands

From repo root or installed binary:

```powershell
nex.exe --status
nex.exe --status-json
nex.exe --quit
nex.exe --restart
nex.exe --ensure-config
nex.exe --sync-startup
nex.exe --diagnostics-bundle
```

Notes:

- `--status` reports running/stopped and degraded process-without-window state.
- `--status-json` reports structured diagnostics for automation (`memory_snapshot`, icon cache metrics, config reload line, and query latency summary).
- `--quit` attempts graceful stop and falls back to force stop if runtime is stuck.
- `--restart` performs the same stop flow then starts runtime again.
- `--ensure-config` creates `%APPDATA%\Nex\config.toml` if missing.
- `--sync-startup` applies `launch_at_startup` from config to HKCU Run.
- `--diagnostics-bundle` writes a support bundle with summary, sanitized config, and recent logs.

## Update Commands

Channel-aware updater (user-triggered, no background updater service):

```powershell
scripts/windows/update-nex.ps1 -Channel stable
scripts/windows/update-nex.ps1 -Channel beta
scripts/windows/update-nex.ps1 -Channel stable -Version "1.2.0"
```

In-app entry points:

- Command mode action: `Check for Updates`
- Tray menu: `Check for Updates`
- Both launch the stable updater script on demand; no background polling is performed by `nex.exe`.

Updater behavior:

- Downloads release `manifest.json` + `setup.exe` from GitHub release assets.
- Verifies installer SHA256 from manifest before apply.
- Stops runtime and snapshots current install folder for rollback.
- Applies installer silently.
- Restores previous snapshot automatically if update fails.

## Default Hotkey and Config

- Default hotkey: `Ctrl+Space`
- Config file: `%APPDATA%\Nex\config.toml`
- Index DB: `%APPDATA%\Nex\index.sqlite3`
- Install root (scripted install):
- `CurrentUser` scope: `%LOCALAPPDATA%\Programs\Nex\`
- `AllUsers` scope: `%ProgramFiles%\Nex\` (requires elevated install)

Installer scope behavior (`setup.exe`):

- Default: `Current user`
- Optional: `All users` (prompts elevation and installs to machine location)

If config file does not exist, runtime writes defaults to the stable app-data location on startup.
The generated file is a user-focused TOML template with inline comments.

## Settings (Current)

Current behavior:

1. Open launcher with your configured hotkey.
2. Click `?` in the right side of the input area.
3. Nex opens `%APPDATA%\Nex\config.toml`.
4. Edit and save config.
5. Most settings apply automatically within about 1 second.
6. Restart `nex` only after `hotkey` or `index_db_path` changes.

## Change Hotkey via Config File

1. Open `%APPDATA%\Nex\config.toml` directly.
2. Update the `hotkey` value.
3. Restart `nex`.

Notes:

- You can keep inline comments in this file (`// ...`).
- Most users only need to edit `hotkey`.
- `launch_at_startup`, `max_results`, `discovery_roots`, and `discovery_exclude_roots` are optional tuning.
- `index_max_items_total`, `index_max_items_per_root`, and `index_max_items_per_query_seed` tune memory/coverage tradeoffs for large discovery roots.

## Live Config Apply Matrix

- Applies immediately:
- `max_results`, `show_files`, `show_folders`, `search_mode_default`, `search_dsl_enabled`
- `clipboard_*`, `plugins_*`, `web_search_*`
- `idle_cache_trim_ms`, `active_memory_target_mb`
- `index_max_items_total`, `index_max_items_per_root`, `index_max_items_per_query_seed`

- Applies with provider refresh + background reindex:
- `discovery_roots`, `discovery_exclude_roots`
- `windows_search_enabled`, `windows_search_fallback_filesystem`

- Requires restart:
- `hotkey`, `index_db_path`

## Memory Profiling Workflow

Use this for broad-root validation (`C:\`) and regression tracking:

```powershell
scripts/windows/profile-memory-and-icons.ps1
```

Expected output includes:

- `--status-json` snapshot
- `--status` summary
- recent `query_profile`, `memory_snapshot`, `overlay_icon_cache`, and `config reloaded` lines

## Settings Direction

Settings are intentionally file-driven in the current product direction.
`?` opens `%APPDATA%\Nex\config.toml`, and this remains the supported path for:

- hotkey changes
- startup toggle (`launch_at_startup`)
- result-count tuning (`max_results`)
- include/exclude discovery roots (`discovery_roots`, `discovery_exclude_roots`)

Recommended low-conflict hotkeys on Windows:

- `Ctrl+Space` (default)
- `Ctrl+Shift+Space`
- `Ctrl+Alt+Space`
- `Alt+Shift+Space`
- `Ctrl+Shift+P`
- `Ctrl+Alt+P`

Avoid these common system/reserved shortcuts:

- `Win+...` combinations
- `Alt+Tab`
- `Ctrl+Esc`
- `Alt+Space` (can conflict with the window system menu)

## Launcher Flow (Native Overlay)

1. Press hotkey (`Ctrl+Space` by default).
2. A centered floating launcher overlay appears and input is focused.
3. Type query text to fetch ranked results.
4. Use `Up` / `Down` to move selection.
5. Press `Enter` to launch selected result, or single-click a result row.
6. Press `Esc` to hide launcher.

Behavior details:

- Pressing the configured hotkey while launcher is focused hides it.
- Pressing the configured hotkey while launcher is visible but not focused brings focus back.
- Clicking outside launcher hides it.
- Closing launcher clears current query/results (next open is clean).
- On first run (new config), launcher shows a brief onboarding hint with hotkey/config guidance.
- Search and launch failures are shown in launcher status text.
- Typing `log` in launcher adds an action to open `%APPDATA%\Nex\logs`.

## Logs and Diagnostics

- Log directory: `%APPDATA%\Nex\logs`
- Current file: `nex.log`
- Rotation: old logs are archived when current file reaches size threshold.
- Panic/crash details are written to logs via runtime panic hook.
- Overlay icon cache writes diagnostics on cache-clear events:
  - `overlay_icon_cache reason=... hits=... misses=... load_failures=... evictions=... cleared_entries=...`

## UI Characteristics (Final)

- Compact centered launcher bar (default compact height; no oversized blank state).
- Panel colors:
  - background `#272727`
  - border `#424242` (1px)
- Rounded panel with subtle depth and improved typography hierarchy.
- Input placeholder shown directly in the input box (`Type to search`).
- Result rows are structured two-line cards:
  - primary title line (higher contrast, semibold)
  - secondary path line (muted, ellipsized)
- Per-item native icon rendering is used for app/file/folder rows (glyph fallback only when icon load fails).
- Active row uses one soft emphasis style for both keyboard and mouse (no dual selected+hovered highlight).
- Input focus is forced on open and text is selected for immediate re-query.
- Results panel stays collapsed for empty query, and expands downward only after matching query text.
- Results panel has no top gap and uses matched side/bottom spacing.
- Overlay and results transitions are short and smooth:
  - show/hide fade + scale (~150ms)
  - results expand/collapse height + opacity (~150ms)
  - selection transition (~90ms)

## Results Section Rationale (Before/After)

Before:
- rows used compact single-line text and weak hierarchy; long paths reduced legibility.
- selection state was visible but visually close to non-selected rows.

After:
- two-line row hierarchy improves scan speed and disambiguation for similarly named items.
- type glyph gives quick context without visual noise.
- path line remains readable while preserving compact density.
- selection and hover are unified into one active-row state for keyboard + mouse workflows.

## Manual Validation Checklist

Run these on a real Windows host:

1. Start runtime:
   - `cargo run -p nex`
2. Confirm process visibility:
   - Task Manager shows `nex.exe`.
3. Confirm hotkey:
   - configured hotkey opens the overlay.
4. Confirm search + launch:
   - Type a query with known indexed result.
   - Press `Enter` and verify target launches.
5. Confirm hide behavior:
   - Press `Esc` to hide overlay.
   - Press configured hotkey while focused to hide overlay.
   - Click outside overlay and verify it hides immediately.
   - Reopen overlay and verify prior query text is cleared.
6. Confirm visual polish:
   - Window appears compact and centered (not oversized).
   - Empty query shows compact bar only (no visible results list).
   - Typing matching query expands results downward from a fixed top edge.
   - Result rows render as clean two-line title/path entries (no literal tab separators).
   - Icons are crisp (no obvious blur) for app, file, and folder rows at your current display scale.
   - Icons render without a decorative icon backplate/border.
   - Result rows have no hard border selection box; emphasis is subtle/hover-style.
   - Moving mouse across rows updates one active highlight and clears the previous row highlight.
   - Status line color changes for error states.

## Screenshot Notes (Before/After)

Capture two screenshots on a Windows host for release notes:

1. Before polish reference:
   - plain large launcher shell from earlier Sprint 6 state.
2. After polish reference:
   - compact rounded launcher shell with styled input/list/status and fade transitions.

Recommended capture points:

- `overlay-idle.png`: just opened (configured hotkey), empty query, focused input.
- `overlay-results.png`: populated results with one selected row.
- `overlay-error.png`: launch/search error status visible.
- `overlay-expand.gif` (or short video): compact bar expanding downward while typing.
- `overlay-before-after-rows.png`: side-by-side comparison of old vs redesigned results rows.

## Troubleshooting Checklist

1. Hotkey does not trigger:
   - Check startup log for `hotkey registered native_id=...`.
   - If the log shows `hotkey_registration_issue`, use the tray menu and choose `Open Config`.
   - Try changing hotkey in `%APPDATA%\Nex\config.toml` to avoid OS/app conflicts.
   - Restart runtime after config change.
   - Check if another launcher utility (PowerToys, Flow Launcher, etc.) is intercepting your chosen hotkey.
2. Overlay does not focus or appears behind apps:
   - Trigger the configured hotkey twice to force refocus.
   - Ensure runtime is still active (`nex.exe`).
   - Disable conflicting always-on-top/focus-stealing utilities while validating.
3. No results returned:
   - Check startup `indexed_items` value.
   - Confirm discovery roots in config are valid and accessible.
   - Re-run runtime to rebuild index.
4. Launch fails:
   - Read launcher status text (`Launch error: ...`).
   - Confirm selected item path still exists.

5. App icons show shortcut arrow overlays:
   - Expected behavior after current icon pipeline:
     - app `.lnk` entries resolve target/icon metadata first
     - shortcut-file icon fallback is blocked for app shortcuts to avoid arrow overlays
     - `shell:AppsFolder\\...` (UWP/AppX) targets now attempt PIDL/display-name icon resolution before generic fallback
   - If arrows still appear for specific apps:
     - capture the exact app names (for example: Chrome, Visual Studio)
     - restart runtime once (`nex.exe --restart`) to refresh in-memory icon cache
     - re-test with the same query
   - If still reproducible, treat as app-specific shortcut metadata edge case and record:
     - shortcut path
     - whether target executable icon differs from shortcut icon
   - Verify the path is launchable from Explorer/Run dialog.
6. Process not visible:
   - Ensure `cargo run -p nex` is still running.
   - Verify `nex.exe` in Task Manager Details tab.
7. JS tests flaky on Windows:
   - Use `pnpm vitest --run` with repo `vitest.config.ts` (single-fork mode configured).
8. Config open/edit issues:
   - Ensure `%APPDATA%\Nex\config.toml` is writable.
   - Check if the config file is locked by another process/editor.

## Validation Commands

```powershell
pnpm vitest --run
cargo test -p nex
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
```

## Release Lifecycle Validation

Run these on a Windows host before publishing a release.

1. Clean install:
   - install `nex-<version>-windows-x64-setup.exe`.
   - verify launcher starts and hotkey works without Rust/Cargo installed.

2. Upgrade-over-existing:
   - run `scripts/windows/update-nex.ps1 -Channel stable -Version "<newer-version>"`.
   - verify checksum verification passes and install completes without manual cleanup.
   - verify `%APPDATA%\Nex\config.toml` remains intact.

3. Uninstall + reinstall:
   - uninstall from Windows Apps settings (or uninstaller).
   - verify `nex.exe` is not running and hotkey no longer triggers.
   - reinstall and verify normal runtime behavior returns.

4. Rollback:
   - force a failed update path (or reinstall previous known-good setup).
   - verify updater restores previous snapshot automatically when apply fails.
   - verify manual reinstall rollback still works as fallback path.
   - verify runtime starts and open/query/launch/close flow still works.

Release-channel/update policy reference:
- `docs/engineering/windows-update-rollout-strategy.md`
- `docs/engineering/windows-security-release-checklist.md`

## Progress Update (2026-02-13)

Completed from recent hardening pass:
1. Duplicate overlay entries reduced:
   - runtime now deduplicates app-title duplicates and normalized-path duplicates before drawing rows.
2. First-query list jump reduced:
   - overlay suppresses first synthetic hover event after list refresh to keep row 1 stable.
3. Click-outside close stabilized:
   - deactivate path closes overlay reliably when focus moves away (excluding help-tip ownership cases).
4. Shortcut icon quality improved:
   - icon-source normalization includes environment-variable expansion for more valid icon locations.
   - app-shortcut fallback path avoids direct `.lnk` self-icon usage where it causes overlay artifacts.

Operator note:
- Existing validation steps remain valid.
- Add one explicit check after update:
  - type first letter of a query and confirm first row remains visible and selectable without auto-jump.
  - first wheel movement after query update should be discrete (3 rows/notch), without one-time easing.
