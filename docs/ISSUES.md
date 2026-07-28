# Open Issues — Branch: `feat/win-key-hotkey`

## Issue 1: Non-Win hotkey shows but does not hide

**Scope:** Any hotkey that is NOT the Win key alone (e.g. `Ctrl+Space`, `Ctrl+Shift+F`, `Alt+Space`).

**Symptom:**
- First hotkey press: overlay shows correctly
- Second hotkey press: overlay does NOT hide — hotkey appears to do nothing, or shows again

### Root Cause (suspected)

**Likely:** State desync between `OverlayState` (local struct in `runtime_loop.rs`) and `ShimState` (shared `Arc<Mutex<ShimState>>` in `shim.rs`).

Event flow:
1. `OverlayEvent::Hotkey` handler (runtime_loop.rs:904) reads `self.overlay.is_visible()` → reads `ShimState.visible`
2. Syncs `OverlayState.visible` from that value
3. Calls `OverlayState::on_hotkey(has_focus)` to get `ShowAndFocus` or `Hide`

**Race window:** `ShimState.visible` is set true by `show_and_focus()` before the UI thread has actually rendered the window. If a Focus Loss (`WindowEvent::Focused(false)` during WebView build or foreground change) fires an `Escape` event, `Escape` resets `OverlayState.visible = false`. The hotkey handler then treats the subsequent press as "show" not "hide".

**Alternative theory (hook-level):** For modifier+key combos, the `WH_KEYBOARD_LL` hook eats the target key on the first press (return 1). On second press:
- If the overlay WebView consumed the modifier (Ctrl/Alt), the hook's atomic modifier tracking (`CTRL_DOWN`/`ALT_DOWN`) might be stale
- But LL hook fires before dispatch, so physical key events should still be visible

**What was tried:**

| Attempt | Commit | Result |
|---------|--------|--------|
| Rewrote hotkey from `RegisterHotKey` to `WH_KEYBOARD_LL` hook | `9def97e` | Base change — needed for Win key |
| Left/right Win treated equivalent | `13e19fc` | Not related |
| Exclude target key from extra-modifier check | `8429493` | Not related |
| Code review of all hotkey paths | `494a032` | Found no smoking gun |

### Verification

Build succeeds with `cargo build --bin nex`. Manual Windows validation remains: configure `Win`, then repeat show → hide at least ten times and confirm that Start never opens.
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
