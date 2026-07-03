<!-- generated-by: gsd-doc-writer -->
# Nex Architecture Overview

Nex is a keyboard-first Windows launcher (analogous to Raycast or Flow) written in Rust. It provides instant search over apps, files, folders, and clipboard history through a WebView2 overlay that appears on a global hotkey (default Ctrl+Space). The system uses a single-process architecture with a background service that owns indexing, search ranking, hotkey registration, and the UI overlay — all in one native executable, avoiding IPC overhead between core logic and presentation.

---

## System Context Diagram

```
┌──────────────────────────────────────────────────────────────────┐
│                          Windows 10/11 x64                        │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐     │
│  │                     nex.exe                              │     │
│  │                                                          │     │
│  │  ┌──────────────┐  ┌───────────────┐  ┌─────────────┐   │     │
│  │  │  Hotkey       │  │  Runtime       │  │  Search     │   │     │
│  │  │  Listener     │─▶│  Worker        │◀─│  Worker     │   │     │
│  │  │  (dedicated   │  │  (event pump)  │  │  (dedicated │   │     │
│  │  │   thread)     │  │               │  │   thread)   │   │     │
│  │  └──────────────┘  └───────┬───────┘  └─────────────┘   │     │
│  │                            │                             │     │
│  │  ┌──────────────┐  ┌───────▼───────┐  ┌─────────────┐   │     │
│  │  │  CoreService  │◀─│  WebView Host │  │  Icon       │   │     │
│  │  │  (search +    │  │  (main thread,│  │  Cache      │   │     │
│  │  │   indexing)   │  │   tao + wry)  │  │  (LRU)      │   │     │
│  │  └──────┬───────┘  └───────────────┘  └─────────────┘   │     │
│  │         │                                                │     │
│  │  ┌──────▼───────────────────────────────────────┐        │     │
│  │  │  Index Store                                  │        │     │
│  │  │  ┌──────────┐   ┌──────────────────┐          │        │     │
│  │  │  │ SQLite   │   │  Tantivy          │          │        │     │
│  │  │  │ (items,  │   │  (full-text       │          │        │     │
│  │  │  │  query   │   │   search index)   │          │        │     │
│  │  │  │  history)│   │  index.tantivy/   │          │        │     │
│  │  │  │          │   │                   │          │        │     │
│  │  │  └──────────┘   └──────────────────┘          │        │     │
│  │  └──────────────────────────────────────────────┘        │     │
│  │                                                          │     │
│  │  ┌──────────────────────────────────────────────────┐    │     │
│  │  │  Discovery Providers                               │    │     │
│  │  │  ┌──────────────────┐  ┌──────────────────────┐    │    │     │
│  │  │  │ Start Menu Apps  │  │ File System          │    │    │     │
│  │  │  │                 │  │ (Walkdir / Everything│    │    │     │
│  │  │  │                 │  │  SDK IPC)            │    │    │     │
│  │  │  └──────────────────┘  └──────────────────────┘    │    │     │
│  │  └──────────────────────────────────────────────────┘    │     │
│  │                                                          │     │
│  │  Background Threads (managed)                            │     │
│  │  ┌─────────────────┐ ┌──────────────┐ ┌───────────────┐ │     │
│  │  │ Stale Pruner    │ │ Indexer      │ │ File Watchers │ │     │
│  │  │ (every 15s)     │ │ (background  │ │ (per root     │ │     │
│  │  │                 │ │  index)      │ │  RDCW)        │ │     │
│  │  └─────────────────┘ └──────────────┘ └───────────────┘ │     │
│  └─────────────────────────────────────────────────────────┘     │
│                                                                  │
│  ┌────────────────┐   ┌──────────────────┐                       │
│  │ Everything     │   │ File System      │                       │
│  │ Service (IPC)  │   │ (NTFS, drives)   │                       │
│  └────────────────┘   └──────────────────┘                       │
└──────────────────────────────────────────────────────────────────┘
```

---

## Crate Architecture

| Package | Cargo Path | Lib Name | Role |
|---------|-----------|----------|------|
| `nex` (bin) | `apps/core/src/main.rs` | — | CLI entry, console handling, `run_with_options` dispatch |
| `nex` (lib) | `apps/core/src/lib.rs` | `nex_core` | All application logic |

