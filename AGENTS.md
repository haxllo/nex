# Nex — Agent Reference

## Build & Test

**Cargo target location**: Use `G:\nex-target` for Cargo commands when available. Set `$env:CARGO_TARGET_DIR='G:\nex-target'` before `cargo build`, `cargo check`, or `cargo test` to avoid C: drive pressure.

```bash
cargo build --bin Nex                    # debug build (nex only)
cargo build --release --bin Nex --bin NexHelper  # release build (both binaries)
cargo build --release -p nex-helper      # release build (helper only)
cargo test -p nex                        # all unit tests (Windows)
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
cargo test -p nex --test windows_runtime_smoke_test  # CI-only smoke test
```

`vitest --run` for the JS scaffold gate (just checks file existence). Server-side only, no browser.

**Note**: `cargo test -p nex` broken for now — many tests hang on Windows. Build-only verification.

**CI order**: `vitest --run` → `cargo build --release --bin Nex --bin NexHelper` → `cargo test -p nex` → perf gate → smoke gate.

**Helper binary**: `NexHelper.exe` is a separate workspace member (`apps/helper`) built via `--bin NexHelper`. It runs elevated (High IL) via a scheduled task to handle hotkey detection when elevated windows (Task Manager, Settings) are foreground. The scheduled task is created automatically on first run (one-time UAC prompt). The helper lives beside `nex.exe` and is found automatically.

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

**Helper**: On first run nex creates a scheduled task (`NexHelperV2`) to launch `NexHelper.exe` at High IL. This enables hotkey detection and overlay foreground when elevated windows (Task Manager, Settings) are active. The scheduled task creation triggers a one-time UAC prompt.

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

**IMPORTANT**: Release workflows must be run directly by the main agent (me), NOT delegated to subagents. Use subagents only for review/verification of release output if needed.

**Trigger**: When the user asks to run, create, or publish a Nex release, execute this release procedure. It is a manual agent workflow, not a GitHub Actions dispatch.

```bash
# 1. bump version and commit the release changes; do not tag yet
cargo build --release --bin Nex --bin NexHelper

# 2. build artifacts (zip, setup, manifest)
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-artifact.ps1 -Channel stable
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-installer.ps1 -Channel stable

# 3. write release notes to docs/releases/v<ver>-notes.md (copy from docs\releases\RELEASE-TEMPLATE.md) and commit them

# 4. tag only after the release-notes commit, then push
git tag v<ver>
git push origin master --tags

# 5. create GitHub release
gh release create v<ver> --title "v<ver>" --notes-file docs/releases/v<ver>-notes.md artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe,manifest.json}
```

Artifacts land in `artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe}` + manifest.

## Style & Conventions

- `cargo build` warnings: ~12 dead-code warnings (pre-existing, mostly unused overlay/misc functions)
- No clippy, no formatter enforced in CI
- Most platform gating: `#[cfg(target_os = "windows")]` on Windows-specific modules/functions
- Legacy name "SwiftFind" still appears in some env var/constant names — don't rename unless explicitly asked

## Git Commit Author

All commits MUST use the repo owner's identity. Never commit with a bot/AI name.

```bash
git commit --author="Habeeb <mshabeeburrahman786@gmail.com>" -m "message"
```

**Note**: Use this identity for any git operation that requires an author (commits, amends, rebases, etc.).

## Commit Message Content

Commit message bodies MUST contain NO AI agent embedded names. This means absolutely NO `Co-authored-by:` trailers referencing any bot or AI, NO bot signatures, NO mentions of AI assistants, and NO references to agent systems. This applies to commit messages, tag messages, branch descriptions, and any other git-related text.

## Linear Project Tracking

The Nex Linear project is [P-CIQ-2](https://linear.app/ciqada/project/nex-838bd9949214).

- For Nex bug reports, feature requests, and coding tasks that require implementation, search the Nex project for a matching issue before editing code. Reuse the matching issue when one exists; otherwise create an issue in the Nex project. Do not create issues for questions, planning, or status requests that do not ask for implementation.
- Track a release-only request with a Linear Release record, following the configured Nex release pipeline. Do not create a regular issue solely to build, package, tag, or publish a release. Link related implementation issues to the release when available. If no release pipeline is configured or accessible, tell the user and do not create a regular issue as a substitute.
- Keep the issue current as work progresses. Before finishing, record the implemented behavior and the build/test checks that were or were not run.
- Set the issue state to reflect the actual work. Do not say checks passed unless they were run.
- When work is delegated, agents should update the same issue and avoid creating duplicate issues for subtasks. If an agent cannot access Linear, it must ask the user to connect Linear for that agent and wait before starting the Nex task.
- Use project status updates for periodic progress, health, and risks; use milestones for major deliverables with clear outcomes. Do not create a milestone for every issue or a health update without evidence.
- If Linear is unavailable, ask the user to connect it for the current agent before starting implementation.
