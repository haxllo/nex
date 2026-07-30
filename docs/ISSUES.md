# Open Issues — Branch: `feat/win-key-hotkey`

## Issue 1: Non-Win hotkey shows but does not hide — FIXED

**Scope:** Any hotkey that is NOT the Win key alone (e.g. `Ctrl+Space`, `Ctrl+Shift+F`, `Alt+Space`).

**Symptom (original):**
- First hotkey press: overlay shows correctly
- Second hotkey press: overlay does NOT hide — hotkey appears to do nothing, or shows again

### Root cause

Chromium (WebView2) installs its own `WH_KEYBOARD_LL` hook when the WebView gets focus. Since hooks fire in LIFO order (last installed = first called), Chromium's hook runs before ours and consumes the keyboard events. Our hook never fires for the second (or any subsequent) press while the overlay is visible and focused.

Diagnostic confirmed: `Hook: ALL` lines appear before overlay is shown but go completely silent once `window.set_visible(true)` + `focus_input()` run.

### Fix

Dual detection path:

1. **Overlay hidden** (first press): `WH_KEYBOARD_LL` hook detects the hotkey (unchanged).
2. **Overlay visible** (subsequent presses): `WM_INPUT` via `RIDEV_INPUTSINK` detects the hotkey instead — raw input bypasses Chromium's hook entirely.

Changes:
- `register_raw_input_sink(hwnd, suppress_win)` always registers `RIDEV_INPUTSINK` for keyboard raw input. Adds `RIDEV_NOHOTKEYS` only for Win-hotkey configs. Called in Painted handler for EVERY show.
- `unregister_raw_input_sink()` removes the registration on hide (replaces `unregister_hotkey_suppression`).
- `check_raw_input_hotkey(vk)` in `hotkey.rs` reads `HOOK_CTX` to check VK + required/extra modifiers via `GetAsyncKeyState`.
- WM_INPUT handler in `instance_signal_subclass` extended: after Win-key check, also calls `check_raw_input_hotkey()` for non-Win keys.
- `show_pending = true` moved to top of warm-start Show handler to prevent spurious Focused(false) → Escape race.

Additionally, debug logging was added:
- Focused handler: logs all Escape-trigger conditions + SENT/BLOCKED decision
- Escape handler: logs shim/overlay state before/after
- Hotkey handler: logs shim_visible, overlay_state, action
- Hook message loop: heartbeat + WM_QUIT/error exit diagnostics

### What was tried

| Attempt | Finding |
|---------|---------|
| State desync theory (`show_and_focus` updates `ShimState.visible` before window shown) | Logging showed states stayed in sync; Focused(false) was always BLOCKED by `show_pending` or `FOCUS_GRACE_MS` |
| Hotkey modifier tracking theory (CTRL_DOWN atomic stale) | Logging showed mods correct; hook simply didn't fire at all |
| `RIDEV_NOHOTKEYS` blocking hook theory | Conditional registration proved hook stops even without any `RegisterRawInputDevices` call |
| **Hook heartbeat + exit diagnostics** | No `WM_QUIT`, no error, no heartbeat — thread is alive but system stopped posting messages |
| Chromium hook theory | WebView2 installs its own `WH_KEYBOARD_LL`, runs before ours — confirmed by hook silence only when overlay is foreground |

### Verification

Build succeeds with `cargo build --bin nex`. Manual Windows validation:
- Non-Win hotkey (Ctrl+Space): show → hide → show → hide 10×, every press works
- Win hotkey: Start suppressed, keyboard state clean, no corruption
---

## Issue 2: Win key hotkey opens Start menu on second press — FIXED

**Scope:** Hotkey configured as `Win` alone.

**Symptom (original):**
- First Win press: overlay shows correctly, Start menu suppressed
- Second Win press: Start menu opens, overlay may or may not hide

### Root cause

`WH_KEYBOARD_LL` hook cannot prevent the Raw Input Thread (RIT) from processing the physical Win key press. The RIT generates `SC_WINKEY` / `SC_WINMENU` / Start gesture independently of the hook. Masking with `SendInput(0xE8)` loses the race — the RIT processes the Win press before the synthesized mask arrives.

### Final fix

**Strategy:** Suppress the Win key at the RIT level using `RegisterRawInputDevices(RIDEV_NOHOTKEYS | RIDEV_INPUTSINK)` instead of trying to beat the race from the hook.

