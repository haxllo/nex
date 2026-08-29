## Hotkey → Task Manager — Bug Hunt Summary

### Fixed Bugs (4 independent issues, all in `feat/win-key-hotkey`)

1. **Hook proc `GetMessageW` never wakes** for Win key hotkeys (no `RegisterHotKeyW`)  
   → `PostThreadMessageW(WM_NEX_WAKE)` after `HOTKEY_FIRED=true` in hook proc

2. **`WM_NEX_WAKE` `continue` skipped all hotkey processing**  
   → Removed `continue`, falls through to `HOTKEY_FIRED` check

3. **`force_foreground` `AttachThreadInput` fails across UIPI** *(Medium→High IL)*  
   → Tolerates failure (returns 0 instead of 1; `SetForegroundWindow` may still work)

4. **`OVERLAY_HWND` `OnceLock` caches NULL permanently**  
   `FindWindowW` called before overlay created → null forever → all subsequent dispatches skip `SetForegroundWindow`  
   **→ FIX: Removed the entire cached-HWND approach.** Helper `SetForegroundWindow` always fails anyway because overlay is `.with_visible(false)` (hidden window can't be foreground). Only `AllowSetForegroundWindow(nex_pid)` remains — grants nex.exe permission to call `SetForegroundWindow` in `force_foreground` after showing the window.

### Fixes from previous round
- `hold_mask_before_hide()` / `release_mask_after_hide()` in `UiCommand::Hide`
- Helper calls `AllowSetForegroundWindow(nex_pid)` before each HOTKEY dispatch (via `--target-pid` CLI arg)
- Helper: removed `pipe.metadata()` check, propagates write errors

### What We Learned
- Task Manager (High IL) blocks `WM_INPUT` (RIDEV_INPUTSINK) delivery to Medium IL
- Helper (High IL) `WH_KEYBOARD_LL` hook + named pipe bypasses this
- `SetForegroundWindow` on hidden window always fails — must call AFTER `set_visible(true)`
- Tray icon click works → means `show_and_focus()` works → problem was entirely in hotkey detection/delivery chain

### Open Question
Does `AllowSetForegroundWindow` from helper (High IL) + nex.exe `SetForegroundWindow` (Medium IL, after `set_visible(true)`) actually work with Task Manager foreground? Need user to test latest build.
