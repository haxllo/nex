# Settings

Settings live in `%APPDATA%\Nex\config.toml`. Nex writes the file with comments on
first launch. Most keys also appear in the in-app Settings view, which writes the
same file.

## How changes take effect

| Change | Applied |
|---|---|
| Any key the Settings view edits | Within about a second, without a restart |
| `discovery_roots`, `discovery_exclude_roots`, `show_files`, `show_folders`, `discovery_max_depth`, `file_discovery_backend` | The provider and watchers are rebuilt and the index refreshes |
| `hotkey` | The hotkey is re-registered |
| `index_db_path` | After a restart |

The file is watched, so editing it in a text editor works as well as using the
Settings view. Windows paths need doubled backslashes: `C:\\Users\\you\\Docs`.

The `version` key is the internal schema version. Do not edit it; migrations read
it and a wrong value makes Nex re-run migrations that already happened.

## Starter

These are the keys people change first.

| Key | Default | Meaning |
|---|---|---|
| `hotkey` | `"Win"` | The global shortcut. `"Win"` alone is safe; `Alt+Tab` and `Ctrl+Esc` are not. |
| `launch_at_startup` | `true` | Start in the background when you sign in |
| `max_results` | `20` | Rows shown per query, 5 to 100 |
| `show_files` | `true` | Include files in results |
| `show_folders` | `true` | Include folders in results |
| `panel_background` | `"glass"` | `glass`, `black`, or `white` |
| `view_mode` | `"expanded"` | `expanded` shows the idle app list; `compact` is a bare search field until you type |
| `grid_view` | `true` | Tiles for applications. Files, folders, and actions always stay as rows. |
| `game_mode_enabled` | `false` | Suppress the hotkey while a fullscreen game has focus |

## What is indexed

| Key | Default | Meaning |
|---|---|---|
| `discovery_roots` | Your user profile | Folders scanned for files and folders |
| `discovery_exclude_roots` | `AppData\Local\Temp`, `AppData\Local\Microsoft\Windows\INetCache` | Folders never scanned |
| `discovery_max_depth` | `0` | Folder depth below each root, 0 meaning unlimited, 256 maximum |
| `file_discovery_backend` | `"auto"` | `auto` uses the NTFS master file table where it pays off and falls back otherwise. `walkdir` always walks directories. |
| `index_max_items_total` | `500000` | Cap on rows held in the index |
| `index_max_items_per_root` | `200000` | Cap per discovery root |
| `index_max_items_per_query_seed` | `5000` | Candidates pulled from the full-text index per file query, 250 to 200000 |

Beyond `discovery_exclude_roots`, Nex always skips common noise: `Windows`,
`Program Files`, `AppData`, `node_modules`, `.git`, `__pycache__`. Those built-in
exclusions affect file and folder indexing only, never application discovery.

The old `everything` backend value still loads and is treated as `auto`. The
Everything SDK backend no longer exists.

## Search behaviour

| Key | Default | Meaning |
|---|---|---|
| `search_mode_default` | `"all"` | `all`, `apps`, `files`, `folders`, `actions`, or `clipboard` |
| `search_dsl_enabled` | `true` | Enables `@`, `>`, `ext:`, `modified:`, `AND`, `OR`, `NOT`, and `-term` |
| `uninstall_actions_enabled` | `true` | Offer uninstall actions in command mode |
| `open_url_in_default_browser` | `true` | Treat a bare domain as a link |
| `url_tlds` | `com`, `net`, `org`, `io`, and 29 more | Which endings make a typed word a domain |
| `create_actions_enabled` | `true` | Offer to create a file or folder when nothing matches |
| `create_file_extensions` | `txt`, `md`, `py`, `ps1`, `bat`, `cmd`, `js`, `ts`, `json`, `yaml`, `yml`, `toml`, `ini`, `cfg`, `log`, `csv`, `html`, `css` | Extensions the create row offers |
| `default_create_dir` | Empty | Where created files go. Empty means your Desktop. |
| `web_search_provider` | `"google"` | `google`, `duckduckgo`, `bing`, `brave`, `startpage`, `ecosia`, `yahoo`, or `custom` |
| `web_search_custom_template` | Empty | Used only with `custom`. Must contain `{query}`. |

[search-syntax.md](search-syntax.md) covers the operators themselves.

## Clipboard

| Key | Default | Meaning |
|---|---|---|
| `clipboard_enabled` | `true` | Record clipboard history |
| `clipboard_retention_minutes` | `480` | How long entries are kept, 5 to 43200 |
| `clipboard_exclude_sensitive_patterns` | `password`, `passcode`, `otp`, `token`, `secret`, `apikey`, `api_key` | Substrings that stop an entry being recorded |

History is local. Nothing is sent anywhere.

## Appearance and performance

| Key | Default | Meaning |
|---|---|---|
| `idle_cache_trim_ms` | `900` | Delay after hiding before the icon cache is trimmed, 100 to 10000 |
| `active_memory_target_mb` | `72` | Sizes the icon cache, 20 to 512 |
| `ui_warm_release_ms` | `5000` | Delay before the icon cache is cleared, 500 to 600000. The web view stays warm regardless. |

Hiding the panel never tears down the web view, which is what makes the next
press instant. Only the icon cache is released.

## Quick Launch

Quick Launch is the idle app list.

| Key | Default | Meaning |
|---|---|---|
| `quick_launch.max_items` | `6` | Slots to fill, 3 to 12. Pinned items always show, even past this number. |
| `quick_launch.pinned` | Empty | Apps pinned to the front, in this order. Paths survive renames better than titles. |
| `quick_launch.auto_fill` | `true` | Fill unused slots from what you open most |

Compact view mode hides Quick Launch entirely.

## Web bookmarks

`web_bookmarks` is a list of `{ title, url, icon_path }`. Each entry becomes a row
that opens in your browser, with the site's favicon as its icon. Additions made
in the panel write back to this list.

## Plugins

| Key | Default | Meaning |
|---|---|---|
| `plugins_enabled` | `true` | Load plugins from `plugin_paths` |
| `plugins_safe_mode` | `true` | Keep plugin command execution disabled |
| `plugin_paths` | `%APPDATA%\Nex\plugins` | Where to look |

The plugin SDK is a preview. Leave safe mode on unless you wrote the plugin.

## Keys you probably should not touch

| Key | Default | Why |
|---|---|---|
| `index_db_path` | `%APPDATA%\Nex\index.sqlite3` | Changing it needs a restart and loses the current index |
| `version` | `27` | Read by migrations |

## Next

- [search-syntax.md](search-syntax.md) for the query operators
- [troubleshooting.md](troubleshooting.md) when a setting does not seem to take
- [../development/configuration.md](../development/configuration.md) for how
  migrations and live reload work internally