| Component | Mechanism |
|-----------|-----------|
| Start suppression (overlay visible) | `RegisterRawInputDevices` with `RIDEV_NOHOTKEYS` — block `WM_HOTKEY` for the keyboard at the RIT level |
| Win key detection while suppressed | `WM_INPUT` handler in `instance_signal_subclass` — raw input bypasses `WH_KEYBOARD_LL` suppression |
| First press (no overlay) | `WH_KEYBOARD_LL` eats Win key-down + mask key `0xE8` via `SendInput` covers the brief gap before RIDEV_NOHOTKEYS activates |
| Toggle dispatch | Key-down via hook (first press) or `WM_INPUT` → `OverlayEvent::Hotkey` (second press with RIDEV_NOHOTKEYS) |
| Key-up passthrough | Win key-up is NEVER eaten — it passes through to the system so `GetAsyncKeyState` stays correct |

**Key insight that killed all prior approaches:** When the hook ate the Win key-up (`return 1`), the system's key-state tracker never saw the physical release → `GetAsyncKeyState(VK_LWIN)` stayed TRUE → pressing "e" triggered Win+E (Explorer), "d" triggered Win+D (Show Desktop). This caused the post-hide keyboard corruption. The fix was to **let Win key-up pass through** — key-up never opens Start.

**What was tried (chronological):**

| Attempt | Approach | Result |
|---------|----------|--------|
| Basic `WH_KEYBOARD_LL` swallow Win-down | Return 1 from hook on VK_LWIN | Start still opened on 2nd press |
| Mask key flash (`0xE8` down+up) on Win-down | SendInput to trick RIT | RIT still won the race |
| Hold mask for entire Win press | `send_mask_down()` on Win-down, no release until hide | Start suppressed but keyboard state corrupted after hide |
| Focus-sink window | Separate WS_POPUP that receive foreground on hide | `SetForegroundWindow` returned FALSE on every variant |
| Sink window with `GWLP_HWNDPARENT` owner change | Set overlay's owner to sink before hide | Same — activation failed silently |
| `hold_mask_before_hide()` + spin-wait | Re-send mask from UI thread before hide, spin for RIT to process | Did not fix race; left mask held for non-Win hides |
| `RIDEV_NOHOTKEYS` | Raw input device registration suppresses Win at RIT level | **Works for Start suppression** |
| Sink window removal | Focus-sink was causing keyboard state corruption when it became foreground | Removed dead code |
| **Don't eat Win key-up** | Key-up passes through, `GetAsyncKeyState` updates normally | **Fixes keyboard corruption** |
| Remove `hold_mask_before_hide()` | Mask key redundant with RIDEV_NOHOTKEYS; its extra SendInput was corruption source | Cleaner state |
| Restore `send_mask_up()` on consumed Win-down | Mask released on each Win key-up, no residual input state | Clean key state |

### Current state

**FIXED.** Verified by user:

- 1st Win press → overlay shows, no Start ✓
- 2nd Win press → overlay hides, no Start ✓
- `CheckKeyboardState(+200ms)`: `win_down=false` ✓
- After hide, pressing "e" types "e" (not Explorer) ✓
- Multiple show/hide cycles clean ✓

### Remaining debug logging

- `UiCommand::CheckKeyboardState` — delayed key-state poll after hide
- Hook logs Win key-down consumption and key-up pass-through

Can strip after stabilization period.
---

## How to test

```powershell
# Build
cargo build --release --bin nex

# Run with logging
nex --foreground
```

Test Issue 1: Set hotkey to `Ctrl+Space` in config → cycle show/hide 10x → verify toggles every time
Test Issue 2: Set hotkey to `Win` in config → press Win to show → press Win to hide → repeat 10x → verify Start menu never appears

## Related files

| File | Role |
|------|------|
| `apps/core/src/overlay/hotkey.rs` | `WH_KEYBOARD_LL` hook proc, mask key logic, state tracking |
| `apps/core/src/overlay/host.rs` | Hide UI command |
| `apps/core/src/runtime_loop.rs` | Hotkey event dispatch, `OverlayState` sync |
| `apps/core/src/overlay_state.rs` | Toggle logic (show/hide/focus) |
| `apps/core/src/overlay/shim.rs` | `is_visible()`, `has_focus()`, `show_and_focus()`, `hide()` |
| `docs/win-key-hotkey-research-report.md` | Research on Win-key hotkey suppression techniques |

