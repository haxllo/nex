# Windows Runtime Validation Checklist

Use this checklist on a real Windows host after building the current branch.

## Preconditions

- Rust toolchain available (`cargo --version`)
- Node + pnpm available (`node -v`, `pnpm -v`)
- Repository dependencies installed (`pnpm install`)

## Automated Validation

Run:

```powershell
scripts/windows/run-sprint4-validation.ps1
```

Expected:

- Windows runtime smoke harness test passes.
- Launcher UI flow tests pass.

## v2.1 Reliability Scenarios

1. Structured status output
- Run: `cargo run -p nex -- --status-json`
- Expected: valid JSON with `runtime_state`, `diagnostics.startup_lifecycle`, `diagnostics.startup_indexing`, `diagnostics.cache_compaction`, `diagnostics.memory_snapshot`, `diagnostics.icon_cache`, `diagnostics.config_reload`, and `query_latency`.
- Expected: `diagnostics.startup_lifecycle` includes:
  - `overlay_ready`
  - `hotkey_ready`
  - `indexing_started`
  - `indexing_completed`
  - `cache_applied`
- Expected: `diagnostics.cache_compaction` exposes:
  - `input_total`
  - `retained`
  - `dropped`
  - `retained_apps`
  - `retained_file_folders`
  - `effective_file_seed_cap`
  - `broad_root_mode`
- Expected: `diagnostics.provider_freshness` exposes:
  - `name`
  - `skipped`
  - `last_scan_age_secs`
  - `reconcile_interval_secs`
  - `has_stamp`
- Expected: `diagnostics.stale_prune` exposes:
  - `scanned`
  - `removed`
  - `cached_items_remaining`
- Expected: `diagnostics.icon_cache` exposes `live_entries` and `max_entries`.

