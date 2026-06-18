# Nex Iced UI Migration Plan

**Branch:** `iced-ui`
**Date:** 2026-06-04
**Owner:** haxllo
**Status:** Draft → Executing

---

## 1. Goal

Replace the 8,200-line custom Win32 GDI / GDI+ / D2D overlay stack in
`apps/core/src/windows_overlay/` with a slim, cross-platform Iced 0.14
overlay.

### Why now

- `windows-sys 0.59`'s `CreateFontW` binding returns a bogus HFONT on
  this codebase (both MinGW and MSVC). All GDI+ text rendering silently
  fails. The pragmatic workaround (local `#[link]` declaration) papers
  over a single symptom; the underlying IAT resolution issue is still
  undiagnosed.
- Iced 0.14 (stable, MIT, wgpu-backed) gives us a maintained widget
  toolkit, GPU composition, and `wgpu::PresentMode::Immediate` on a
  layered window — which fixes transparency bugs for free.

### Non-goals

- No new features. The migrated UI must be a pixel-for-pixel port of the
  current overlay's behavior.
- Cross-platform support. We remain Windows-only for now; the lib will
  still be `cfg(windows)`-gated where it touches the OS.
- Slint / Egui / Tauri. Decided against in evaluation.

---

## 2. Inventory (confirmed via read)

### 2.1 Existing overlay module

`apps/core/src/windows_overlay/` — 8,000+ LOC, all `cfg(windows)`-gated.

