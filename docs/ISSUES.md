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

## Issue 2: Win key hotkey opens Start menu on second press — Under verification

**Scope:** Hotkey configured as `Win` alone. User intends to toggle Nex like a Spotlight/Raycast replacement.

**Symptom:**
- First Win press: overlay shows correctly, Start menu suppressed
- Second Win press: Start menu opens, overlay may or may not hide

### Investigation and root cause

Suppression of the Win messages alone is insufficient when hiding returns the foreground to the shell. The important requirement is that the shell observes a non-Win key press while the physical Win key is held. A `WH_KEYBOARD_LL` hook remains the correct interception point, but the mask must be injected synchronously from that hook instead of during the later UI hide transition.

Nex previously dispatched the hotkey on **Win key-down**. On the second press that immediately hid the overlay and moved focus to Explorer while the physical Win key remained down. The later Win key-up could then be interpreted as the Start-menu gesture. `hold_mask_before_hide()` attempted to mask that ordering problem from a second thread; it also injected a mask key for every hide, including non-Win hotkeys, without a matching release.

Further Windows testing found the decisive race: when Nex has focus, its `WindowEvent::Focused(false)` handler emits `OverlayEvent::Escape` to implement click-outside dismissal. A bare Win press can trigger that focus-loss path before the hotkey event finishes. This creates two competing hide paths. If Nex is already unfocused, the focus-loss handler is inactive and the Win hotkey hides cleanly without Start opening.

The same observation also shows why the mask is unreliable while Nex has focus: injected keyboard input is delivered to the focused WebView2 window rather than the shell. Before injecting the `VK 0xE8` mask on a focused-overlay Win press, the hook now hands foreground to the shell. The matching Win-up still performs the actual Nex hide.

### Fix (committed 44de597 + 1 pending)

Three-part approach:

1. **Relinquish focus before hide (main thread):** `hand_focus_to_shell()` in `runtime_loop.rs` runs `SetForegroundWindow(GetShellWindow())` just before `self.overlay.hide()`. Decouples focus transition from window hide — Explorer never sees a focus change during hide, so it doesn't re-check raw input for Start gesture.

2. **Dispatch toggle on Win key-up, inject mask on Win key-down (hook):** Win key-down → send `VK 0xE8` down+up as completed chord + set `SUPPRESS_FOCUS_ESCAPE` guard + eat Win-down. Win key-up → fire `OverlayEvent::Hotkey` + eat Win-up. No hide happens while Win is physically held.

3. **Suppress focus-loss→Escape during bare-Win press:** `is_bare_win_press_active()` guard in `Focused` event handler prevents competing hide paths. `finish_bare_win_press()` clears guard after runtime handles the toggle.

4. **Simplified toggle:** `on_hotkey()` always toggles show/hide (no `FocusExisting`). Second press always hides regardless of focus state.

5. **Removed `hold_mask_before_hide()`:** No longer needed — mask is sent from hook synchronously on Win-down, focus handoff happens on main thread before hide.

**What was tried:**

| Attempt | Commit | Result |
|---------|--------|--------|
| Rewrote to LL hook with basic Win swallow | `9def97e` | Start menu still opened |
| Left/right Win key equivalence | `13e19fc` | Unrelated fix |
| Exclude target from extra-mod check | `8429493` | Unrelated fix |
| Replace swallow-flag with VK_RESERVED suppress-combo (mask key flash) | `a5b75d9` | Start menu still opened on second press |
| Suppress matching Win key-up | `2903dec` | Start menu still opened |
| Added `hold_mask_before_hide()` — re-send mask + spin-wait before window hide | `9e72b0c` | Did not address the ordering bug; also left the mask held for non-Win hides |
| Attempted pass-through approach (let Win key through, rely entirely on mask) | `a46c25c` (separate branch) | Abandoned |
| **Final: key-up dispatch + focus handoff + mask + escape guard** | `44de597` + pending | Builds clean. Needs Windows testing. |

### Verification

Build succeeds with `cargo build --bin nex`. Only pre-existing dead-code warnings. Manual Windows validation required: quick-`win` key show/hide multiple times and watch for Start menu never opening. Also verify non-win hotkeys still work. At least do ten repetitions of show/hide.
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
