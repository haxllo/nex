# Nex Hotkey Conflict Recovery Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Keep Nex usable when the configured hotkey cannot register, while defaulting new installs to app-first mode with files and folders hidden.

**Architecture:** Treat hotkey registration failure as a recoverable runtime condition instead of a fatal startup error. Keep the tray alive, expose a direct config-open path, show a persistent warning when the overlay is opened via tray, and log structured recovery guidance. Change `show_files` / `show_folders` defaults at the config layer so new installs come up app-first without altering explicit user choices.

**Tech Stack:** Rust, Windows tray/overlay runtime, config templates/migration, cargo test

---

### Task 1: Flip config defaults and update tests

**Files:**
- Modify: `apps/core/src/config.rs`
- Modify: `apps/core/tests/config_test.rs`

**Step 1: Write/update failing tests**

- Expect `Config::default()` to set:
  - `show_files = false`
  - `show_folders = false`
- Expect generated templates to write `false` for both fields.
- Expect migration of missing keys to fall back to the new defaults.

**Step 2: Run targeted tests**

Run: `cargo test -p nex config_test`

Expected: FAIL on the old `true` expectations.

**Step 3: Implement minimal config changes**

- Change default values.
- Keep explicit user values untouched.

**Step 4: Run targeted tests**

Run: `cargo test -p nex config_test`

Expected: PASS

### Task 2: Add hotkey recovery helpers and tests

**Files:**
- Modify: `apps/core/src/settings.rs`
- Modify: `apps/core/src/runtime.rs`
- Modify: `apps/core/tests/settings_test.rs`

**Step 1: Write failing tests**

- Add tests for safe fallback preset selection.
- Add tests for hotkey recovery message formatting.

**Step 2: Run targeted tests**

Run: `cargo test -p nex settings_test hotkey_recovery`

Expected: FAIL because helper functions do not exist yet.

**Step 3: Implement helpers**

- Return up to 3 safe preset suggestions excluding the current hotkey.
- Build a user-facing recovery message with:
  - failing hotkey
  - suggested presets
  - config path

**Step 4: Run targeted tests**

Run: `cargo test -p nex settings_test hotkey_recovery`

Expected: PASS

### Task 3: Make hotkey registration failure recoverable

**Files:**
- Modify: `apps/core/src/runtime.rs`
- Modify: `apps/core/src/windows_overlay.rs`

**Step 1: Implement runtime recovery**

- Do not abort startup when `register_hotkey` fails.
- Log structured `hotkey_registration_issue` guidance.
- Keep tray runtime alive.
- Show a status warning when the overlay is opened without a working hotkey.

**Step 2: Implement tray support**

- Add `Open Config` to the tray menu.
- Add tray tooltip state for hotkey-unavailable mode.

**Step 3: Run targeted tests**

Run: `cargo test -p nex status_diagnostics`

Expected: PASS with the new issue line parsing.

### Task 4: Validate and document

**Files:**
- Modify: `docs/engineering/windows-runtime-validation-checklist.md`

**Step 1: Update validation notes**

- Add expectations for tray-only recovery when the hotkey is taken.
- Add status/log lines to inspect:
  - `hotkey_registration_issue`

**Step 2: Run full suite**

Run: `cargo test -p nex`

Expected: PASS