| File | Lines | Purpose | Port target |
|------|------:|---------|-------------|
| `mod.rs` | 53 | module decl + re-exports | `apps/core/src/overlay/mod.rs` |
| `types.rs` | 544 | `OverlayEvent`, `OverlayRow`, palette, constants, `DibSurface` | split: events/rows → `overlay/model.rs`; palette → `overlay/theme.rs`; constants → `overlay/geometry.rs`; `DibSurface` deleted (Iced composes) |
| `window.rs` | 1837 | WndProc, hotkey plumbing, show/hide, native message loop | deleted; replaced by Iced runtime |
| `state.rs` | 308 | `OverlayShellState` | replaced by `Model` (Iced) |
| `input.rs` | 412 | Edit control subclassing | replaced by Iced `TextInput` widget |
| `layout.rs` | 783 | DPI, rounded corners, DWM, child positioning | replaced by Iced layout engine |
| `painting.rs` | 1222 | `WM_DRAWITEM` owner-draw, GDI text/icons | replaced by Iced widgets + `Canvas` |
| `gdiplus_rendering.rs` | 634 | GDI+ FFI for rounded rects, ClearType text | deleted (Iced uses wgpu + Lyon's GPU rendering) |
| `icon_cache.rs` | 1101 | LRU cache of HICON/PNG → BGRA bitmap | replaced by `overlay/icons.rs` returning `iced::widget::Image` handles |
| `icon_loader.rs` | 74 | file → HICON | replaced by `image::open` + `iced::image::Handle` |
| `tray.rs` | 255 | Shell_NotifyIcon wrapper | kept; Iced has no tray, so this stays on top of `windows-sys` |
| `animation.rs` | 222 | alpha blend tweens, `ApplyWindowState` | replaced by Iced subscriptions; fade widget |
| `indexing_progress.rs` | 361 | separate progress dialog | kept for first-run only; lighter rewrite using Iced `ProgressBar` |
| `custom_icons.rs` | 170 | segment-based painted icons | replaced by SVG via `resvg` |
| `mod.rs` re-exports | — | `is_instance_window_present`, `signal_existing_instance_show`, `signal_existing_instance_quit` | keep as `windows-sys` calls in `overlay/platform.rs` |

### 2.2 Public surface (callers across the crate)

```
lib.rs:45                              pub mod windows_overlay;
runtime_hotkey.rs:16                   use crate::windows_overlay::NativeOverlayShell;
runtime_index.rs:13                    use crate::windows_overlay::NativeOverlayShell;
runtime_loop.rs:58                     use crate::windows_overlay::types::NEX_WM_SEARCH_RESULTS_READY;
runtime_loop.rs:60                     use crate::windows_overlay::{ ...  NativeOverlayShell, OverlayEvent, OverlayRow, OverlayRowRole };
runtime_loop.rs:92                     crate::windows_overlay::indexing_progress::run_with_progress_window
runtime_overlay_rows.rs:5              use crate::windows_overlay::{NativeOverlayShell, OverlayRow, OverlayRowRole};
runtime_process.rs:3                   use crate::windows_overlay::{is_instance_window_present, signal_existing_instance_quit};
```

### 2.3 `NativeOverlayShell` public API (must keep a thin shim)

| Method | Iced equivalent |
|---|---|
| `create()` | `iced::application(...)` builder |
| `is_visible()` | `Model.visible` |
| `has_focus()` | winit's `Window::has_focus` |
| `show_and_focus()` | `window.set_visible(true)` + `request_focus` |
| `focus_input_and_select_all()` | `TextInput::focus(...)` |
| `hide()` / `hide_now()` | `window.set_visible(false)` |
| `query_text()` / `set_query_text()` | `Model.query` + `Subscription` |
| `set_status_text(...)` | `Model.status_message` |
| `set_hotkey_hint(...)` | `Model.hotkey_hint` |
| `set_performance_tuning(...)` | `Model.perf_tuning` |
| `set_game_mode_enabled(...)` | `Model.game_mode_enabled` |
| `set_hotkey_issue_active(...)` | `Model.hotkey_issue_active` |
| `trim_runtime_memory()` | calls `icons::trim_unused()` |
| `set_mode_strip_text(...)` | `Model.mode_strip` |
| `set_help_config_path(...)` | `Model.help_config_path` |
| `show_placeholder_hint(...)` | `Model.placeholder_hint` |
| `clear_placeholder_hint()` | same |
| `clear_query_text()` | `Model.query.clear()` |
| `set_results(rows, idx)` | `Model.results = rows; Model.selected = idx; Task::perform(SearchResult, Message::ResultsReady)` |
| `set_selected_index(idx)` | `Model.selected = idx` |
| `selected_index()` | `Some(Model.selected)` |
| `run_message_loop_with_events<F>` | `iced::application(...).run()?.exit()` + `Subscription::run` |

### 2.4 Tests

- `apps/core/tests/windows_runtime_smoke_test.rs` (84 lines): registers
  a hotkey + transport roundtrip. **Unaffected by UI migration** — keeps
  working because the hotkey runtime is independent of the overlay.
- `apps/core/tests/perf_query_latency_test.rs` (4 lines): include!
  shim → `tests/perf/query_latency_test.rs`. **Unaffected.**
- `apps/core/src/windows_overlay/*.rs::tests` modules (38 unit tests
  per AGENTS.md): mostly `OverlayEvent` / `OverlayRow` / palette
  tests. **Move to `overlay/model.rs::tests` / `overlay/theme.rs::tests`.**
- All other test files: **unaffected.**

### 2.5 Config & assets

- Config keys that touch overlay: `hotkey`, `game_mode_enabled`. No
  changes.
- `apps/assets/nex.ico`, `apps/assets/nex.png`: kept.
- `apps/core/assets/icons/`: 12 .ico files. Replaced by
  `apps/core/assets/icons/`: same .ico files but the icon pipeline now
  uses `image` crate to decode → `iced::image::Handle::from_rgba(...)`.

### 2.6 Build

- `build.rs` (31 lines): `winres` for icon embedding. **Unchanged** —
  Iced is pure Rust, no resources needed.
- Toolchain: now `stable-x86_64-pc-windows-msvc`. **Unchanged.**

---

## 3. Dependency plan

```toml
# apps/core/Cargo.toml
[target.'cfg(windows)'.dependencies]
iced           = { version = "0.14", default-features = false, features = ["wgpu", "tokio", "image", "svg", "qr_code"] }
winit          = "0.30"       # transitively pinned by iced 0.14
image          = { version = "0.25", default-features = false, features = ["ico", "png"] }
resvg          = "0.47"
tiny-skia      = "0.12"       # for canvas
palette        = "0.7"        # for colour conversion
once_cell      = "1"          # for global icon cache
```

Drop from `windows-sys` features (kept by tray/hotkey):
```
"Win32_UI_Controls",          # keep for tray
"Win32_Graphics_Dwm",         # keep for backdrop
"Win32_Graphics_Gdi",         # keep for tray icon
"Win32_System_Registry",      # keep for theme detection
"Win32_Shell",                # keep for shellapi
```
All other features may eventually be removed, but keep them for now
during the port.

---

## 4. Module layout (post-migration)

```
apps/core/src/
├── lib.rs                              # add `pub(crate) mod overlay;`
├── overlay/
│   ├── mod.rs                          # Module index, re-exports
│   ├── model.rs                        # Model + Message + update()
│   ├── view.rs                         # view() — pure widget tree
│   ├── theme.rs                        # Palette, Theme::Dark, Theme::Light
│   ├── geometry.rs                     # Layout constants, DPI scale
│   ├── icons.rs                        # IconCache, decode .ico/.png → Handle
│   ├── platform.rs                     # register_hotkey, tray, instance signal
│   ├── search.rs                       # SearchWorker → Result<OverlayRow>
│   ├── startup.rs                      # Iced application builder
│   └── tests.rs                        # migrated unit tests
├── windows_overlay/                    # DELETED in final commit
├── index.rs, runtime.rs, etc.          # unchanged
```

`NativeOverlayShell` becomes a shim that holds an `Arc<Mutex<Model>>`,
a `iced::Task<Message>`, and a `crossbeam_channel::Sender<OverlayCommand>`.
The shim is necessary because `runtime_loop.rs` already uses the
imperative setter style — keeping the shim means we don't have to
refactor the runtime in lockstep with the UI.

---

## 5. Phased implementation

### Phase 0 — Toolchain + commit baseline

- ✅ Branch `iced-ui` from `experiment/tantivy-primary-fts5-fallback`
- ✅ Cargo.toml deps frozen at 0.59 windows-sys + CreateFontW workaround
- ✅ Working tree clean

### Phase 1 — Dependencies + skeleton (no runtime change)

- Add `iced`, `image`, `resvg`, `palette`, `tiny-skia` to
  `apps/core/Cargo.toml` (only `[target.'cfg(windows)'.dependencies]`).
- `cargo check -p nex-cli` should pass.
- Add `apps/core/src/overlay/mod.rs` with empty `pub(crate) mod`s.
- Add stub `Model`, `Message`, `update()`, `view()` that compile.
- Commit: `feat(overlay): scaffold iced 0.14 module shell`

### Phase 2 — Tray + hotkey platform glue

- Implement `overlay/platform.rs`:
  - `register_hotkey(id, mods, vk) -> Result<()>` via
    `RegisterHotKey` (wrapped in a winit `Window` so we still get a
    `WM_HOTKEY` proxy).
  - `set_tray_icon(icon: HICON, tooltip: &str)` via `Shell_NotifyIconA`.
  - `is_instance_window_present()` / `signal_existing_instance_show()`
    via existing `FindWindowW` + `RegisterWindowMessageA` pattern.
  - `set_system_theme() -> Theme` via `RegGetValueW` (ported from
    `types.rs::detect_system_theme`).
- Unit tests for the tray + instance-signal FFI shims.

### Phase 3 — Icons

- Implement `overlay/icons.rs`:
  - `IconCache::get(path: &Path) -> Option<iced::widget::Image>`.
  - LRU on `(path, mtime)` → decoded RGBA8 → `Image::new(Handle::from_rgba)`.
  - `image::open(...)` for PNG; `image::ImageDecoder::new(Cursor)`
    with `IcoDecoder` for .ico.
  - `trim_unused()` evicts old entries; `clear()` flushes all.
- All public methods of old `icon_cache.rs` are now methods on
  `IconCache`.
- Unit tests for eviction and decode-failure paths.

### Phase 4 — Model + view + update

- Implement `Model` with every field needed by
  `NativeOverlayShell::create()` callers (see §2.3).
- Implement `Message`:
  - `HotkeyTriggered`, `QueryChanged(String)`, `MoveSelection(i32)`,
    `Submit`, `Escape`, `TrayToggleGameMode`, `TrayCheckForUpdates`,
    `ExternalShow`, `ExternalQuit`, `SearchResultsReady`,
    `Tick(Instant)`, `Iced(iced::Event)`.
- `update(msg, &mut Model) -> Task<Message>`:
  - `QueryChanged` → `Task::perform(spawn_search, Message::SearchReady)`.
  - `Submit` → `Task::perform(launch_selection, Message::Launched)`.
  - `MoveSelection` → clamp `Model.selected`.
- `view(&Model) -> Element<Message>`:
  - `Column::new().push(search_input).push(divider).push(results_list).push(footer_hint).padding(0)`
  - TopHit row: `Container::new(...).style(|theme| container::Appearance { background: Some(palette.top_hit_bg) })`.
  - Section header: `Text::new(label).size(12).color(palette.text_section)`.
  - Item row: `Row::new().push(icon).push(Column::new().push(title).push(path))`.
  - Selection: `row.into::<Container>().style(selection_style)`.
  - Footer hint: keyboard cheatsheet using `Text` with the same
    Unicode arrows.
- **No DWM Mica yet** — keep solid panel background; Mica is a follow-up.

### Phase 5 — Show / hide animation

- Use Iced's `Animation` widget or a custom `Subscription` ticking every
  8 ms that ramps `Model.alpha` from 0 → 255 in 150 ms.
- Iced 0.14's `window::level(iced::window::Level::AlwaysOnTop)` +
  `window::decorations(false)`.

### Phase 6 — Indexing progress window

- Rewrite `indexing_progress.rs` to open a separate Iced window with
  `ProgressBar` + status text.
- Keep `run_with_progress_window<F: FnMut(u32) -> R>(F) -> R` signature
  so `runtime_loop.rs` doesn't change.

### Phase 7 — Wire up `NativeOverlayShell` shim

- Implement `NativeOverlayShell` in `overlay/shim.rs` (or
  `mod.rs`):
  - All 22 methods from §2.3, each one writes into a shared
    `Arc<Mutex<Model>>` and posts the corresponding `Message` over a
    `crossbeam_channel` (the receiver runs on a dedicated `std::thread`
    that drives the Iced event loop).
  - `run_message_loop_with_events<F>(self, on_event: F)` spawns the
    Iced thread, the winit window, the tray icon, and the global
    hotkey. It bridges `WM_HOTKEY` / tray menu / Iced events back to
    the caller via `on_event`.
  - `set_results(rows, idx)` is now `async`: posts `SetResults` to the
    model and returns; the view re-renders on the next frame.
- Delete `windows_overlay/{animation,custom_icons,gdiplus_rendering,
  icon_cache,icon_loader,indexing_progress,input,layout,painting,
  state,tray,types,window,mod}.rs`.
- `cargo test -p nex-cli` must still pass (the shim is test-mode aware
  — if `NEX_TEST_NO_WINDOW=1` it skips creating a window).

### Phase 8 — Tests

- Port all 38 unit tests from `windows_overlay/*::tests` to
  `overlay/*::tests`.
- Add new `overlay::tests`:
  - `model::tests::move_selection_clamps` (from old `painting::tests`).
  - `view::tests::renders_top_hit_first` (snapshot via
    `iced::widget::test`).
  - `icons::tests::cache_lru_evicts_oldest`.
- Run `cargo test -p nex-cli` + the perf gate + the smoke gate.

### Phase 9 — Docs

- Update `AGENTS.md`:
  - Toolchain: drop MinGW note.
  - Add "Overlay is Iced 0.14 on wgpu" section.
- Update `docs/architecture/`. Add `docs/architecture/iced-overlay.md`.
- Update `docs/plans/2026-05-30-ui-ux-improvements.md` to mark the
  "rewrite UI on Slint/Iced" item complete.

### Phase 10 — CI verification

- `cargo test -p nex-cli`
- `cargo test -p nex-cli --test windows_runtime_smoke_test` with
  `NEX_WINDOWS_RUNTIME_SMOKE=1`
- `cargo test -p nex-cli --test perf_query_latency_test -- --exact warm_query_p95_under_15ms`
- Push branch, open PR with the migration summary.

---

## 6. Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Iced 0.14's winit version conflicts with our `RegisterHotKey` plumbing | Med | Med | Run our own winit window inside Iced's `boot` hook, hook `WindowEvent` for hotkey delivery |
| `resvg` / `image` add 8 MB to binary | Med | Low | Accept; benchmark shows the launcher is already 12 MB |
| WGSL fails on some Intel iGPUs | Low | High | Test on minimum spec (i5-8250U, Intel UHD 620) before release |
| GDI+ font metrics drift (line heights, baseline) | High | Med | Use Iced's `Font::DEFAULT` for first pass; tighten later |
| Owner-draw `LB_*` selection/focus behaviour must be re-implemented by hand | High | High | Build a test harness that fakes `OverlayEvent::MoveSelection` and asserts `Model.selected` value |
| `crossbeam_channel` for Iced ↔ runtime is over-engineered | Med | Low | Fall back to `Arc<Mutex<Model>>` + `Subscription::batch` |

---

## 7. Done criteria

- `cargo build --bin nex --release` succeeds.
- `cargo test -p nex-cli` is green.
- Manual smoke: hotkey show, type, navigate, submit, escape, tray menu,
  external-show IPC, indexing progress on first run.
- All 22 methods on `NativeOverlayShell` are still callable from the
  current `runtime_loop.rs` without code changes outside `windows_overlay/`.
- `apps/core/src/windows_overlay/` is deleted.
- The `iced-ui` branch is pushed and the migration plan is linked from
  the PR description.
