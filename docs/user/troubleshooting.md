# Troubleshooting

Start here: run `NexUIProcess.exe --status`. If it reports Nex is not running, the
rest of this page explains why and how to bring it back.

## The hotkey does nothing

Work through these in order. Each one rules out a whole class of cause.

1. **Is Nex running?** `NexUIProcess.exe --status`. If not, run
   `NexUIProcess.exe --restart`.
2. **Is the tray icon showing `Nex (hotkey issue)`?** That means the elevated
   helper is not connected. The helper is what detects the hotkey while a window
   running as administrator has focus, so without it the hotkey goes quiet over
   Task Manager, Settings, and anything else elevated. Restart with
   `NexUIProcess.exe --restart` and watch for the tooltip to return to
   `Nex Launcher`.
3. **Did you decline the User Account Control prompt on first run?** The helper
   cannot start without the `NexHelperV2` scheduled task. Run
   `NexUIProcess.exe --restart` and accept the prompt when it appears.
4. **Was a game in the foreground?** With Game Mode on, the hotkey is suppressed
   over fullscreen games on purpose. The tray tooltip reads `Nex (Game Mode)`.
5. **Did you set a hotkey Windows refuses?** `Alt+Tab`, `Ctrl+Esc`, and a few
   others belong to the OS. Set `hotkey = "Win"` or `hotkey = "Ctrl+Space"` in
   `%APPDATA%\Nex\config.toml` and restart.

If the tooltip says `Nex Launcher` and the hotkey still does nothing, run
`NexUIProcess.exe --diagnostics-bundle` and include the folder it writes from
`%APPDATA%\Nex\support` in a bug report.

## The helper keeps failing

The helper is started through the `NexHelperV2` scheduled task. If it cannot
start, the task itself is usually the problem.

```
schtasks /query /tn NexHelperV2
schtasks /delete /tn NexHelperV2 /f
```

Deleting it is safe. Nex recreates it on the next start and asks for elevation
once more. Run those commands from an elevated prompt.

## The panel will not close

Press `Escape`. If a view such as the assistant or the media panel is open,
`Escape` closes that view and leaves the panel up; a second press hides the panel.

If a confirmation card is open, `Escape` dismisses the card rather than the panel,
because an unanswered card should not be able to swallow your keystrokes.

## A new program does not show up

Applications are discovered from the Start menu and from installed program
records. A program that installs without registering in either place is invisible
to Nex until it is restarted and, in some cases, until the machine is restarted
so the shell refresh registers it.

Restart Nex first: `NexUIProcess.exe --restart`. If the program still does not
appear, the installer did not register it with the Start menu, and Nex has nothing
to find.

## Search found nothing and used to

Check three settings in `config.toml`:

- `show_files` and `show_folders` both default to `true`. If either is `false`, a
  whole kind of result is hidden.
- `discovery_roots` must contain the folder you are searching. It defaults to your
  user profile, which means another drive is not indexed until you add it.
- `discovery_exclude_roots` and the built-in exclusions may be skipping the
  folder. `node_modules`, `.git`, `AppData`, and `Temp` are skipped by design.

After changing a discovery root, the index rebuilds. Large roots take a minute or
more, and a progress window appears in the bottom right while a full pass runs.

## The index seems wrong or stale

Delete it. Close Nex first, then remove
`%APPDATA%\Nex\index.sqlite3` and start Nex again. The index is rebuilt from
scratch and nothing else is lost.

## The panel looks opaque instead of glass

The glass material needs Windows transparency enabled and a working graphics
path. It also stops mid-session if the renderer fails, rather than leaving a blank
panel.

- Turn on **Settings > Personalisation > Colours > Transparency effects**.
- If transparency is already on, check `panel_background` in `config.toml`. `black`
  and `white` are deliberately opaque.
- On a remote desktop session or a virtual machine without GPU acceleration, the
  fallback is expected.

Setting `NEX_REFRACT_LAB=0` in the environment forces the fallback, which is
useful for telling a glass problem apart from a page problem.

## The panel is slow to open the first time

The first show after a long idle period and the first show at a panel height not
seen since Nex started can each take a fraction of a second longer. After that it
is immediate. This is the renderer catching up, not a fault.

If every show is slow, look at `active_memory_target_mb` and
`ui_warm_release_ms`. Raising the memory target keeps more icons resident.

## Updating

There is a built-in updater. When it installs a release, a pill offering What's
New appears in the panel, once per version.

If an update leaves Nex unable to start, download the current release and run the
installer over the existing copy. Your configuration and index are not touched by
an install.

## Reporting a bug

1. Run `NexUIProcess.exe --diagnostics-bundle`.
2. The folder it writes is `%APPDATA%\Nex\support\diagnostics-<timestamp>`. It
   holds a summary, a sanitised copy of your configuration, index health, and
   recent logs.
3. Attach it to an issue at <https://github.com/haxllo/nex/issues>.

The bundle is designed to be safe to share. Raw configuration and full logs are
only included when `NEX_INCLUDE_RAW_DIAGNOSTICS=1` is set, which you should not
need.

## Next

- [install.md](install.md) for install and uninstall
- [settings.md](settings.md) for every key
- [using-nex.md](using-nex.md) for the keyboard
