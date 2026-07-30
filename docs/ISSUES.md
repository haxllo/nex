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

## Issue 4: Changing hotkey in config requires restart — FIXED

**Scope:** Runtime config hotkey change without restarting nex.

**Symptom (original):**
- Edit `config.toml` → change `hotkey = "Ctrl+Space"` to something else
- Save file
- Nex polls config every 500ms, detects hotkey changed
- Logs: `"config hotkey changed (... -> ...), restart required to apply"`
- Shows status message: "Restart required to apply hotkey changes"
- Old hotkey still active until nex restarts

**Root cause:** `HotkeyListener` registers the hotkey once at startup via `RegisterHotKey` / `WH_KEYBOARD_LL`. Config reload path (`runtime_index.rs:284`) detects the change but never re-registers the listener with the new hotkey string.

### Fix

**Changes:**
| Change | File | Detail |
|--------|------|--------|
| `HOOK_CTX` OnceLock → `Mutex<Option<HookContext>>` | `hotkey.rs` | Allows updating context on re-registration. All 5 readers updated to lock+clone pattern. |
| Drop: kill helper in scheduled-task mode | `hotkey.rs` | Drop now uses `taskkill /F /IM NexHelper.exe` when `helper_process_handle == 0` (scheduled task mode). Previously only killed direct-spawn helper. |
| `maybe_apply_runtime_config_reload` returns `bool` | `runtime_index.rs` | Returns `hotkey_changed` so caller can act on it. Status text changed from "Restart required" to "Hotkey updated". |
| Re-registration in config reload path | `runtime_loop.rs` | When hotkey changes: drops old listener (kills helper), waits 300ms, spawns new listener. On failure, falls back to hook mode (existing crash recovery handles it). |

**Sequence:**
1. Config change detected → hotkey_changed = true
2. Old `HotkeyListener` dropped → helper killed (TerminateProcess or taskkill)
3. 300ms wait for helper to fully die
4. New `HotkeyListener::start()` called with new hotkey → spawns new helper or falls back to hook
5. New `HOOK_CTX` written with correct `target_is_win` flag
6. Both nex hook proc and helper hook proc use correct logic for the new hotkey

**Related files:**
| File | Role |
|------|------|
| `apps/core/src/runtime_index.rs` | Config reload logic, returns `hotkey_changed` |
| `apps/core/src/runtime_loop.rs` | Hotkey listener lifecycle + re-registration |
| `apps/core/src/overlay/hotkey.rs` | HOOK_CTX Mutex, Drop cleanup, start/stop |

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
| 2. Dynamic hotkey re-registration | ✅ Done (Issue 4 — feature impl; Issue 10 — stability fixes: 11 bugs resolved) |

**Infrastructure (done):**
- [x] Win key fires on key-up (no Start menu)
- [x] Win+ chords pass through natively
- [x] Elevated window support (helper)
- [x] Dynamic hotkey re-registration

**To ship as default:**
- [ ] Change default hotkey in `config.rs` template from `Ctrl+Space` → `Win`
- [ ] First-run overlay explains "Win key opens Nex"
- [ ] In-app hotkey picker (UI in overlay to change without editing config)
- [ ] File/folder creation from Nex (Issue 5)
- [ ] Config migration path for existing users
- [ ] Edge-case hardening (fast double-press, sleep/wake, session lock)

**Future (nice to have):**
- [ ] Start Menu-style app launcher tabs/categories
- [ ] System tray → full Start replacement settings

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
| Helper hook proc | Mirrors nex hook proc — key-down: mark+mask+pass through, chord clears `CONSUMED_WIN_VK`, key-up: fire hotkey if no chord. Applied in commit b20245d. |
| WM_INPUT handler | Ignores Win key entirely — raw input only tracks `RAW_WIN_DOWN` for mask key cleanup. Both show and hide handled by hook/helper on key-up. |
| Second press (overlay visible) | Hook/helper fires on Win key-up, same as first press. Win+E passes through (chord detection clears `CONSUMED_WIN_VK`), Explorer opens + overlay stays visible until bare Win release. |

**Key decisions:**
- **No timer thread** — hotkey fires on key-up, not a delayed timer. Eliminates race between timer and chord detection entirely.
- **No key-down eat** — Win passes through the hook, so `GetAsyncKeyState` stays accurate and Win+ chords work natively.
- **First press via hook, second via WM_INPUT** — separate detection paths with `OVERLAY_VISIBLE` as gate.

**Trade-off accepted:** Nex toggles on Win key-up for both show and hide. For a quick tap (~50-150ms between press and release) this is imperceptible. For a held Win, overlay toggles on release — users learn to tap.

**What was tried (chronological):**

| Attempt | Approach | Result |
|---------|----------|--------|
| Eat Win-down, check `GetAsyncKeyState` for chord | Captured Win, polled chord keys | `GetAsyncKeyState` returns true for next key BEFORE it arrives in hook queue — unreliable |
| Bitfield tracking (KS0/KS1/KS2) | Mark key state on each hook event | Same timing issue — chord key's bit arrives same cycle as Win |
| `any_non_mod_held()` + `win_chord_held_via_async()` | Check if any non-mod key physically held | Misses chord because Win-down processed first, chord key not yet pressed |
| Timer thread (120ms) | Spawn thread on Win-down, fire hotkey after 120ms if no chord | Race: timer fires hotkey before chord key-down arrives → flash + inconsistency |
| **Key-up only + chord detection** | Win-down: mark+sink, Win-up: fire if no chord | **FIXED** — no races, no timer, no flash |
| **Helper key-up mismatch (regression)** | Helper fired on key-down + WM_INPUT fired on key-down → second press hid on key-down then re-showed on key-up (Issue 7 regression 2026-07-30) | **Fixed** — removed WM_INPUT Win toggle, updated helper to key-up approach |

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

