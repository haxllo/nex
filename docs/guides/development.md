<!-- generated-by: gsd-doc-writer -->
# Development

## Local Setup

### Prerequisites

- **Rust**: stable toolchain (1.75+)
- **Windows 10/11** (64-bit) — nex is Windows-only
- **WebView2 Runtime**: ships with Windows 11 and recent Windows 10 builds; install manually if missing
- **MinGW-w64** (local builds): required for `stable-x86_64-pc-windows-gnu` target

### Toolchain

Local builds use the `stable-x86_64-pc-windows-gnu` target with MinGW-w64:

```
rustup toolchain install stable-x86_64-pc-windows-gnu
```

CI uses `dtolnay/rust-toolchain@stable` (auto-detects native target, no manual setup).

### Clone and Install

```bash
git clone https://github.com/haxllo/nex.git
cd nex
cargo build --bin nex
```

Windows resource embedding requires `nex.ico` in `apps/core/`. If the icon is missing during a release build, set `NEX_ALLOW_MISSING_ICON=1` to bypass:

```bash
$env:NEX_ALLOW_MISSING_ICON=1; cargo build --release --bin nex
```

### First Run

```bash
cargo run --bin nex -- --foreground
```

Keeps the terminal attached. On first launch, a default config is created at `%APPDATA%\Nex\config.toml` and the index at `%APPDATA%\Nex\index.sqlite3`.

Press `Ctrl+Space` to summon the overlay (configurable in config.toml).

---

## Build Commands

| Command | Description |
|---|---|
| `cargo build --bin nex` | Debug build |
| `cargo build --release --bin nex` | Release build (optimized, GUI subsystem — no console window) |
| `cargo build -p nex` | Debug build (package-scoped, equivalent) |
| `cargo check -p nex` | Type-check without producing a binary |

The release build enables `windows_subsystem = "windows"` via `#![cfg_attr]`, suppressing the console window. Debug builds keep the console attached for development convenience.

---

## Running

Run from the project root:

```bash
nex --foreground              # Dev mode: terminal attached, logs to stdout
nex --quit                    # Stop running instance
nex --status                  # Check if running
nex --restart                 # Restart the instance
nex --diagnostics-bundle      # Dump diagnostics to a zip archive
nex                           # Normal: background hotkey runtime
```

When running via `cargo run`, pass `--bin nex` and separate cargo flags from program arguments with `--`:

```bash
cargo run --bin nex -- --foreground
```

---

## Test Commands

### Unit and Integration Tests

All tests are run with:

```bash
cargo test -p nex
```

### Running a Single Test

```bash
cargo test -p nex -- <test_name>
```

### Performance Tests

The warm-query latency gate measures P95 latency against a 10k-item dataset:

```bash
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
```

Threshold: **P95 ≤ 15 ms**.

### Windows Runtime Smoke Test

Disabled by default. Enable with the `NEX_WINDOWS_RUNTIME_SMOKE` environment variable:

```bash
$env:NEX_WINDOWS_RUNTIME_SMOKE=1
cargo test -p nex --test windows_runtime_smoke_test
```

This test registers a real global hotkey and performs an IPC round-trip. Requires a running Windows session.

### CI Test Order

Per the CI pipeline:

1. `vitest --run` — JS scaffold gate (file existence checks only)
2. `cargo test -p nex` — all unit and integration tests
3. Perf gate — `warm_query_p95_under_15ms`
4. Smoke gate — `windows_runtime_smoke_test`

---

## Project Layout

