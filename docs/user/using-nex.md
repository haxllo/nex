# Using Nex

Press **Win**. The panel drops in under your cursor. Type. Press **Enter**.

That is the whole loop. Everything below is optional.

## The panel

The panel opens centred on the monitor that holds the cursor, in the upper third
of the screen. It sizes itself to its contents: a bare search field while you
type, a list of rows when there are results, and a grid when Grid view is on.

The default appearance is glass: what is behind the panel shows through, bent and
brightened at the edges. If Windows transparency is off, or the graphics path
cannot start, Nex falls back to an opaque surface and looks the same as it did
before the glass material existed.

Closing the panel does not shut Nex down. The web view stays resident so that the
next press is a show rather than a cold start.

## Keyboard

These work whenever the search field has focus.

| Key | Effect |
|---|---|
| Type | Search |
| `Down`, `Ctrl+J` | Select the next result |
| `Up`, `Ctrl+K` | Select the previous result |
| `Left`, `Right` | Move within a row in Grid view |
| `Ctrl+Home`, `Ctrl+End` | Select the first or last result |
| `Enter` | Open the selected result |
| `Tab` | Open the assistant, then the media view, then back to the search field |
| `Escape` | Close the open view, or hide the panel |
| `@` or `>` | Enter command mode |
| `Backspace` on an empty box in command mode | Leave command mode |

The assistant and the media view are a single cycle. The first `Tab` opens the
assistant. If something is playing, the next `Tab` shows the media view and the
one after that returns to the search field. `Escape` closes either view without
hiding the panel.

In the media view, `Space` plays or pauses, `Left` and `Right` skip tracks, and
`Up` and `Down` change the volume in steps of five. Typing any character returns
to search with that character in the box.

## Results

Each row shows an icon, a title, and a subtitle. Applications appear first, then
files and folders, then actions. Grid view arranges applications as tiles; files,
folders, and actions always stay as rows.

Right-click any row for its actions:

| Action | Available for |
|---|---|
| Open | Everything |
| Run as administrator | Applications and files |
| Open file location | Files and folders |
| Copy path | Files, folders, and applications |
| Pin to Quick Launch | Applications |
| Uninstall | Installed applications |

Pinned applications appear in Quick Launch, the idle list, in the order you
pinned them. Unpinning removes the row. A pinned path that no longer exists is
hidden rather than shown broken, but the pin itself stays in your config, so a
program on a drive you reconnect comes back without pinning it again.

## Quick Launch

When the panel opens with an empty search box it shows Quick Launch: a short list
of applications. Pinned items come first and always show. Remaining slots fill
from what you open most, up to the configured limit.

In Compact view mode the idle list is hidden and the panel is a bare search field
until you type.

## The assistant

Press `Tab` from an empty search box. Ask for something in plain words. The
assistant can find files, read and write them, move and delete them, and run a
shell command, but each write or launch is shown as a card you have to approve.

[assistant.md](assistant.md) covers what it can do and what it will refuse.

## Media

If something is playing, the media view shows the current track and the
transport. It works with any app that reports to the Windows media session, which
includes most browsers, Spotify, and the Windows apps.

## Searching more precisely

A plain query matches everything. Prefixes narrow it:

| Type | Scope |
|---|---|
| `@apps`, `@files`, `@folders` | One kind of result |
| `@clip` | Clipboard history |
| `@actions` | Actions and web searches |
| `>` | Command mode: web searches and actions |

Filters and boolean operators are covered in [search-syntax.md](search-syntax.md).

## Game Mode

With Game Mode on, the hotkey is suppressed while a fullscreen game has focus, so
a stray Win press does not pull you out of the game. The tray tooltip reads
`Nex (Game Mode)` while it is engaged.

## Tray icon

The tray icon has a menu with Settings, Restart, and Quit. Its tooltip reports
`Nex (hotkey issue)` when the elevated helper is not running, which is the usual
reason a hotkey stops working over elevated windows.

Settings also open from the gear button in the panel footer.

## What's New

After an update, a small pill appears in the panel offering the release notes. It
opens once per version. A fresh install records the current version silently and
does not show it. The same page is always available from Settings.

## Next

- [search-syntax.md](search-syntax.md) for operators and filters
- [assistant.md](assistant.md) for the assistant and its approval cards
- [settings.md](settings.md) for every setting
