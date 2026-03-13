# Master Phase and Task Tracker

This file is the single planning/status source of truth for Nex execution.

Last updated: 2026-02-23

## Phase 0: Foundation
Status: `COMPLETED`

Tasks:
- [x] Repository scaffolding
- [x] Rust core service skeleton
- [x] Launcher UI shell baseline
- [x] Config load/save plumbing

## Phase 1: Search MVP
Status: `COMPLETED`

Tasks:
- [x] App/file discovery pipeline
- [x] SQLite index store + in-memory search flow
- [x] Fuzzy ranking and keyboard navigation
- [x] Launch/open actions
- [x] Baseline tests and performance harness

## Phase 2: Quality and UX Parity
Status: `COMPLETED`

Major tasks:
- [x] Visual tokenized black-shade UI pass
- [x] Compact idle + downward-only expansion behavior
- [x] Animation smoothness and interaction polish
- [x] Keyboard/mouse parity hardening (Enter/Esc/Up/Down/click)
- [x] Result dedupe + first-keystroke list stability fixes
- [x] Deterministic first-wheel scrolling + unified hover/selection active-row behavior
- [x] Config-file settings workflow finalized (`?` -> config.toml)

## Phase 3: Beta Hardening
Status: `COMPLETED`

Task list (from parity/hardening plan):
- [x] Task 6: Installer lifecycle hardening (`RunOnceId`, uninstall cleanup hardening)
- [x] Task 7: Upgrade/rollback validation checklist additions
- [x] Task 8: Update-channel/rollout strategy (`stable`/`beta`) without runtime updater
- [x] Task 9: Windows security release checklist and release gating docs
- [x] Task 10: Include/exclude roots controls (`discovery_roots`, `discovery_exclude_roots`)

Additional Phase 3 hardening completed:
- [x] AppsFolder/UWP icon fallback for edge generic-icon reduction
- [x] Packaging/docs alignment for release operations

Phase 3 closeout note:
- [x] Closed for stable release preparation

## Phase 4: Stable Release and Post-Launch
Status: `IN PROGRESS`

Major tasks:
- [ ] Execute final Windows release-candidate manual/security checklist and record evidence
- [x] Publish stable `v1.0.0` artifacts and release notes
- [ ] Run initial post-release triage window (issues/logs/hotfix readiness)
- [ ] Validate patch-release path if critical issues appear
- [x] Command-mode action UX polish: dynamic web-search action + URL launch path for action rows
- [x] Runtime query latency hardening: short-query app bias + adaptive candidate limits + indexed prefix cache reuse
- [x] Runtime/overlay tuning wiring: `idle_cache_trim_ms` and `active_memory_target_mb` now drive live icon-cache cleanup and cache budget behavior
- [x] Startup critical-path hardening: overlay/hotkey boot no longer blocked by incremental indexing; indexing now applies asynchronously with cache refresh on completion

Phase 4 execution notes (2026-02-23):
- [x] Search pipeline now emits stage-level query profile logs for slow queries (`query_profile`) to support real-machine performance diagnosis.
- [x] Search path now avoids unnecessary wide scans on first-character queries and reuses cached indexed seed candidates for incremental typing.
- [x] Empty-cache startup now shows launcher immediately and performs index refresh in background (`Indexing in background...` status until first async apply).
- [x] Query DSL now supports explicit extension filtering via `ext:` / `extension:` tokens in addition to existing filters.
- [x] Runtime query path now uses an in-session final-query result cache and adaptive indexed seed limits from observed indexed latency.
- [x] Status diagnostics now report both recent and historical query-latency summaries and symbol-only skip counters.
- [x] Config migration path now backs up older config files and rewrites updated template fields while preserving user values.
- [x] Top-hit confidence guard refined to prefer higher-confidence source/match combinations without breaking short-query app intent.
- [x] Overlay colors now follow Windows system theme (dark/light) at startup using theme-aware palettes.
- [ ] Run Windows field validation pass and capture startup/query timing evidence from production logs.

## Deferred Backlog (Non-Blocking)

- [ ] Runtime auto-update mechanism (intentionally deferred)
- [x] Additional UWP/AppX icon edge-case polish (shortcut AppsFolder token extraction + extra shell fallback)
