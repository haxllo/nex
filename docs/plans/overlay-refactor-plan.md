# Overlay Refactor Plan — `windows_overlay.rs`

> **Original:** 5,576 lines (single `mod imp { ... }` block)  
> **Now:** 11 modules, 5,913 total lines  
> **Status:** ✅ **Complete** (file split done, tests added)  
> **Approach:** Converted `windows_overlay.rs` into a `windows_overlay/` directory. The `mod.rs` carries `#[cfg(target_os = "windows")]` and conditionally includes sub-modules. All consumers (just `lib.rs` and `runtime.rs`) continue to work via `use crate::windows_overlay::*`.

---

## 1. Current Problems

| Problem | Severity | Description |
|---------|----------|-------------|
| **God file** | 🔴 Critical | Single file handles window creation, painting, animation, input, theming, tray, layout |
| **Tight coupling** | 🔴 Critical | `OverlayShellState` has ~40 fields; all subsystems share mutable access via raw pointers |
| **No testability** | 🔴 Critical | Every function takes `&OverlayShellState` — impossible to unit test painting/layout in isolation |
| **Magic constants** | 🟡 High | ~80 layout tokens scattered as `const` at module level |
| **GDI resource leaks** | 🟡 High | Brushes, fonts, icons managed manually — no RAII wrappers |
| **Event handling spaghetti** | 🟡 High | `overlay_wnd_proc` is one giant `match` with inline logic |

---

## 2. Proposed Module Structure

> **Note on `mod imp` pattern:** The current `windows_overlay.rs` wraps everything in `#[cfg(target_os = "windows")] mod imp { ... }`. After splitting, each sub-module should either have its own `#[cfg(target_os = "windows")]` gate, or the parent `mod.rs` should conditionally include the directory. The latter is cleaner — `mod.rs` holds the gate and only exposes modules on Windows.

```
windows_overlay/
├── mod.rs                  # #[cfg(target_os = "windows")] — re-exports + public API facade
├── types.rs                # Types, constants, OverlayTheme, OverlayPalette
├── window.rs               # Window creation, registration, positioning
├── painting.rs              # All GDI/Direct2D painting (panel, rows, input)
├── layout.rs                # Layout calculations (measure, arrange, animation)
├── input.rs                 # Input handling (keyboard, mouse, wheel)
├── tray.rs                  # System tray icon + context menu
├── animation.rs             # Window animation, content fade, badge animation
├── icon_cache.rs            # Icon loading, caching, LRU eviction
└── state.rs                 # OverlayShellState struct + helpers
```

### Module Responsibilities

#### `types.rs` (~200 lines)
- Move all `const` layout tokens here
- `OverlayTheme`, `OverlayPalette`, `OverlayRow`, `OverlayRowRole`, `OverlayEvent`
- Palette definitions (`PALETTE_DARK`, `PALETTE_LIGHT`)
- `palette_for_theme()`, `detect_system_theme()`

#### `state.rs` (~200 lines)
- `OverlayShellState` struct (same fields, reorganized into logical groups)
- Default impl
- Cleanup helper (`cleanup_state_resources`)

#### `window.rs` (~800 lines)
- Window class registration
- `CreateWindowExW` and child control creation
- `WM_CREATE`, `WM_DESTROY`, `WM_NCDESTROY` handlers
- `center_window()`, `apply_rounded_corners()`
- `show_and_focus()`, `hide()`, `hide_now()`
- Single instance guard
- `NativeOverlayShell::create()` and public methods

#### `painting.rs` (~1,000 lines)
- `draw_panel_background()` — panel background + border
- `draw_list_row()` — owner-drawn listbox rows
- `paint_edit_placeholder()` — placeholder text overlay
- `paint_edit_command_prefix()` — ">" prefix + badge
- `paint_help_tip()` — help tooltip popup
- `paint_footer_hint()` — footer hints
- `WM_PAINT`, `WM_CTLCOLOR*` handlers
- `WM_DRAWITEM`, `WM_MEASUREITEM` handlers
- Font creation helpers (`create_font()`)
- GDI resource management (RAII wrappers for brushes, fonts, pens)

#### `layout.rs` (~500 lines)
- `layout_children()` — positions all child windows
- `compute_input_text_rect()` — edit control text area
- Row height calculation, listbox sizing
- `initial_visible_row_count()` — adaptive row count
- `target_top_index_for_selection()` — scroll positioning

#### `input.rs` (~500 lines)
- `control_subclass_proc()` — subclassed WNDPROC for edit, list, help controls
- Keyboard handling (arrows, enter, escape, backspace, char input)
- Mouse handling (hover, click, wheel)
- `handle_wheel_input()` — smooth scrolling with delta accumulation
- `hide_input_caret()` — caret suppression
- `is_cursor_over_window()`, `row_is_selectable()`

