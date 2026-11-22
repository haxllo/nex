# Architecture

Nex is three processes built from one library. The split exists to keep privilege
honest: the part of the program that renders a web page never indexes your disk,
and the part that runs elevated never decides when another process starts.

The engine is `apps/core`, a library crate named `nex_core`. The three binaries in
`apps/ui`, `apps/indexer`, and `apps/helper` are thin entry points over it.

## The three processes

| Process | Windows integrity | Owns |
|---|---|---|
| `NexUIProcess.exe` | Medium | The panel and its WebView2 content, the hotkey and its supervisor, the tray icon, settings, and search over the published snapshot |
| `NexIndexerProcess.exe` | Medium | File and application discovery, SQLite writes, the file and application watchers, and Tantivy |
| `NexHelper.exe` | High | Low-level keyboard and mouse hooks that a Medium process cannot receive over elevated windows, foreground grants, and input simulation |

Each binary is its own Cargo package on purpose. A Windows resource script is
linked into every binary of a package and inherited by dependents, so sharing one
package would give all three processes the same name in Task Manager.

### Why the split is not decoration

Windows gates input by integrity level, through User Interface Privilege
Isolation. A low-level keyboard hook installed in a Medium process does not fire
for input routed to an elevated window. When Task Manager or Settings has focus,
the only way to see the Win key is to have a hook in an elevated process.

That single constraint forces the helper to exist. Everything else follows from
keeping the helper's job as small as possible, which is why it owns hooks and
focus and nothing else.

## Who starts what

The UI is the session owner. It starts the indexer and the helper, supervises
both, and stops them on exit. Neither of the other two ever starts another
process.

- **Indexer.** The UI spawns it at startup and watches its snapshot endpoint. A
  snapshot read that finds the indexer unreachable respawns it. The indexer also
  watches its parent: if the UI disappears, the indexer exits on its own rather
  than becoming an orphan.
- **Helper.** The UI starts it through the `NexHelperV2` scheduled task, which
  runs it elevated without a prompt after the first time. A supervisor thread
  keeps the detection pipe connected and reconnects with exponential backoff from
  two seconds up to thirty seconds when the pipe closes. While the helper is
  away, the UI's own in-process hook keeps working for non-elevated input, so a
  helper outage degrades the hotkey over elevated windows rather than breaking it
  everywhere.

Stopping the UI stops the other two. `--quit` sends the indexer a shutdown
request before exiting so a graceful quit does not leave it running.

## Data ownership

Two stores, two owners, one deliberate exception.

| Store | Owner | Notes |
|---|---|---|
| SQLite index at `index.sqlite3` | Indexer | Holds items, usage counters, per-query selection memory, and a full-text table |
| Tantivy index beside it | Indexer | The primary search backend. The UI never writes it. |
| Launch and selection history | UI | The UI records what you launch, because that is what the click knows and the indexer does not |

The UI reads a published snapshot rather than querying the indexer per keystroke.
The indexer publishes a generation number with each snapshot, and the UI applies
either a full snapshot or a delta against the generation it holds. A generation
mismatch forces a full read.

This is what makes search fast during indexing: typing never waits on a pipe or on
a write lock. The tradeoff is that a result can be a moment out of date.

## The show path

A single hotkey press travels through several stages, and each one exists because
of a specific failure:

1. A low-level hook sees the key. If an elevated window has focus, the hook that
   sees it lives in the helper, and the press is forwarded over the detection pipe.
2. The UI decides whether the press is a show, a hide, or a refocus, using the
   panel's current visibility and whether it holds focus.
3. The panel window is positioned on the monitor under the cursor and shown.
4. The web view receives a state snapshot as JSON through
   `PostWebMessageAsJson`. Nothing waits for a synchronous script result.
5. For the glass material, a separate renderer thread captures the desktop behind
   the panel and draws the pane. The reveal waits for a frame at the panel's real
   size, which is why the first show at a new height can take slightly longer.

[overlay-and-glass.md](overlay-and-glass.md) covers the rendering half.
[ipc-and-privileges.md](ipc-and-privileges.md) covers the pipes and the rules about
which side may create them.

## State delivery to the page

Rust pushes, the page renders. There is no request and response between them on
the hot path.

- Rust builds a JSON snapshot of what should be on screen and posts it.
- The page applies it in a single render pass.
- The page sends user intent back as short messages: query changes, submits,
  context menu actions, settings writes.

The consequence worth knowing: the page holds no authority. It can ask the host to
launch something, but the host decides whether that is allowed.

## Startup order

When `NexUIProcess.exe` runs:

1. Logging initialises and the configuration loads, running any pending migrations.
2. If the process is meant to be in the background, it re-executes itself detached.
3. The indexer is started and the first snapshot is requested.
4. The helper is started through the scheduled task, and the supervisor connects.
5. The hotkey registers, and the window is created hidden.
6. The tray icon appears.

The overlay window and the settings window share one WebView2 environment, which
is why the second window costs little to open.

## What each process refuses to do

These are invariants, not conventions. A change that breaks one is a design
change, not a refactor.

| Invariant | Because |
|---|---|
| The UI never indexes | Two indexers on one database corrupt it, and the UI must stay responsive while indexing runs |
| The indexer never renders | It has no WebView2 and no window |
| The helper never owns process lifecycle | An elevated process deciding what runs is a privilege escalation surface |
| The UI always creates the pipes it writes to | A Medium process cannot write into a High process's pipe |

The pipe direction rules are detailed in
[ipc-and-privileges.md](ipc-and-privileges.md).

## Where to read next

- [index-and-search.md](index-and-search.md): discovery, the stores, ranking
- [overlay-and-glass.md](overlay-and-glass.md): the panel and the glass material
- [chat-and-tools.md](chat-and-tools.md): the assistant loop
- [ipc-and-privileges.md](ipc-and-privileges.md): pipes, protocols, and integrity
- [adr/](../adr/): why each of these decisions was taken
