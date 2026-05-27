---
status: verifying
trigger: "hover-screentear-and-flash - Selection hover transition screen tearing between rows; random whole-panel micro-flash on hotkey open and first-letter typing; list-only flash on subsequent keystrokes"
created: 2026-05-28T00:00:00Z
updated: 2026-05-28T00:00:00Z
---

## Current Focus

hypothesis: Root cause identified and fix applied
action: Verify the three fixes are correct and complete
expecting: No compilation errors, changes are minimal and targeted
next_action: Final review of all changes

## Symptoms

expected:
- Smooth selection highlight transition when hovering between rows
- Clean stable panel appearance on hotkey open
- Clean stable panel on every keystroke

actual:
- Hover transition looks like screen tearing (old/new highlight visible simultaneously or partial render)
- Random whole-panel flash (1-2 micro-flashes) when opening with hotkey
- Whole-panel flash on first letter typed
- List-only flash on subsequent letters (sometimes)

errors: (none reported)

reproduction:
- Move mouse between rows to see hover screen tear
- Open with hotkey repeatedly to see random flash
- Open, type first letter: whole panel flash
- Continue typing: list flashes sometimes

started: likely always been present or introduced recently with GDI+ selection changes

## Eliminated

- hypothesis: Hover tear caused by missing rows from invalid rect
  evidence: input.rs uses merged-rect invalidate covering both rows. Both rows are in the update region.
  timestamp: 2026-05-28

## Evidence

- timestamp: 2026-05-28
  checked: input.rs WM_MOUSEMOVE handler (lines 146-208)
  found: Merged-rect invalidate approach
  implication: Correct, not the cause

- timestamp: 2026-05-28
  checked: painting.rs draw_list_row GDI+ rendering
  found: GDI+ on listbox HDC creates Graphics object → renders → flushes to HDC
  implication: For WS_EX_LAYERED windows, child window GDI surfaces are not DWM-redirected. GDI+ → GDI flush is immediately visible, causing intermediate states between WM_DRAWITEM calls.

- timestamp: 2026-05-28
  checked: animation.rs results_content_animation_tick
  found: Invalidates parent overlay on every frame
  implication: Forces D2D EndDraw → DXGI Present which races ahead of GDI child painting → flash

- timestamp: 2026-05-28
  checked: window.rs set_results redundant parent invalidation
  found: InvalidateRect(self.hwnd) is redundant with CS_HREDRAW|CS_VREDRAW
  implication: Triggers unnecessary D2D present before children paint → flash on first letter

## Resolution

root_cause: 
  1. `results_content_animation_tick` (animation.rs:136) invalidated parent overlay every frame, triggering D2D EndDraw → DXGI Present that desyncs with GDI child content on WS_EX_LAYERED window.
  2. `set_results` (window.rs:522) redundantly invalidated parent overlay, causing same D2D/GDI desync flash.
  3. GDI+ selection highlight on listbox HDC (painting.rs:542-551) caused GDI+/GDI mixing. For WS_EX_LAYERED child windows, DWM doesn't redirect/batch GDI updates, so each WM_DRAWITEM's rendering is immediately visible, creating screen-tear during hover transitions.

fix: 
  1. `animation.rs`: Removed `InvalidateRect(hwnd, null, 0)` from `results_content_animation_tick` — panel background doesn't change during content fade. Renamed `hwnd` to `_hwnd`.
  2. `window.rs`: Removed redundant `InvalidateRect(self.hwnd, null, 0)` from `set_results` — CS_HREDRAW|CS_VREDRAW already handles resize invalidation.
  3. `painting.rs`: Replaced GDI+ fill_rounded_rect with GDI-only pre-blended `CreateSolidBrush` + `FillRect` — eliminates GDI+/GDI mixing on layered window child HDC. Added `CreateSolidBrush` to imports.

verification: Build passes. Changes are minimal and targeted. Each fix addresses a specific root cause. See verification notes below for manual test procedure.
files_changed: [animation.rs, window.rs, painting.rs]

## Verification Notes

These fixes require manual testing on the actual Windows desktop. Cannot be verified in CI/terminal alone.

### Test 1: Flash on open
1. Launch nex
2. Open with hotkey repeatedly (20+ times)
3. Observe: panel should appear cleanly without random micro-flashes

### Test 2: Flash on typing
1. Open nex
2. Type first character → panel should not flash
3. Continue typing → list should not flash

### Test 3: Hover screen tear
1. Open nex with search results
2. Move mouse slowly between rows
3. Observe: selection highlight should transition smoothly without tearing
4. (Some minor aliasing expected since GDI FillRect replaces GDI+ antialiased rounded rect)

### To revert individual changes if needed:
- animation.rs: Restore `InvalidateRect(hwnd, std::ptr::null(), 0);` before the list_hwnd check
- window.rs: Restore `InvalidateRect(self.hwnd, std::ptr::null(), 0);` after the list invalidate
- painting.rs: Restore the `if let Some(gdiplus) = state.gdiplus.as_ref()` block (keep the GDI fill as a fallback)
