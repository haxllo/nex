# Nex — Agent Reference

## Non-negotiables (read before editing)

1. **Linear first.** For any user-reported bug, feature request, or implementation
   task, search the Nex project (P-CIQ-2) for a matching issue *before touching
   code*. Reuse it, or create one if none matches. Keep it current. Questions,
   planning, and status requests do not need an issue. Details: "Linear Project
   Tracking" below.
2. **Never close an issue yourself.** Finished work moves to **In Review**; only
   a merged PR moves it to **Done**. Setting Done before the PR merges is not
   allowed, no matter how complete the implementation looks.
3. **Verify, then report.** Use the release profile and `G:\nex-target`, run the
   relevant gate, and never claim a check passed unless it was run.
4. **Commit identity.** Author is the repo owner; commit bodies contain no
   AI/bot names or co-author trailers. Do not commit unless asked.
5. **Process boundaries.** UI (Medium IL) never indexes; indexer (Medium IL)
   never renders; helper (High IL) is a broker, not a supervisor. See
   "Process Model & Privilege Boundaries" below.

## Build & Test

**Cargo target location**: Use `G:\nex-target` for Cargo commands when available. Set `$env:CARGO_TARGET_DIR='G:\nex-target'` before `cargo build`, `cargo check`, or `cargo test` to avoid C: drive pressure.

```bash
cargo build --bin NexUIProcess         # debug build (UI only)
cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess  # release build
cargo build --release -p nex-helper      # release build (helper only)
cargo test -p nex                        # all unit tests (Windows)
cargo test -p nex-indexer --test process_isolation_test  # IPC/process-isolation e2e
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
cargo test -p nex --test windows_runtime_smoke_test  # CI-only smoke test
```

`vitest --run` for the JS scaffold gate (just checks file existence). Server-side only, no browser.

**Note**: `cargo test -p nex` broken for now — many tests hang on Windows. Build-only verification. `cargo test -p nex-helper` cannot run either: the helper embeds a `requireAdministrator` manifest, so the test harness exits with "The requested operation requires elevation" (os error 740).

**CI order**: `vitest --run` → `cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess` → `cargo test -p nex` → perf gate → smoke gate.

**Helper binary**: `NexHelper.exe` is a separate workspace member (`apps/helper`) built via `--bin NexHelper`. It runs elevated (High IL) via a scheduled task to handle hotkey detection when elevated windows (Task Manager, Settings) are foreground. The scheduled task is created automatically on first run (one-time UAC prompt). The helper lives beside `NexUIProcess.exe` and is found automatically.

## Running

`Nex.exe` is not a runtime; the UI entry point is `NexUIProcess.exe`.

```bash
.\target\release\NexUIProcess.exe --foreground   # dev mode, keeps terminal attached
.\target\release\NexUIProcess.exe --quit         # stop running instance
.\target\release\NexUIProcess.exe --status       # check if running
.\target\release\NexUIProcess.exe --restart      # restart
.\target\release\NexUIProcess.exe --diagnostics-bundle
.\target\release\NexUIProcess.exe                # normal: background hotkey runtime
```

Config created at `%APPDATA%\Nex\config.toml` on first launch. Index at `%APPDATA%\Nex\index.sqlite3`.

**Helper**: On first run nex creates a scheduled task (`NexHelperV2`) to launch `NexHelper.exe` at High IL. This enables hotkey detection and overlay foreground when elevated windows (Task Manager, Settings) are active. The scheduled task creation triggers a one-time UAC prompt.

## Project Structure

- Workspace members: `apps/core` (crate `nex`, lib `nex_core`), `apps/ui` (bin `NexUIProcess`), `apps/indexer` (bin `NexIndexerProcess`), `apps/helper` (bin `NexHelper`)
- Binary entry: `apps/ui/src/main.rs` → `nex_core::runtime::run_with_options`
- Library entry: `apps/core/src/lib.rs`
- **Two-process split**: `NexUIProcess.exe` owns the overlay, hotkeys, tray, and search over the indexer's published snapshot. `NexIndexerProcess.exe` owns discovery, SQLite writes, file/app watchers, and Tantivy. There is no in-process indexing fallback.
- **One binary per package on purpose**: a build script's Windows resource is linked into all of its package's binaries and inherited by dependent packages, so sharing a package would give every process the same `FileDescription` (Task Manager name).
- **Windows-only modules**: `runtime_loop`, `everything`, `runtime_overlay_rows`
- Tests in `apps/core/src/runtime.rs:tests` and `apps/core/tests/`, `tests/perf/`, plus process-isolation e2e in `apps/indexer/tests/`

