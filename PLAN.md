# Plan: Win Key Hotkey via Low-Level Keyboard Hook

## Problem
`RegisterHotKey` cannot use the Win key alone — it only works as a modifier
in chords. The OS reserves the Win key for the Start Menu.

## Solution
Replace `RegisterHotKey` + `GetMessageW` in `hotkey.rs` with
`SetWindowsHookEx(WH_KEYBOARD_LL)` + `LowLevelKeyboardProc`.
This intercepts keystrokes before the OS processes them, so we can
swallow the Win key and fire the overlay instead of opening Start Menu.

## Files to Change

### 1. `apps/core/src/overlay/hotkey.rs` — Core rewrite
- Keep `HotkeyListener::start(hotkey_str, event_tx) -> Result<Self, String>` signature
- Replace `RegisterHotKey` thread with a `WH_KEYBOARD_LL` hook thread
- Parse hotkey into target VK code + required modifiers
- Hook proc uses `GetAsyncKeyState` to check modifier state, fires event
  and swallows key on match
- Handle `"Win"` alone (target = VK_LWIN/VK_RWIN, no modifiers) and
  standard chords like `"Ctrl+Space"`, `"Win+F1"`, `"Ctrl+Win"` uniformly
- No global/static state needed — use heap-allocated callback context
  passed via `SetWindowsHookEx`'s `dwThreadId=0` + thread-local or
  boxed closure trick (via raw pointer in hook proc)

### 2. `apps/core/src/settings.rs` — Validation changes
- `normalize_modifier()`: allow `Win`/`Windows`/`Meta` (currently rejected)
- `validate_hotkey()`: allow single-part hotkeys (`"Win"` alone)
- `is_reserved_hotkey()`: add `Win` alone to reserved list? No — it's the
  whole point. But keep other reserved checks.

### 3. `apps/core/src/config.rs` — Template update
- Add `Win` and `Win+Shift+F1` etc. to `hotkey_recommended`
- Update help text to mention Win key support

### 4. `apps/core/src/overlay/model.rs` — No changes needed
- `OverlayEvent::Hotkey(i32)` stays the same

## Hook Proc Logic

```
On WM_KEYDOWN/WM_SYSKEYDOWN:
  1. Check if pressed key matches target (or Win alias)
  2. Use GetAsyncKeyState to check required modifiers are held
  3. Use GetAsyncKeyState to check NO extra modifiers are held
  4. If match: send OverlayEvent::Hotkey(1), return 1 (swallow)
  5. Otherwise: CallNextHookEx

On WM_KEYUP/WM_SYSKEYUP:
  1. CallNextHookEx (pass through)
```

## Edge Cases
- Win+other key while overlay is showing → let through (not swallowed)
- Win held → another key pressed → the other key passes normally
  (Windows doesn't know Win is held since we swallowed keydown)
- Fast double-tap Win → each tap fires one event
- Clean shutdown: UnhookWindowsHookEx in Drop, PostMessage to
  wake the message loop so the thread exits
