# Spec: Apps-First Result Ordering + Row-Only Non-App Rendering

Branch: `feat/apps-first-ordering` (from master, after merge of `feat/result-sorting`)
Status: approved, not implemented
Date: 2026-08-04

## Requirement (user words)

When typing a query, results must ALWAYS show apps on top, then files/folders,
then actions — no matter which view (`grid_view`) is selected in config.
Files, folders, and actions must ALWAYS render as rows (inline flex), never as
grid cells. Grid view may only ever apply to apps.

## Part A — Apps-top ordering (kind beats tier)  [SPRINT 1]

| # | Part | Files |
|---|------|-------|
| A1 | Reorder `overlay_rows()`: apps group first, then folders, then files, then actions, then clipboard/other. Match tier sorts WITHIN a kind group only. **Supersedes FIX1 tier-beats-kind from feat/result-sorting.** | `apps/core/src/runtime_overlay_rows.rs` |
| A2 | TopHit (row 0) = best-scored app when any app matches; NOT raw `results[0]`. | `runtime_overlay_rows.rs`, possibly `search.rs` |
| A3 | **DECIDED: yes** — when query yields no apps (e.g. `@files` mode, file search), TopHit = best-scored file/folder. | same |
| A4 | command_mode ordering aligned (actions sorted, not raw pass-through). | `runtime_overlay_rows.rs` |
| A5 | Verify files/folders always render before actions in every combo (folded into A1). | verify |

Current gaps (verified, file:line on feat/result-sorting):
- `runtime_overlay_rows.rs:52-58` row 0 = raw `results[0]`, any kind.
- `runtime_overlay_rows.rs:73,110-122` tier beats kind: exact folder bucket renders above fuzzy app.
- `search.rs:405-420` action source_rank=1 — strong action can outscore app.
- `runtime_overlay_rows.rs:44-50` command_mode skips regroup.

## Part B — Grid only for apps, non-apps always rows  [SPRINT 2]

| # | Part | Files |
|---|------|-------|
| B1 | `grid-view` class applies only to app rows; files/folders/actions always inline-flex rows. | `assets/app.js`, `assets/style.css` |
| B2 | View setting ignored for non-app kinds in every combo. | same |
| B3 | Reconcile "Search Web for*" single-row exemption with new rule. | same |

Current gaps:
- `assets/app.js:732-733` toggles `grid-view` on whole `#list`.
- `assets/style.css:163-167,193-200` grid layout applies to ALL `.row` cells including files/folders/actions.
- Only "Search Web for*" actions exempt via `single-row` (app.js:166).

## Part C — Combo verification gate [GATE]

Verify ordering apps > files/folders > actions holds (manual, `cargo test` broken on Windows):
- only apps (show_files=F, show_folders=F)
- files + apps (T,F)
- folders + apps (F,T)
- all true (T,T)
- `@files` mode (no apps → A3 fallback top hit)
- command_mode
- grid_view ON and OFF — non-app rows identical rows either way

### Grid-view rendering checklist (Sprint 2)

| Combo | grid_view | Expected |
|-------|-----------|----------|
| only apps | ON | Apps render as grid cells (column layout, 36px icons, centered text) |
| only apps | OFF | Apps render as standard rows (inline-flex, 24px icons, left text) |
| files + apps | ON | Apps = grid cells; files = full-width rows (span all columns, row layout) |
| files + apps | OFF | All rows = standard inline-flex rows |
| folders + apps | ON | Apps = grid cells; folders = full-width rows |
| folders + apps | OFF | All rows = standard inline-flex rows |
| all true | ON | Apps = grid cells; files, folders, actions = full-width rows |
| all true | OFF | All rows = standard inline-flex rows |
| Search Web for* | ON | Action row = full-width row (row-list), not grid cell |
| Search Web for* | OFF | Action row = standard row (same as always) |
| calculator | ON | Calculator row spans full width, 64px height, 21px title (unchanged) |

## Part D — Polish [LAST]

- D1: update tests asserting old tier/order expectations.
- D2: README feature line re ordering (apps top) if needed.

## Constraints

- Don't rename SwiftFind legacy names. No config/JSON template changes.
- Build check: `cargo build --bin Nex` passes, no new warnings beyond ~38 dead-code.
- Don't re-run full test suite (hangs on Windows).