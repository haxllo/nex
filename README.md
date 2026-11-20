<div align="center">

<img src="https://cdn.nexapp.live/nex.png" alt="UI" height="80"/><br>


A keyboard-first launcher for Windows. Press a global hotkey to summon a floating search bar and quickly find and launch applications, files, folders, and custom actions.<br><br>

  <a href="https://github.com/haxllo/nex"><img src="https://www.shieldcn.dev/github/stars/haxllo/nex.svg?variant=secondary&size=xs&theme=zinc" alt="GitHub Stars" /></a>
  <a href="https://x.com/haxllo_"><img src="https://www.shieldcn.dev/x/follow/haxllo_.svg?variant=branded&size=xs&theme=zinc" alt="X Follow" /></a>
  <a href="https://github.com/haxllo/nex/actions"><img src="https://www.shieldcn.dev/github/ci/haxllo/nex.svg?variant=secondary&size=xs&theme=zinc" alt="CI" /></a><br>
  <a href="https://github.com/haxllo/nex/blob/master/LICENSE"><img src="https://www.shieldcn.dev/github/license/haxllo/nex.svg?variant=secondary&size=xs&theme=zinc" alt="MIT License"/></a>
  <a href="https://github.com/haxllo/nex/actions"><img src="https://www.shieldcn.dev/github/merged-prs/haxllo/nex.svg?variant=secondary&size=xs&theme=zinc" alt="Merged PR"/></a>
  <a href="https://www.rust-lang.org/"><img src="https://www.shieldcn.dev/badge/-Rust-000000.svg?logo=rust&variant=secondary&size=xs&theme=zinc" alt="Rust"/></a>
</div>

<br>

<div align="center">
  <img src="https://cdn.nexapp.live/demo-v2.webp" alt="Press Win, type, launch" width="900"/>
</div>

<br>

**Press `Win`. Type. Enter.** That is the whole app.

Nex replaces the Start menu. The panel drops in under your cursor, indexes your
whole drive, and usually has the thing you want by the third character. No
window switching, no mouse, no plugin to configure first.

Built for anyone who opens the same handful of apps and folders all day and is
tired of hunting through the Start menu for them.

## Install

