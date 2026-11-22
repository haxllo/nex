# Maintaining these docs

Docs change **in the same commit as the code that made them wrong**. Not in a
follow-up, not in a docs sweep later. A follow-up never happens, and this tree
already has one history of that failure behind it.

## Why this rule is written down

The previous documentation grew to 118 files between February and November 2026.
Every release was documented properly in its own release note. The guides still
drifted into describing a `nex.exe` that never existed, an Iced overlay that was
dropped in June, and a config version sixteen releases out of date.

Nothing went wrong with any individual change. The failure was structural: no
single person was ever responsible for noticing, so nobody ever did. A document
that describes the system is wrong the moment the system moves, and the cost of
being wrong is paid by whoever reads it next, who cannot tell that it is wrong.

The rule below works by making the check happen where the change is made, which is
the only place where knowing "this changed" is free.

## The trigger table

Find the row for what you changed. Update that document in the same commit.

| If you changed | Update | Also |
|---|---|---|
| A config key, its default, its range, or a migration | [../user/settings.md](../user/settings.md) | [configuration.md](configuration.md) |
| A CLI flag, an install path, or `%APPDATA%` layout | [../user/install.md](../user/install.md) | |
| A keyboard binding, a tray string, or what the panel shows | [../user/using-nex.md](../user/using-nex.md) | |
| A query operator, filter, or mode | [../user/search-syntax.md](../user/search-syntax.md) | [index-and-search.md](index-and-search.md) |
| A tool, its annotation, its refusal, or the approval rule | [../user/assistant.md](../user/assistant.md) | [chat-and-tools.md](chat-and-tools.md) |
| A process boundary, a pipe's owner, or an integrity rule | [architecture.md](architecture.md) or [ipc-and-privileges.md](ipc-and-privileges.md) | |
| The discovery path or the ranking order | [index-and-search.md](index-and-search.md) | |
| The effect chain, a fallback, or capture exclusion | [overlay-and-glass.md](overlay-and-glass.md) | |
| A CI gate, a test command, or an opt-in env var | [testing.md](testing.md) | |
| A release step, a version field, or an artifact name | [releasing.md](releasing.md) | |
| A path bundled into a release artifact | [validation-checklist.md](validation-checklist.md) | Check the packaging scripts |

Two rows deserve emphasis because they are easy to miss:

- **A new tool in the assistant is a user-visible capability.** It needs a row in
  the user guide's approval table and a line in the development doc, or the
  feature effectively does not exist for the person using it.
- **A reversed decision needs a new ADR**, with the old one marked
  `Superseded by ADR-XXXX`. Do not edit an accepted ADR. Its text is the only
  record of why the previous shape was chosen.

## Three documents are build inputs

These are not ordinary documentation and must not be reorganised on a whim.

| File | Consumed by | Consequence of moving it |
|---|---|---|
| `docs/whats-new.md` | `apps/core/build.rs`, baked in at compile time | The app ships an empty tips fallback, and the release workflow fails |
| `docs/releases/v<ver>-notes.md` | The release workflow, as the GitHub release body | The release fails, or gets an empty page |
| `docs/development/validation-checklist.md` | `scripts/windows/package-windows-artifact.ps1` | The release zip loses its checklist |

If you move any of the three, update the consumer in the same commit. The builds
do not read this table.

## Finding out you are wrong

Not every stale claim is found by the person who made it stale. If you read a
document that contradicts the code and you were not asked to fix it, you have three
options, in order of preference:

1. **Fix it**, if the correction is one sentence and you have the evidence.
2. **Correct the claim and note what you could not verify**, if the fix is larger.
3. **Leave it and say so** in your final message to the user, naming the file and
   the contradiction. Silence is the only bad outcome.

Do not "fix" a document by softening it into vagueness. A sentence that is careful
and useless reads as current and costs the next reader the same investigation
again.

## Adding a document

Prefer extending an existing document to creating a new one. A new file is
justified when it answers a different reader's question, not when it covers a
different subsystem.

When you do add one:

1. Give it a place in the tree that matches its audience.
2. Add it to the goal table in [../README.md](../README.md) and, if it is a
   development document, to the layout listing.
3. Add a row to [../coverage.md](../coverage.md) saying what it was checked
   against, and what it does not cover.
4. Link it from its nearest sibling so it is reachable by following a link rather
   than by guessing a path.

## Keeping coverage.md honest

[../coverage.md](../coverage.md) records what each document was verified against
and when. Update a document's row when you change that document, and refresh the
pass date only when you have actually re-checked the document against the code.

A row claiming an old date on a document that has moved is worse than no row at
all, because it reads as evidence when it is not.
