# History

This directory explains how Nex arrived at its current shape. It is not required
reading to work on Nex, and nothing here describes current behaviour.

Two rules govern everything in this folder:

- **Nothing here is current.** If a document in `history/` disagrees with
  `docs/development/` or with the code, the code is right.
- **Nothing here is rewritten.** History is appended to, not corrected. A decision
  that changed is recorded as a new entry, and the old one keeps its text.

## What is here

| Document | Contains |
|---|---|
| [timeline.md](timeline.md) | What each era of the project changed, and which approaches were tried and dropped |

## The complete record lives elsewhere

`docs/releases/` holds one page per published version, from
`v1.0.0-notes.md` onward. Those pages are the authoritative record of what shipped
when, and they were written at release time rather than reconstructed.

`CHANGELOG.md` at the repository root carries the same history in a classified
form for versions that predate the automatic release pages.

## Why an audience for this exists

Code shows what was built. It cannot show what was tried first and failed, and that
is the part that gets relitigated. Someone will eventually propose replacing the
discovery backend, or moving the UI to a native toolkit, or bundling a third-party
indexer. The timeline records that all three were tried, what it cost, and why each
was dropped, so the proposal can start from the previous answer rather than from
scratch.

The decisions themselves are recorded properly in [../adr/](../adr/). The timeline
is the timeline: a summary of sequence, not a substitute for reasoning.