The crate is a single Rust workspace member (`apps/core`), edition 2021, targeting `x86_64-pc-windows-gnu` (stable). The binary is compiled with `#![windows_subsystem = "windows"]` in release mode to suppress console allocation on startup.

---

## Module Responsibilities

### Entry & Runtime

| Module | Path | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | CLI argument parsing, parent console attach, `run_with_options` |
| `runtime` | `src/runtime.rs` | `RuntimeOptions`/`RuntimeCommand` types, `run_with_options`, startup orchestration, logging helpers |
| `runtime_loop` | `src/runtime_loop.rs` | Windows main runtime: creates overlay, hotkey, tray, indexer; owns `RuntimeWorker::on_event` message pump |
| `runtime_actions` | `src/runtime_actions.rs` | Action execution logic for built-in actions (diagnostics bundle, web search, uninstall) |
| `runtime_commands` | `src/runtime_commands.rs` | CLI command handlers (`--status`, `--quit`, `--restart`, etc.) |
| `runtime_diagnostics` | `src/runtime_diagnostics.rs` | Status JSON builder, diagnostics bundle writer, query profile summarizer |
| `runtime_hotkey` | `src/runtime_hotkey.rs` | Game mode detection, foreground window snapshot logic |
| `runtime_index` | `src/runtime_index.rs` | Background index refresh lifecycle, config file watcher, queued re-index |
| `runtime_overlay_rows` | `src/runtime_overlay_rows.rs` | Overlay row layout, dedup, uninstall filters, selection navigation |
| `runtime_process` | `src/runtime_process.rs` | Single-instance guard, background process spawn, updater launch |
| `runtime_search_session` | `src/runtime_search_session.rs` | Per-query-session state: prefix cache, adaptive seed limits, result limits |

### Core Service & Index

| Module | Path | Purpose |
|--------|------|---------|
| `core_service` | `src/core_service.rs` | Central `CoreService` struct: search, launch, cache, indexing orchestration, stale pruning, file watcher lifecycle |
| `index_store` | `src/index_store.rs` | SQLite persistence layer: item CRUD, query selection history, provider freshness stamps |
| `tantivy_search` | `src/tantivy_search.rs` | Full-text search index via Tantivy (id, title, path, kind, extension fields) |
| `search` | `src/search.rs` | In-memory ranking: fuzzy match, scoring tiers, personalization boosts, `SearchFilter` |
| `search_worker` | `src/search_worker.rs` | Dedicated search thread: coalesces keystroke requests, holds per-session caches |
| `model` | `src/model.rs` | `SearchItem` struct, text normalization, fuzzy matching utility |
| `query_dsl` | `src/query_dsl.rs` | Query parser: free text, command mode (`>`), DSL filters, time windows |

### Discovery & File Watching

| Module | Path | Purpose |
|--------|------|---------|
| `discovery` | `src/discovery.rs` | `DiscoveryProvider` trait, `StartMenuAppDiscoveryProvider`, `FileSystemDiscoveryProvider` (walkdir), exclusion policies |
| `everything_bridge` | `src/everything_bridge.rs` | Voidtools Everything SDK FFI bridge (dynamic library load, IPC for file discovery) |
| `file_watcher` | `src/file_watcher.rs` | `ReadDirectoryChangesW` per-root watcher, debouncing, event emission |
| `file_watcher_consumer` | `src/file_watcher_consumer.rs` | `FileWatcherHandle`: translates `WatcherEvent` into live index upsert/delete |

### Overlay (WebView2 UI)

