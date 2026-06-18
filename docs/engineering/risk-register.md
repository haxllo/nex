# Risk Register

> **STALE — last meaningful update pre-restart (v6.x)**: This risk register reflects the v6.x-era threat model. Several risks listed (e.g., `R-001` file-watcher event loss, `R-002` UI framework overhead) are either resolved (file_watcher not wired; UI is native Win32, not framework-based) or no longer applicable. If a v1.x risk register is needed, create a new doc `docs/engineering/risk-register-v1.md` rather than updating this one.
>
> Current v1.3.0 known risks are scattered across [`../plans/audit-and-roadmap.md`](../plans/audit-and-roadmap.md) and the inline risk notes in [`../plans/everything-first-migration.md`](../plans/everything-first-migration.md).

## `R-001` Index Size Growth Hurts Latency

- Impact: query slowdown, poor UX
- Likelihood: medium
- Mitigation:
- Keep hot index in memory with bounded metadata
- Segment large path roots and prioritize recent paths
- Add perf tests at medium and large dataset sizes

## `R-002` File Watcher Event Loss

- Impact: stale or missing results
- Likelihood: medium
- Mitigation:
- Periodic reconciliation scan
- Track watcher health metrics
- Manual rebuild index command

## `R-003` Hotkey Conflicts

- Impact: launcher not opening for some users
- Likelihood: high
- Mitigation:
- Detect registration failure and show fallback suggestions
- Offer first-run hotkey setup

## `R-004` Unsafe Launch Semantics

- Impact: security incidents or broken launches
- Likelihood: low to medium
- Mitigation:
- Strict target validation and action allowlist
- Security-focused tests for command injection paths

## `R-005` UI Framework Overhead

- Impact: memory budget misses
- Likelihood: medium
- Mitigation:
- Keep overlay icon/image caches bounded and aggressively trim on hide
- Keep ranking and indexing in the core runtime hot path
- Profile active and idle memory envelopes continuously
