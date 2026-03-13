# System Architecture

## Stack Direction

- Core service: Rust
- UI shell: Native Win32 owner-draw overlay (inside `nex`)
- Local storage: SQLite
- Config format: JSON

## Process Model

- `nex.exe`
- Always-on background process
- Owns hotkey registration, indexing, search, ranking, and launching
- Hosts and renders the floating search bar overlay directly (no separate UI process)

Rationale:
- Keeps heavy logic in one fast native service
- Removes IPC/process orchestration overhead between core and UI

## High-Level Components

- `HotkeyManager`
- Registers and handles global shortcut events

- `OverlayController`
- Opens and closes floating window, sets focus, drives keyboard events

- `DiscoveryService`
- Enumerates app sources and configured file roots

- `Indexer`
- Builds and updates searchable index from discovery results

- `SearchEngine`
- Executes fuzzy matching and ranking against precomputed tokens

- `ActionExecutor`
- Launches apps and files, opens parent folders, handles elevated action requests

- `SettingsService`
- Reads and validates user config, emits updates to subscribed components

- `TelemetryService` (optional)
- Local metrics in MVP, remote only if user opt-in exists

## Data Flow

1. Core starts and loads config.
2. Indexer initializes cached index from SQLite.
3. Hotkey press triggers overlay open.
4. Overlay input events are handled in-process.
5. SearchEngine returns ranked results for direct overlay rendering.
6. User action is sent to ActionExecutor.
7. Core records usage event for ranking improvements.

## Data Model (MVP)

`SearchItem`:
- `id`: stable identifier
- `kind`: app | file | folder | command
- `title`: display name
- `path`: filesystem path or command payload
- `tokens`: normalized search tokens
- `last_used_at`: timestamp
- `use_count`: integer

## Performance Strategy

- Keep hot index data in memory
- Use incremental refresh, not full rescans
- Precompute normalized tokens
- Return compact payloads to UI
- Debounce query pipeline by a few milliseconds to reduce churn
