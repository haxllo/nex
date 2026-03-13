# Nex V1 Robustness Roadmap

Last updated: 2026-02-23

## Purpose

Define the post-`v1.0.0` execution roadmap to improve runtime reliability, installer safety, search quality, and release discipline without regressing performance.

This roadmap is Windows-first and keeps `config.toml` as the source of truth.

## Baseline (Starting Point)

Current state from shipped work:
- Stable launcher UX contract implemented (compact idle, downward expansion, keyboard/mouse launch flows).
- Core search/index pipeline and ranking are production-usable.
- Installer packaging flow exists (`zip` + `setup.exe`) and release operations are documented.
- Known deferred items remain:
  - runtime auto-update mechanism
  - deeper UWP/AppX icon edge-case polish

## Non-Negotiable Constraints

- No startup responsiveness regression.
- No always-on secondary UI process.
- No default telemetry/query exfiltration.
- Preserve current hotkey/search/launch behavior.
- Pass existing test/perf gates on every milestone.

## Release Train

### v1.0.1 (Hotfix and Runtime Safety)

Goal:
- Eliminate upgrade/install friction and add stronger runtime recovery diagnostics.

Must ship:
- Installer lock-contention hardening:
  - deterministic runtime stop before overwrite
  - clear recovery messaging for file-lock paths
  - retry-friendly upgrade behavior
- Runtime self-heal guard:
  - detect stale/ghost runtime state
  - reliable `--status`, `--quit`, `--restart` flow
- Diagnostics bundle command:
  - export logs + sanitized config snapshot + runtime summary for support triage

Nice-to-have:
- Better installer error copy for `DeleteFile`/access-denied scenarios.
- Faster recovery after failed upgrade attempt.

Acceptance criteria:
- Upgrade over running instance succeeds without manual folder cleanup.
- No ghost `nex.exe` process remains after uninstall.
- `--status` reflects real process state after quit/restart flows.

Task breakdown (implementation):
1. Installer lifecycle lock hardening:
   - pre-install runtime stop hook in setup flow
   - explicit kill fallback to avoid overwrite lock hangs
2. Runtime self-heal lifecycle commands:
   - improve `--status` to report degraded process-without-window state
   - harden `--quit` and `--restart` with graceful stop + forced fallback path
3. Diagnostics bundle export:
   - add `--diagnostics-bundle` command
   - write bundle with summary, sanitized config snapshot, and recent logs

Current status:
- [x] Task 1 implemented
- [x] Task 2 implemented
- [x] Task 3 implemented

### v1.1.0 (Indexing and Relevance Hardening)

Goal:
- Improve relevance consistency and reduce indexing overhead in normal runtime.

Must ship:
- Incremental indexing path:
  - changed/new/deleted item processing without full rebuild each run
- Ranking v2:
  - explicit weighted scoring for exact/prefix/fuzzy + source priority + recency/frequency
- Stale target hygiene:
  - faster prune after failed launches
  - reduced stale result recurrence

Nice-to-have:
- Provider diagnostics summary in runtime status output.
- Safer tie-break policy tuned for deterministic cross-machine ordering.

Acceptance criteria:
- Query relevance improves on mixed app/file queries.
- Warm-query latency gate remains green.
- No startup penalty compared to v1.0.0 baseline.

Task breakdown (implementation):
1. Incremental indexing hardening:
   - add provider change-stamp support
   - skip unchanged provider scans with periodic reconcile guard
   - persist provider incremental metadata in SQLite
2. Ranking v2 formalization:
   - explicit weighted tiers for exact/prefix/substring/fuzzy match quality
   - explicit source-priority + recency/frequency bonuses
   - deterministic tie-break ordering independent of input order
3. Stale launch target hygiene:
   - carry structured launch failure code from shell launch path
   - prune entries immediately on known missing-target failures (`2` / `3`)

Current status:
- [x] Task 1 implemented
- [x] Task 2 implemented
- [x] Task 3 implemented

### v1.2.0 (Update and Operations Maturity)

Goal:
- Reduce manual maintenance cost while keeping rollback-safe release control.

Must ship:
- Channel-aware update strategy implementation:
  - `stable` and `beta` update paths
  - rollback-safe artifact handling
- Update integrity checks:
  - verify downloaded artifact/manifest before apply
- Recovery path:
  - fallback to previous known-good version on failed update apply

Nice-to-have:
- Background update check policy with explicit user control.
- In-app notification for new version availability.

Acceptance criteria:
- Update path works from prior stable to current stable.
- Rollback path is validated in operator checklist.
- No data-loss for `%APPDATA%\Nex` config/index/logs.

Task breakdown (implementation):
1. Channel-aware updater flow:
   - add Windows update script with `stable`/`beta` channel selection
   - resolve release assets by channel and version
2. Integrity checks before apply:
   - ship manifest with artifact checksums
   - verify downloaded installer checksum against manifest before install
