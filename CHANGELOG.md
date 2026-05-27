# Changelog

All notable changes to Nex are documented in this file.

This changelog is intentionally backfilled from the most reliable sources in the repo: tagged milestones, release notes, and shipped commit history. Older tags can be expanded later if you want a full historical pass.

## [1.3.0] - 2026-05-28

### Changed (Breaking)
- **D2D/DWrite rendering removed entirely** — all rendering now goes through GDI+ (panel background, help tip, footer, list row highlights). The `d2d_renderer.rs` module (~800 lines) has been deleted. No D2D or DWrite code remains in the codebase. GDI+ is now a hard requirement at startup — the overlay closes with an error if GDI+ cannot initialize (previously warned about a non-existent fallback).
- **SpaceMono replaced with Inter as bundled font** — Inter v4.1 (Regular, Bold, Medium, SemiBold) is now bundled at `apps/assets/fonts/Inter/`. When private fonts load successfully, the UI uses `"Inter"` instead of `"Segoe UI"`. The SpaceMono font directory has been removed.
- **Text rendering quality unified** — all GDI+ text now uses ClearType (`TextRenderingHintClearTypeGridFit`). The misleading `SMOOTHING_MODE_HIGH_QUALITY` constant (which was identical to `ANTI_ALIAS`) has been removed.
- **GDI+ init failure is now fatal** — previously printed a warning about a non-existent "GDI fallback"; now posts `WM_CLOSE` and disables the overlay.

### Fixed
- **Help tip border clipping** — float-based rounded rectangle path with 0.5px inset ensures full 1px pen stroke visibility.
- **Footer hint rendering** — background and separator now rendered via GDI+ `fill_rect`; text remains GDI `TextOutW` (required for `WS_CHILD` DC of `WS_EX_LAYERED` parent).
- **`fill_rounded_rect_on_graphics` degeneration** — when corner diameter ≥ width or height, falls back to `GdipFillRectangleI` to avoid self-intersecting `FILL_MODE_ALTERNATE` paths during panel expand animation.
- **Pre-created GDI+ font handles** — 7 font objects created once during `WM_CREATE` instead of per-frame `SelectObject`.
- **`register_private_fonts()`** — now correctly searches for `Inter-Regular.ttf` / `Inter-Bold.ttf` (was searching Inter directories for SpaceMono filenames — always failing).
- **`resolve_font_family()`** — returns `"Inter"` when private fonts load (was returning `"Segoe UI"` unconditionally — bundled font was never used).
- **CI workflow** — fixed pnpm setup order (was installing Node.js before pnpm); removed conflicting explicit `pnpm/action-setup` version (now auto-detects from `packageManager`).
- **Hanging UI tests** — all test files using `.tmp` extension with `ShellExecuteW` now use `.txt` (opens Notepad asynchronously) with `taskkill` cleanup to avoid "How do you want to open this file?" dialogs.
- **CI multiplied jobs** — dropped `ubuntu-latest` from matrix (Nex is Windows-only); dropped duplicate `push` + `pull_request` triggers.
- **Scaffold test path** — fixed `windows_overlay.rs` → `windows_overlay/mod.rs`.
- **All `cargo test -p nex` commands** — corrected to `-p nex-cli` (actual package name).

## [1.2.0] - 2026-05-27

### Fixed
- **Screen-tear on hover** — eliminated visual desync between D2D and GDI rendering surfaces by ensuring consistent invalidation and ordering.
- **First-keystroke panel flash** — addressed the degenerate paint during panel expand animation where corner diameter temporarily exceeded panel height.

## [1.1.1] - 2026-05-24

### Changed
- **Search is now fully async** — query processing moved to a dedicated background thread. The UI thread no longer blocks during search, keeping the overlay responsive even during heavy queries.

### Fixed
- **Overlay no longer freezes during search** — previously, every keystroke triggered a synchronous search on the UI thread, blocking window message processing. Now handled by `SearchWorker` with stale-request draining and `PostMessageW` result notification.
- **Thread safety for CoreService** — `Arc<Mutex<CoreService>>` ensures the SQLite connection is never accessed concurrently.
- **Worker thread lifecycle** — `SearchWorker::Drop` signals thread exit via channel closure and joins the thread cleanly.

## [1.1.0] - 2026-05-24

### Added
- **Everything SDK integration** — instant file search via Everything64.dll bundled with the installer. No manual DLL placement required. Nex auto-detects Everything at runtime and falls back gracefully when not available.
- **Plugin SDK foundations** — trait-based plugin interface with WASM distribution path prepared. The `plugin_sdk.rs` module now supports manifest parsing and store protocol scaffolding for future extension discovery.
- **Window management system** — 8 preset tile layouts (left/right half, top/bottom half, four quadrants, center, maximize, restore). Accessible via command palette or configurable hotkeys.
- **Calculator (basic)** — inline arithmetic evaluation directly in the search bar. Supports `+`, `-`, `*`, `/`, `%`, and parenthesized expressions. Result appears as the top hit with one-tab copy.
- **Emoji picker** — type `:` followed by a keyword to search and insert emoji. Glyphs are rendered inline with results for quick selection.

