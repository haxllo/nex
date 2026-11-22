# Configuration internals

Configuration is a versioned TOML document with migrations, validation, atomic
writes, and a file watcher. Adding a key means touching all four. The relevant
module is `config`, with the watcher in `config_watch`.

## Format and location

`%APPDATA%\Nex\config.toml` is the file Nex reads and writes. TOML is the only
format it writes.

JSON and JSON5 are still read, for a configuration created by an older build, and
are converted to TOML on the next write. An installer that finds only
`config.json` or a `%APPDATA%\SwiftFind\config.json` migrates what it can and
leaves the original in place.

Writes are atomic: the new content goes to a temporary file and is renamed over the
target, with a backup kept beside it. A crash during a write therefore leaves the
previous configuration intact rather than a truncated file.

## The generated template

`write_user_template_toml` produces the commented file a new user gets. It is the
only place the user-facing comments live.

The template is generated rather than shipped as a static file, because the
comments include the current values of `hotkey_recommended`, the discovery roots,
and the platform's paths.

One rule that is easy to break: new keys go into the TOML template, never into a
JSON template. A parallel JSON template drifted out of sync once and is not to be
reintroduced.

## Versioning and migrations

`CURRENT_CONFIG_VERSION` in `config` is the schema version. The `version` key in
the file records which schema that file was written under.

Loading runs `apply_migrations`, which walks the source version upward and applies
each step in order. A step exists for two kinds of change:

- A key that needs a new default because the old value is now wrong.
- A key that changed meaning or was removed.

A migration checks whether the raw file actually contained a key before overwriting
it, so an explicit user value is not silently replaced by a new default. A file
that never had `idle_cache_trim_ms` gets the new default; a file that set it
deliberately keeps the user's number.

A migration that bumps the version is written even when nothing else changed, so
the file does not migrate again on the next load.

## Validation

`validate` runs after parsing and rejects the whole configuration rather than
clamping a value, so a mistake is visible instead of silently corrected.

| Key | Rule |
|---|---|
| `max_results` | 5 to 100 |
| `clipboard_retention_minutes` | 5 to 43200 |
| `index_db_path`, `config_path` | Must be present |
| `hotkey` | Must not be blank, and must parse as a supported chord |
| `discovery_max_depth` | 0 to 256 |
| `index_max_items_per_query_seed` | 250 to 200000 |
| `active_memory_target_mb` | 20 to 512 |
| `ui_warm_release_ms` | 500 to 600000 |
| `idle_cache_trim_ms` | 100 to 10000 |
| `quick_launch.max_items` | 3 to 12 |

An invalid configuration is handled differently at the two moments it can arrive.

- **At startup**, the invalid file stops the process. It is a hard failure, not a
  silent fall back to defaults, because quietly discarding a user's configuration
  is worse than refusing to start.
- **On reload**, the invalid file is skipped and the running configuration is kept.
  The reason is logged. Editing the file into an invalid state while Nex is running
  therefore changes nothing until the next valid save.

## Live reload

The file is watched. A change is parsed, validated, and applied without a restart
for everything except the index database path.

Reload work is not uniform, and the difference is worth knowing:

| Change | Action |
|---|---|
| Query and display keys | Applied to the running runtime |
| `hotkey` | The hotkey is re-registered |
| Discovery roots, exclusions, `show_files`, `show_folders`, `discovery_max_depth`, `file_discovery_backend` | Providers and watchers are rebuilt and the index refreshes |
| `index_db_path` | Needs a restart |

`discovery_settings_changed` is the predicate that decides the third row. It
deliberately excludes query-time limits such as
`index_max_items_per_query_seed`, because changing how many candidates a query
pulls does not change what is indexed and must not trigger a rescan.

The indexer reloads its own configuration too, and reports whether the change
needed a reindex. That is what allows a discovery change to reach the indexer
without restarting it.

## Adding a key

1. Add the field to `Config` with a default in `Config::default`.
2. Add it to the TOML template with a comment a user can act on.
3. Add a validation rule if the value has bounds.
4. If an existing file's value would now be wrong, add a migration step and bump
   `CURRENT_CONFIG_VERSION`.
5. If the key changes what is discovered or indexed, add it to
   `discovery_settings_changed`.
6. If it belongs in the in-app Settings view, add the row and its write path.
7. Document it in [../user/settings.md](../user/settings.md).

Skipping step 5 is the common mistake: the key appears to work until a restart,
because nothing told the indexer to re-read the world.

## Quirks worth knowing

- **The template advertises an operator the parser does not implement.** The
  comment above `search_dsl_enabled` names `kind:` alongside `ext:` and
  `modified:`. Only the latter two exist. See
  [../user/search-syntax.md](../user/search-syntax.md).
- **`everything` still loads as `auto`.** The backend string survives for old
  configurations even though the Everything SDK backend was removed, and loads
  with a warning rather than a validation failure.

## Next

- [../user/settings.md](../user/settings.md) for the user-facing reference
- [index-and-search.md](index-and-search.md) for what the discovery keys change
- [testing.md](testing.md) for the test patterns this module uses
