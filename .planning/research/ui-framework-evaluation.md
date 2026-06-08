# UI Framework Evaluation: Iced 0.14 vs Slint for Nex

**Date:** 2026-06-08
**Decision:** Stay with Iced 0.14. Do not migrate to Slint.
**Status:** CONFIRMED — Iced migration already in progress on branch `iced-ui`.

## Context

Nex currently uses ~8,200 lines of custom Win32 GDI/GDI+/DWM rendering code in
`apps/core/src/windows_overlay/`. The codebase has rendering bugs, text-rendering
failures (CreateFontW binding issue), and maintenance burden. The project needs
a modern, maintainable UI layer.

The user asked to evaluate Slint as an alternative to the already-chosen Iced 0.14
migration.

## Slint Analysis

### What Slint is
- Declarative UI framework for Rust/C++/JS with its own `.slint` DSL
- Renders via `femtovg` (OpenGL) or `skia` backends
- Mature for embedded/desktop widgets, smaller ecosystem

### Critical blockers for Nex

| Requirement | Slint Support | Severity |
|---|---|---|
| Transparent/layered popup window (WS_EX_LAYERED) | No direct API; requires winit hacks | BLOCKER |
| Always-on-top z-order (HWND_TOPMOST) | No API; raw HWND manipulation needed | BLOCKER |
| Toolwindow (no taskbar/Alt-Tab) | No API | BLOCKER |
| Per-window alpha fade animation (SetLayeredWindowAttributes) | Not supported | BLOCKER |
| Mica backdrop (DWMWA_SYSTEMBACKDROP_TYPE) | Not possible | HIGH |
| DWM rounded corners (DWMWA_WINDOW_CORNER_PREFERENCE) | Not possible | HIGH |
| Global hotkey registration (RegisterHotKey) | Would need separate Win32 thread anyway | MEDIUM |
| ClearType text rendering | GPU text only (no subpixel) | LOW |
| Virtual scrolling for large result lists | Slint renders all items | MEDIUM |
| Glyph-based icon fonts (Segoe Fluent Icons) | No per-character color control | LOW |

### Slint pros
- Declarative `.slint` DSL with CSS-like theming
- Cross-platform (but Nex doesn't need this today)
- Good widget toolkit (Text, Image, ListView, etc.)

### Slint cons
- Requires separate `.slint` compiler build step
- Would still need all the same Win32 escape hatches for hotkey, tray,
  instance signaling, theme detection, and Mica
- Limited production examples of system-tray/hotkey launcher apps
- Smaller community than Iced

## Iced 0.14 Analysis

### What Iced is
- Pure Rust, Elm-inspired widget toolkit with wgpu rendering backend
- Transparent windows, `AlwaysOnTop`, borderless — all built-in
- Stable, MIT-licensed, actively maintained

### Why Iced wins for Nex

| Criterion | Iced 0.14 | Slint |
|---|---|---|
| Layered/transparent window | `transparent: true` + `decorations: false` — works today | winit hacks required |
| Always-on-top | `Level::AlwaysOnTop` — standard | No direct API |
| Toolwindow (no taskbar) | Via raw HWND in boot | No API |
| GPU rendering | wgpu (Vulkan/DX12/Metal) — proven on Windows | femtovg or skia |
| Build system | Pure Rust, `cargo build` | Requires slint-build |
| Community/examples for launcher use | Yes | Rare |
| Hotkey integration | Dedicated thread (unchanged) — compatible | Same approach possible |
| System tray | Win32 kept outside Iced | Same |
| Mica backdrop | Planned via DWM call in boot | Not possible |
| DWM rounded corners | Planned via DWM call | Not possible |

## Recommendation

**Stay the course with Iced 0.14.** The migration is already in progress
(branch `iced-ui`, partial implementation in `apps/core/src/overlay/`).
Slint would:

1. Add a `.slint` compiler step to the build
2. Still require all the same Win32 escape hatches
3. Have worse Windows transparency/z-order behavior
4. Require learning a separate DSL
5. Be a full rewrite of work already done

The current Iced path is documented in `.planning/plans/iced-migration.md`
(10 phases, phases 0-4 substantially complete).

## Non-goals (confirmed)
- Slint, Egui, Tauri — all rejected in prior evaluation
- Cross-platform — Windows-only for now
- New features during migration — pixel-for-pixel port, features follow after
