# SwiftFind Reliability + UX Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix high RAM spikes with broad discovery roots, make config changes apply immediately where safe, and make app icons reliable for system/UWP/packaged apps like Notepad and WezTerm.

Scope note (2026-03-02): Task 4 (icon resolution changes) is deferred from the active implementation set for this plan iteration.

**Architecture:** Keep indexing and launch behavior Windows-native, but separate live runtime updates from index-provider lifecycle and split search caching into app-first hot cache plus DB-backed file/folder retrieval. Add deterministic icon resolution pipeline for `shell:AppsFolder` items with explicit fallback ordering and diagnostics.

**Tech Stack:** Rust (`swiftfind-core`), Windows Shell APIs, PowerShell discovery bridge, SQLite via `rusqlite`, existing overlay runtime (`windows_overlay.rs`), config model (`config.rs`).

---

### Task 1: Baseline + Repro Harness

**Files:**
- Modify: `apps/core/src/runtime.rs`
- Modify: `apps/core/src/logging.rs`
- Test: `apps/core/tests/windows_runtime_smoke_test.rs`
- Create: `scripts/windows/profile-memory-and-icons.ps1`

**Step 1: Add structured status output for memory and icon health**

Implement a `--status-json` command variant exposing:
- last `memory_snapshot` values
- icon cache metrics
- config reload timestamp
- provider summary

**Step 2: Add script to reproduce current problem states**

Script should run:
- launch runtime in background
- apply `discovery_roots=["C:\\"]`
- trigger rebuild
- capture status JSON + recent logs
- search for `notepad`, `wezterm`

**Step 3: Verify baseline data capture**

Run: `cargo run -p swiftfind-core -- --status-json`
Expected: valid JSON including memory + icon sections.

**Step 4: Commit**

```bash
git add apps/core/src/runtime.rs apps/core/src/logging.rs apps/core/tests/windows_runtime_smoke_test.rs scripts/windows/profile-memory-and-icons.ps1
git commit -m "chore(diagnostics): add status-json and repro profile script"
```

---

### Task 2: Live Config Apply (No Restart for Discovery Changes)

**Files:**
- Modify: `apps/core/src/runtime.rs`
- Modify: `apps/core/src/core_service.rs`
- Modify: `apps/core/src/config.rs`
- Test: `apps/core/tests/discovery_test.rs`
- Test: `apps/core/tests/config_test.rs`

**Step 1: Add runtime config apply policy table**

Add explicit policy:
- **Hot apply now:** `max_results`, `show_files`, `show_folders`, `search_mode_default`, `search_dsl_enabled`, `clipboard_*`, `plugins_*`, `web_search_*`, overlay tuning fields.
- **Hot apply with provider refresh:** `discovery_roots`, `discovery_exclude_roots`, `windows_search_enabled`, `windows_search_fallback_filesystem`.
- **Still restart-required:** `hotkey` only.

**Step 2: Add provider reconfiguration API to `CoreService`**

Add method:
- `reconfigure_runtime_providers(&self, cfg: &Config)` to rebuild provider list from new config.

**Step 3: Trigger automatic incremental rebuild on discovery config change**

In `maybe_apply_runtime_config_reload`, when discovery fields change:
- call `reconfigure_runtime_providers`
- trigger background incremental rebuild
- update status text: `"Discovery settings updated; reindexing..."`.

**Step 4: Write/adjust tests**

Add tests ensuring:
- discovery root changes no longer only log “restart recommended”
- provider roots change is applied at runtime
- hotkey still logs restart requirement.

**Step 5: Commit**

```bash
git add apps/core/src/runtime.rs apps/core/src/core_service.rs apps/core/src/config.rs apps/core/tests/discovery_test.rs apps/core/tests/config_test.rs
git commit -m "feat(config): apply discovery settings live and auto-rebuild providers"
```

---

### Task 3: Memory Envelope for `C:\` Discovery

**Files:**
- Modify: `apps/core/src/config.rs`
- Modify: `apps/core/src/discovery.rs`
- Modify: `apps/core/src/core_service.rs`
- Test: `apps/core/tests/discovery_test.rs`
- Test: `apps/core/tests/core_service_test.rs`

**Step 1: Add index budget config**

Add new config keys:
- `index_max_items_total` (default: `120000`)
- `index_max_items_per_root` (default: `40000`)
- `index_max_items_per_query_seed` (default: `5000`)

Write template comments explaining trade-off (coverage vs RAM/startup).

**Step 2: Enforce provider-side caps**

In Windows Search and filesystem discovery:
- stop collecting once per-root and total caps are reached
- log capped counts per provider/root
- preserve deterministic ordering.

**Step 3: Reduce in-memory footprint by kind**