---

## Issue 10: Dynamic hotkey change freezes or becomes unresponsive — FIXED

**Scope:** Changing hotkey in `config.toml` while nex is running.

**Symptom:**
- After changing the hotkey in config, the new hotkey doesn't work
- The old hotkey may keep working for a brief window
- Tray icon clicks stop working
- Process becomes "not responding" (frozen)

### Bugs found and fixed

| # | Bug | File | Fix |
|---|-----|------|-----|
| 1 | **HOOK_CTX never cleared on Drop** — `check_raw_input_hotkey()` read the stale old config during the re-registration gap, firing spurious hotkey events for the old combo. All atomics (`CTRL_DOWN`, `ALT_DOWN`, `SHIFT_DOWN`, `CONSUMED_WIN_VK`, `SUPPRESS_FOCUS_ESCAPE`) retained values from the dead hook thread. | `hotkey.rs` | Clear `HOOK_CTX` to `None` and reset all 5 atomics **before** signalling threads in Drop |
| 2 | **Drop blocked runtime worker on `handle.join()`** — hook-mode Drop joined the old hook thread, freezing the event loop. Windows marked the process "not responding" during the 500ms re-registration gap. | `hotkey.rs` | Detach hook thread (`drop(handle)`) instead of joining — same pattern the helper path already used |
| 3 | **HOOK_CTX written before thread confirmed running** — `start_internal` set HOOK_CTX, then spawned the thread. If thread failed, HOOK_CTX was left orphaned with no corresponding hook. | `hotkey.rs` | Clear HOOK_CTX on spawn failure and on ready-channel error |
| 4 | **Old hook's WM_HOTKEY fired after exit signalled** — `RegisterHotKeyW` fallback had queued WM_HOTKEY before WM_QUIT. Old thread read the new HOOK_CTX and sent stale events. | `hotkey.rs` | Skip WM_HOTKEY in the old thread when `should_exit` is set |
| 5 | **`is_alive()` returned false during thread startup** — narrow window where `is_finished()` returned true before `thread_id` was set, causing the recovery path to drop valid listeners | `hotkey.rs` | Return `true` if `thread_id` OnceLock is empty (thread still starting) |
| 6 | **250ms tick was a no-op** — `recv(tick) -> _ => {}` never called `on_event`, so config reload and health checks only ran when user events arrived. After old hook killed, nothing could generate events → deadlock. | `shim.rs`, `model.rs` | Added `OverlayEvent::Tick` variant; tick calls `on_event(Tick)` |
| 7 | **Config reload infinite loop** — `config::load` writes back the file (migration/template), changing mtime. Next poll detected this as a change → reload → write → reload... | `runtime_index.rs` | Re-read `last_modified` after `config::load` to absorb self-writes |
| 8 | **Recovery path blocked the event loop** — dead-listener recovery called `HotkeyListener::start()` inside `on_event`, which blocked for up to 1.5s on pipe connect retries | `runtime_loop.rs` | Spawn recovery on separate thread (`nex-hotkey-recover`) |
| 9 | **Concurrent re-registration threads** — rapid config saves spawned multiple re-registration threads, creating zombie hook threads + duplicate hotkey events | `runtime_loop.rs` | `RE_REGISTERING` AtomicBool gate — only one re-registration at a time |
| 10 | **Listener dropped before gate check** — `*guard = None` ran before `RE_REGISTERING` CAS. If gate was blocked, listener was dropped and never reinstated. | `runtime_loop.rs` | Move drop inside the gate, after CAS succeeds |
| 11 | **Old helper at High IL intercepted keystrokes** — `start_no_helper` never wrote `helper-config.json`, so the scheduled task auto-restarted the helper with stale config. High IL hook sat above Medium IL in-process hook. | `runtime_loop.rs`, `hotkey.rs` | `schtasks /end` stops the task, `taskkill` kills the process, then `HotkeyListener::start()` writes new config and relaunches the helper cleanly |

### Verification

Build succeeds. Manual testing:
- Change hotkey in config while nex is running → new hotkey works, old hotkey stops
- Tray icon works after change
- Rapid config saves don't create duplicate events or zombie threads
- Win ↔ Ctrl+Space transitions work both ways
- Helper restarts with correct config after re-registration

---

## Issue 11: Shutdown/restart/lock not possible when Win key replaces Start Menu — FEATURE

**Scope:** Users who configure Win key as Nex hotkey lose the Start Menu's power menu (shutdown, restart, sleep, lock, sign out).

**Symptom:** Win key opens Nex instead of Start Menu. User has no obvious way to shutdown/restart/lock/sleep without the Start power button. Win+X (Quick Link) → U → U (shutdown) still works as a workaround, but is not discoverable.

**Goal:** Provide power management actions from within Nex so users don't need the Start Menu.

**Possible approaches:**

| Approach | Description |
|----------|-------------|
| 1. Search commands | Type "shutdown", "restart", "lock", "sleep" in Nex → execute the action (with confirmation for destructive ones) |
| 2. Overlay power button | Small power icon in the overlay footer/tray area |
| 3. System tray submenu | Right-click tray icon → Shutdown / Restart / Lock / Sleep |
| 4. Custom hotkey chord | e.g. Win+X within Nex opens a power menu |

**Why:** Core gap for Win-key-as-default — users must not lose the ability to shutdown/restart their machine. Without this, Win key default is a downgrade.
