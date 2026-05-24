# Changelog

All notable changes to Nex are documented in this file.

This changelog is intentionally backfilled from the most reliable sources in the repo: tagged milestones, release notes, and shipped commit history. Older tags can be expanded later if you want a full historical pass.

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