| Module | Path | Purpose |
|--------|------|---------|
| `overlay/mod` | `src/overlay/mod.rs` | Module docs, re-exports of `OverlayEvent`, `OverlayRow`, `OverlayRowRole`, `NativeOverlayShell` |
| `overlay/host` | `src/overlay/host.rs` | tao event loop + wry WebView: window creation, IPC, `UiCommand` handler, warm-release timer |
| `overlay/model` | `src/overlay/model.rs` | `OverlayEvent`, `OverlayRow`, `OverlayRowRole`, `ShimState`, `Theme` types |
| `overlay/shim` | `src/overlay/shim.rs` | `NativeOverlayShell`: public imperative API, shared state mutex, `UiCommand` proxy posting |
| `overlay/icons` | `src/overlay/icons.rs` | LRU icon cache: decode `.ico`/`.png` → PNG bytes, base64 data URIs for JS |
| `overlay/hotkey` | `src/overlay/hotkey.rs` | `HotkeyListener`: `RegisterHotKey` + `GetMessageW` on dedicated thread |
| `overlay/tray` | `src/overlay/tray.rs` | System tray icon with context menu (settings, quit, diagnostics) |
| `overlay/platform` | `src/overlay/platform.rs` | System theme detection (registry `AppsUseLightTheme`), instance signaling (FindWindowW) |
| `overlay/indexing_progress` | `src/overlay/indexing_progress.rs` | Standalone progress window for first-time indexing (separate tao + wry instance) |

### Config & Plumbing

| Module | Path | Purpose |
|--------|------|---------|
| `config` | `src/config.rs` | TOML/JSON/JSON5 config loading, validation, migrations (v16), `Config` struct, `SearchMode`, `DiscoveryBackend` |
| `action_executor` | `src/action_executor.rs` | Path launch: `ShellExecuteW`, file:// and shell: protocol handling, error classification |
| `action_registry` | `src/action_registry.rs` | Built-in action IDs and metadata (diagnostics bundle, web search, clipboard, etc.) |
| `clipboard_history` | `src/clipboard_history.rs` | Clipboard polling and history management |
| `contract` | `src/contract.rs` | `CoreRequest`/`CoreResponse` JSON API types (search, launch) |
| `transport` | `src/transport.rs` | JSON request/response serialization for the IPC surface |
| `plugin_sdk` | `src/plugin_sdk.rs` | Plugin loading and registry (provider items + action items from external scripts) |
| `calculator` | `src/calculator.rs` | Inline calculator evaluation |
| `settings` | `src/settings.rs` | Settings UI helpers, hotkey preset suggestions |
| `startup` | `src/startup.rs` | Windows startup registry entry management (Run key) |
| `updater` | `src/updater.rs` | Self-update: discovers and launches `update-nex.ps1` script |
| `uninstall_registry` | `src/uninstall_registry.rs` | Enumerates Windows installed applications for uninstall actions |
| `logging` | `src/logging.rs` | Structured logging with JSON format |
| `console_signal` | `src/console_signal.rs` | Ctrl+C handler for `--foreground` mode |
| `overlay_state` | `src/overlay_state.rs` | `OverlayState` — visible/hidden state machine, `HotkeyAction` enum |

### Index Store

Data is stored across two backends, both at `%APPDATA%\Nex\`:

| Backend | File | Purpose |
|---------|------|---------|
| **SQLite** | `index.sqlite3` | Primary item store with `items` table (id, kind, title, path, use_count, last_accessed, etc.), `query_selections` table for personalization, provider freshness stamps |
| **Tantivy** | `index.tantivy/` | Full-text search index over id, title, path, subtitle, kind, extension. Uses mmap directory. Built from SQLite on startup, kept in sync incrementally. |

---

## Data Flow: Keystroke to Results

```
User presses Ctrl+Space
  │
  ▼
Hotkey Listener Thread (nex-hotkey-listener)
  ├── RegisterHotKey(NULL, 1, MOD_CONTROL, VK_SPACE)
  ├── GetMessageW loop blocks on WM_HOTKEY
  ├── Receives WM_HOTKEY
  └── Sends OverlayEvent::Hotkey(1) via crossbeam channel
  │
  ▼
Runtime Worker Thread (nex-runtime) — event pump
  ├── Recv OverlayEvent::Hotkey(1)
  ├── OverlayState::on_hotkey() → HotkeyAction::ShowAndFocus
  ├── Calls overlay.show() → posts UiCommand::Show via EventLoopProxy
  │
  ▼
Main Thread — tao event loop
  ├── Recv UiCommand::Show
  ├── Lazily creates wry WebView if released
  ├── Shows WS_POPUP window with acrylic backdrop
  ├── WebView loads embedded HTML/CSS/JS from assets/
  ├── JS fires registration: window.nex.apply(state)
  └── WebviewReady sent back to event channel
  │
  ▼
User types query in overlay text input
  │
  ▼