### Changed
- **Packaging now bundles Everything64.dll** — the `package-windows-artifact.ps1` script downloads, extracts, and includes the Everything SDK DLL automatically. TLS 1.2 is enforced for download compatibility on older Windows/PowerShell versions.
- **Stale docs updated** — `system-architecture.md` (fixed config format from JSON→TOML), `project-charter.md` (fixed default hotkey from Alt+Space→Ctrl+Space), `requirements.md` (same hotkey fix), `windows-runtime-behavior.md` (fixed config format reference), `windows-packaging-readiness.md` (updated version placeholders), and `docs/README.md` (rewritten to reflect current product).
- **Version bumped to 1.1.0** — Cargo.toml, build artifacts, and packaging scripts aligned.

### Fixed
- **PowerShell packaging compatibility** — `Invoke-WebRequest` failed on older Windows due to TLS 1.2 not being negotiated. Fixed by explicitly setting `[Net.ServicePointManager]::SecurityProtocol`. Zip extraction and SHA256 hashing now use fallback methods for cross-version PowerShell compatibility.
- **Everything SDK extraction** — the zip extraction now supports three fallback methods (.NET ZipFile, Expand-Archive, Shell.Application COM) to handle all PowerShell versions from 2.0 through 7+.
- **All stale documentation references** corrected to match the current v6.4.0/v1.1.0 reality (TOML config, Ctrl+Space default, web search availability).

### Known Issues
- Calculator supports basic arithmetic only; unit conversion and scientific functions deferred to a future release.
- Window management is command-palette-only; configurable hotkey bindings coming in v1.2.
- Plugin SDK is still in preview — no public extension store or WASM runtime yet.

## [6.4.0] - 2026-03-31

### Added
- On-demand updater entry points inside Nex.
  - Added built-in `Check for Updates` command action.
  - Added tray menu `Check for Updates`.
  - Both launch the existing stable PowerShell updater without adding a background updater service.
- Hotkey conflict recovery.
  - Nex now stays alive when the configured global hotkey cannot be registered.
  - Added structured `hotkey_registration_issue` diagnostics.
  - Added tray-based `Open Config` recovery flow and visible status guidance.
- Freshness diagnostics without heavy file watchers.
  - Added `provider_freshness` and `stale_prune` diagnostics to logs and status output.

### Changed
- New installs now default to app-first search behavior:
  - `show_files = false`
  - `show_folders = false`
- Discovery scope hardening now excludes low-value system, cache, and noise paths by default for filesystem indexing.
- Startup diagnostics now expose lifecycle markers for overlay readiness, hotkey readiness, indexing start/completion, and cache application.
- Broad-root indexing now tightens file/folder cache retention more aggressively to keep memory behavior predictable.

## [6.3] - 2026-03-13

### Added
- Windows hybrid discovery improvements for apps, files, and folders.
  - Added system app and Windows Search-backed discovery.
  - Added explicit `show_files` / `show_folders` config toggles.
- Command-palette uninstall workflow.
  - Added uninstall actions.
  - Added quick uninstall mode with command affordances.
- Runtime quality-of-life features.
  - Added live config reload for most safe settings.
  - Added delayed query execution smoothing.
  - Added tray-backed Game Mode toggle.
- Installer improvements.
  - Added startup-choice prompt during install.
  - Added current-user vs all-users install scope support.
- Rebrand from `SwiftFind` to `Nex`.
  - Product/runtime branding, installer naming, config paths, and docs were updated to the new name.

### Changed
- Config format migrated to TOML with legacy JSON fallback.
- Default hotkey changed to `Ctrl+Space`.
- Default web search provider changed to Google.
- App result rows were enriched with publisher/subtitle metadata and cleaner footer/result styling.
- Installed payload was trimmed and icon rebuild tracking was hardened.

### Fixed
- Excluded documentation, samples, FAQ pages, manuals, and web shortcut noise from app discovery.
- Suppressed stale uninstall entries and stale broken app hits more reliably.
- Fixed duplicate config-file opening from the overlay help/config entry.
- Fixed hotkey behavior on desktop shell surfaces and fullscreen-related edge cases.
- Fixed installer scope duplication, shutdown behavior, and related uninstall/registry issues.
- Fixed shell app launching for `shell:` targets via Explorer.
- Fixed multiple Windows overlay polish issues across footer spacing, alignment, and symbol rendering.

## [v2.1.0] - 2026-03-02

### Added
- `--status-json` runtime diagnostics command for machine-readable support and performance reporting.
- Index budget controls:
  - `index_max_items_total`
  - `index_max_items_per_root`
  - `index_max_items_per_query_seed`
- `Trim Memory Now` maintenance action.
- `scripts/windows/profile-memory-and-icons.ps1` for reproducible runtime profiling.

### Changed
- Most safe config changes now hot-apply without restart.
- Discovery provider settings now reconfigure in-process and trigger background reindexing.
- File/folder cache compaction was improved to lower steady-state memory while keeping app items hot.
- Config migration guidance was improved and schema advanced to version `7`.

### Fixed
- Improved runtime reliability and memory behavior for large discovery roots.

## Earlier History

Earlier tags exist in the repository (`v2.2`, `v2`, `v1.x`, `v0.5.0`, and sprint tags), but they are not fully expanded here yet.