#### `animation.rs` (~400 lines)
- `WindowAnimation` struct
- `start_window_animation()`, `window_animation_tick()`
- `animate_show()`, `animate_results_height()`
- `results_content_animation_tick()`
- `command_badge_animation_tick()`
- `TIMER_*` constants

#### `tray.rs` (~300 lines)
- Tray icon creation (`add_tray_icon`, `remove_tray_icon`)
- Tray context menu (`show_tray_context_menu`)
- Icon loading (`load_tray_icon_handle`)
- `update_tray_icon()` for game mode / hotkey status

#### `icon_cache.rs` (~300 lines)
- Icon cache (`HashMap<String, isize>`) + LRU `VecDeque`
- `load_icon_for_path()` — `ExtractIconExW` / `SHGetFileInfoW`
- `clear_icon_cache()`, `schedule_icon_cache_idle_cleanup()`
- Cache metrics tracking

#### `mod.rs` (~200 lines)
- Re-exports `NativeOverlayShell`, `OverlayEvent`, `OverlayRow`, `OverlayRowRole`
- Exports `is_instance_window_present()`, `signal_existing_instance_*()`
- Module declarations

---

## 3. Refactoring Strategy — Status

### Phase 1 — Extract types and constants ✅
1. Create `types.rs` — move all `const` tokens, enums, palette definitions ✅
2. Create `state.rs` — move `OverlayShellState` ✅
3. Update all references ✅

### Phase 2 — Extract painting ✅
1. Create `painting.rs` — move `draw_panel_background`, `draw_list_row`, `paint_edit_*`, `paint_help_tip`, `paint_footer_hint` ✅
2. Create GDI RAII wrappers (`GdiBrush`, `GdiFont`, `GdiIcon`) ⏳ **Deferred** — see Section 4 notes
3. Move `WM_PAINT`, `WM_CTLCOLOR*`, `WM_DRAWITEM`, `WM_MEASUREITEM` handlers ✅
4. Ensure all painting functions take state by reference ✅

### Phase 3 — Extract layout ✅
1. Create `layout.rs` ✅
2. Move `layout_children()`, `compute_input_text_rect()`, all sizing helpers ✅
3. Extract animation-related layout from `animation.rs` ✅

### Phase 4 — Extract input handling ✅
1. Create `input.rs` ✅
2. Move `control_subclass_proc()` ✅
3. Move keyboard/mouse/wheel handling ✅
4. Extract hover tracking logic ✅

### Phase 5 — Extract animation, tray, icon cache ✅
1. Create `animation.rs`, `tray.rs`, `icon_cache.rs` ✅
2. Move corresponding code ✅
3. Added `icon_loader.rs` for async shell icon loading (not in original plan) ✅

### Phase 6 — Simplify `mod.rs` and `window.rs` ✅
1. Window creation and lifecycle moves to `window.rs` ✅
2. Public `NativeOverlayShell` API stays in `mod.rs` ✅
3. Clean up remaining `extern "system"` dispatch in `overlay_wnd_proc` by routing to module handlers ✅

---

## 4. GDI Resource Management

The current code manages long-lived GDI objects (brushes, fonts, pens) in `OverlayShellState` fields with manual creation in `WM_CREATE` and cleanup in `cleanup_state_resources()`. This is correct and not leaking, but would benefit from RAII wrappers (`GdiBrush`, `GdiFont`, `GdiIcon` implementing `Drop`) for additional safety.

**Status:** ⏳ Deferred — low priority since current cleanup is correct. Would require changing all `isize` fields in `OverlayShellState` to wrapper types, touching `state.rs`, `window.rs`, `painting.rs`, `layout.rs`, `icon_cache.rs`, and `input.rs`.

```rust
struct GdiBrush(isize);

impl GdiBrush {
    fn create(color: u32) -> Self {
        Self(unsafe { CreateSolidBrush(color) } as isize)
    }
}

impl Drop for GdiBrush {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { DeleteObject(self.0 as _); }
        }
    }
}
```

---

## 5. Testing Strategy

After refactoring, each module should be independently testable. Current test coverage:

| Module | Test Approach | Status |
|--------|--------------|--------|
| `types.rs` | Unit test theme detection, palette selection, wide string helpers | ✅ 11 tests |
| `layout.rs` | Unit test sizing calculations, row counts, scroll targets | ✅ 12 tests |
| `animation.rs` | Unit test animation curve, progress calculation | ✅ 8 tests |
| `icon_cache.rs` | Unit test LRU eviction, cache metrics, icon codepoints | ✅ 11 tests |
| `painting.rs` | Snapshot-based visual regression test (future) | ⏳ Not started |
| `input.rs` | Behavior tests via mocked HWND (future) | ⏳ Not started |
| `window.rs` | Integration test via actual window creation (future) | ⏳ Not started |
