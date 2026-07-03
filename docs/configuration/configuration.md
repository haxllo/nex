<!-- generated-by: gsd-doc-writer -->
# Configuration

Nex uses a TOML configuration file auto-created on first launch. Also supports JSON and JSON5 (legacy backward compatibility).

## Config File

| Property | Value |
|---|---|
| **Primary path** | `%APPDATA%\Nex\config.toml` |
| **Legacy paths** | `%APPDATA%\Nex\config.json`, `%APPDATA%\SwiftFind\config.json` |
| **Format** | TOML (`.toml`). JSON/JSON5 read but written as TOML. |
| **Write strategy** | Atomic temp-file write + replace. Backups kept (last 5). |
| **Version** | `16` (field: `version`). Migrations applied via `apply_migrations()`. |
| **Index database** | `%APPDATA%\Nex\index.sqlite3` (configurable via `index_db_path`) |

Config is loaded at startup. Nex watches the file for changes and hot-applies most settings. Hotkey and `index_db_path` changes require a restart.

## Settings

All settings, their defaults, valid ranges, and descriptions:

| Setting | Type | Default | Range | Description |
|---|---|---|---|---|
| `version` | `u32` | `16` | — | Config schema version. Managed by migrations. |
| `hotkey` | `string` | `"Ctrl+Space"` | — | Global hotkey to toggle the overlay. |
| `launch_at_startup` | `bool` | `true` | — | Start Nex automatically when you sign in. |
| `max_results` | `u16` | `20` | `5..100` | Number of results shown per query. |
| `show_files` | `bool` | `false` | — | Show files in search results. |
| `show_folders` | `bool` | `false` | — | Show folders in search results. |
| `search_mode_default` | `string` | `"all"` | `all`, `apps`, `files`, `actions`, `clipboard` | Default search mode. |
| `search_dsl_enabled` | `bool` | `true` | — | Enable query operators (`kind:`, `modified:`, `created:`, `AND`, `OR`, `NOT`, `-term`). |
| `uninstall_actions_enabled` | `bool` | `true` | — | Enable command-mode uninstall actions (`> uninstall appname`). |
| `web_search_provider` | `string` | `"google"` | `google`, `duckduckgo`, `bing`, `brave`, `startpage`, `ecosia`, `yahoo`, `custom` | Web search provider for command mode. |
| `web_search_custom_template` | `string` | `""` | — | URL template for custom provider. Must include `{query}` placeholder. |
| `clipboard_enabled` | `bool` | `false` | — | Enable clipboard history provider. |
| `clipboard_retention_minutes` | `u32` | `480` | `5..43200` | Clipboard entry retention in minutes (default 8 hours). |
| `clipboard_exclude_sensitive_patterns` | `string[]` | `["password", "passcode", "otp", "token", "secret", "apikey", "api_key"]` | — | Substring patterns that prevent clipboard capture. |
| `file_discovery_backend` | `string` | `"auto"` | `auto`, `everything`, `walkdir` | File/folder discovery backend. `auto`: try Everything SDK, fall back to walkdir. |
| `plugins_enabled` | `bool` | `true` | — | Enable plugin SDK. |
| `plugins_safe_mode` | `bool` | `true` | — | Prevent plugin command execution when `true`. |
| `plugin_paths` | `string[]` | `["%APPDATA%\\Nex\\plugins"]` | — | Directories scanned for plugins. |
| `game_mode_enabled` | `bool` | `false` | — | Suppress the hotkey while a fullscreen/game app is active. |
| `idle_cache_trim_ms` | `u32` | `900` | `100..10000` | Cache trim delay after overlay hide (ms). |
| `active_memory_target_mb` | `u16` | `72` | `20..512` | Active memory target in MB. |
| `ui_warm_release_ms` | `u32` | `5000` | `500..600000` | How long (ms) the WebView stays resident after hide before teardown. |
| `index_max_items_total` | `u32` | `120000` | `10000..2000000` | Maximum indexed items across all roots. |
| `index_max_items_per_root` | `u32` | `40000` | `1000..1000000` | Maximum indexed items per discovery root. Must be ≤ `index_max_items_total`. |
| `index_max_items_per_query_seed` | `u32` | `5000` | `250..200000` | Runtime candidate budget for per-query file/folder retrieval. |
| `discovery_roots` | `string[]` | `["C:\\Users\\<user>"]` | — | Folders scanned for local file/folder discovery. |
| `discovery_exclude_roots` | `string[]` | `["%USERPROFILE%\\AppData\\Local\\Temp", "%USERPROFILE%\\AppData\\Local\\Microsoft\\Windows\\INetCache"]` | — | Folders excluded from file discovery. |

### Internal / Managed Fields

These fields are managed by Nex and written into the saved config:

| Field | Type | Description |
|---|---|---|
| `config_path` | `string` | Absolute path to the active config file. |
| `index_db_path` | `string` | Absolute path to the SQLite index database. |
| `hotkey_help` | `string` | Inline help text for hotkey configuration. |
| `hotkey_recommended` | `string[]` | List of recommended hotkey alternatives. |

### Search Mode Values

| Value | Description |
|---|---|
| `all` | All result types (default). |
| `apps` | Installed applications only. |
| `files` | Local files only. |
| `actions` | Command-mode actions only. |
| `clipboard` | Clipboard history only. |

### Discovery Backend Values

