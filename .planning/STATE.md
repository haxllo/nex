# Nex — Project State

**Last Updated:** 2026-06-08
**Current Phase:** Phase 8 (Perf: Search Quality) — **Not Started**

**Active Branch:** `iced-ui`
## Current Status

The Iced 0.14 UI migration (Phases 1-5) is complete. Stability hardening (Phase 6) is complete. Performance: Indexing & Memory (Phase 7) is complete.

### Overlay Module (complete)

| File | Purpose |
|------|---------|
| `boot.rs` | Iced application boot, State, visibility control |
| `view.rs` | Widget tree (search icon, input, rows with icons, keycap footer, mode strip, tooltip) |
| `model.rs` | Model, Message, update(), OverlayEvent |
| `theme.rs` | Dark/light palettes |
| `geometry.rs` | Layout constants |
| `icons.rs` | LRU icon cache (Arc-threaded) |
| `platform.rs` | Win32 glue (hotkey, instance signal, theme detection) |
| `hotkey.rs` | RegisterHotKey on dedicated thread |
| `tray.rs` | System tray icon with context menu |
| `shim.rs` | NativeOverlayShell (27 methods) |
| `indexing_progress.rs` | Stub (full ProgressBar deferred) |

## Phase Progress

| Phase | Status |
|-------|--------|
| 1 — Platform Glue | COMPLETE |
| 2 — Icons | COMPLETE |
| 3 — View & Animation | COMPLETE |
| 4 — Shim & Wire-Up | COMPLETE |
| 5 — Legacy Removal | COMPLETE |
| 6 — Stability | COMPLETE |
| 7 — Perf: Indexing & Memory | COMPLETE |
| 8 — Perf: Search Quality | Not Started |
| 8 — Perf: Search Quality | Planned |
| 9 — UI Polish | Not Started |
| 10 — Snippets | Not Started |
| 11 — Testing | Not Started |

## Key Decisions

- **2026-06-08 (Phase 2):** IconCache threaded as `Arc` through Boot → State → ViewFn rather than embedding in Model.
- **2026-06-08:** Confirmed Iced 0.14 as the UI framework. Rejected Slint after evaluation.
- **2026-06-04:** Iced migration plan created (`.planning/plans/iced-migration.md`).

## Next Actions

1. ~~Complete Phase 1-5 — Iced migration~~ ✅
2. ~~Begin stability hardening (Phase 6)~~ ✅
3. ~~Execute Phase 7 plans~~ ✅
4. **Plan Phase 8** — `/gsd-plan-phase 8` (can run in parallel with future phases)
5. Plan Phase 9 (UI Polish)
6. Plan Phase 10 (Snippets)
7. Plan Phase 11 (Testing)
