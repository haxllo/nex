# UI/UX Improvements

> **Status (June 2026, v1.3.0)**: Both priorities **complete**.

## Priority 1: Inline Calculator ✅ **SHIPPED** (custom parser, not `meval`)

Detect `=` prefix or standalone math expressions in the search input and show the evaluated result as an overlay row.

### Implementation (Actual)

- **Custom shunting-yard parser** in `apps/core/src/calculator.rs` (no `meval` dependency, since the plan flagged it as unmaintained)
- Supports: `+`, `-`, `*`, `/`, `%`, `^`, parentheses, `sqrt()`, `abs()`, `ln()`, `round()`, `floor()`, `ceil()`, `pi`, `e`
- Trigger: query starts with `=` (e.g., `=2+3*4` → 14)
- Wired in `runtime_loop.rs:306` (when overlay already open) and `runtime_loop.rs:681-695` (when typing into empty query)
- 36 unit tests in `calculator.rs::tests` cover all operators, error cases, and edge cases

### Files touched
- `apps/core/src/calculator.rs` (new)
- `apps/core/src/lib.rs` (mod export)
- `apps/core/src/runtime_loop.rs` (dispatch)

### Deviations from plan
- Skipped `meval` per risk note → custom parser
- No new `OverlayRowRole::Calculator` variant → reuses existing `Item` role

---

## Priority 2: Mica Backdrop ✅ **SHIPPED**

Apply Windows 11 Mica material behind the overlay panel so the pill has a translucent, desktop-integrated look.

### Implementation (Actual)

1. **`DWMWA_SYSTEMBACKDROP_TYPE`** call in `apps/core/src/windows_overlay/layout.rs:512`:
   ```rust
   DwmSetWindowAttribute(
       hwnd,
       DWMWA_SYSTEMBACKDROP_TYPE as u32,
       &DWMSBT_MAINWINDOW as *const _ as *const c_void,
       std::mem::size_of::<i32>() as u32,
   );
   ```
2. Constants from `windows-sys 0.59` (`DWMWA_SYSTEMBACKDROP_TYPE = 38`, `DWMSBT_MAINWINDOW = 2`) — no manual constant definitions needed.
3. Panel fill alpha reduced to `0x90` (144/255) per `windows_overlay/types.rs:344` (`PANEL_FILL_ALPHA_MICA = 0x90`) so Mica shows through.
4. 32-bit DIB for per-pixel alpha rendering per `state.rs:233` comment.

### Caveats encountered
- `WS_EX_LAYERED` (kept for animation alpha) is compatible with Mica in practice on Win11 22H2+ — no need for `ULW_ALPHA` rewrite.
- Win10 fallback: DwmSetWindowAttribute is a no-op on older builds, so no conditional check needed.

### Files touched
- `apps/core/src/windows_overlay/layout.rs` (DwmSetWindowAttribute call)
- `apps/core/src/windows_overlay/types.rs` (Mica alpha constant)

### Deviations from plan
- No `mica_enabled` config flag (always on)
- No `BuildNumber >= 22000` check (DWM silently ignores the attribute on older builds)