**[Download the latest release](https://github.com/haxllo/nex/releases/latest)**
— installer or portable zip, whichever you prefer.

Run it and Nex starts in the background, ready on the **Win** key. No setup,
no account, no configuration file to edit.

> **Requirements:** Windows 10 or 11, 64-bit. The helper process launches
> elevated so the hotkey still works over Task Manager and Settings. Windows
> asks for that once, on first run.

## Why Nex

Most launchers make you wait for a window, or make you configure a plugin
before they do anything useful. Nex is built around three things:

- **It opens instantly.** The WebView stays resident after you hide the panel,
  so summoning it is a show rather than a cold start.
- **It searches the real disk.** A native MFT scan reads the NTFS master file
  table directly instead of walking directories, so a full-drive index is
  practical rather than a background job that never finishes.
- **It looks like part of Windows.** Acrylic backdrop, rounded corners and a
  contact shadow, with a CSS fallback on older builds.

## Features

| | |
|---|---|
| **Keyboard-first** | Glows up on the Win key from anywhere, replacing the Start menu |
| **Fuzzy search** | Tantivy full-text index across apps, files and folders |
| **Native discovery** | NTFS MFT scan, with a walkdir fallback for non-NTFS roots |
| **Live updates** | File and application watchers keep results current without a rescan |
| **Calculator** | Inline arithmetic in the search bar |
| **Clipboard history** | Recently copied items, one keystroke away |
| **Assistant** | Ask in plain words; it can read, write, move and delete files and run commands, behind a confirmation card |
| **Actions** | Custom commands and web searches, run straight from a result |
| **Game mode** | Suppresses the hotkey over fullscreen games, and says so in the tray |
| **Auto-update** | Built-in updater, with What's New shown after it installs |

## Usage

Press **Win**, type, press **Enter**.

- **Right-click a result** for run as admin, copy path, open containing folder,
  uninstall, and pin or unpin.
- **Tray icon** for quit, restart, and game-mode status.

<details>
<summary><b>Search syntax</b></summary>

<br>

| Prefix | Scope |
|---|---|
| *(none)* | Fuzzy search across everything indexed |
| `>` | Command mode: actions and web searches |
| `@apps` | Applications only |
| `@files` | Files only |
| `@folders` | Folders only |
| `kind:file`, `ext:md` | Key/value filters |

</details>

<details>
<summary><b>Command-line flags</b></summary>

<br>

Not needed for daily use, but available:

| Command | Description |
|---|---|
| `NexUIProcess.exe --foreground` | Run attached to a terminal |
| `NexUIProcess.exe --status` | Check whether Nex is running |
| `NexUIProcess.exe --quit` | Stop the running instance |
| `NexUIProcess.exe --restart` | Restart the instance |
| `NexUIProcess.exe --diagnostics-bundle` | Write a diagnostics zip |

</details>

<details>
<summary><b>Configuration</b></summary>

<br>

On first launch Nex writes `%APPDATA%\Nex\config.toml`. The index lives beside
it in `%APPDATA%\Nex\index.sqlite3`.

| Setting | Default | Description |
|---|---|---|
| `hotkey` | `Win` (left) | Global summon shortcut |
| `max_results` | `20` | Results shown per query |
| `show_files` | `false` | Include files in results |
| `show_folders` | `false` | Include folders in results |
| `launch_at_startup` | `true` | Start with Windows |

Most settings are also editable from the settings view in the panel.

</details>

## Build from source

**Requirements:** Windows 10 or 11, 64-bit. [Rust](https://rustup.rs) stable.
The MSVC or GNU toolchain both work.

```bash
git clone https://github.com/haxllo/nex.git
cd nex
cargo build --release --bin NexUIProcess --bin NexIndexerProcess --bin NexHelper
```

Then run `target/release/NexUIProcess.exe --foreground`. The `--foreground`
flag keeps the terminal attached, which is what you want while developing.

> Building needs all three binaries, not just the UI. `NexUIProcess.exe` owns
> the overlay and search, `NexIndexerProcess.exe` owns discovery and the index,
> and `NexHelper.exe` handles input from elevated windows. The UI launches the
> other two itself.

## Architecture

Three processes, split so that none of them does another's job:

| Process | Integrity | Owns |
|---|---|---|
| `NexUIProcess.exe` | Medium | Overlay and WebView2, hotkeys, tray, settings, search over the published index snapshot |
| `NexIndexerProcess.exe` | Medium | Discovery, SQLite writes, file and app watchers, Tantivy |
| `NexHelper.exe` | High | Elevated hotkey detection, foreground grant, input simulation |

The UI spawns and supervises the other two. The indexer watches its parent and
exits if the UI disappears. The helper is a privileged broker and never owns
process lifecycle.

The split is not decoration. A Medium-integrity process cannot receive input
destined for an elevated window, so hotkey detection for Task Manager and
Settings has to live in the helper and be forwarded back over a pipe.

<details>
<summary><b>Design decisions worth knowing</b></summary>

<br>

- **Fire-and-forget state.** Rust pushes JSON snapshots to the WebView with
  `PostWebMessageAsJson`. Nothing blocks on synchronous script evaluation.
- **Warm release.** The WebView stays alive between shows. Only the icon cache
  is trimmed, a few seconds after hiding.
- **Embedded UI.** HTML, CSS and JS are compiled into the binary with
  `include_str!` and served over a `nexasset://` custom protocol, so there is
  no asset directory to lose.
- **Cursor-anchored positioning.** The panel centres on the monitor under the
  cursor, in the upper third, Raycast style.
- **Forced foreground.** `AttachThreadInput` is required for reliable focus on
  Windows; the window manager alone is not enough.

</details>

<details>
<summary><b>Project layout</b></summary>

<br>

```
apps/
├── core/       nex_core — the engine shared by every binary
│   └── src/
│       ├── overlay/    WebView2 host, hotkeys, tray, icons (tao + wry)
│       ├── chat/       assistant, tool calling, confirmation cards
│       ├── discovery   MFT and walkdir file discovery
│       └── ...         search, config, calculator, clipboard, updater
├── ui/         NexUIProcess — overlay, tray, search UI
├── indexer/    NexIndexerProcess — discovery and indexing
└── helper/     NexHelper — elevated input broker
```

Each binary is its own package on purpose: a Windows resource script is linked
into every binary of its package and inherited by dependents, so sharing one
would give all three processes the same Task Manager name.

</details>

## Development

```bash
cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess
cargo test -p nex                        # unit tests
cargo test -p nex --lib chat             # one module
cargo test -p nex-indexer --test process_isolation_test   # IPC end to end
```

Build output goes to `target/`. If that disk is tight, or a single-job build
suits your machine better, point Cargo somewhere else:

```bash
export CARGO_TARGET_DIR=~/nex-target CARGO_BUILD_JOBS=1
```

Web assets (`index.html`, `style.css`, `app.js`) are embedded at compile time.
Editing them without rebuilding changes nothing at runtime.

## Documentation

- [Architecture notes](docs/README.md)
- [Changelog](CHANGELOG.md)
- [What's New](docs/whats-new.md)

## License

MIT. See [LICENSE](LICENSE).
