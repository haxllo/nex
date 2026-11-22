# Nex documentation

Nex is a keyboard-first launcher for Windows, written in Rust. Press a hotkey,
type, press Enter. This directory holds every document about it.

Everything under `user/`, `development/`, and `adr/` describes the source tree at
`HEAD`. If a document disagrees with the code, the code is right and the document
is a bug.

## I want to

| Goal | Read |
|---|---|
| Install Nex and get it running | [user/install.md](user/install.md) |
| Learn the panel, keyboard, and results | [user/using-nex.md](user/using-nex.md) |
| Query with filters and operators | [user/search-syntax.md](user/search-syntax.md) |
| Use the assistant | [user/assistant.md](user/assistant.md) |
| Look up a setting | [user/settings.md](user/settings.md) |
| Fix something that is broken | [user/troubleshooting.md](user/troubleshooting.md) |
| Set up a development environment | [development/onboarding.md](development/onboarding.md) |
| Understand how the processes fit together | [development/architecture.md](development/architecture.md) |
| Work on discovery, indexing, or search | [development/index-and-search.md](development/index-and-search.md) |
| Work on the overlay or the glass material | [development/overlay-and-glass.md](development/overlay-and-glass.md) |
| Work on the assistant | [development/chat-and-tools.md](development/chat-and-tools.md) |
| Work on IPC, pipes, or the helper | [development/ipc-and-privileges.md](development/ipc-and-privileges.md) |
| Change a config key or add a migration | [development/configuration.md](development/configuration.md) |
| Run or add a test | [development/testing.md](development/testing.md) |
| Validate a release by hand | [development/validation-checklist.md](development/validation-checklist.md) |
| Cut a release | [development/releasing.md](development/releasing.md) |
| Learn why the code looks this way | [adr/README.md](adr/README.md) |
| Read about a published version | [releases/](releases/) |
| Read external material the code draws on | [reference/](reference/) |
| Know how current these documents are | [coverage.md](coverage.md) |

## How this tree is organised

```
docs/
  README.md        you are here
  coverage.md      which documents are grounded, and against what
  whats-new.md     in-app What's New page, baked into the binary
  user/            for people running Nex
  development/     for people changing Nex
  adr/             decisions, with the reasoning that produced them
  history/         how the project got here, and what was abandoned
  reference/       external material kept verbatim, not our writing
  releases/        release pages; drafted at tag time, mostly committed here
```

## What is authoritative

Some facts live in exactly one place. Do not copy them, link to them.

| Fact | Source of truth |
|---|---|
| What shipped in a version | `docs/releases/v<ver>-notes.md` |
| What changed for users, in-app | `docs/whats-new.md` |
| Every config key and its default | `apps/core/src/config.rs`, in `Config::default` and `write_user_template_toml` |
| Build, test, and release gates | [development/onboarding.md](development/onboarding.md), [development/testing.md](development/testing.md), [development/releasing.md](development/releasing.md) |
| Contribution and agent rules | `AGENTS.md` at the repository root |

`AGENTS.md` is the working agreement for coding agents and is deliberately not
duplicated here. It is the authority on process; this tree is the authority on
what the software does.

## Two release documents, not one

They look similar and are not interchangeable.

| File | Audience | Written by |
|---|---|---|
| `docs/releases/v<ver>-notes.md` | people on the GitHub release page | the release workflow, from the commit log |
| `docs/whats-new.md` | the person using Nex, in-app | a human, every release |

Never copy one into the other. `docs/whats-new.md` is compiled into the binary by
`apps/core/build.rs`, so editing it without rebuilding changes nothing at runtime.

## History

[history/](history/) explains how the codebase arrived at its current shape,
including the approaches that were tried and dropped. [releases/](releases/) holds a
release page for 94 of the 111 tags, from `v1.0.0` to `v3.0.0`; versions after that
have a page only on the GitHub release, because the notes file is drafted during the
release run and is not committed back to the tree.

Neither is required reading to work on Nex; they exist so that a decision does not
have to be re-litigated from memory.
