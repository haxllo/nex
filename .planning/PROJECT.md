# Nex — Project Context

**Initialized:** 2026-06-08
**Repository:** github.com/haxllo/nex
**Platform:** Windows 10/11 (64-bit)
**Language:** Rust (stable-x86_64-pc-windows-gnu)

## Elevator Pitch

Nex is a keyboard-first launcher for Windows. Press a global hotkey to summon a
floating search bar and quickly find and launch applications, files, folders,
and custom actions. Built in Rust for minimal memory footprint and near-instant
responsiveness.

## Current State

Nex is a functional Windows launcher with:
- Global hotkey (Alt+Space) to summon overlay
- Fuzzy search across indexed apps, files, folders
- Actions, web search, clipboard history (optional)
- Plugin system scaffolding, game mode
- TOML config at `%APPDATA%\Nex\config.toml`
- Tantivy-powered FTS index at `%APPDATA%\Nex\index.sqlite3`
- Winget distribution, cargo install, binary releases

The overlay UI is built on ~8,200 lines of custom Win32 GDI/GDI+/DWM code.
An active migration to Iced 0.14 is in progress on branch `iced-ui` (phases 0-4
substantially complete).

## Vision

Expand and mature Nex beyond the current README description. The core launcher
concept is sound — the focus is on quality, polish, and performance. The UI
should feel native, responsive, and visually clean (cmdk/raycast aesthetic).
Snippets/text expansion is the next major feature, built atop the stable Iced
overlay.

## Top Priorities

1. **Stability & Polish** — Fix rendering bugs, crash resilience, edge cases
2. **Performance & Architecture** — Optimize indexing, reduce memory/CPU, clean
   up the architecture
3. **Better UI** — Complete the Iced migration, achieve cmdk-level visual polish
4. **Snippets** — Build a snippets/text expansion system

## Known Pain Points

| Area | Issue |
|---|---|
| Overlay rendering | GDI+/GDI rendering glitches, flickering, CreateFontW binding failures |
| Search quality | Fuzzy matching could be smarter, ranking needs work |
| Indexing performance | File/app discovery is slow or resource-heavy |
| Memory/CPU | Idle resource usage higher than ideal |
| Crash resilience | Missing error recovery, poor edge-case handling |
| Windows integration | Startup behavior, hotkey conflicts, multi-monitor issues |
| Testing gaps | Insufficient test coverage, CI flakiness |
| Config/migration | Config versioning and migration complexity |

## Tech Stack

- **Rust** (stable) — entire codebase
- **Iced 0.14** — overlay UI (migrating from custom Win32)
- **Tantivy** — full-text search indexing
- **windows-sys 0.59** — Win32 bindings (hotkey, tray, DWM, shell)
- **winit 0.30** — windowing (via Iced)
- **wgpu** — GPU rendering backend (via Iced)
- **crossbeam** — channel-based inter-thread communication
- **TOML** — config format (primary); JSON/JSON5 backward compat

## Non-Goals

- Cross-platform support (remain Windows-only for now)
- Slint, Egui, Tauri — evaluated and rejected
- Browser-based UI
- Breaking changes to the config format or public CLI surface

## Architecture Overview

```
apps/core/src/
├── main.rs                    # Binary entry → nex_core::runtime::run_with_options
├── lib.rs                     # Library entry, module declarations
├── runtime.rs                 # Main runtime orchestrator
├── runtime_loop.rs            # Event loop, search worker bridge
├── runtime_overlay_rows.rs    # Result row construction
├── runtime_hotkey.rs          # Hotkey registration/management
├── runtime_index.rs           # Tantivy index management
├── search.rs                  # Search indexing & querying
├── discovery.rs               # File/app discovery
├── config.rs                  # Config loading, migrations, templates
├── logging.rs                 # Logging infrastructure
├── overlay/                   # NEW — Iced 0.14 overlay (migrating)
│   ├── boot.rs                # Iced application boot, State, visibility
│   ├── view.rs                # Pure widget tree
│   ├── model.rs               # Model, Message, update(), OverlayEvent
│   ├── theme.rs               # Dark/light palettes
│   ├── geometry.rs            # Layout constants
│   ├── icons.rs               # LRU icon cache
│   ├── platform.rs            # Win32 glue (hotkey, tray, instance)
│   ├── shim.rs                # NativeOverlayShell imperative API
│   └── indexing_progress.rs   # First-run progress window
└── windows_overlay/           # LEGACY — to be deleted after migration
    ├── window.rs              # WndProc, window creation, message pump
    ├── painting.rs            # Owner-draw, GDI text/icons
    ├── gdiplus_rendering.rs   # GDI+ FFI (30+ functions)
    ├── animation.rs           # Alpha fade via SetLayeredWindowAttributes
    ├── layout.rs              # DPI, DWM, child positioning
    ├── icon_cache.rs          # LRU HICON cache
    ├── state.rs               # OverlayShellState (60+ fields)
    ├── input.rs               # Edit control subclassing
    ├── tray.rs                # Shell_NotifyIcon wrapper
    └── types.rs               # Constants, palettes, OverlayEvent/Row
```

## Key Dependencies

```toml
# apps/core/Cargo.toml
iced = { version = "0.14", features = ["wgpu", "tokio", "image", "svg"] }
windows-sys = { version = "0.59", features = [...] }
tantivy = "0.22"
crossbeam = "0.8"
toml = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
image = "0.25"
resvg = "0.47"
```

## Documentation

- `AGENTS.md` — agent reference (build, test, architecture)
- `docs/README.md` — architecture notes
- `CHANGELOG.md` — release notes
- `.planning/plans/iced-migration.md` — detailed Iced migration plan