```
nex/
├── Cargo.toml                   # Workspace root (resolver = 2)
├── apps/
│   └── core/                    # Single crate (nex / nex_core)
│       ├── Cargo.toml
│       ├── build.rs             # Windows resource embedding (icon)
│       ├── nex.ico              # Application icon (release builds)
│       ├── src/
│       │   ├── main.rs          # Binary entry point
│       │   ├── lib.rs           # Library root (nex_core)
│       │   ├── runtime.rs       # Event loop, CLI arg parsing, lifecycle
│       │   ├── config.rs        # Config loading, migration, TOML template
│       │   ├── search.rs        # Fuzzy search core
│       │   ├── tantivy_search.rs# Tantivy-backed full-text search
│       │   ├── discovery.rs     # File/app discovery logic
│       │   ├── index_store.rs   # SQLite index persistence
│       │   ├── core_service.rs  # High-level service orchestration
│       │   ├── transport.rs     # IPC transport (JSON over named pipe)
│       │   ├── contract.rs      # Request/response types
│       │   ├── model.rs         # Core data types (SearchItem, etc.)
│       │   ├── action_executor.rs / action_registry.rs
│       │   ├── calculator.rs    # Inline calculator
│       │   ├── clipboard_history.rs
│       │   ├── plugin_sdk.rs    # Plugin host
│       │   ├── updater.rs       # Self-update logic
│       │   ├── logging.rs       # File logger with rotation
│       │   ├── settings.rs      # User settings
│       │   ├── startup.rs       # Windows startup registration
│       │   ├── hotkey.rs / hotkey_runtime.rs
│       │   ├── query_dsl.rs     # Query language parser
│       │   ├── search_worker.rs # Background search worker
│       │   ├── overlay_state.rs # Overlay state machine
│       │   ├── runtime_*.rs     # Runtime sub-components (actions, commands, diagnostics, etc.)
│       │   ├── console_signal.rs        # Windows-only console handler
│       │   ├── everything_bridge.rs     # Windows-only Everything SDK bridge
│       │   ├── file_watcher*.rs         # Windows-only file watching
│       │   ├── runtime_loop.rs          # Windows-only message pump
│       │   ├── runtime_overlay_rows.rs  # Windows-only result rendering
│       │   └── overlay/                 # Windows-only WebView2 overlay
│       │       ├── host.rs              # Tao event loop, wry WebView, positioning
│       │       ├── model.rs             # OverlayEvent, OverlayRow, Theme types
│       │       ├── icons.rs             # LRU icon cache → base64 PNG data URIs
│       │       ├── shim.rs              # Imperative API for runtime → overlay push
│       │       ├── hotkey.rs            # RegisterHotKey + GetMessageW thread
│       │       ├── tray.rs              # System tray icon + context menu
│       │       ├── platform.rs          # Theme detection, instance signaling
│       │       └── indexing_progress.rs # Secondary tao/wry for indexing UI
│       └── tests/
│           ├── action_executor_test.rs
│           ├── config_test.rs
│           ├── contract_test.rs
│           ├── core_service_test.rs
│           ├── discovery_test.rs
│           ├── hotkey_runtime_test.rs
│           ├── hotkey_test.rs
│           ├── index_store_test.rs
│           ├── perf_query_latency_test.rs  # Delegates to tests/perf/
│           ├── search_test.rs
│           ├── settings_test.rs
│           ├── startup_test.rs
│           ├── transport_test.rs
│           └── windows_runtime_smoke_test.rs
├── tests/
│   ├── perf/
│   │   └── query_latency_test.rs   # Actual perf test implementation
│   └── smoke/
│       └── scaffold.test.ts        # JS scaffold gate
├── scripts/
│   └── windows/
│       ├── package-windows-artifact.ps1   # Create zip + manifest
│       ├── package-windows-installer.ps1  # Create setup.exe
│       ├── install-nex.ps1
│       ├── uninstall-nex.ps1
│       ├── update-nex.ps1
│       ├── nex.iss              # Inno Setup script
│       ├── inspect-start-app-sources.ps1
│       ├── record-manual-e2e.ps1
│       └── profile-memory-and-icons.ps1
├── apps/assets/                # Branding assets (nex.svg)
└── docs/                       # Documentation
    ├── architecture/
    ├── configuration/
    ├── engineering/
    ├── guides/
    └── releases/
```

### Platform-Gated Modules

Modules marked `#[cfg(target_os = "windows")]` in `lib.rs`:

| Module | Purpose |
|---|---|
| `overlay` | WebView2 overlay (tao + wry window, hotkey, tray, icons, indexing progress) |
| `runtime_loop` | Windows message pump for background runtime |
| `runtime_overlay_rows` | Overlay result rendering logic |
| `everything_bridge` | Integration with Everything SDK for instant file search |
| `file_watcher` | NTFS file change notifications |
| `file_watcher_consumer` | File change event consumer |
| `console_signal` | Console event handler |

