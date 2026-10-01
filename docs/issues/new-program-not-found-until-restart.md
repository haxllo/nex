# New program not found until restart — OPEN

## Reproduction

1. Keep Nex running.
2. Install a new program.
3. Search for it in Nex.

## Observed behavior

- The new program does not appear in results.
- Restarting Nex makes it appear.

## Root cause

Nex has no live path that creates `app:` index entries after startup:

1. The filesystem watcher only creates `file:`/`folder:` items
   (`file_watcher_consumer.rs`, `path_to_search_item`). Start Menu
   shortcuts never become `app:{path}` entries with shortcut
   resolution and publisher subtitles outside a provider scan.
2. The all-users Start Menu (`C:\ProgramData\...\Start Menu\Programs`)
   is outside every watched root. Watchers start only on
   `discovery_roots` (`core_service.rs`, `start_file_watchers`), which
   default to the user profile, so most installer-created shortcuts
   produce no event at all.
3. Incremental rescans skip the `start-menu-apps` provider for 30
   minutes (`PROVIDER_RECONCILE_INTERVAL_SECS`), because its stamp is
   root identity plus existence only. `Tick`
   (`runtime_loop.rs`) runs no periodic reindex, so after the startup
   refresh the index only mutates via watcher file/folder upserts and
   overflow-triggered full resyncs.

## Recommended solution

1. Watch both Start Menu roots (per-user and all-users), either by
   extending watcher roots or by watching the
   `StartMenuAppDiscoveryProvider` roots explicitly.
2. Route Start Menu `.lnk`/`.exe` watcher events through the same
   shortcut-resolution, filtering, and publisher pipeline as
   `discover_start_menu_root`, upserting/deleting them as kind `app`
   with id `app:{path}`.
3. On such events, schedule a short-debounce refresh scoped to the
   `start-menu-apps` provider only, bypassing the 30-minute skip for
   that provider. This also covers Store apps visible only through
   the `Get-StartApps` enumeration, without a full rebuild.
4. Reuse the existing batch-cap, last-wins coalescing, and
   single-flight resync machinery so installer shortcut bursts
   collapse into one cheap targeted refresh.
