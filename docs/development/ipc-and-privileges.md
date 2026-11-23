# IPC, pipes, and privilege boundaries

Nex has two different transport mechanisms for two different reasons. Getting the
direction wrong on any of them produces a silent failure, so this page states the
ownership of each channel and the rule that forces it. The relevant modules are
`ipc`, `mft_protocol`, `mft_channel`, `overlay::hotkey`, and the helper's entry
point.

## Two transports

| Path | Transport | Why |
|---|---|---|
| UI to indexer | Named pipe, name scoped to user and session | The two processes are at the same integrity level, so the boundary that matters is *the user*, not the privilege level. A pipe name is the only one of the two that can carry an owner |
| UI to helper, and indexer to helper | Named pipes | Integrity levels differ, so the pipe's creator decides who may use it |

A second indexer that finds its endpoint already owned exits on startup rather than
fighting for it. Ownership is an explicit claim, not a side effect: `ipc_transport`
creates a `Local\<endpoint>-v1-<scope>` mutex and treats `ERROR_ALREADY_EXISTS` as
"another indexer owns this link".

The scope is the session id plus a hash of the user's SID, and the pipe's DACL is
generated from that SID. Two consequences follow, and both are the point of the
design:

- The boundary is **the user and the session, not the process**. Another user cannot
  open the pipe at all, and a second session gets its own link instead of reaching
  this one. A process running as the *same* user in the same session is still
  allowed, and that is deliberate: a named pipe cannot tell two processes of one
  user apart, and pretending otherwise in this document would be the kind of claim
  that reads as security while holding none. What the pipe removes is the
  *machine-wide, unauthenticated* reach of the old fixed port, which any account on
  the box could call, and the cross-session fall-through where one session's UI
  read another session's index.
- Two sessions are two separate links. The earlier transport used a fixed loopback
  port, which is per machine while the runtime singleton is per session, so a
  second session's indexer could not bind and its UI silently read the first
  session's index (CIQ-242).

## The UI to indexer protocol

The indexer answers newline-delimited JSON: one request line in, one response line
out, per connection, framed in byte mode and bounded by explicit caps (1 MiB
request, 256 MiB response). Each request carries a protocol version, and a mismatch
is rejected rather than guessed at.

| Method | Purpose |
|---|---|
| `ping` | Liveness and version |
| `shutdown` | Stop the indexer, used by a graceful quit |
| `status` | Index health |
| `snapshot` | The published item snapshot with its generation |
| `sync` | Incremental changes since a generation |
| `search` | A query against the index |
| `search_seed` | Candidate ids for a query, used by the UI's own ranking |
| `apps`, `files`, `folders`, `actions`, `clipboard` | One kind of item at a time |
| `recent_documents` | Recently used files |
| `record_launch`, `record_selection` | Usage counters written by the UI |
| `delete_item` | Remove one item from the index |
| `warm_index` | Ask the indexer to warm its caches |

The UI does not query this on every keystroke. It keeps a snapshot and applies
deltas. See [index-and-search.md](index-and-search.md).

## Named pipes and the integrity rule

Windows User Interface Privilege Isolation, UIPI, stops a Medium-integrity process
writing into a pipe created by a High-integrity one. The error is `Access is
denied`, and it appears at write time, not at connect time.

The consequence is a rule that must not be inverted:

> **The pipe a process writes to must be created at that process's integrity level
> or lower, never higher.**

Read down is allowed. Write up is not. That single asymmetry determines every pipe
in the system.

| Pipe | Created by | Read or written by | Consequence if inverted |
|---|---|---|---|
| `\\.\pipe\NexHelperLog\<ui pid>` | Helper (High) | UI reads | None if inverted, but the helper must own its own log server to survive the UI being absent |
| `\\.\pipe\NexHelperCmd\<ui pid>` | UI (Medium) | Helper reads | The helper silently receives nothing |
| `\\.\pipe\nex-hotkey-<pid>` | UI (Medium) | UI reads, helper writes as client | The helper writes into its own server end and the UI sees no keystrokes |
| `\\.\pipe\NexMftCmd\<ui pid>` | Indexer (Medium) | Helper connects as client and streams framed snapshots back | The indexer cannot read the journal |

Two of those were found the hard way. A helper-created detection pipe made the UI
the reader of a High-integrity pipe, which UIPI refuses; the helper's writes
succeeded into its own end and Nex received nothing. The fix was to have the UI
create the detection pipe with the helper as the connecting client.

The MFT pipe names itself after the UI's process id, which the helper derives from
the indexer's parent. This is what stops a stale helper from attaching to a
different Nex instance's channel.

## Detection paths and duplicates

Several mechanisms can observe the same physical key press:

| Path | Covers |
|---|---|
| Low-level keyboard hook in the UI | Ordinary input |
| Raw input in the UI | When the hook is starved |
| Polling with `GetAsyncKeyState` | When both of the above fail |
| Low-level hook in the helper | Anything routed to an elevated window |

The helper is authoritative while it is connected, and the UI's own paths stand
down when it is. Without that gating, one press is reported by more than one path
and toggles the panel twice. The window that treats two detections as one press is
narrow and deliberate.

The helper is also given a short grace period to unhook itself before it is
terminated. Killing a process with low-level hooks installed leaves the system
waiting on a hook owner that no longer exists, which surfaces as the mouse freezing
after the tray icon disappears.

## Helper configuration

The helper reads its configuration from a shared section named
`Global\nex-hotkey-config-<pid>`, written by the UI. It carries the hotkey, the
target virtual key, and the modifier state, so the helper can detect the chord
without reading `config.toml` itself.

Single-instance enforcement uses a mutex named `Local\NexRuntimeSingleton`. A
second UI process exits quietly rather than creating a second tray icon and a
second hotkey registration.

## Supervision and backoff

The UI keeps the helper connected through a supervisor thread. When the detection
pipe closes, it retries with capped exponential backoff: two seconds initially,
doubling to a maximum of thirty seconds, and resetting on success. The backoff is
sliced into small waits so a quit is never held behind the retry timer.

While the helper is away, the UI's own hook keeps ordinary input working. A helper
outage degrades the hotkey over elevated windows and nothing else.

## Rules for changing this area

- Never move pipe creation to the higher-integrity side without checking who
  reads it.
- Never add a write from a lower-integrity process into a pipe owned by a higher
  one. It compiles, it connects, and it fails at runtime with `Access is denied`.
- Never let the helper own process lifecycle. It detects input and grants focus;
  it does not decide what runs.
- Never add a second writer to the detection path without the duplicate-press
  guard.

## Next

- [architecture.md](architecture.md) for the process model these channels serve
- [overlay-and-glass.md](overlay-and-glass.md) for the detection paths' role in the show flow
- [../adr/](../adr/) for the decision to run the helper elevated