WebView IPC handler
  ├── Sends OverlayEvent::Input(key_data) to event channel
  │     (each keystroke)
  │
  ▼
Runtime Worker Thread
  ├── Buffers query text across Input events
  ├── Sends SearchRequest { generation, query, max_results } via mpsc channel
  │
  ▼
Search Worker Thread (nex-search-worker)
  ├── Coalesces: drains any pending SearchRequests (drops intermediate)
  ├── Keeps only the latest request
  ├── Acquires service.try_write() lock (non-blocking)
  ├── If Tantivy index has candidates:
  │   ├── Query Tantivy → get pre-ranked candidate list
  │   ├── Re-rank in-memory with personalization boosts
  │   └── Augment with in-memory cache if under limit
  ├── Else (no Tantivy results):
  │   └── Scan in-memory cached_items list with ranking
  ├── Returns SearchResult { generation, results } via mpsc channel
  │
  ▼
Runtime Worker Thread
  ├── Checks generation matches (discards stale results)
  ├── Deduplicates results (same title, same normalized path)
  ├── Applies uninstall suppression filters
  ├── Arranges results into overlay rows with icons
  ├── Pushes ShimState to NativeOverlayShell
  └── NativeOverlayShell posts UiCommand::Apply via EventLoopProxy
  │
  ▼
Main Thread — tao event loop
  ├── Recv UiCommand::Apply
  ├── Serializes state to JSON (rows, selection index, theme)
  ├── Calls ICoreWebView2::PostWebMessageAsJson
  └── JS event handler → applies state → re-renders list