---

## Code Style

### Linting and Formatting

- **No clippy or formatter enforced in CI.** The project does not gate on clippy or rustfmt.
- Pre-existing dead-code warnings (~12) from unused overlay/misc functions — these are accepted and not cleaned up.
- Legacy name "SwiftFind" still appears in some environment variable and constant names (`SWIFTFIND_SUPPRESS_STDIO`, `SWIFTFIND_ALLOW_MISSING_ICON`, `swiftfind.log`). Do not rename unless explicitly instructed.

### Conventions

- Platform gating uses `#[cfg(target_os = "windows")]` on entire modules or individual functions.
- Config is TOML format (primary), with JSON/JSON5 backward compatibility. Never add new keys to the JSON template — only the TOML template (`config.rs::write_user_template_toml`).
- Config is versioned (`CURRENT_CONFIG_VERSION`) with migrations in `apply_migrations()`.
- The `pub(crate)` visibility is used heavily for cross-module internal APIs; public exports are limited to `lib.rs` re-exports.
- CLI commands run synchronously and exit; only the no-argument mode spawns a background GUI process.

### Module Organization

- Binary entry (`main.rs`) is minimal — it parses CLI args and delegates to `runtime::run_with_options`.
- Core domain logic lives in `core_service.rs` and `search.rs`.
- Overlay code is isolated under `overlay/` — the runtime communicates with it through the shim API (`shim.rs`).
- Test helpers are accessed via `pub(crate)` re-exports from `runtime.rs` (gated with `#[cfg(test)]`).

---

## Debugging Tips

### Foreground Mode

Always use `--foreground` during development. This keeps the terminal attached and logs output to stdout:

```bash
cargo run --bin nex -- --foreground
```

### Log Files

Runtime logs are written to:

```
%APPDATA%\Nex\logs\nex.log
```

The file rotates at 1 MB; up to 5 archives are kept. Panics are captured to the log automatically via a panic hook.

### Diagnostics Bundle

Generate a comprehensive diagnostics snapshot for debugging:

```bash
cargo run --bin nex -- --diagnostics-bundle
```

Outputs a zip archive with logs, config, index stats, and process info.

### Environment Variables

| Variable | Purpose |
|---|---|
| `NEX_SUPPRESS_STDIO=1` | Suppress stdout/stderr logging (also `SWIFTFIND_SUPPRESS_STDIO`) |
| `NEX_WINDOWS_RUNTIME_SMOKE=1` | Enable the Windows runtime smoke test (otherwise skips) |
| `NEX_ALLOW_MISSING_ICON=1` | Allow release build without `nex.ico` (also `SWIFTFIND_ALLOW_MISSING_ICON`) |

### Common Issues

**Missing icon on release build:** Ensure `nex.ico` exists in `apps/core/` or set `NEX_ALLOW_MISSING_ICON=1`.

**Overlay doesn't appear:** Verify WebView2 Runtime is installed. Check `nex.log` for overlay initialization errors.

**Port conflict:** Nex uses named pipes for IPC. If another instance is running, `--quit` it first or use `--restart`.

**Dead-code warnings:** Pre-existing ~12 warnings are accepted. Do not add `#[allow(dead_code)]` to new code without a reason.

---

## Release Process

1. **Bump version** in `apps/core/Cargo.toml`, commit, create a tag:

```bash
# Edit version in Cargo.toml, then:
git add apps/core/Cargo.toml
git commit -m "release: vX.Y.Z"
git tag vX.Y.Z
```

2. **Build release binary:**

```bash
cargo build --release --bin nex
```

3. **Package artifacts:**

```bash
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-artifact.ps1 -Channel stable
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-installer.ps1 -Channel stable
```

Outputs to `artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe,manifest.json}`.

4. **Write release notes** at `docs/releases/v<ver>-notes.md` (copy from `docs/releases/RELEASE-TEMPLATE.md`).

5. **Push and create GitHub release:**

```bash
git push origin master --tags
gh release create v<ver> --title "v<ver> — <title>" --notes-file docs/releases/v<ver>-notes.md artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe,manifest.json}
```
