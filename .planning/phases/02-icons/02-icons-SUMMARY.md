---
phase: 2
plan: icons
subsystem: overlay
tags: [icons, view, iced, caching]
dependencies:
  requires: [01-platform-glue]
  provides: [icon-rendering]
  affects: [overlay-view, overlay-boot, runtime-loop]
tech-stack:
  added: []
  patterns: [Arc<IconCache> threading, LRU icon cache, owned Handle widgets]
key-files:
  created: []
  modified:
    - apps/core/src/overlay/icons.rs
    - apps/core/src/overlay/view.rs
    - apps/core/src/overlay/boot.rs
    - apps/core/src/overlay/shim.rs
    - apps/core/src/runtime_loop.rs
decisions:
  - "Pass IconCache as separate parameter to view() rather than embedding in Model (keeps Model Clone-friendly)"
  - "Use IconCache::get_image() wrapper around resolve() for cleaner view code"
  - "Wire IconCache through Arc<> chain: NativeOverlayShell -> Boot -> State -> ViewFn"
metrics:
  duration: "~20 minutes"
  completed-date: 2026-06-08
---

# Phase 2 Plan Icons: Icons & Assets Summary

Wired the LRU `IconCache` (ICO/PNG loading, trim, clear) into the Iced
view layer, replacing empty rectangle placeholders with actual file-based
icon rendering.

## What was built

- **`icons.rs`**: Made `IconCache` struct public; added `get_image()` method
  that returns `Option<iced::widget::Image>` from a file path
- **`view.rs`**: Modified `view()`, `row_view()`, and `result_row_widget()`
  to accept `&IconCache`; replaced `icon_placeholder()` with `result_icon()`
  that resolves icon paths via `IconCache::get_image()` and renders 32x32
  `Image` widgets; falls back to a colored rectangle when path is empty
  or file is not found
- **`boot.rs`**: Added `icon_cache: Arc<IconCache>` to both `Boot` and
  `State` structs; wired through `State::boot()` and `ViewFn` to pass
  to `build_view()`
- **`shim.rs`**: Added `NativeOverlayShell::icon_cache()` accessor
- **`runtime_loop.rs`**: Passes `overlay.icon_cache()` into the `Boot`
  constructor

## How it works

The `IconCache` is created once in `NativeOverlayShell::create()` and
shared via `Arc`. It flows through:

```
NativeOverlayShell  ──→  Boot  ──→  State  ──→  ViewFn  ──→  view(model, icon_cache)
                                  (Arc clone)    (&State)      result_icon(path, cache, palette)
```

At render time, `result_icon()` calls `IconCache::get_image(path)` which
internally checks the LRU cache (populated by the background `prefetch_rows`
thread on `set_results()`). If the path is empty or decode fails, a
colored placeholder rectangle is rendered instead.

## Build verification

`cargo check -p nex-cli` passes with zero new errors. All warnings are
pre-existing (dead code in legacy modules, unused `Message` variants).

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None. The icon caching and rendering are fully wired end-to-end.

## Threat Flags

None — no new network endpoints, auth paths, or trust boundaries introduced.
