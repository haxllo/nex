# Timeline

Six eras, from the first commit in February 2026 to the current release. Each entry
says what changed and, where it matters more, what was abandoned.

Dates come from the release tags. Behaviour claims come from the code at `HEAD`;
the shipped record for each version is in [../releases/](../releases/).

## February to May 2026: a launcher appears

The project started as a single Windows executable with a hand-drawn panel, a
global hotkey, and fuzzy search over applications, files, and folders.

What shipped:

- Fuzzy search across apps, files, folders, custom actions, and clipboard history.
- Custom actions and web search.
- A plugin interface and a game mode that suppresses the hotkey over fullscreen
  games.

What shaped the code later:

- **The panel was drawn with Win32 immediate-mode calls.** GDI and GDI+ painting,
  with DWM for the window frame. Screen tearing and a panel flash on show were
  tracked down to a Direct2D and GDI hybrid and fixed by dropping Direct2D
  entirely. That fixed the tearing and confirmed the real problem: every visual
  change was a code change.
- **Search blocked the UI thread.** The first release ran the query synchronously
  on the keystroke path. A dedicated search worker with stale-request draining
  replaced it, which is still the shape today.

## May 2026: going faster than a directory walk

Whole-drive indexing was too slow to finish. Three approaches were tried in
sequence, and only one survived.

| Approach | Outcome |
|---|---|
| SQLite `LIKE` with a leading wildcard | Cannot use an index. Query time ranged from tens of milliseconds to seconds. Replaced. |
| Everything's SDK | Worked, and was rejected later. It made Nex depend on a third-party service being installed and running. |
| Tantivy | Adopted. A real inverted index with BM25 ranking, and it does not need another process to be alive. |

Tantivy arrived on 2026-05-31 alongside a SQLite full-text fallback. Both were
kept, and the relationship between them is recorded in
[../adr/0004-tantivy-search.md](../adr/0004-tantivy-search.md).

The first version of the file index was capped by configuration rather than
unbounded, which is why the item caps exist today.

## June 2026: the panel becomes a web page

This is the era the current architecture belongs to, and it happened in one step.

**2026-06-18.** The native panel was replaced by a WebView2 control hosted through
`tao` and `wry`. The reasoning is in
[../adr/0001-rust-tao-wry-webview2.md](../adr/0001-rust-tao-wry-webview2.md).

**2026-06-19.** A synchronous `evaluate_script` call on the keystroke path was
replaced with fire-and-forget `PostWebMessageAsJson`. This is the decision that
made the panel fast under a busy web engine, recorded in
[../adr/0007-fire-and-forget-ui-state.md](../adr/0007-fire-and-forget-ui-state.md).

### The abandoned Iced migration

An alternative was in flight when this happened: a native Rust UI toolkit, Iced
0.14, with the overlay split into boot, view, model, theme, geometry, and shim
modules across ten planned phases.

Most of those phases had been completed. The migration was then dropped, because it
reproduced the problem it was meant to solve: the panel's design still lived in
Rust widget code, so visual iteration was still a code change.

That planning tree was abandoned rather than deleted, and survived in the
repository for months afterwards describing files that no longer existed. It was
removed during the documentation rebuild in November 2026. The lesson is recorded
here rather than in the tree: **a plan that is superseded must be deleted or
marked, not left in place.** A stale plan reads as current to everyone who finds
it later.

## July and August 2026: an elevated helper without a prompt

Elevated hotkey detection had been added, but the helper was started with the
`runas` verb, so every crash recovery raised a User Account Control prompt.

**2026-07-29.** The helper moved to a scheduled task named `NexHelperV2`, created
once behind a single prompt and started afterwards without one. Recorded in
[../adr/0005-elevated-helper-scheduled-task.md](../adr/0005-elevated-helper-scheduled-task.md).

This era also produced the pipe-direction rules that are now invariants. A
detection pipe created by the helper made the UI the reader of a High-integrity
pipe, which Windows refuses; the helper's writes succeeded into its own end and Nex
received nothing. The rule that came out of it is in
[../development/ipc-and-privileges.md](../development/ipc-and-privileges.md).

## September and October 2026: the panel becomes glass

**2026-10-05.** A first attempt made the glass a transparent Direct3D child window
beneath the WebView. It presented without error and never appeared on screen. Input
reached it; pixels did not.

The material moved to a separate top-level window, drawn on the GPU from a DXGI
desktop capture with a Direct2D effect chain and an HLSL shader. Recorded in
[../adr/0006-native-glass-renderer.md](../adr/0006-native-glass-renderer.md).

The material shipped disabled, as an option, and later became the default. Its
consequences are still being paid down: the display affinity that keeps the panel
out of screen capture also makes it impossible to screenshot, which is why glass
verification uses GPU probes and photographs instead.

**2026-10-07.** The assistant arrived. What began as a chat view became a
tool-calling loop with an explicit approval boundary, recorded in
[../adr/0008-assistant-tool-approval.md](../adr/0008-assistant-tool-approval.md).

## November 2026: two processes, then three

The last structural change, and the largest.

**2026-11-04 and following.** Indexing moved out of the UI process into a separate
`NexIndexerProcess`, which owns discovery, SQLite, the watchers, and Tantivy. The UI
searches a snapshot the indexer publishes. The legacy single-process entry point
and the in-process indexing fallback were deleted, not disabled.

**Early November.** Directory walking was replaced by an NTFS master file table
read, with the USN change journal for deltas and directory walking kept as the
fallback. Measured at a 40,000 item per-root cap, one volume produced 1.13 million
records in about 16 seconds. Recorded in
[../adr/0003-mft-discovery.md](../adr/0003-mft-discovery.md).

**2026-11-18.** Version 3.0.0 shipped, with the assistant reaching full capability
and the What's New page rebuilt as a curated in-app document rather than a copy of
the release body.

**2026-11-22.** Version 3.3.0 shipped the current glass shader, the helper shutdown
handshake, and the single-WebView2-environment change.

The process model that came out of this era is in
[../adr/0002-three-process-split.md](../adr/0002-three-process-split.md).

## What this history is for

Three lessons are worth carrying forward.

1. **A dependency on another running service is a real cost.** The third-party
   indexer worked and was still removed, because it made Nex depend on something
   it did not control.
2. **A plan that was superseded must be deleted or marked.** The abandoned Iced
   tree misled readers for four months.
3. **A platform constraint is cheaper to design around than to fight.** Elevated
   input, pipe direction, and capture exclusion are all Windows rules. Each
   workaround cost less than the alternative of pretending the rule did not exist.
