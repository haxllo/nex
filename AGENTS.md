# Nex — Agent Reference

## Build & Test

```bash
cargo build --bin nex                    # debug build
cargo build --release --bin nex          # release build
cargo test -p nex                        # all unit tests (cross-platform)
cargo test -p nex-cli --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
cargo test -p nex-cli --test windows_runtime_smoke_test  # CI-only smoke test
```

`vitest --run` for the JS scaffold gate (just checks file existence). Server-side only, no browser.

**CI order**: `vitest --run` → `cargo test -p nex-cli` → perf gate → smoke gate.

## Release

```bash
# 1. Build portable zip + manifest (requires Rust)
.\scripts\windows\package-windows-artifact.ps1 -Version "x.y.z"

# 2. Build setup.exe (requires Inno Setup 6 at default path)
.\scripts\windows\package-windows-installer.ps1 -Version "x.y.z"

# 3. Upload assets to GitHub Release
gh release create vx.y.z --title "vx.y.z" --notes-file docs/releases/vx.y.z-notes.md
gh release upload vx.y.z artifacts/windows/nex-x.y.z-windows-x64.zip
gh release upload vx.y.z artifacts/windows/nex-x.y.z-windows-x64-setup.exe
gh release upload vx.y.z artifacts/windows/nex-x.y.z-windows-x64-manifest.json
```

## Running

```bash
nex --foreground              # dev mode, keeps terminal attached
nex --quit                    # stop running instance
nex --status                  # check if running
nex --restart                 # restart
nex --diagnostics-bundle      # dump diagnostics to a zip
nex                           # normal: background hotkey runtime (Ctrl+Space)
```

Config created at `%APPDATA%\Nex\config.toml` on first launch. Index at `%APPDATA%\Nex\index.sqlite3`.

## Project Structure

- Single Rust workspace member: `apps/core` (crate `nex-cli`, lib name `nex_core`)
- Binary entry: `apps/core/src/main.rs` → `nex_core::runtime::run_with_options`
- Library entry: `apps/core/src/lib.rs`
- **Windows-only modules**: `rt`-gated `windows_overlay`, `runtime_loop`, `everything`, `runtime_overlay_rows`
- Tests in `apps/core/src/runtime.rs:tests` and `apps/core/tests/`, `tests/perf/`

## Toolchain

**Local (Windows)**: `stable-x86_64-pc-windows-gnu` with MinGW-w64 at `C:\Users\Admin\AppData\Local\Microsoft\WinGet\...\mingw64\bin`.
**CI**: `dtolnay/rust-toolchain@stable` (auto-detected native target).

## Environment Variables

| Variable | Purpose |
|---|---|
| `NEX_SUPPRESS_STDIO=1` | Suppress stdout logging (also `SWIFTFIND_SUPPRESS_STDIO`) |
| `NEX_WINDOWS_RUNTIME_SMOKE=1` | Enable the windows runtime smoke test (otherwise skips) |
| `NEX_ALLOW_MISSING_ICON=1` | Allow release build without `nex.ico` (also `SWIFTFIND_ALLOW_MISSING_ICON`) |

## Overlay Rendering Stack

Two concurrent APIs — GDI+ owns all non-listbox painting; GDI handles only `WM_DRAWITEM`:

- **GDI+** (`gdiplus.dll` FFI, `gdiplus_rendering.rs`): panel background (rounded rects, lines, fills), help tip, footer hint, and list-row selection highlight. Uses `GdipCreateFontFromDC` for text, `GdipFillRoundedRectangle` for highlights. Always uses ClearType (`TextRenderingHintClearTypeGridFit`).
- **GDI** (`windows-sys`, pure Win32): list row text, icons, background fills in `WM_DRAWITEM` (`painting.rs::draw_list_row`). Uses `DrawTextW`, `DrawIconEx`, `FillRect`, GDI font objects. Non-visual GDI calls (`GetTextExtentPoint32W`) still used for layout measurement in footer hints.
- **D2D + DWrite** (`d2d_renderer.rs`): **fully dead code** — was used for panel background but fully replaced by GDI+. Do not resurrect.

## Key Overlay Architecture

- Overlay is a `WS_POPUP | WS_EX_LAYERED | WS_EX_TOOLWINDOW` window
- Animates show/hide via `SetLayeredWindowAttributes` alpha (0–255) + `SetWindowPos`
- Listbox child window handles `WM_DRAWITEM` (all rows painted from parent dialog proc)
- Mica backdrop via `DWMWA_SYSTEMBACKDROP_TYPE` planned but not yet implemented
- `hover_index: i32` tracks the hovered/selected row; `=-1` when none selected
- `selected_index()` reads native `LB_GETCURSEL`; keyboard and Submit use this

## Config

TOML format (primary), with JSON/JSON5 backward compatibility. **Never add new keys to the JSON template** — only TOML template (`apps/core/src/config.rs::write_user_template_toml`). Config is versioned (`CURRENT_CONFIG_VERSION = 13`); migrations in `apply_migrations()`.

## Style & Conventions

- `cargo build` warnings: ~22 dead-code warnings (pre-existing, mostly unused D2D functions)
- No clippy, no formatter enforced in CI
- Most platform gating: `#[cfg(target_os = "windows")]` on Windows-specific modules/functions
- Legacy name "SwiftFind" still appears in some env var/constant names — don't rename unless explicitly asked
