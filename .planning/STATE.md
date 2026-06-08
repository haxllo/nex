# Nex — Project State

**Last Updated:** 2026-06-08
**Current Phase:** 2 (Iced Migration — Icons & Assets) **— COMPLETE**
**Active Branch:** `iced-ui`

## Current Status

The Iced 0.14 UI migration is in progress. The `apps/core/src/overlay/` module
exists with substantial implementation:

- `boot.rs` — Iced application boot, State management, visibility control
- `view.rs` — Widget tree (input, rows, footer)
- `model.rs` — Elm-style Model, Message, update()
- `theme.rs` — Dark/light palettes
- `geometry.rs` — Layout constants
- `icons.rs` — LRU icon cache
- `platform.rs` — Win32 glue (hotkey, tray)
- `shim.rs` — NativeOverlayShell imperative API
- `indexing_progress.rs` — First-run progress window

The legacy `windows_overlay/` module (~8,200 lines) is still active and used
by the current `main` branch.

## Phase Progress

| Phase | Status | Branch |
|-------|--------|--------|
| 1 — Platform Glue | COMPLETE | `iced-ui` |
| 2 — Icons | COMPLETE | `iced-ui` |
| 3 — View & Animation | In Progress | `iced-ui` |
| 4 — Shim & Wire-Up | In Progress | `iced-ui` |
| 5 — Legacy Removal | Not Started | — |
| 6 — Stability | Not Started | — |
| 7 — Perf: Indexing & Memory | Not Started | — |
| 8 — Perf: Search Quality | Not Started | — |
| 9 — UI Polish | Not Started | — |
| 10 — Snippets | Not Started | — |
| 11 — Testing | Not Started | — |

## Key Decisions

- **2026-06-08 (Phase 2):** IconCache threaded as `Arc` through Boot → State →
  ViewFn rather than embedding in Model, keeping Model `Clone`-friendly.
- **2026-06-08:** Confirmed Iced 0.14 as the UI framework. Rejected Slint after
  evaluation (see `.planning/research/ui-framework-evaluation.md`).
- **2026-06-04:** Iced migration plan created (`.planning/plans/iced-migration.md`).
- **Prior:** Slint, Egui, Tauri all rejected in favor of Iced.

## Active Risks

| Risk | Mitigation |
|------|------------|
| winit 0.30 event loop conflicts with hotkey thread | Run hotkey on dedicated thread; bridge via crossbeam |
| Iced widget styling doesn't match legacy pixel-for-pixel | Iterative visual comparison; keep shim for fallback |
| WGSL fails on Intel iGPUs | Test on minimum spec (i5-8250U, Intel UHD 620) before release |
| crossbeam channel overhead for Iced ↔ runtime sync | Fall back to Arc<Mutex<Model>> + Subscription::batch |

## Next Actions

1. ~~Complete Phase 1 — ensure hotkey + tray + instance signaling work end-to-end~~ ✅
2. ~~Complete Phase 2 — verify all icon types render correctly in Iced~~ ✅
3. Complete Phase 3 — finish widget tree and animation
4. Complete Phase 4 — wire up shim, run full test suite
5. Delete legacy code (Phase 5)
6. Begin stability hardening (Phase 6)

Run `/gsd-plan-phase 3` to create a detailed plan for Phase 3.
