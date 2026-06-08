# Nex — Requirements

**Version:** 1
**Date:** 2026-06-08
**Status:** Draft

## Scope

This document covers the next phase of Nex development: completing the Iced 0.14
UI migration, stabilizing the codebase, and building the snippets/text-expansion
feature. All requirements assume the Iced migration is the foundation — new
features ship on top of the new overlay.

---

## R1: Complete Iced 0.14 UI Migration

### R1.1 — Pixel-for-pixel port
The Iced overlay must render identically to the legacy Win32 overlay for:
- Input field (placeholder text, search icon, command prefix)
- Result rows (icon, title, path, selection highlight, section headers)
- Footer hint (keyboard shortcuts cheatsheet)
- Help tooltip (config path display)
- Mode strip (current search mode indicator)
- Indexing progress window (progress bar + status text)

### R1.2 — Animation parity
- Show: alpha fade 0→255 over 150ms with quadratic ease-out
- Height expansion: smooth grow to full panel height (110ms)
- Results content fade: 120ms listbox-only invalidation
- Hide: immediate or animated (configurable)
- Loading spinner: braille character cycle (96ms interval)

### R1.3 — Hotkey integration
- Global hotkey registration on dedicated thread (unchanged from legacy)
- `WM_HOTKEY` → `OverlayEvent` forwarding via crossbeam_channel
- Hotkey conflict detection and user notification

### R1.4 — System tray
- `Shell_NotifyIcon` tray icon with right-click context menu
- Menu items: Show/Hide, Game Mode toggle, Check for Updates, Quit
- Must work outside Iced (pure Win32, unchanged from legacy)

### R1.5 — Instance signaling
- `FindWindowW` + `RegisterWindowMessageA` for single-instance enforcement
- `is_instance_window_present()`, `signal_existing_instance_show()`,
  `signal_existing_instance_quit()` must keep identical signatures

### R1.6 — NativeOverlayShell shim
- All 22 public methods must work identically
- No changes required in `runtime_loop.rs`, `runtime_overlay_rows.rs`,
  `runtime_hotkey.rs`, `runtime_process.rs`

### R1.7 — Build & test compatibility
- `cargo build --bin nex --release` succeeds
- `cargo test -p nex-cli` all green
- Windows runtime smoke test passes
- Perf query latency gate passes (p95 under 15ms warm)
- No regressions in config loading, migrations, or CLI commands

### R1.8 — Legacy code removal
- After validation, delete `apps/core/src/windows_overlay/`
- Remove unused dependencies from `Cargo.toml`

---

## R2: Stability & Bug Fixes

### R2.1 — Crash resilience
- Graceful error recovery in search worker (Tantivy panics)
- Graceful error recovery in icon loading (corrupt .ico files)
- Hotkey thread crash detection and restart
- Config parse errors show user-friendly message, don't crash

### R2.2 — Rendering correctness
- No flickering on show/hide transitions
- No GDI+/GDI desync artifacts
- Text rendering consistent across DPI scales
- Icon rendering correct for all supported formats (.ico, .png)

### R2.3 — Windows integration
- Correct startup behavior (launch at login option)
- Hotkey conflict resolution (show which app stole the hotkey)
- Multi-monitor: overlay appears on monitor with cursor focus
- Correct DPI scaling on mixed-DPI setups

### R2.4 — Config & migration
- Config migrations apply cleanly across all version jumps
- No silent config corruption on write
- Clear error messages for malformed config values

---

## R3: Performance & Architecture

### R3.1 — Indexing performance
- Initial index build: target under 30s for 100K items
- Incremental updates: target under 500ms
- No UI freeze during indexing (background thread)

### R3.2 — Memory footprint
- Idle memory: target under 50MB
- Active search memory: target under 100MB
- Icon cache: LRU eviction with configurable max size
- Tantivy index: periodic compaction

### R3.3 — Search quality
- Fuzzy matching: prioritize exact prefix matches, then substring, then fuzzy
- Ranking: app launches weight higher than file matches
- Typo tolerance: 1-2 character errors for queries ≥ 3 chars

### R3.4 — Architecture cleanup
- Remove dead code (D2D references, unused modules)
- Consistent error handling (no unwrap() in production paths)
- Reduce module coupling between overlay, search, and runtime

---

## R4: Snippets / Text Expansion

### R4.1 — Snippet storage
- Store snippets in config or a dedicated file
- Each snippet: trigger keyword → expanded text
- Support for multi-line snippets
- Support for dynamic snippets (date, time, clipboard)

### R4.2 — Snippet matching
- Prefix-triggered: type trigger keyword, press Tab/Enter to expand
- Inline matching: detect trigger keyword anywhere in query
- Case-sensitive and case-insensitive modes

### R4.3 — Snippet expansion
- Replace trigger text with expanded content in active application
- Use SendInput or similar for keystroke simulation
- Handle special keys (Enter, Tab) within snippet text

### R4.4 — Snippet UX
- Snippet icon/indicator in result rows
- Preview of expanded text in result details
- Snippet management UI (add/edit/delete) — TBD phase

---

## R5: UI Polish (cmdk-level)

### R5.1 — Visual quality
- Dark theme: cohesive dark panel with appropriate contrast ratios
- Light theme: cohesive light panel
- Selection highlight: smooth rounded rect with subtle color
- Section headers: muted, distinct from result rows
- Footer: keyboard hints with keycap-style rendering

### R5.2 — Mica backdrop (Windows 11)
- Enable `DWMWA_SYSTEMBACKDROP_TYPE` for Mica material
- Fall back to solid background on Windows 10

### R5.3 — DWM rounded corners
- Enable `DWMWA_WINDOW_CORNER_PREFERENCE` on Windows 11
- Fall back to `CreateRoundRectRgn` on Windows 10

### R5.4 — Typography
- Segoe UI Variable (Windows 11) / Segoe UI (Windows 10)
- Consistent font sizing: 14px input, 13px results, 12px footer
- Proper ClearType rendering where possible

---

## R6: Testing

### R6.1 — Unit test coverage
- Model update logic (move selection, query change, submit)
- Theme palette consistency
- Icon cache eviction
- Config migration correctness

### R6.2 — Integration tests
- Hotkey registration roundtrip (existing smoke test)
- Search query latency (existing perf test)
- Config load/save roundtrip
- Instance signal roundtrip

### R6.3 — Manual test checklist
- Hotkey show/hide on primary monitor
- Hotkey show/hide on secondary monitor
- Type, navigate with arrows/ctrl+j/k, submit with Enter
- Escape to dismiss
- Tray menu: all items functional
- `nex --status`, `nex --quit`, `nex --restart`
- First-run indexing progress window
- Config migration from old versions
