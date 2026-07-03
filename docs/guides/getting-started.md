<!-- generated-by: gsd-doc-writer -->
# Getting Started with Nex

This guide walks you through installing, running, and configuring Nex for the first time. It covers everything from downloading the binary to making your first search.

---

## Prerequisites

- **Operating system**: Windows 10 or 11 (64-bit)
- **WebView2 Runtime**: Ships with Windows 11 and recent Windows 10 builds. If missing, the [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) is installed automatically by the Nex installer or can be downloaded separately.
- **Hardware**: Any x64 PC. Nex uses ~50–100 MB RAM when idle (WebView2 process released after inactivity).
- **Optional — Rust toolchain**: Required only if building from source. Install via [rustup.rs](https://rustup.rs/). Minimum version 1.75+.

---

## Installation

### Option 1: Installer (Recommended)

1. Download the latest release from the [Releases page](https://github.com/haxllo/nex/releases/latest).
2. Run the `.exe` installer and follow the setup wizard.
3. After installation, Nex starts automatically in the background.

The installer configures launch-at-startup, creates the config file, and copies `nex.exe` to `%LOCALAPPDATA%\Programs\Nex\bin\`.

### Option 2: Install with Cargo

Requires Rust installed.

```bash
cargo install nex
```

Then launch:

```bash
nex
```

### Option 3: Build from Source

```bash
git clone https://github.com/haxllo/nex.git
cd nex
cargo build --release --bin nex
```

The binary is at:

```text
target/release/nex.exe
```

Run it directly or copy it anywhere on your `%PATH%`.

### Option 4: PowerShell Install Script

Use the installer script from the repository for more control:

```powershell
# From the repo root
.\scripts\windows\install-nex.ps1 -BuildFromSource -StartAfterInstall $true
```

Parameters:

| Parameter | Values | Description |
|-----------|--------|-------------|
| `-BuildFromSource` | Switch | Build release binary from source before installing |
| `-LaunchAtStartup` | `Ask` (default), `True`, `False` | Whether to auto-start with Windows |
| `-InstallScope` | `CurrentUser` (default), `AllUsers` | Install location scope |
| `-StartAfterInstall` | `$true` (default), `$false` | Start Nex immediately after install |

---

## First Run

### Start Nex

If you used the installer or `cargo install`, run:

```bash
nex
```

No output appears — Nex runs as a background process with a system tray icon.

If you built from source:

```bash
.\target\release\nex.exe
```

### Verify It's Running

```bash
nex --status
```

Expected output:

```text
Nex is running
```

For machine-readable output:

```bash
nex --status-json
```

### First-Time Behavior

On first launch, Nex:

1. Creates the config directory at `%APPDATA%\Nex\`.
2. Writes a default `config.toml` file.
3. Initializes the SQLite search index at `%APPDATA%\Nex\index.sqlite3`.
4. Scans Windows Start Menu for installed applications.
5. If no cached items exist, shows a brief indexing progress window.
6. Registers the global hotkey (`Ctrl+Space` by default).
7. Adds a system tray icon with a context menu.

Indexing runs in the background and is incremental — the launcher is usable immediately.

---

## Your First Search

1. Press **Ctrl+Space** anywhere on your desktop. A floating search bar appears at the top of your screen.
2. Start typing the name of an application (e.g., `notepad`, `terminal`, `chrome`).
3. Results appear instantly as you type. Press **Enter** to launch the selected item.
4. Press **Esc** or click outside the overlay to dismiss it.

### Search Syntax

| Input | Behavior | Example |
|-------|----------|---------|
| Plain text | Fuzzy search across all indexed items | `calc` → finds Calculator |
| `>` prefix | Search actions (commands) | `> shutdown` → shows shutdown action |
| `@` prefix | Search apps only | `@code` → finds VS Code |
| `:` prefix | Search files and folders only | `:report` → finds report.docx |
| `?` prefix | Web search (opens browser) | `?rust programming` → searches Google |

---

## CLI Reference

All commands run from a terminal (cmd, PowerShell, Windows Terminal):

| Command | Description |
|---------|-------------|
| `nex` | Launch in background (default) |
| `nex --foreground` | Dev mode: keep terminal attached, log to stdout |
| `nex --background` | Explicit background mode |
| `nex --status` | Check if Nex is running |
| `nex --status-json` | Machine-readable status as JSON |
| `nex --quit` | Stop the running instance |
| `nex --restart` | Restart the running instance |
| `nex --ensure-config` | Create default config if missing |
| `nex --sync-startup` | Sync Windows startup entry with config |
| `nex --set-launch-at-startup=true` | Enable auto-start with Windows |
| `nex --set-launch-at-startup=false` | Disable auto-start with Windows |
| `nex --diagnostics-bundle` | Dump diagnostics to a zip archive |
| `nex --probe-index` | Print index statistics |
| `nex --help` / `nex -h` | Print usage summary |

---

## Configuration Basics

### Config File Location

`%APPDATA%\Nex\config.toml`

Open it in any text editor:

```bash
notepad "%APPDATA%\Nex\config.toml"
```

### Common Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `hotkey` | `Ctrl+Space` | Global hotkey to summon/hide the launcher |
| `launch_at_startup` | `true` | Start Nex automatically on Windows sign-in |
| `max_results` | `20` | Number of search results shown |
| `show_files` | `false` | Include files in search results |
| `show_folders` | `false` | Include folders in search results |
| `clipboard_enabled` | `false` | Enable clipboard history (experimental) |
| `web_search_provider` | `google` | Search engine: `google`, `duckduckgo`, `bing`, `brave`, `startpage`, `ecosia`, `yahoo` |

### Quick Changes via CLI

```bash
# Enable auto-start with Windows
nex --set-launch-at-startup=true

# Disable auto-start
nex --set-launch-at-startup=false

# Ensure default config exists
nex --ensure-config
```

For the full settings reference, see [configuration docs](../configuration/configuration.md).

---

## Common Setup Issues

### Hotkey Not Working

- **Cause**: Another application is using the same hotkey (e.g., PowerToys, Discord, or another launcher).
- **Fix**: Change the hotkey in `%APPDATA%\Nex\config.toml` under the `hotkey` setting. Example: `hotkey = "Alt+Space"`. Restart Nex after changing: `nex --restart`.

### Nex Fails to Start

- **Cause**: Missing WebView2 Runtime on older Windows 10 builds.
- **Fix**: Download and install the [WebView2 Evergreen Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/).
- **Diagnose**: Run `nex --foreground` to see error output in the terminal.

### "Nex is not recognized"

- **Cause**: The `nex.exe` binary is not on your `%PATH%`.
- **Fix**: Either:
  - Run from the install directory: `"%LOCALAPPDATA%\Programs\Nex\bin\nex.exe"`, or
  - Add the Nex bin directory to your PATH environment variable.

### Index Seems Empty

- **Cause**: First-time indexing may still be in progress for large directories.
- **Fix**: Run `nex --probe-index` to check index status. Indexing is incremental and non-blocking — apps are available immediately, files populate as the scan completes.

### Config File Won't Save

- **Cause**: The config file is open in another editor or permissions are restricted.
- **Fix**: Close any editor that has `config.toml` open. Check file permissions on `%APPDATA%\Nex\config.toml`. Nex uses atomic writes (write to temp file, rename) so partial writes from a crash are safe.

---

## Next Steps

- [Architecture Overview](../architecture/overview.md) — Understand how Nex works under the hood
- [Configuration Reference](../configuration/configuration.md) — Full settings documentation
- [Changelog](../../CHANGELOG.md) — Release history and version notes
