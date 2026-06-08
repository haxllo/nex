# Nex — Roadmap

**Date:** 2026-06-08
**Drives from:** `.planning/REQUIREMENTS.md`

## Phase Structure

Each phase is a self-contained unit of work with its own plan, tests, and
verification. Phases are sequential — each depends on the previous phase
being complete and stable.

---

## Phase 1: Iced Migration — Tray & Hotkey Platform Glue

**Status:** Planned
**Branch:** `iced-ui`
**Plans:** 1 plan

Complete the Win32 platform glue that Iced cannot provide:
- ~~Hotkey registration on dedicated thread~~ ✅ (already implemented in `overlay/hotkey.rs`)
- System tray icon with context menu (pure Win32, unchanged)
- ~~Instance signaling~~ ✅ (already implemented in `overlay/platform.rs`)
- ~~System theme detection via registry~~ ✅ (already implemented in `overlay/platform.rs`)

**Requires:** R1.4 (R1.3 and R1.5 already complete)
**Verification:** Smoke test, tray menu functional, hotkey fires overlay

Plans:
- [ ] 01-01-PLAN.md — System tray icon implementation (TrayIcon module + runtime wiring)

---

## Phase 2: Iced Migration — Icons & Assets

**Status:** COMPLETE
**Branch:** `iced-ui`

Complete the icon pipeline:
- ✅ LRU icon cache using `image` crate (ICO/PNG → `iced::widget::Image`)
- ✅ Port all public methods from legacy `icon_cache.rs`
- ✅ IconCache wired into view via Arc chain (Boot → State → ViewFn)
- ✅ Result-row icons rendered via `result_icon()` with fallback placeholder
- Action icon classification, embedded PNG rendering (deferred to future phase)

**Requires:** R1.1 (icon rendering), R3.2 (LRU eviction)
**Verification:** `cargo check -p nex-cli` passes, all icon Infrastructure wired

---

## Phase 3: Iced Migration — View & Animation

**Status:** Partially Implemented
**Branch:** `iced-ui`

Complete the widget tree and animations:
- Search input with placeholder, search icon, command prefix
- Result rows: icon, title, path, selection highlight
- Section headers with separator lines
- Footer hint with keyboard cheatsheet
- Help tooltip
- Mode strip
- Show/hide alpha fade animation (150ms, ease-out)
- Height expansion animation (110ms)
- Loading spinner

**Requires:** R1.1, R1.2, R5.1, R5.4
**Verification:** Visual comparison with legacy overlay, animation timing

---

## Phase 4: Iced Migration — Shim & Wire-Up

**Status:** Partially Implemented
**Branch:** `iced-ui`

Complete the `NativeOverlayShell` shim:
- All 22 public methods wired to Iced Model via Arc<Mutex<>>
- `run_message_loop_with_events` spawns Iced on main thread
- Search results delivery via polling subscription
- Indexing progress window using Iced ProgressBar

**Requires:** R1.6, R1.7
**Verification:** `cargo test -p nex-cli` green, manual smoke test pass

---

## Phase 5: Iced Migration — Legacy Removal & Cleanup

**Branch:** `iced-ui`

Delete legacy code and verify nothing breaks:
- Delete `apps/core/src/windows_overlay/` (all 15 files)
- Remove unused `windows-sys` features
- Remove unused dependencies from Cargo.toml
- Port remaining unit tests from legacy to overlay module
- Update AGENTS.md and docs

**Requires:** R1.8
**Verification:** Full CI gate (build + test + smoke + perf)

---

## Phase 6: Stability — Crash Resilience & Bug Fixes

**Status:** Planned
**Branch:** TBD (from `iced-ui` or `main` after merge)
**Plans:** 3 plans

Systematic hardening:
- Add error recovery in search worker (catch Tantivy panics)
- Add error recovery in icon loading (corrupt files)
- Hotkey thread crash detection and auto-restart
- Config parse error UX improvement
- Fix rendering edge cases (DPI scaling, mixed monitors)
- Fix hotkey conflict detection messaging

**Requires:** R2.1, R2.2, R2.3, R2.4
**Verification:** Targeted tests for each recovery path, manual edge-case testing

Plans:
- [ ] 06-01-PLAN.md — Crash resilience: search worker, icon decode, hotkey thread recovery
- [ ] 06-02-PLAN.md — Config error UX, multi-monitor positioning, DPI fixes
- [ ] 06-03-PLAN.md — Hotkey conflict detection, atomic config save with backups

---

## Phase 7: Performance — Indexing & Memory

**Branch:** TBD

Performance optimization:
- Profile and optimize initial index build time
- Profile and optimize incremental update latency
- Memory profiling: identify and fix leaks/bloat
- Icon cache tuning (size limits, eviction policy)
- Tantivy index compaction strategy
- Reduce idle CPU usage

**Requires:** R3.1, R3.2
**Verification:** Perf gate (p95 under 15ms), memory under targets

---

## Phase 8: Performance — Search Quality

**Branch:** TBD

Search ranking improvements:
- Tune fuzzy matching algorithm (prefix > substring > fuzzy)
- Boost app matches over file matches
- Typo tolerance for queries ≥ 3 characters
- Result diversity (don't show 8 variants of the same app)

**Requires:** R3.3
**Verification:** Manual search quality evaluation, automated ranking tests

---

## Phase 9: UI Polish — Mica, Corners, Typography

**Branch:** TBD

Visual polish for the Iced overlay:
- Enable Mica backdrop on Windows 11 (DWMWA_SYSTEMBACKDROP_TYPE)
- Enable DWM rounded corners on Windows 11
- Font consolidation (Segoe UI Variable on Win11, Segoe UI on Win10)
- Theme consistency audit (dark and light)
- Footer keycap-style rendering
- Selection highlight refinement

**Requires:** R5.2, R5.3, R5.4, R5.1
**Verification:** Visual review on Windows 10 and Windows 11

---

## Phase 10: Snippets / Text Expansion

**Branch:** TBD

Build the snippets system:
- Snippet storage format and file
- Trigger keyword matching and expansion
- Keystroke simulation (SendInput)
- Dynamic snippets (date, time, clipboard)
- Snippet UX in overlay (icon, preview, trigger hint)

**Requires:** R4.1, R4.2, R4.3, R4.4
**Verification:** End-to-end snippet trigger → expansion workflow

---

## Phase 11: Testing — Coverage & CI Hardening

**Branch:** TBD

Expand test coverage:
- Model update logic unit tests
- Theme palette consistency tests
- Config migration roundtrip tests
- Search ranking correctness tests
- CI stability improvements (flaky test fixes)

**Requires:** R6.1, R6.2, R6.3
**Verification:** Coverage report, CI consistently green

---

## Dependency Graph

```
Phase 1 (Platform) ──┐
Phase 2 (Icons)    ──┤
Phase 3 (View)     ──┤──► Phase 4 (Shim) ──► Phase 5 (Legacy Removal)
                      │
                      └──► Phase 6 (Stability) ──► Phase 7 (Perf: Index) ──► Phase 8 (Perf: Search)
                                                            │
                                                            └──► Phase 9 (UI Polish) ──► Phase 10 (Snippets)
                                                                                              │
                                                                                              └──► Phase 11 (Testing)
```

Phases 6-11 all depend on Phase 5 (Iced migration complete). Phases 6-8 can
run in parallel with 9-10 once the foundation is stable.
