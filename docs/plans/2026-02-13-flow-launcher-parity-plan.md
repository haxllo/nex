# Nex Flow-Launcher Parity Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Elevate Nex from a working MVP launcher to a polished, reliable Windows launcher with crisp rendering, strong defaults, and production-grade install/update behavior.

**Architecture:** Keep the current Rust-native overlay/runtime architecture. Improve quality by hardening rendering (DPI/icon/text), search/ranking behavior, UX interactions, and installer lifecycle. Roll out in small, testable increments with strict pass gates and Windows manual validation at each phase.

**Tech Stack:** Rust (`windows-sys`, `rusqlite`), native Win32 overlay, Inno Setup packaging, PowerShell scripts, Vitest + Rust tests.

---

## Phase 1: Visual Fidelity and Rendering Quality

### Task 1: Lock visual token system
**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Modify: `docs/engineering/windows-runtime-behavior.md`

**Steps:**
1. Move all panel/list/text/hover colors to explicit constants.
2. Add comments for COLORREF/BGR format to avoid wrong hex usage.
3. Keep requested base palette:
   - Background: `#272727`
   - Border: `#424242`
   - Hover: `#313131`
4. Ensure idle and expanded panel use same base background.
5. Verify no hardcoded color literals remain in paint paths.

**Done when:** no visual regressions and token-only styling in rendering code.

### Task 2: DPI and icon sharpness pass
**Files:**
- Modify: `apps/core/src/runtime.rs`
- Modify: `apps/core/src/windows_overlay.rs`
- Modify: `apps/core/Cargo.toml`

**Steps:**
1. Keep per-monitor DPI awareness enabled early.
2. Request larger shell icons and draw downscaled with stable metrics.
3. Re-check text metrics and row/icon alignment at 100%, 125%, 150%.
4. Verify no bitmap-scaled blur on common monitors.

**Done when:** text and icons are crisp on multiple scaling levels.

### Task 3: Font quality + fallback confidence
**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Test: `apps/core/src/windows_overlay.rs` (unit tests)

**Steps:**
1. Keep explicit font weights by surface (input/title/meta/status).
2. Resolve family through: env override -> Geist (if loaded) -> default.
3. Add tests for font family resolution behavior.
4. Add runtime status log entry indicating selected font family.

**Done when:** font path is deterministic and test-covered.

---

## Phase 2: Interaction and Search UX Parity

### Task 4: Result quality and density tuning
**Files:**
- Modify: `apps/core/src/runtime.rs`
- Modify: `apps/core/src/search.rs`
- Modify: `apps/core/src/windows_overlay.rs`
- Test: `apps/core/tests/search_test.rs`

**Steps:**
1. Keep ranking priority: apps > local files > others.
2. Trim subtitles/path display for readability and stability.
3. Ensure 5 visible rows with smooth expand/collapse.
4. Remove micro-jitter in first-type and hover transitions.

**Done when:** stable first keystroke behavior and predictable top results.

### Task 5: Mouse/keyboard parity hardening
**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Test: `apps/core/src/runtime.rs` tests

**Steps:**
1. Synchronize hover and keyboard selection without flicker.
2. Ensure single-click launch is deterministic.
3. Keep `Enter`, `Esc`, Up/Down behavior unchanged and fast.
4. Confirm overlay closes correctly on outside-click and post-launch.

**Done when:** zero stuck-focus or hover-flicker reports.

---

## Phase 3: Packaging, Install, and Uninstall Reliability

### Task 6: Installer lifecycle hardening
**Files:**
- Modify: `scripts/windows/nex.iss`
- Modify: `scripts/windows/package-windows-installer.ps1`
- Modify: `docs/engineering/windows-packaging-readiness.md`

**Steps:**
1. Keep uninstall logic that terminates runtime + clears startup key.
2. Add `RunOnceId` for `[UninstallRun]` entries to remove warnings.
3. Ensure icon path resolution is absolute and validated.
4. Keep deterministic output paths in `artifacts/windows`.

**Done when:** uninstall leaves no running process and no startup residue.

### Task 7: Update and rollback validation checklist
**Files:**
- Modify: `docs/engineering/windows-operator-runbook.md`
- Modify: `docs/engineering/windows-runtime-validation-checklist.md`

**Steps:**
1. Add clean install checklist.
2. Add upgrade-over-existing checklist.
3. Add uninstall + reinstall checklist.
4. Add rollback path from bad release artifact.

**Done when:** release QA can be repeated by any operator.

---

## Required Pass Gates (every phase)
- `cargo check -p nex`
- `cargo test -p nex`
- `pnpm vitest --run`
- `cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms`

## Required Windows Manual Validation (every phase)
- `cargo run -p nex`
- Verify crisp text/icons at 100% / 125% / 150% scaling
- Verify hotkey opens compact bar and results expand downward
- Verify launch + close behavior with keyboard and mouse
- Verify Task Manager process behavior and uninstall cleanup

## Commit Strategy
- One commit per task.
- No unrelated file edits.
- Every commit references the exact task in commit message.

## Success Criteria
- No blur complaints across typical Windows display settings.
- No process-residue complaints after uninstall.
- No first-keystroke or hover-jitter regressions.
- Release can be installed and used by non-technical users without scripts.