---

## Issue 3: Installer post-install launch flashes briefly — UNRESOLVED

**Scope:** Inno Setup installer's "Launch Nex" checkbox at end of install.

**Symptom:**
- After install completes, clicking the launch checkbox starts nex.exe
- Brief visual flash appears before nex settles
- SUBSYSTEM confirmed WINDOWS (2) — not a console window
- All CLI spawns eliminated from installer (registry, taskkill, `IsNexInstalled` guard)

**Root cause:** Unknown.

**Hypotheses:**

| # | Theory | Status |
|---|--------|--------|
| 1 | WebView2 overlay window renders before HTML/CSS/JS is ready — framebuffer shows white/transparent acrylic briefly | Untested |
| 2 | Inno Setup `Shellexec` creates a brief window flash during process creation | Untested |
| 3 | nex.exe initializes with console-attached behavior from Inno Setup's process inheritance | Untested |

**What was tried:**

| Attempt | Result |
|---------|--------|
| Removed all `[Run]` sections that spawn nex.exe | Other flashes eliminated, launch checkbox still flashes |
| Replaced PowerShell `Get-CimInstance + Stop-Process` with `taskkill /IM /F` | Removed one flash source |
| Replaced `nex --set-launch-at-startup=true` with direct `[Registry]` writes | Removed another flash source |
| Guarded `StopNexRuntime()` behind `IsNexInstalled()` | No more unnecessary spawned processes on fresh install |
| Verified `cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")` | Binary subsystem = 2 (GUI) — not a console |

**Mitigation in place:** `[Run]` launch checkbox is user-optional (they can uncheck and launch manually). Brief flash is cosmetic only — nex runs correctly afterwards.

---

## Issue 4: Changing hotkey in config requires restart — FEATURE

**Scope:** Runtime config hotkey change without restarting nex.

**Symptom:**
- Edit `config.toml` → change `hotkey = "Ctrl+Space"` to something else
- Save file
- Nex polls config every 500ms, detects hotkey changed
- Logs: `"config hotkey changed (... -> ...), restart required to apply"`
- Shows status message: "Restart required to apply hotkey changes"
- Old hotkey still active until nex restarts

**Root cause:** `HotkeyListener` registers the hotkey once at startup via `RegisterHotKey` / `WH_KEYBOARD_LL`. Config reload path (`runtime_index.rs:284`) detects the change but never re-registers the listener with the new hotkey string.

**What exists:**
| Component | Detail |
|-----------|--------|
| `CONFIG_RELOAD_POLL_INTERVAL` | 500ms poll |
| `RuntimeConfigWatcher` | mtime-based change detection |
| `hotkey_changed` flag | Detected but ignored (log + notify only) |
| Hotkey crash recovery | `runtime_loop.rs:797-823` — restarts listener with new config if thread died |

**What's needed:**
- When config hotkey changes, unregister old hotkey and re-register with new value
- No restart required
- On failure, fall back to restart-required notification (current behavior)

**Related files:**
| File | Role |
|------|------|
| `apps/core/src/runtime_index.rs` | Config reload logic (line 284: hotkey_changed) |
| `apps/core/src/runtime_loop.rs` | Hotkey listener lifecycle (line 797-823) |
| `apps/core/src/overlay/hotkey.rs` | Hotkey registration/parsing |
| `apps/core/src/config.rs` | Config defaults + schema |

---

## Issue 5: Create files/folders from Nex — FEATURE

