# Nex On-Demand Updater UX Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a user-triggered updater entry point in Nex without introducing any background updater service.

**Architecture:** Reuse the existing Windows PowerShell updater script instead of duplicating update logic in Rust. Add one built-in launcher action plus one tray menu entry that both invoke the same updater-launch helper, and ship only the updater script in the installed payload so the runtime can find it outside the repo.

**Tech Stack:** Rust, Windows tray/overlay runtime, PowerShell updater script, Inno Setup, cargo test

---

### Task 1: Add updater-launch helper

**Files:**
- Create: `apps/core/src/updater.rs`
- Modify: `apps/core/src/lib.rs`
- Test: `apps/core/src/updater.rs`

**Step 1:** Add a small updater module with:
- `UpdateChannel`
- `UpdateLaunchError`
- candidate path resolution for:
  - installed layout: `scripts/update-nex.ps1`
  - repo/dev layout: `scripts/windows/update-nex.ps1`
- Windows-only `launch_updater(UpdateChannel)` that spawns PowerShell with `-ExecutionPolicy Bypass`

**Step 2:** Add unit tests for resolver behavior.

**Step 3:** Run:
- `cargo test -p nex updater -- --nocapture`

### Task 2: Expose updater in built-in actions

**Files:**
- Modify: `apps/core/src/action_registry.rs`
- Modify: `apps/core/src/runtime.rs`
- Test: `apps/core/src/action_registry.rs`
- Test: `apps/core/src/runtime.rs`

**Step 1:** Add `ACTION_CHECK_UPDATES_ID` to built-in actions.

**Step 2:** Handle the action in runtime by calling the updater helper and logging the resolved script path.

**Step 3:** Add tests proving the action is searchable and surfaced in command mode.

**Step 4:** Run:
- `cargo test -p nex action_registry::tests runtime::tests -- --nocapture`

### Task 3: Add tray entry for updates

**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Modify: `apps/core/src/runtime.rs`

**Step 1:** Add a tray menu item `Check for Updates`.

**Step 2:** Send a dedicated overlay event back to runtime.

**Step 3:** Reuse the same updater helper used by the built-in action.

**Step 4:** Keep UX simple:
- no background checks
- no new config
- tray click just launches stable updater

### Task 4: Ship only the updater script in installs

**Files:**
- Modify: `scripts/windows/nex.iss`
- Modify: `docs/engineering/windows-operator-runbook.md`
- Modify: `docs/engineering/windows-runtime-validation-checklist.md`

**Step 1:** Install only `scripts/update-nex.ps1` from the packaged stage.

**Step 2:** Document:
- built-in `Check for Updates`
- tray `Check for Updates`
- no background auto-update service

**Step 3:** Add validation notes for Windows manual QA.

### Task 5: Validate and commit

**Files:**
- Modify if needed: touched files above

**Step 1:** Run full suite:
- `cargo test -p nex`

**Step 2:** Commit with a milestone message.

**Step 3:** Push `master`.
