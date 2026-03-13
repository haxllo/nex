# Windows Runtime Behavior (Current)

Current hotkey-to-launcher behavior in Nex (`nex`):

1. Start runtime with `cargo run -p nex`.
2. Runtime loads config (JSON/JSON5 with comments), logs startup mode/hotkey/paths, builds or opens index.
   - indexing path is incremental-first: unchanged providers can be skipped using provider change stamps
   - periodic reconcile scan still runs after the configured interval to prevent drift
3. Runtime registers global hotkey from config (default `Ctrl+Space`).
4. Runtime creates a native borderless top-most launcher window (hidden by default).
5. Hotkey behavior:
   - configured hotkey shows launcher and focuses input when hidden.
   - configured hotkey hides launcher when launcher is already focused.
   - configured hotkey refocuses launcher if visible but not focused.
6. Launcher interaction:
   - typing runs core search against indexed items
   - `Up`/`Down` changes the active result row
   - `Enter` launches selected result and hides launcher immediately on success
   - single-click on a result launches it immediately
   - typing a query starting with `log` includes a built-in action: `Open Nex Logs Folder`
   - `Esc` hides launcher
   - clicking outside launcher hides launcher
   - any close path clears query/results so next open starts fresh
   - clicking `?` opens `%APPDATA%\Nex\config.toml` for manual edits
7. Search and launch errors are surfaced inside launcher status text.
8. Settings persistence behavior:
   - config remains source of truth (`%APPDATA%\Nex\config.toml`)
   - save path uses safe temp-write + replace flow
   - startup behavior is controlled by config values
   - local file discovery honors include/exclude roots (`discovery_roots`, `discovery_exclude_roots`)
9. Runtime diagnostics:
   - runtime writes local logs to `%APPDATA%\Nex\logs`
   - panic hook records crash context in log file
   - provider indexing logs include `skipped=true/false` for incremental visibility
10. Visual/UX polish:
   - compact Spotlight/Wofi-like default bar state
   - no results panel shown when query is empty
   - results panel expands downward when query has matches
   - results are grouped with visible section headers (`Top Hit`, `Applications`, `Files`, `Actions`, `Other`)
   - first best match is rendered as a distinct `Top Hit` card row
   - no-match searches show a single non-launchable `No results` row in the results area
   - empty-query `Enter` keeps compact state and shows `Start typing to search` in the input placeholder until input changes
   - footer hint strip is visible while results are shown (`Enter Open • ↑↓ Move • Esc Close`)
   - panel background `#272727` with border `#424242`
   - structured two-line result rows (`title` + `path`) with native Windows/file-type icons
   - rounded results panel; top edge flush to input section
   - bottom margin matches left/right margin
   - single active-row emphasis for mouse + keyboard (no separate dual highlight state)
   - lightweight show/hide and results expansion animations

## Runtime Host and Lifecycle

- `--background`: starts detached background runtime process.
- `--foreground`: explicit attached mode (primarily for debugging).
- `--status`: reports `running`, `stopped`, or degraded runtime state.
- `--quit`: attempts graceful quit, then force-terminates stuck runtime if needed.
- `--restart`: stops runtime (graceful + fallback) and starts again.
- `--ensure-config`: creates config template if missing.
- `--sync-startup`: applies `launch_at_startup` from config to Windows Run key.
- `--diagnostics-bundle`: writes support bundle under `%APPDATA%\Nex\support`.

Update operations:

- runtime does not run a background auto-updater.
- channel-aware updates are user-triggered via `scripts/windows/update-nex.ps1`.
- updater verifies installer checksum from release manifest and performs rollback restore on failed apply.

Single-instance behavior:

- Mutex guard enforces one active runtime.
- Duplicate launches signal existing instance to show/focus overlay instead of creating a second hotkey owner.

Known limitations in this milestone:

- Launcher is native Win32 shell (not a full React/WebView overlay).
- Animations are intentionally lightweight to prioritize runtime stability.
- Runtime must remain active in its process; stopping `nex.exe` unregisters hotkey and closes launcher.
- Hotkey registration changes still require process restart to apply globally.
- Native settings UI is not planned in the near term; `?` keeps config-file edit flow.

## Spotlight-Parity Direction

Nex now tracks a Spotlight-like architecture direction while staying performance-first:

- keep index and ranking on-device by default
- keep overlay UI thin and move heavy work to background indexing paths
- prefer incremental updates over full rebuilds in normal runtime
- merge and rank multi-provider results in one deterministic list
- ranking uses explicit weighted match tiers (exact/prefix/substring/fuzzy) plus source and usage signals
- preserve hard latency and memory guardrails

See:
- `docs/engineering/spotlight-parity-architecture.md`

Operator steps and troubleshooting are documented in:
- `docs/engineering/windows-operator-runbook.md`

Screenshot capture checklist is documented in:
- `docs/engineering/windows-operator-runbook.md` (`Screenshot Notes (Before/After)`)

## Progress Update (2026-02-13)

Completed behavior updates:
- Result deduplication is applied before overlay rendering to reduce duplicate app/file entries.
- Selection/hover stability improvements reduce first-keystroke list jump behavior.
- Click-outside close behavior was hardened through activation-path fixes.
- Shortcut icon resolution pipeline now prioritizes resolved icon/target paths and avoids shortcut-overlay artifacts where possible.
- Overlay quality improvements shipped with black-shade visual system and refined list interactions.

Known intentional state:
- `?` keeps file-based hotkey edit flow as the intended settings approach.
- Runtime remains performance-first; no always-on secondary UI process was added.