2. Baseline profile harness
- Run: `scripts/windows/profile-memory-and-icons.ps1`
- Expected: script updates config for `C:\` profiling, starts runtime, prints `--status-json`, and dumps recent `query_profile`/`memory_snapshot` lines.
- Expected: recent log output contains `startup_phase`, `cache_compaction`, `provider_freshness`, and `stale_prune` markers.

3. Memory envelope with broad discovery root
- Set `discovery_roots = ["C:\\"]` and keep:
- `windows_search_enabled = true`
- `windows_search_fallback_filesystem = true`
- `index_max_items_total`, `index_max_items_per_root`, `index_max_items_per_query_seed` at defaults unless testing overrides.
- Exercise launcher with short and medium queries for at least 2 minutes.
- Expected: active working set tracks close to `active_memory_target_mb`; idle trims occur after hide.
- Expected: `cache_compaction` shows `broad_root_mode = true` and a smaller `effective_file_seed_cap` than the configured `index_max_items_per_query_seed`.
- Expected: app results remain complete even when file/folder cache compaction is active.
- Expected: `provider_freshness` shows low `last_scan_age_secs` after a fresh reindex and `has_stamp = true` for incremental providers.

4. Live config apply (no restart for discovery/search tuning)
- Keep runtime running.
- Edit and save `%APPDATA%\Nex\config.toml` fields:
- hot-apply fields: `max_results`, `show_files`, `show_folders`, `search_mode_default`, `search_dsl_enabled`, `clipboard_*`, `plugins_*`, `web_search_*`, `idle_cache_trim_ms`, `active_memory_target_mb`, `index_max_items_*`.
- provider-refresh fields: `discovery_roots`, `discovery_exclude_roots`, `windows_search_enabled`, `windows_search_fallback_filesystem`.
- Expected: launcher status updates to `Settings applied`, `Discovery settings updated; reindexing...`, or `Discovery settings queued; reindex starts after debounce`; no process restart required.

5. Restart-required behavior
- Change `hotkey` or `index_db_path` and save config.
- Expected: launcher status indicates restart requirement; setting is not fully active until restart.

## Manual E2E Flow (Required)

1. Start runtime/application process for this milestone build.
- Expected: process is running without immediate crash.

2. Press the configured hotkey with another app focused.
- Expected: launcher overlay opens; query input is focused.
- Expected: launcher opens in compact bar state (no visible results list).
- On the first cold run with an empty cache, expect `Indexing in background...` until the background index finishes.
- Once that first background index is applied, expect a one-shot `Index ready` status when the overlay is shown idle.

3. Type a query that should match indexed content (for example `code` or `report`).
- Expected: result list updates with real indexed items.
- Expected: results panel expands downward only (top edge remains fixed).
- Expected: rows show clean title + path hierarchy (no raw tab separators).

4. Use `ArrowDown` / `ArrowUp` to change selected result.
- Expected: selected row changes as keys are pressed.
- Expected: moving mouse over rows updates the same active row state (no separate stale selected+hovered highlight).
- Expected: first wheel movement after query update scrolls in discrete steps (3 rows per notch), without one-time easing.

5. Press `Enter` on a valid result.
- Expected: selected launch path is executed.
 - Expected: launcher closes immediately after successful launch.

6. Single-click a valid result row.
- Expected: clicked result launches immediately (no separate confirm click required).

7. Trigger an invalid launch target (missing path or denied access).
- Expected: user-visible error message appears in launcher UI.

8. Close behavior checks.
- Press `Esc`: launcher hides and query clears.
- Click outside launcher: launcher hides and query clears.
- Reopen with hotkey: input starts clean with no stale query text.

9. Settings access checks.
- Click `?` in launcher input area.
- Expected: `%APPDATA%\Nex\config.toml` opens for manual edits.
- Edit `hotkey` or `max_results`, save, and verify behavior updates.
- Restart is only required if you changed `hotkey` or `index_db_path`.

10. Future settings UI note.
- Native settings UI is intentionally disabled from `?` for now.
- Expected: no crash; manual config path remains available.

11. Lifecycle command checks.
- Run `nex.exe --status` while runtime is active.
- Expected: reports running.
- Run `nex.exe --quit`, then `--status`.
- Expected: reports stopped after quit.

12. Clean install checks.
- Install from packaged artifact (`setup.exe` or install script from zip).
- Expected: install completes without requiring Rust/Cargo.
- Expected: runtime can start and hotkey works on first launch.
- Force a hotkey conflict with another launcher or temporary test app.
- Expected: runtime still starts, tray stays available, log includes `hotkey_registration_issue`, and `Open Config` is available from the tray menu.

13. Upgrade-over-existing checks.
- Install a newer build over an existing installed build.
- Expected: install succeeds without manual uninstall.
- Expected: runtime restarts cleanly and hotkey registration remains valid.
- Expected: config file in `%APPDATA%\Nex\config.toml` is preserved.

14. Channel updater checks.
- Trigger `Check for Updates` from command mode and from the tray menu.
- Expected: both entry points launch the stable updater flow without requiring a background updater service.
- Run `scripts/windows/update-nex.ps1 -Channel stable`.
- Expected: manifest and installer are downloaded.
- Expected: installer checksum is verified before apply.
- Expected: update applies cleanly and runtime can be started.
- Run `scripts/windows/update-nex.ps1 -Channel beta` (on beta tag availability).
- Expected: beta channel resolves beta-tagged release only.

15. Uninstall + reinstall checks.
- Uninstall from Windows Apps settings or installer uninstaller.
- Expected: runtime process is no longer present in Task Manager.
- Expected: hotkey no longer triggers launcher.
- Reinstall latest setup.
- Expected: launcher works again and startup registration can be applied.

16. Rollback checks.
- After installing a newer build, reinstall the previous known-good build.
- Expected: older runtime starts successfully.
- Expected: no stuck background process from replaced version.
- Expected: core launcher flow (open, query, launch, close) works after rollback.

Record pass/fail evidence:

```powershell
scripts/windows/record-manual-e2e.ps1
```

Expected output file:

- `artifacts/windows/manual-e2e-result.json`
- `all_passed: true` for release readiness.

## Release Blockers

Do not mark the milestone release-ready if any gate below fails:

- Memory gate: active working set exceeds configured target by more than 25% for 3 consecutive profile runs.
- Discovery gate: editing discovery config does not trigger automatic background reindex.
- Config gate: hot-apply fields require process restart to take effect.

Note: icon-specific release gating is intentionally deferred from this validation set.
