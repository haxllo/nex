<div align="center">

<img src="apps/assets/nex.svg" alt="Nex" height="90" />

# Nex

A keyboard-first launcher for Windows — press a global hotkey to summon a floating search bar and instantly find and launch anything.

[![crates.io](https://img.shields.io/crates/v/nex-cli?label=crates.io)](https://crates.io/crates/nex-cli)
[![Platform](https://img.shields.io/badge/Platform-Windows-lightgrey)](#)
![CI](https://github.com/haxllo/nex/actions/workflows/ci.yml/badge.svg)
[![License](https://img.shields.io/badge/License-MIT-yellow)](LICENSE)

</div>

---

## Features

- **Global Hotkey** (Ctrl+Space) — summon anywhere
- **Fuzzy Search** — find apps, files, folders by partial name
- **Web Search** — `?query` to search the web
- **Calculator** — inline arithmetic (`2+2`)
- **Emoji Picker** — `:keyword` to find and insert emoji
- **Window Management** — tile layouts, maximize, restore
- **Clipboard History** — recent copied items
- **Everything SDK** — instant file search when Everything is installed
- **Game Mode** — suppress launcher while gaming
- **Extensible** — plugin SDK with WASM distribution path

## Install

### Binary (recommended)

Download the latest installer from the [Releases page](https://github.com/haxllo/nex/releases/latest).

### From source

```bash
git clone https://github.com/haxllo/nex.git
cd nex
cargo build --release
```

Binary at `target/release/nex.exe`. Run it once — config is auto-created at `%APPDATA%\Nex\config.toml`.

## Quick Start

| Command | Action |
|---|---|
| `nex` | Launch in background (Ctrl+Space to show) |
| `nex --status` | Check if running |
| `nex --quit` | Stop the launcher |
| `nex --restart` | Restart |

Type in the search bar to find items. Prefix with `>` for actions, `@` for apps, `:` for files/folders, `?` for web search.

## Search Syntax

| Input | What it does |
|---|---|
| `code` | Fuzzy search apps, files, folders named "code" |
| `>shutdown` | Run a command action |
| `@code` | Filter to apps only |
| `:docs` | Filter to files/folders only |
| `?rust lang` | Web search |
| `:smile` | Emoji picker |
| `= 1024 * 768` | Inline calculation |

## Project Structure

```
nex/
├── apps/
│   ├── core/           # Rust application (crate nex-cli / nex_core)
│   │   ├── src/
│   │   │   ├── main.rs           # Binary entry point
│   │   │   ├── lib.rs            # Library root
│   │   │   ├── runtime.rs        # Runtime lifecycle
│   │   │   ├── windows_overlay/  # GDI+ overlay window
│   │   │   ├── core_service.rs   # Search & launch service
│   │   │   └── ...
│   │   └── tests/
│   └── assets/         # Icons, fonts
├── tests/              # Integration tests
├── scripts/            # Build & packaging
└── docs/               # Architecture & engineering docs
```

## Requirements

- Windows 10/11 (64-bit)
- Rust 1.75+ (to build from source)

## Building

```bash
cargo build              # debug
cargo build --release    # release
cargo test -p nex-cli    # unit tests
```

## Documentation

- [Architecture](docs/README.md)
- [Config Reference](docs/architecture/configuration-spec.md)
- [Changelog](CHANGELOG.md)

## License

MIT
