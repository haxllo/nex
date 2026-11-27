# Install Nex

Nex runs on Windows 10 or 11, 64-bit. It needs no account and no configuration
to start working.

## Requirements

| Requirement | Detail |
|---|---|
| Windows | 10 or 11, 64-bit |
| WebView2 Runtime | Ships with Windows 11 and current Windows 10. The installer adds it if it is missing. |
| Elevation | The helper process runs elevated, so the hotkey keeps working over Task Manager and Settings. Windows asks for this once. |
| Disk | The index lives beside your configuration. A full-drive index of a typical machine is a few tens of megabytes. |

## Install

Download the installer or the portable zip from the
[releases page](https://github.com/haxllo/nex/releases/latest) and run it.

Nex starts in the background on the **Win** key. There is no window to keep open
and no first-run wizard.

The installer asks one question - whether to add a desktop shortcut and to start
with Windows - and follows your Windows light or dark setting. It installs for the
current account without an admin prompt.

The first launch registers a Windows scheduled task called `NexHelperV2` and
shows one User Account Control prompt. Accepting it is what allows the hotkey to
respond while an elevated window is in front. Declining it leaves Nex working for
everything except those windows.

### Portable zip

The zip needs no installation. Unpack it and run `NexUIProcess.exe`. The
scheduled task and configuration directory are created the same way. Nex is not
added to the Start menu or to Add or remove programs, and it does not register an
uninstaller.

## Where your files go

| Path | Contents |
|---|---|
| `%APPDATA%\Nex\config.toml` | Your settings. Created on first launch. |
| `%APPDATA%\Nex\config.toml.invalid` | A config Nex could not read, kept for repair. Nex restores the newest backup over `config.toml` and says so in the panel; see [Settings](settings.md#when-a-value-is-wrong). |
| `%APPDATA%\Nex\config.backup-<time>.toml` | A copy of the previous settings, written before each save. The newest one is what a startup restore uses. |
| `%APPDATA%\Nex\index.sqlite3` | The search index. Safe to delete while Nex is closed; it is rebuilt. |
| `%APPDATA%\Nex\state.json` | Small runtime state, such as which release's What's New you have seen. |
| `%APPDATA%\Nex\icon-cache\` | Extracted application icons. Safe to delete. |
| `%APPDATA%\Nex\bookmark-icons\` | Favicons for web bookmarks. |
| `%APPDATA%\Nex\plugins\` | Plugin directory. Empty unless you add plugins. |
| `%APPDATA%\Nex\logs\nex.log` | The runtime log |
| `%APPDATA%\Nex\support\` | Diagnostics bundles written by `--diagnostics-bundle` |

A machine that once ran the pre-1.x build under the name SwiftFind may have a
`%APPDATA%\SwiftFind` directory. Nex migrates what it can from it and leaves the
old directory alone.

## Command line

The command line is not needed for daily use. Every flag belongs to
`NexUIProcess.exe`.

| Command | Effect |
|---|---|
| `NexUIProcess.exe` | Start in the background and wait for the hotkey. |
| `NexUIProcess.exe --foreground` | Start attached to the terminal, for development. |
| `NexUIProcess.exe --background` | Start detached. This is the default. |
| `NexUIProcess.exe --status` | Print whether Nex is running. |
| `NexUIProcess.exe --status-json` | Same, as JSON. |
| `NexUIProcess.exe --quit` | Stop the running instance. |
| `NexUIProcess.exe --restart` | Stop and start. |
| `NexUIProcess.exe --ensure-config` | Write the config template if no config exists. |
| `NexUIProcess.exe --sync-startup` | Re-apply the launch-at-startup registration from config. |
| `NexUIProcess.exe --set-launch-at-startup=true` | Change launch at startup without editing the file. |
| `NexUIProcess.exe --set-launch-at-startup=false` | Same, disabling it. |
| `NexUIProcess.exe --diagnostics-bundle` | Write a diagnostics folder to `%APPDATA%\Nex\support` for a bug report. |
| `NexUIProcess.exe --probe-index` | Report what the index currently holds. |
| `NexUIProcess.exe --update-plan` | Show which installation an update would modify, whether it needs elevation, and what it would install. Downloads nothing. |
| `NexUIProcess.exe --update` | Apply an update now. Downloads and verifies the release, stops Nex, installs it, and restarts it. |
| `NexUIProcess.exe --update --force` | With the native engine, reinstall the published version even when it is already installed. |

`--update` refuses when the installed version is already current, and refuses
when the copy you are running is older than the installed one — updating the
installed tree would leave you looking at a different build.

Running `NexUIProcess.exe` with no arguments when an instance is already running
does not start a second copy. The existing instance is brought forward.

## Three processes

Nex is one application built from three executables. They are kept separate so
that no part of the program holds more privilege than its job requires.

| Process | Windows integrity | Owns |
|---|---|---|
| `NexUIProcess.exe` | Medium | The panel, the hotkey, the tray icon, settings, and searching the published index |
| `NexIndexerProcess.exe` | Medium | Finding files and apps, writing the index, watching for changes |
| `NexHelper.exe` | High | Detecting the hotkey while an elevated window has focus, and granting the panel focus |

The UI starts the other two and restarts them if they stop. The indexer exits on
its own if the UI disappears. The helper never decides when another process runs.

Do not start `NexIndexerProcess.exe` or `NexHelper.exe` by hand. They expect a
parent and a set of named pipes that only the UI creates.

## Uninstall

Use Add or remove programs for an installed copy, or delete the folder for a
portable one. Two things outlive the binaries:

- **The `NexHelperV2` scheduled task.** Remove it with
  `schtasks /delete /tn NexHelperV2 /f` from an elevated prompt if you want it
  gone.
- **`%APPDATA%\Nex`.** Delete it to discard your settings, index, and caches.

## Next

- [using-nex.md](using-nex.md) for the keyboard and the panel
- [settings.md](settings.md) for every setting
- [troubleshooting.md](troubleshooting.md) when the hotkey or the panel misbehaves