3. Rollback-safe apply:
   - snapshot current install directory before update
   - restore previous snapshot automatically if installer apply/verify fails

Current status:
- [x] Task 1 implemented
- [x] Task 2 implemented
- [x] Task 3 implemented

### v1.3.0 (Quality Expansion, No Performance Regressions)

Goal:
- Add targeted quality features while preserving low-overhead runtime behavior.

Candidate scope:
- Additional UWP/AppX icon fallback polish.
- Accessibility pass (high-contrast and larger text mode).
- Optional command-mode actions (`>`-prefixed control actions) if no runtime overhead.

Acceptance criteria:
- Feature flags default-safe for existing users.
- No noticeable idle memory/runtime responsiveness regression.
- UI interaction contracts remain unchanged unless explicitly versioned.

Current status:
- [x] Additional UWP/AppX icon edge-case fallback polish implemented (improved AppsFolder token extraction and shell fallback chain).
- [x] Command-mode UX polish implemented with dynamic web-search action (`>` query) and URL launch support for action rows.
- [ ] Accessibility pass (high-contrast and larger text mode).

## Feature Option Backlog (Prioritized)

Priority A (robustness first):
- Config/index integrity check and auto-repair on startup.
- Runtime watchdog and safe restart guard.
- Error bundle export for support diagnostics.

Priority B (quality and parity):
- Local learned ranking boost from successful launches.
- Query intent routing (`apps_first`, `files_first`, `balanced` profiles).
- Expanded icon fidelity for remaining edge shortcuts/UWP entries.

Priority C (power-user controls):
- `clear_query_on_hide`
- `max_visible_rows`
- `ranking_profile`
- `diagnostics_level`

## Post-v1.0.x Runtime Hardening Snapshot (2026-02-23)

Completed in current pass:
- [x] Adaptive query candidate limits by query length/mode to reduce broad-query compute spikes.
- [x] Indexed prefix-cache reuse for incremental typing (`v` -> `vi` -> `viv`) to reduce repeated full index scoring.
- [x] Stage-level slow-query profiling (`query_profile`) expanded with candidate/seed/cache-hit dimensions.
- [x] Startup critical-path hardening: runtime now initializes overlay/hotkey first and performs incremental indexing asynchronously.
- [x] Config-to-runtime tuning hookup for overlay cache behavior:
  - `idle_cache_trim_ms` now controls icon-cache idle cleanup timer.
  - `active_memory_target_mb` now controls icon-cache budget envelope.
- [x] Query DSL extension operator support:
  - `ext:` / `extension:` now filters results by file extension (for example: `ext:txt`, `@files extension:md`).
- [x] Runtime query execution hardening:
  - in-session final-query result cache avoids repeat compute for identical query+filter requests.
  - indexed seed limit now auto-tunes down/up based on recent indexed-latency telemetry.
- [x] Diagnostics/status depth increased:
  - status now reports recent vs historical query-latency summaries.
  - status now reports symbol-only query skip counters from `query_guard` logs.
- [x] Config migration/update resilience:
  - older config files are backed up and rewritten to the latest template while preserving user-set values.
  - legacy performance defaults are migrated to current tuned defaults when detected.
- [x] Overlay theme parity improvement:
  - launcher palette now follows Windows system dark/light app theme.

Next recommended tasks:
1. Startup responsiveness hardening:
   - completed in current pass; next step is validating cold-start UX and indexing-complete transition timing on real Windows installs.
2. Field performance validation:
   - capture and compare `query_profile` p95 timings on large real-user indexes.
3. Memory profile validation:
   - verify active/idle RAM behavior across low/high `active_memory_target_mb` values on Windows.

## Required Gates (Every Milestone)

```powershell
cargo check -p nex
cargo test -p nex
pnpm vitest --run
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
```

Manual Windows checks:
- hotkey open/toggle/focus behavior
- query + arrow + enter launch flow
- single-click launch flow
- click-outside hide behavior
- install, upgrade, uninstall, rollback
- process stability in Task Manager

## Risk Register (Execution Focus)

- Installer race and file lock risk:
  - mitigation: pre-install stop hooks + force-kill fallback + retry guidance
- Ranking regressions from new weights:
  - mitigation: deterministic tests + golden query fixtures
- Incremental indexing drift:
  - mitigation: periodic reconcile scan + stale-prune safety checks
- Update mechanism blast radius:
  - mitigation: staged channel rollout + rollback-first design

## Documentation and Operations Policy

For each milestone:
- Update runbook and validation checklist in same change window.
- Record manual validation evidence before release publish.
- List any non-blocking limitation in release notes with workaround and target fix milestone.

## Execution Order Recommendation

1. `v1.0.1` hotfix/reliability hardening.
2. `v1.1.0` incremental indexing + ranking v2.
3. `v1.2.0` updater + rollback infrastructure.
4. `v1.3.0` optional quality expansion.