## Process Model & Privilege Boundaries

| Process | Integrity | Owns |
|---|---|---|
| `NexUIProcess.exe` | Medium | Overlay/WebView2, hotkeys, tray, settings, search over the published snapshot. Session owner: spawns, supervises, and stops the indexer and helper. |
| `NexIndexerProcess.exe` | Medium | Discovery, SQLite writes, file/app watchers, Tantivy. Parent-PID watchdog; exits when the UI disappears. |
| `NexHelper.exe` | High (elevated) | Narrow privileged broker: elevated hotkey detection, foreground grant, input simulation. **Never owns process lifecycle.** |

**Pipe integrity rule (UIPI):** a Medium-IL process cannot write into a
High-IL pipe (`Access is denied`). Therefore:
- pipes the UI only *reads* may be created by the helper (`nex-hotkey`,
  `NexHelperLog`);
- the command pipe the UI *writes* must be created by the UI (`NexHelperCmd`),
  with the helper as the read-only client.
Do not invert that ownership.

**Low-level hooks are UIPI-gated too:** `WH_KEYBOARD_LL`/`WH_MOUSE_LL` in the
Medium-IL UI never fire for input routed to an elevated window (Task Manager,
Settings). Keep that detection in the helper and forward it over the detection
pipe (`HOTKEY`, `MOUSEDOWN`); the UI re-checks overlay visibility and the
panel rect.

**Supervision:** the UI's hotkey supervisor keeps the helper connected and
reconnects with 2s→30s backoff when the pipe closes; the in-process hook stays
active meanwhile. The snapshot-sync loop respawns a dead indexer.

## Agent Tooling

- `scripts/git-hooks/commit-msg` enforces the non-negotiables: it strips AI
  co-author trailers and rejects a commit that changes `apps/`, `scripts/`, or
  `manifests/` without a `CIQ-<n>` id. Enable it per clone with
  `git config core.hooksPath scripts/git-hooks`. Exceptional commits can bypass
  once via `NEX_ALLOW_NO_LINEAR=1`.
- `CLAUDE.md`, `.github/copilot-instructions.md`, and `.cursor/rules/nex.mdc`
  repeat the non-negotiables for agents that do not load this file.
- CI rejects pull requests that change code paths without a Linear id in the
  title or body.

## Toolchain

**Local (Windows)**: `stable-x86_64-pc-windows-gnu` with MinGW-w64 at `C:\Users\Admin\AppData\Local\Microsoft\WinGet\...\mingw64\bin`.
**CI**: `dtolnay/rust-toolchain@stable` (auto-detected native target).

## Environment Variables

| Variable | Purpose |
|---|---|
| `NEX_SUPPRESS_STDIO=1` | Suppress stdout logging (also `SWIFTFIND_SUPPRESS_STDIO`) |
| `NEX_WINDOWS_RUNTIME_SMOKE=1` | Enable the windows runtime smoke test (otherwise skips) |
| `NEX_DISCOVERY_BACKEND` | `mft` forces the NTFS MFT snapshot backend, `walkdir` disables it; unset = auto (MFT when a per-root cap is >= 100k) |

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
# 1. bump version in apps/core, apps/ui, and apps/indexer (all
#    three build scripts stamp FileVersion from their own package); commit the
#    release changes; do not tag yet
cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess

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
- **State policy:** agents may move issues to In Progress or In Review. Only a
  merged PR closes an issue (moves it to Done). Do not set Done directly, even
  for retroactive or verification-only work.
- When work is delegated, agents should update the same issue and avoid creating duplicate issues for subtasks. If an agent cannot access Linear, it must ask the user to connect Linear for that agent and wait before starting the Nex task.
- Use project status updates for periodic progress, health, and risks; use milestones for major deliverables with clear outcomes. Do not create a milestone for every issue or a health update without evidence.
- If Linear is unavailable, ask the user to connect it for the current agent before starting implementation.
