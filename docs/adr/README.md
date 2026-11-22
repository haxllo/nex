# Architecture decision records

An ADR records a decision, the constraints that forced it, the alternatives that
were rejected, and what it costs. It exists so that a future reader can tell
"this is deliberate" from "this is an accident" without re-deriving the reasoning.

`AGENTS.md` and the code show what Nex does. These records show why.

## Format

Each record uses the same sections:

| Section | Contains |
|---|---|
| Status | `Accepted`, `Superseded by ADR-XXXX`, or `Deprecated` |
| Date | When the decision was taken |
| Context | The constraints and the problem, without the answer |
| Decision | What was chosen, in one or two sentences |
| Alternatives considered | What else was plausible, and why it lost |
| Consequences | What this costs, including the parts that are not benefits |

## Index

| Id | Decision | Status |
|---|---|---|
| [0001](0001-rust-tao-wry-webview2.md) | Rust with tao and wry hosting a WebView2 panel | Accepted |
| [0002](0002-three-process-split.md) | Split the application into three processes by privilege | Accepted |
| [0003](0003-mft-discovery.md) | Read the NTFS master file table instead of walking directories | Accepted |
| [0004](0004-tantivy-search.md) | Tantivy as the primary search index, with SQLite as the record | Accepted |
| [0005](0005-elevated-helper-scheduled-task.md) | Start the elevated helper through a scheduled task, not `runas` | Accepted |
| [0006](0006-native-glass-renderer.md) | Draw the glass material natively in a separate top-level window | Accepted |
| [0007](0007-fire-and-forget-ui-state.md) | Push UI state fire-and-forget instead of evaluating script synchronously | Accepted |
| [0008](0008-assistant-tool-approval.md) | Confirm by tool annotation, with one deliberate exception | Accepted |

## Writing a new one

Add the next number, fill in every section, and add it to the index above. Do not
rewrite an accepted record when a decision changes: write a new one, set the old
one's status to `Superseded by ADR-XXXX`, and leave its text alone. The old record
is the only evidence of why the previous shape was chosen.

The original `docs/decisions/ADR-001-core-stack.md` was superseded by 0001 and has
been folded into it.