```

---

## Threading Model

| Thread Name | Count | Created By | Purpose |
|-------------|-------|-----------|---------|
| **Main** | 1 | OS | tao event loop + wry WebView host. Required by tao/winit. Drives show/hide/resize, IPC, warm-release timer. |
| `nex-runtime` | 1 | `runtime_loop.rs` | Event pump: drains `OverlayEvent` channel, calls `on_event` for each message. Owns search session, config watcher, overlay state. |
| `nex-hotkey-listener` | 1 | `overlay/hotkey.rs` | `RegisterHotKey(NULL, ...)` + `GetMessageW` loop. Forwards `WM_HOTKEY` as `OverlayEvent::Hotkey` to event channel. |
| `nex-search-worker` | 1 | `search_worker.rs` | Coalesces keystroke requests, runs Tantivy/in-memory search, returns results. Holds per-session `OverlaySearchSession`. |
| *unnamed* | 1 | `runtime_index.rs` | Background index rebuild: creates temporary `CoreService`, runs `rebuild_index_incremental_with_report`. Signals completion via `AtomicBool`. |
| `nex-stale-pruner` | 1 | `core_service.rs` | Every 15 s: scans cached items in batches, removes stale (deleted-on-disk) entries from SQLite, Tantivy, and in-memory cache. Uses `try_write` to never block searches. |
| `nex-tray-updater` | 1 | `runtime_loop.rs` | Listens on two crossbeam channels for game mode and hotkey issue state changes; updates tray icon accordingly. |
| *unnamed* | 1 per root | `file_watcher.rs` | `ReadDirectoryChangesW` loop on each configured root directory. Posts `WatcherEvent` to consumer thread via mpsc. |
| *unnamed* | 1 per root | `file_watcher_consumer.rs` | Receives `WatcherEvent`s, debounces, applies exclusion policy, upserts/deletes items in `CoreService`. |
| `nex-test-show` | 0-1 | `runtime_loop.rs` | (CI only) Sends synthetic `OverlayEvent::Hotkey(1)` after 2 s delay for automated testing. |

### Thread Safety Strategy

- **CoreService** is wrapped in `Arc<RwLock<CoreService>>`. The search worker acquires a `try_write()` lock — if the background indexer holds the write lock, the search tick is skipped and retried on the next event.
- **`CoreService` internal caches** (`cached_items`, `cached_app_items`, `config`) use individual `RwLock`s so the search path reads without blocking index writes.
- **Tantivy** uses `Mutex<IndexWriter>` for writes; the `IndexReader` is cloned per query (cheap, wait-free).
- **Everything SDK** uses a process-global `Mutex<()>` (`SDK_LOCK`) because the SDK stores query state in global variables.
- **Warm-release timer** posts a `UiCommand::Teardown(generation)` via the event loop proxy; the actual WebView drop happens on the main thread.
- **Icon cache** uses `Mutex<Inner>` — no lock contention because it's accessed only during overlay state push (serialized by the runtime worker).

---

## Configuration

- **Format**: TOML (primary), with JSON/JSON5 backward compatibility.
- **Location**: `%APPDATA%\Nex\config.toml`
- **Version**: `CURRENT_CONFIG_VERSION = 16` with automatic migration in `apply_migrations()`.
- **Key settings**: hotkey, launch_at_startup, max_results, discovery_roots, file_discovery_backend (Auto/Everything/Walkdir), search_mode_default, game_mode_enabled, ui_warm_release_ms, active_memory_target_mb.
- **Template**: Written by `write_user_template_toml()` on first launch.

---

## Overlay Architecture

The overlay uses a **WS_POPUP** window hosting a **WebView2** control via `tao + wry`. No GDI/GDI+/D2D — all rendering is HTML/CSS/JS.

- **HTML/CSS/JS** are embedded in the binary via `include_str!` in `overlay/host.rs` (assets at `apps/core/assets/`).
- **State push**: `ICoreWebView2::PostWebMessageAsJson` — fire-and-forget, never blocks the host event loop.
- **Icons**: Decoded from `.ico`/`.png`, cached as PNG bytes in an LRU (`overlay/icons.rs`), served as base64 data URIs embedded in the state JSON snapshot.
- **Positioning**: Placed on the monitor under the cursor, resized to hug web content height.
- **Window effects**: Acrylic backdrop via `window-vibrancy` crate. Mica backdrop planned via `DWMWA_SYSTEMBACKDROP_TYPE`.
- **Warm-release**: After the overlay is hidden, a timer thread waits `ui_warm_release_ms` then drops the WebView (and its heavy Chromium child processes). Re-created lazily on next show.
- **Theme detection**: Windows registry `AppsUseLightTheme` (`overlay/platform.rs`).

---

## Discovery & Indexing

### Discovery Providers

1. **StartMenuAppDiscoveryProvider**: Enumerates Windows Start Menu shortcuts (`shell:AppsFolder`, `%ProgramData%\Microsoft\Windows\Start Menu`, `%AppData%\Microsoft\Windows\Start Menu`).
2. **FileSystemDiscoveryProvider**: Walks configured `discovery_roots` using either:
   - **walkdir**: Pure Rust recursive directory traversal.
   - **Everything SDK** (via IPC): Queries the Voidtools Everything service for near-instant file enumeration.

Both apply exclusion policies (well-known dirs like `node_modules`, `.git`, `appdata`, system dirs).

### Indexing Lifecycle

1. **Startup**: `run_windows_runtime` checks `cached_items_len()`. If empty (first run), spawns a progress window and runs full index rebuild. If non-empty, spawns background async index refresh.
2. **Background refresh**: Temporary `CoreService` runs `rebuild_index_incremental_with_report()` on a spawned thread. Results are applied when the runtime worker acquires the service write lock.
3. **File watchers**: After the first cache is applied, per-root `ReadDirectoryChangesW` watchers start. Events are debounced (200 ms), deduplicated, and applied to the live index.
4. **Stale pruner**: Every 15 s, 16 cached items are checked for staleness. Entries pointing to deleted files are removed.
5. **Cache compaction**: On every `refresh_cache_from_store()`, the in-memory cache is compacted: file/folder items beyond `effective_file_seed_cap` are dropped, constrained by `active_memory_target_mb`.

---

## Key Design Decisions

- **Single-process** avoids IPC overhead between core and UI — all data flows through Rust `Arc` and channels.
- **Per-keystroke search** is handled by the `SearchWorker` on a dedicated thread with request coalescing (only the latest query is processed).
- **`try_write`** on the service lock ensures a background index rebuild never delays the message pump. Missed ticks retry on the next `OverlayEvent`.
- **Warm-release** tears down the Chromium WebView process when idle, keeping memory footprint low when the launcher is hidden.
- **Tantivy as secondary index**: SQLite is the source of truth; Tantivy is rebuilt from SQLite data and used for full-text search performance. On schema change or corruption, the Tantivy index is reset and rebuilt.