**Scope:** File system operations from the overlay (like macOS Finder's "New File" / "New Folder").

**Goal:** User types "new file" or "new folder" in Nex → creates item in current/active directory or specified path.

**Use cases:**
- `new file ~/Desktop/report.md` → creates empty file
- `new folder projects/nex` → creates directory
- Template support: `new file index.html` → creates from a default HTML template

**Why:** Core gap vs Start Menu / Spotlight — being able to create files without touching mouse is a major productivity win.

---

## Issue 6: Win key as default hotkey (replace Start Menu) — FEATURE

**Scope:** Default config change + UX to make `Win` the default hotkey instead of `Ctrl+Space`.

**Goal:** Nex becomes the Start Menu replacement — Win key opens Nex, not Start.

**What's needed:**
- Change default config `hotkey` from `"Ctrl+Space"` to `"Win"` 
- Ensure Win-key start suppression works reliably (Issue 2 fix covers this)
- Migration path: existing users keep their current config, new installs get `Win`
- Potential: detect if user hasn't changed hotkey and suggest migrating

**Why:** The Win key is the most accessible keyboard shortcut. Ctrl+Space conflicts with IDEs (VS Code, IntelliJ). Making Win the default makes Nex a true Start Menu replacement.

**Roadmap to win key default:**

| Phase | What |
|-------|------|
| 1. Fix Win key issues | ✅ Done (Issue 2 — RIDEV_NOHOTKEYS + key-up passthrough; Issue 7 — chord detection) |
| 2. Dynamic hotkey re-registration | ⏳ Issue 4 — changing config hotkey without restart |
| 3. Default config change | `hotkey = "Win"` in template |
| 4. Onboarding | First-run prompt: "Set Win key as Nex hotkey (replaces Start Menu)?" |

---

## Issue 7: Win key hotkey blocks all Win+ shortcuts — FIXED

**Scope:** Hotkey configured as `Win` alone.

**Symptom (original):**
- Pressing `Win+E` (Explorer) → Nex opens instead of Explorer
- Pressing `Win+D` (Show Desktop) → Nex opens instead
- Pressing `Win+R` (Run), `Win+I` (Settings), etc. → all captured by Nex
- Win key cannot function as both Nex hotkey AND Windows modifier simultaneously

**Root cause:** Two independent problems:
1. **Hook proc ate Win key-down** — chord key (E, D, etc.) never arrived at the target window because the hook consumed the Win press.
2. **WM_INPUT handler fired on first press** — the raw input sink toggled the overlay on Win key-down, racing with the hook.

### Fix

**Strategy:** Fire hotkey only on Win key-up. Never eat Win key-down. Use chord detection on key-down to classify the press.

| Component | Mechanism |
|-----------|-----------|
| Hook win key-down | Mark `CONSUMED_WIN_VK`, send mask, **don't eat** — message passes through normally |
| Chord detection | Non-modifier key-down (E, D, R, etc.) while `CONSUMED_WIN_VK` set → clears `CONSUMED_WIN_VK`, releases mask, passes through |
| Hook win key-up | If `CONSUMED_WIN_VK` still matches (no chord) → fire hotkey. If chord cleared it → no-op. |
| WM_INPUT handler | Checks `OVERLAY_VISIBLE` atomic — only fires toggle when overlay is visible (second press to hide). First press exclusively handled by hook on key-up. |
| Second press (overlay visible) | WM_INPUT handler fires on Win key-down immediately via `GetAsyncKeyState`. Win+E hides overlay + opens Explorer. |

**Key decisions:**
- **No timer thread** — hotkey fires on key-up, not a delayed timer. Eliminates race between timer and chord detection entirely.
- **No key-down eat** — Win passes through the hook, so `GetAsyncKeyState` stays accurate and Win+ chords work natively.
- **First press via hook, second via WM_INPUT** — separate detection paths with `OVERLAY_VISIBLE` as gate.

**Trade-off accepted:** Nex opens on Win key-up, not key-down. For a quick tap (~50-150ms between press and release) this is imperceptible. For a held Win, Nex opens on release — users learn to tap.

**What was tried (chronological):**

| Attempt | Approach | Result |
|---------|----------|--------|
| Eat Win-down, check `GetAsyncKeyState` for chord | Captured Win, polled chord keys | `GetAsyncKeyState` returns true for next key BEFORE it arrives in hook queue — unreliable |
| Bitfield tracking (KS0/KS1/KS2) | Mark key state on each hook event | Same timing issue — chord key's bit arrives same cycle as Win |
| `any_non_mod_held()` + `win_chord_held_via_async()` | Check if any non-mod key physically held | Misses chord because Win-down processed first, chord key not yet pressed |
| Timer thread (120ms) | Spawn thread on Win-down, fire hotkey after 120ms if no chord | Race: timer fires hotkey before chord key-down arrives → flash + inconsistency |
| **Key-up only + chord detection** | Win-down: mark+sink, Win-up: fire if no chord | **FIXED** — no races, no timer, no flash |

---

## Issue 8: Test suite is broken and incomplete — UNRESOLVED

**Scope:** All test files in `apps/core/tests/` and inline `#[cfg(test)]` modules.

### Symptoms

- `cargo test -p nex` fails to run (rlib metadata errors)
- 12 tests broken (1 compile fail, 11 runtime fails)
- 19 modules have zero test coverage
- Tests haven't kept up with WebView2 migration and Win-key hotkey changes

### Broken Tests (12)

**Compile failure (1 file):**
| File | Tests | Cause |
|------|-------|-------|
| `config_test.rs` | 11 (all fail) | `SearchBackend::Tantivy` and `.search_backend` field removed from Config |

**Runtime failures (11 tests):**
| File | Tests | Cause |
|------|-------|-------|
| `core_service_test.rs` | 2 | Missing temp file panic, `use_count` not ≥1 |
| `search_test.rs` | 3 | Dedup logic changed — app vs file, missing chrome id |
| `settings_test.rs` | 2 | Win hotkey now allowed (asserts reversed) |
| `overlay_state.rs` | 1 | Hide vs FocusExisting logic changed |
| `runtime.rs` | 2 | Message format + validation changed |
| `overlay/platform.rs` | 1 | Another Nex instance found |
| `discovery_test.rs` | 1 | tempdir timing (flaky, intermittent) |

### Zero-Coverage Modules (19)

| Module | Visibility | Notes |
|--------|------------|-------|
| `console_signal` | `pub(crate)` | Windows-only |
| `hotkey` | `pub` | Only 1 parse test in `hotkey_test.rs` |
| `model` | `pub` | Data types only |
| `overlay/host` | `pub(crate)` | Windows-only — overlay Show/Hide/positioning |
| `overlay/model` | `pub(crate)` | Windows-only — data types |
| `overlay/shim` | `pub(crate)` | Windows-only — imperative overlay API |
| `overlay/tray` | `pub(crate)` | Windows-only — system tray |
| `overlay/indexing_progress` | `pub(crate)` | Windows-only — progress window |
| `runtime_actions` | `pub(crate)` | Launch dispatch |
| `runtime_commands` | `pub(crate)` | Status/memory commands |
| `runtime_hotkey` | `pub(crate)` | Hotkey registration |
| `runtime_index` | `pub(crate)` | Config reload, index rebuild |
| `runtime_loop` | `pub(crate)` | Main event loop — most complex |
| `runtime_overlay_rows` | `pub(crate)` | Row rendering |
| `runtime_process` | `pub(crate)` | Process finding |
| `runtime_search_session` | `pub(crate)` | Search session state |
| `search_worker` | `pub(crate)` | Search dispatch |

### Notes

- Windows-only modules are hard to test without a window station (Wry/WebView2)
- 12 passing integration test files + 136 passing inline tests still work
- `hotkey_runtime_test.rs`, `startup_test.rs`, `windows_runtime_smoke_test.rs` have cfg-gated tests
- Perf test `warm_query_p95_under_15ms` still passes

---

## Issue 9: Quick Launch items cause visible delay on open — FIXED

**Scope:** Overlay show performance with pinned Quick Launch items.

**Symptom:**
- No pinned items: overlay appears instantly, search bar ready immediately
- With pinned items: search bar appears first, then ~100-300ms later the quick launch list appears below
- Visual "pop-in" — the list appears after the search bar, feels sluggish

**Root cause found — investigation uncovered two bottlenecks:**

| # | Bottleneck | Location | Delay |
|---|------------|----------|-------|
| 1 | **Resize debounce (primary)** — Show sets window to 60px (search bar). JS signals resize with full height. Growth goes through 100ms debounce. Window stays at 60px for 100ms → search bar visible first, items "pop in" after. | `host.rs:470` (resize handler) | ~100ms |
| 2 | **DB pre-read (secondary)** — `warm_search_cache()` reads entire SQLite DB (10-30MB) via `std::fs::read` on every show. Blocks `show_and_focus()`. | `core_service.rs:974` | 20-100ms |

### Fix

| Fix | Change | Gain |
|-----|--------|------|
| 1. Bypass resize debounce on first show | `host.rs`: `first_resize_after_show` flag set on Show, checked in Resize handler — initial growth applies immediately | ~100ms eliminated |
| 2. Remove `std::fs::read(&db_path)` | `core_service.rs`: deleted the full-DB read; Tantivy warmup + lock warmup retained | ~20-100ms eliminated |

**Result:** Content appears in a single frame — no staggered "search bar first, items later" pop-in.
