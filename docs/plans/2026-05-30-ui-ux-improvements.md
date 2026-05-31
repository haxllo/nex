# UI/UX Improvements

## Priority 1: Inline Calculator

Detect `=` prefix or standalone math expressions in the search input and show the evaluated result as an overlay row.

### Implementation

1. **Add `meval` crate** — zero-dependency Rust math expression parser/evaluator (`cargo add meval`). Supports `+`, `-`, `*`, `/`, `^`, `%`, `sqrt()`, `sin()`, `cos()`, `pi`, `e`, etc.

2. **Detect calculator query** — in `runtime_overlay_rows.rs::overlay_rows()` or the query dispatch path, check if the raw query starts with `=` or is a standalone math expression. If so, evaluate and produce a single `OverlayRow` with role `TopHit` or a new `Calculator` variant (need to add `OverlayRowRole::Calculator`).

3. **Result display** — the row shows the expression (truncated if long) as title, the evaluated result as path/meta with monospace formatting.

4. **Edge cases** — division by zero, invalid syntax, overflow. Show error as a status row instead of crashing.

### Files to touch
- `apps/core/Cargo.toml` — add `meval`
- `apps/core/src/runtime_overlay_rows.rs` — calculator detection + row creation
- `apps/core/src/runtime_loop.rs` — pass raw query for calculator check
- `apps/core/src/windows_overlay/types.rs` — optional `OverlayRowRole::Calculator` variant
- `apps/core/src/windows_overlay/painting.rs` — render calculator rows (same as `Item` but different font/color)
- `apps/core/src/windows_overlay/layout.rs` — optional measurement tweaks

### Risks
- `meval` is unmaintained (last publish 2021). If it fails to compile with current Rust, fall back to a hand-rolled shunting-yard evaluator (small, ~200 lines).

---

## Priority 2: Mica Backdrop

Apply Windows 11 Mica material behind the overlay panel so the pill has a translucent, desktop-integrated look.

### Implementation

1. **Set `DWMWA_SYSTEMBACKDROP_TYPE`** — call `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_MAINWINDOW, sizeof(DWMSBT_MAINWINDOW))` after window creation in `window.rs::create()`.

2. **Add constant** — `DWMWA_SYSTEMBACKDROP_TYPE = 38`, `DWMSBT_MAINWINDOW = 2` (already in windows-sys 0.59 via `Win32_Graphics_Dwm`).

3. **Update panel rendering** — the panel background must become translucent for Mica to show:
   - In `painting.rs::draw_panel_background`, change the fill color alpha from `0xFF` to `0x80`–`0xA0` (or use `Acrylic` blending).
   - The pill border can stay opaque or become semi-transparent.

4. **Theme handling** — Mica has separate dark/light modes. Use `DWMWA_USE_IMMERSIVE_DARK_MODE` to match the system theme or config.

5. **Windows 11 check** — Mica only works on Win11 build 22000+. Use `RtlGetVersion` or check `BuildNumber >= 22000` before applying.

### Layered window caveat
`WS_EX_LAYERED` with `SetLayeredWindowAttributes` (alpha-per-window) may fight with Mica. Two approaches:
- **Option A**: Remove `WS_EX_LAYERED` and use only Mica + `UpdateLayeredWindow` for per-pixel transparency (the `experiment/skia-renderer` branch approach). Mica renders into the non-client area.
- **Option B**: Keep `WS_EX_LAYERED` but use `ULW_ALPHA` with a transparent bitmap where the Mica area should show through.

Option A is cleaner but requires the per-pixel-alpha rendering path to land first.

### Files to touch
- `apps/core/src/windows_overlay/window.rs` — DwmSetWindowAttribute call after CreateWindowExW
- `apps/core/src/windows_overlay/painting.rs` — panel fill alpha, update_window_display global alpha
- `apps/core/src/windows_overlay/types.rs` — Mica constants if not in windows-sys
- `apps/core/src/windows_overlay/state.rs` — optional mica_enabled flag

### Risks
- Mica + layered window interaction is poorly documented. May need fallback to solid color on Win10 or if compositing fails.
- Performance: Mica samples the desktop wallpaper each frame, adding GPU overhead on battery.