| Value | Description |
|---|---|
| `auto` | Try Everything SDK (`Everything64.dll`/`Everything32.dll`); fall back to walkdir + `ReadDirectoryChangesW`. |
| `everything` | Require Everything. Fails back to walkdir with a warning. |
| `walkdir` | Always use built-in walkdir + `ReadDirectoryChangesW`. |

### Web Search Provider Values

`google`, `duckduckgo`, `bing`, `brave`, `startpage`, `ecosia`, `yahoo`, `custom`

When set to `custom`, `web_search_custom_template` must be a URL containing `{query}`.

## Built-in Discovery Exclusions

Nex automatically skips these paths during file/folder discovery in addition to `discovery_exclude_roots`:

- System roots: `Windows`, `Program Files`, `$Recycle.Bin`, `System Volume Information`
- User noise: `AppData`
- Dev/cache: `node_modules`, `.git`, `.venv`, `venv`, `__pycache__`, `dist`, `build`, `.gradle`, `.m2`
- Sensitive: `.ssh`, `.dropbox.cache`
- Special files: `pagefile.sys`, `hiberfil.sys`

Start-menu app discovery is independent of these filesystem exclusions.

## Environment Variables

| Variable | Legacy Name | Purpose |
|---|---|---|
| `NEX_SUPPRESS_STDIO` | `SWIFTFIND_SUPPRESS_STDIO` | Set to `1` or `true` to suppress stdout logging. |
| `NEX_WINDOWS_RUNTIME_SMOKE` | — | Set to `1` to enable the Windows runtime smoke test (CI only). |
| `NEX_ALLOW_MISSING_ICON` | `SWIFTFIND_ALLOW_MISSING_ICON` | Set to `1` to allow release build without `nex.ico`. |

## CLI Flags

Nex is a Windows GUI subsystem application. When run from a terminal, it reattaches to the parent console for CLI output. All flags are used as standalone arguments.

| Flag | Description |
|---|---|
| *(none)* | Normal mode: launches Nex as a background hotkey runtime. Default. |
| `--background` | Force background mode (default). |
| `--foreground` | Dev mode: keeps the terminal attached. |
| `--status` | Print runtime status to stdout. |
| `--status-json` | Print runtime status as JSON to stdout. |
| `--quit` | Stop the running instance. |
| `--restart` | Restart the running instance. |
| `--ensure-config` | Ensure the config file exists (create default if missing). |
| `--sync-startup` | Sync the startup registry entry with config. |
| `--set-launch-at-startup=<true/false>` | Enable or disable launch-at-startup. |
| `--diagnostics-bundle` | Dump diagnostics to a zip archive. |
| `--probe-index` | Print index statistics. |
| `--help` / `-h` | Print usage summary. |

## Validation Rules

Config is validated on load and save. Violations produce a startup error:

- `hotkey` must be a non-empty string and pass runtime hotkey validation.
- `max_results`: `5..100`.
- `index_db_path` and `config_path` must be present.
- `clipboard_retention_minutes`: `5..43200`.
- `idle_cache_trim_ms`: `100..10000`.
- `active_memory_target_mb`: `20..512`.
- `ui_warm_release_ms`: `500..600000`.
- `index_max_items_total`: `10000..2000000`.
- `index_max_items_per_root`: `1000..1000000`; must be ≤ `index_max_items_total`.
- `index_max_items_per_query_seed`: `250..200000`.
- `discovery_roots`, `discovery_exclude_roots`, `plugin_paths`: no empty entries.
- `clipboard_exclude_sensitive_patterns`: no empty patterns.
- `version`: must be `>= 1`.
- When `web_search_provider = "custom"`, `web_search_custom_template` must be non-empty and contain `{query}`.

## Example

```toml
hotkey = "Ctrl+Space"
launch_at_startup = true
max_results = 20
show_files = false
show_folders = false
search_mode_default = "all"
search_dsl_enabled = true
uninstall_actions_enabled = true

discovery_roots = [
  "C:\\Users\\Admin",
]

discovery_exclude_roots = [
  "C:\\Users\\Admin\\AppData\\Local\\Temp",
  "C:\\Users\\Admin\\AppData\\Local\\Microsoft\\Windows\\INetCache",
]

file_discovery_backend = "auto"
clipboard_enabled = false
clipboard_retention_minutes = 480
clipboard_exclude_sensitive_patterns = [
  "password",
  "passcode",
  "otp",
  "token",
  "secret",
  "apikey",
  "api_key",
]

web_search_provider = "google"

plugins_enabled = true
plugins_safe_mode = true
plugin_paths = [
  "C:\\Users\\Admin\\AppData\\Roaming\\Nex\\plugins",
]

idle_cache_trim_ms = 900
active_memory_target_mb = 72
ui_warm_release_ms = 5000

index_max_items_total = 120000
index_max_items_per_root = 40000
index_max_items_per_query_seed = 5000
```

## Migration & Backup Behavior

- When Nex detects a legacy config (`config.json` or `SwiftFind\config.json`), it reads it, applies schema migrations, and writes the migrated config as TOML to `%APPDATA%\Nex\config.toml`.
- The original legacy file is preserved with a `config.v<N>-backup-<timestamp>.json` name in the new config directory.
- On save, the previous config is backed up as `config.backup-<timestamp>.toml`. Only the 5 most recent backups are kept.
- Config write uses atomic rename (write to `.nex-config-<timestamp>.tmp`, rename to target, cleanup). A `.nex-config.backup` file serves as a crash-recovery safety net.
