<div align="center">

<img src="apps/assets/nex.svg" alt="Nex" height="90" />

A keyboard-first launcher for Windows. Press a global hotkey to summon a floating search bar and quickly find and launch applications, files, folders, and custom actions.

[![Platform](https://img.shields.io/badge/Platform-Windows-green)](#)
[![License](https://img.shields.io/github/license/haxllo/nex?color=yellow)](LICENSE)
[![GitHub](https://img.shields.io/badge/GitHub-haxllo/nex-blue?logo=github)](https://github.com/haxllo/nex)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)

![Stars](https://img.shields.io/github/stars/haxllo/nex?color=white)

<img src="https://cdn.nexapp.live/UI_2x.webp" alt="UI" width="800" />

</div>

## Features

- **Keyboard-first** — Global hotkey (Alt+Space) summons Nex from anywhere, instantly
- **Fuzzy search** — Tantivy-powered full-text search across apps, files, and folders
- **Everything SDK** — Optional Voidtools Everything integration for real-time file search
- **Calculator** — Inline arithmetic evaluation in the search bar
- **Clipboard history** — Recently copied items at your fingertips
- **Actions & plugins** — Custom commands, web searches, and extensible plugin SDK
- **Game mode** — Automatic suppression while gaming
- **Auto-updater** — Stay current with built-in update mechanism

## Getting Started

### Install

Download the latest installer from the [Releases page](https://github.com/haxllo/nex/releases/latest). Run it — Nex starts in the background, ready on **Alt+Space**.

### Build from Source

```bash
git clone https://github.com/haxllo/nex.git
cd nex
cargo build --release
# Binary: target/release/nex.exe
```

**Requirements:** Windows 10/11 (64-bit), Rust 1.75+

### Configuration

On first launch, Nex creates a config at `%APPDATA%\Nex\config.toml`.

| Setting | Default | Description |
|---|---|---|
| `hotkey` | `Alt+Space` | Global summon shortcut |
| `max_results` | `8` | Results shown |
| `show_files` | `false` | Include files |
| `show_folders` | `false` | Include folders |
| `launch_at_startup` | `false` | Auto-start with Windows |

## Usage

### Search Syntax

| Prefix | Scope |
|---|---|
| *(none)* | Fuzzy search all indexed items |
| `>` | Actions |
| `@` | Applications |
| `:` | Files & folders |
| `?` | Web search |

### Commands

| Command | Description |
|---|---|
| `nex` | Launch background hotkey runtime |
| `nex --foreground` | Dev mode (attached terminal + stdout) |
| `nex --status` | Check if running |
| `nex --status-json` | Machine-readable JSON status |
| `nex --quit` | Stop the running instance |
| `nex --restart` | Restart the instance |
| `nex --diagnostics-bundle` | Dump diagnostics to zip |
| `nex --probe-index` | Check search index status |

## Project Structure

```
nex/
├── apps/core/           # Main Rust application
│   ├── src/
│   │   ├── main.rs      # Entry point
│   │   ├── lib.rs       # Library root (nex_core)
│   │   ├── runtime.rs   # Core orchestration
│   │   ├── overlay/     # WebView2 UI (tao + wry)
│   │   ├── search.rs    # Query DSL
│   │   ├── tantivy_search.rs # Full-text engine
│   │   ├── everything_bridge.rs # Everything SDK
│   │   ├── calculator.rs
│   │   ├── clipboard_history.rs
│   │   ├── plugin_sdk.rs
│   │   ├── updater.rs
│   │   └── config.rs
│   └── Cargo.toml
├── apps/assets/         # Branding assets
├── scripts/             # Build & packaging
├── tests/               # Integration & perf
└── docs/                # Architecture & plans
```

## Architecture

Nex renders its overlay as a native Windows popup using **tao** (window management) and **wry** (WebView2 embedding). All UI is HTML/CSS/JS — no GDI or Direct2D.

| Component | File | Purpose |
|---|---|---|
| Host | `overlay/host.rs` | Event loop, WebView, Win32 chrome, positioning |
| Model | `overlay/model.rs` | Event/state/theme types |
| Icons | `overlay/icons.rs` | LRU cache, base64 PNG encoding |
| Shim | `overlay/shim.rs` | Runtime-to-overlay API |
| Hotkey | `overlay/hotkey.rs` | `RegisterHotKey` + message loop |
| Tray | `overlay/tray.rs` | System tray + context menu |
| Platform | `overlay/platform.rs` | Theme detection, IPC signaling |
| Indexing | `overlay/indexing_progress.rs` | First-time indexing UI |

**Key design decisions:**

- **Fire-and-forget state** — Rust pushes JSON snapshots to WebView via `PostWebMessageAsJson`. No synchronous script evaluation on the critical path.
- **Warm-release** — WebView stays resident for instant open. Icon cache clears ~5 seconds after hide.
- **Acrylic backdrop** — Rounded corners + acrylic blur via DWM APIs, with CSS fallback on older Windows.
- **Cursor-anchored positioning** — Window centers on the monitor under the cursor, upper-third placement (Raycast/Spotlight style).
- **Force-foreground** — `AttachThreadInput` ensures reliable focus on show; winit/tao alone isn't sufficient on Windows.
- **Instance signaling** — Registered window messages let a second process show/quit the running instance.
- **Embedded UI** — HTML, CSS, and JS compiled into the binary via `include_str!`, served through `nexasset://` custom protocol.

## Building & Testing

```bash
cargo build --bin nex              # Debug
cargo build --release --bin nex    # Release
cargo test -p nex                  # Tests
```

## Documentation

- [Architecture Notes](docs/README.md)
- [Changelog](CHANGELOG.md)

## License

MIT — see [LICENSE](LICENSE).
