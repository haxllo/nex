# Nex — Agent Reference

## Build & Test

```bash
cargo build --bin nex                    # debug build
cargo build --release --bin nex          # release build
cargo test -p nex                        # all unit tests (Windows)
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
cargo test -p nex --test windows_runtime_smoke_test  # CI-only smoke test
```

`vitest --run` for the JS scaffold gate (just checks file existence). Server-side only, no browser.

**CI order**: `vitest --run` → `cargo test -p nex` → perf gate → smoke gate.

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

- Single Rust workspace member: `apps/core` (crate `nex`, lib name `nex_core`)
- Binary entry: `apps/core/src/main.rs` → `nex_core::runtime::run_with_options`
- Library entry: `apps/core/src/lib.rs`
- **Windows-only modules**: `runtime_loop`, `everything`, `runtime_overlay_rows`
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

## Overlay Architecture

**WebView2** (tao + wry). Overlay is a `WS_POPUP` window hosting a WebView2
control. No GDI/GDI+/D2D — all rendering is HTML/CSS/JS.

- Panel built from embedded HTML (`INDEX_HTML`, `STYLE_CSS`, `APP_JS` in
  `overlay/host.rs`), served via `nexasset://` custom protocol.
- State pushed to JS via `ICoreWebView2::PostWebMessageAsJson` (fire-and-forget,
  non-blocking). No synchronous `evaluate_script` on the critical path.
- Icons decoded to PNG, embedded as base64 data URIs in the state snapshot
  (`overlay/icons.rs`). LRU cache keyed by file path.
- Window positioning, DPI handling, acrylic backdrop in `overlay/host.rs`.
- Hotkey listener on dedicated thread (`overlay/hotkey.rs`). System tray icon
  with context menu (`overlay/tray.rs`).
- Single warm-release timer thread (crossbeam channel, re-arm on Hide); clears icon cache only — WebView stays warm.
- Theme detection: Windows registry `AppsUseLightTheme` (`overlay/platform.rs`).
- Indexing progress window: separate tao + wry instance (`overlay/indexing_progress.rs`).
- Mica backdrop via `DWMWA_SYSTEMBACKDROP_TYPE` planned but not yet implemented.

## Config

TOML format (primary), with JSON/JSON5 backward compatibility. **Never add new keys to the JSON template** — only TOML template (`apps/core/src/config.rs::write_user_template_toml`). Config is versioned (`CURRENT_CONFIG_VERSION = 13`); migrations in `apply_migrations()`.

## Release

```bash
# 1. bump version in Cargo.toml, commit, tag
cargo build --release --bin nex

# 2. build artifacts (zip, setup, manifest)
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-artifact.ps1 -Channel stable
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-installer.ps1 -Channel stable

# 3. write release notes to docs/releases/v<ver>-notes.md (copy from RELEASE-TEMPLATE.md)

# 4. push + create GitHub release
git push origin master --tags
gh release create v<ver> --title "v<ver> — <title>" --notes-file docs/releases/v<ver>-notes.md artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe,manifest.json}
```

Artifacts land in `artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe}` + manifest.

## Style & Conventions

- `cargo build` warnings: ~12 dead-code warnings (pre-existing, mostly unused overlay/misc functions)
- No clippy, no formatter enforced in CI
- Most platform gating: `#[cfg(target_os = "windows")]` on Windows-specific modules/functions
- Legacy name "SwiftFind" still appears in some env var/constant names — don't rename unless explicitly asked
