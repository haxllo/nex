# ADR-0002: Split the application into three processes by privilege

## Status

Accepted.

## Date

2026-11-04

## Context

Nex must see keyboard input while an elevated window has focus. When Task Manager
or Settings is in front, Windows routes input to a High-integrity process, and a
low-level keyboard hook installed in a Medium-integrity process never fires for it.
The hotkey simply does nothing.

The only way to observe that input is to have a hook in an elevated process. That
puts a privileged component in the system, and a privileged component that also
indexes the user's files, renders a web page, and manages its own lifecycle is a
large attack surface for a small job.

Separately, indexing a large drive is slow and can stall. When it ran in the same
process as the panel, a scan could be felt as typing lag, and a crash took the
launcher with it.

## Decision

Three processes, split so that privilege and responsibility line up:

| Process | Integrity | Owns |
|---|---|---|
| `NexUIProcess.exe` | Medium | Panel and WebView2, hotkey and its supervisor, tray, settings, search over the published snapshot |
| `NexIndexerProcess.exe` | Medium | Discovery, SQLite writes, watchers, Tantivy |
| `NexHelper.exe` | High | Low-level hooks, foreground grants, input simulation, MFT snapshots |

The UI is the session owner. It starts and supervises the other two; neither starts
another process; the indexer exits when its parent disappears.

## Alternatives considered

### One process with an elevated thread

A single executable that elevates only the parts that need it.

Rejected because Windows privilege is a property of a process, not a thread. There
is no way to elevate one thread of a Medium process, and running the whole process
elevated would make a webview running untrusted HTML a High-integrity surface.

### Two processes, with indexing inside the helper

The helper is already elevated, so it could own discovery too.

Rejected because it makes the privileged broker do long-running work. A broker that
is busy scanning a drive cannot answer a hotkey, and every bug in the indexing path
would then be a bug in the elevated path.

### Keep the helper's job and accept one process for the rest

Leave the hotkey in the helper and keep indexing in the UI process.

Rejected on the measured complaint: a scan during indexing was felt as typing lag,
and an indexer crash closed the launcher. Separating the indexer also let the store
be owned by exactly one writer.

## Consequences

- The same Rust library is linked into three binaries. Each is its own Cargo
  package because a Windows resource script is inherited by every binary in a
  package, and sharing one would give all three the same Task Manager name.
- Cross-process communication is now part of the design, with pipe-direction rules
  enforced by Windows and not by convention. Getting a pipe's creator wrong fails
  silently at runtime. See [../development/ipc-and-privileges.md](../development/ipc-and-privileges.md).
- Search reads a published snapshot rather than querying the indexer per keystroke.
  A result can be a moment out of date, in exchange for typing never waiting on the
  indexer.
- The helper is a permanent privilege boundary that must stay small. Any proposal
  to give it more work is a change to this decision, not an implementation detail.
- Startup is more complex: process creation, an IPC probe, and a scheduled task
  launch all sit on the path to a first show.
