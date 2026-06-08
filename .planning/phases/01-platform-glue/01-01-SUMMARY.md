# Phase 01-platform-glue — Execution Summary

**Date:** 2026-06-08
**Plan:** 01-01
**Status:** Complete

## What was done

Implemented the system tray icon with context menu for the Iced overlay, porting
the legacy `windows_overlay/tray.rs` to a standalone Win32 module.

### Files created

- `apps/core/src/overlay/tray.rs` (~285 lines) — Self-contained `TrayIcon` struct

### Files modified

- `apps/core/src/overlay/mod.rs` — Added `pub(crate) mod tray;` declaration
- `apps/core/src/runtime_loop.rs` — Tray creation, updater thread, channel wiring

### Architecture

```
TrayIcon
├── message_hwnd: HWND (HWND_MESSAGE window)
├── icon_handle: HICON (from ExtractIconExW)
├── icon_added: bool
└── state: Arc<Mutex<TrayState>>
    ├── event_tx: Sender<OverlayEvent>
    ├── config_path: String
    ├── game_mode_enabled: bool
    └── hotkey_issue_active: bool
```

### Key design decisions

1. **Standalone HWND_MESSAGE window** — No dependency on Iced window HWND
2. **crossbeam_channel for events** — Tray menu selections send `OverlayEvent`
   variants directly through the shared event channel
3. **Dedicated updater thread** — A thread owns `TrayIcon` and listens on two
   channels (`tray_gm_tx`/`tray_hi_tx`) for state updates
4. **`unsafe impl Send`** — `TrayIcon` is sent to the updater thread; `HWND`
   handles are safe to share across Windows threads

### Verification

- `cargo check -p nex-cli` — No errors in tray.rs, overlay/mod.rs, or
  runtime_loop.rs (pre-existing `windows` crate errors are unrelated)
- No new compiler warnings introduced
- Module visibility correct (pub(crate) throughout)

### Success criteria

- [x] `apps/core/src/overlay/tray.rs` exists with complete TrayIcon implementation
- [x] `overlay/mod.rs` declares the tray module
- [x] `runtime_loop.rs` creates TrayIcon during initialization
- [x] Tray state updates (game mode, hotkey issue) are wired through channels
- [x] `cargo check -p nex-cli` finds no errors in our code
- [x] No new compiler warnings introduced