Change cache strategy:
- keep full app cache in memory
- keep file/folder cache as capped subset for short-query responsiveness
- rely on indexed DB for deeper retrieval paths.

**Step 4: Add tests**

Add tests that verify:
- caps are honored
- rebuild completes with caps and non-empty result set
- app results are unaffected by file caps.

**Step 5: Commit**

```bash
git add apps/core/src/config.rs apps/core/src/discovery.rs apps/core/src/core_service.rs apps/core/tests/discovery_test.rs apps/core/tests/core_service_test.rs
git commit -m "feat(index): add discovery item budgets and memory-aware caching"
```

---

### Task 4: Reliable Icon Resolution for Notepad/WezTerm and Other `AppsFolder` Entries

**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Modify: `apps/core/src/discovery.rs`
- Modify: `apps/core/src/model.rs` (if icon metadata field needed)
- Test: `apps/core/tests/discovery_test.rs`

**Step 1: Normalize and store app identity for `Get-StartApps` entries**

Preserve normalized `app_id` token from `Get-StartApps` so icon resolution does not depend on fragile path text parsing.

**Step 2: Add stricter icon resolver chain for app items**

For app kind:
1. explicit `app_id` -> `shell:AppsFolder\\{app_id}` parse
2. shortcut target icon if `.lnk`
3. executable icon
4. system image list fallback
5. semantic glyph fallback

Track which stage succeeded.

**Step 3: Add missing-icon diagnostics**

When icon fails for app:
- log `icon_resolution_failed kind=app title=... source=...`
- include failure counters in status diagnostics.

**Step 4: Add regression tests for `AppsFolder` token handling**

Unit tests for:
- `extract_appsfolder_token`
- candidate generation
- known app IDs with package/family separators.

**Step 5: Commit**

```bash
git add apps/core/src/windows_overlay.rs apps/core/src/discovery.rs apps/core/src/model.rs apps/core/tests/discovery_test.rs
git commit -m "fix(icons): harden AppsFolder icon resolution and diagnostics"
```

---

### Task 5: UX Improvements that Directly Address Current Confusion

**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Modify: `apps/core/src/action_registry.rs`
- Modify: `apps/core/src/runtime.rs`

**Step 1: Surface config-apply behavior in UI status**

When config changes:
- show `"Settings applied"` for immediate fields
- show `"Reindex started"` for discovery fields
- show `"Restart required for hotkey"` for hotkey changes.

**Step 2: Add command actions for safe operations**

Add/confirm commands:
- `Reindex now`
- `Trim memory now` (clears icon cache and optional query cache)
- `Show diagnostics` (opens logs/status bundle).

**Step 3: Commit**

```bash
git add apps/core/src/windows_overlay.rs apps/core/src/action_registry.rs apps/core/src/runtime.rs
git commit -m "feat(ux): expose config apply states and runtime maintenance actions"
```

---

### Task 6: Validation Matrix + Release Gate

**Files:**
- Modify: `docs/engineering/windows-runtime-validation-checklist.md`
- Modify: `docs/engineering/windows-operator-runbook.md`
- Create: `docs/releases/v2.1.0-notes.md`

**Step 1: Add validation scenarios**

Document pass criteria:
- `discovery_roots=["C:\\"]` memory envelope targets (active and idle)
- icon correctness for Notepad + WezTerm
- live config behavior without restart.

**Step 2: Define release gate thresholds**

Block release if:
- active WS > target by >25% for 3 consecutive runs
- app icon failure rate > 2% on top-200 app results
- discovery change does not trigger rebuild automatically.

**Step 3: Commit**

```bash
git add docs/engineering/windows-runtime-validation-checklist.md docs/engineering/windows-operator-runbook.md docs/releases/v2.1.0-notes.md
git commit -m "docs(release): add reliability validation matrix and v2.1 notes"
```

---

## Implementation Order

1. Task 1 (baseline first, no behavior risk)
2. Task 2 (live config apply correctness)
3. Task 3 (memory envelope)
4. Task 4 (icon reliability)
5. Task 5 (UX clarity)
6. Task 6 (release gate + docs)

## Proposed New Features Included in This Plan

1. Live discovery-config apply with automatic background reindex.
2. Configurable index budgets for large roots (`C:\`) to prevent memory blowups.
3. App icon diagnostics and hardened `AppsFolder` icon pipeline.
4. Runtime maintenance actions (`Trim memory now`, `Show diagnostics`).
5. Structured `--status-json` for measurable performance/reliability tracking.

## Success Criteria

1. `C:\` discovery no longer causes unbounded memory growth; active memory stays within configured envelope.
2. Most config changes apply immediately; only hotkey requires restart.
3. Notepad and WezTerm display proper icons consistently.
4. User-facing status clearly explains what changed and what action (if any) is required.
