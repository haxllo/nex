# Documentation coverage

Every document in this tree was written against the source at `HEAD` rather than
copied forward from an earlier version. This page records what each one was checked
against and, more usefully, what it does not cover.

Last full pass: 2026-11-22, at version 3.3.0.

## How a document earns "grounded"

A claim is grounded when it was read out of the code, not remembered. For this pass
that meant grepping the constant, reading the function, or running the command
before writing the sentence. Where the code disagreed with the previous
documentation, the code won and the document changed.

Two kinds of statement are marked instead of made: anything the code cannot settle
(a design intent, a user's preference) and anything that cannot be verified on a
machine without the right hardware or privilege. Those are written as limits, not
as facts.

## Status by document

| Document | Checked against | Known gaps |
|---|---|---|
| [user/install.md](user/install.md) | `Config::default`, CLI argument parsing in `runtime.rs`, the app-directory helpers in `config.rs`, `CURRENT_CONFIG_VERSION` | Installer internals were read from the packaging script, not run |
| [user/using-nex.md](user/using-nex.md) | `app.js` key handling, `query_dsl.rs` mode detection, tray menu and tooltip strings in `tray.rs` | Copy of the panel's exact wording was not exhaustive |
| [user/search-syntax.md](user/search-syntax.md) | `query_dsl.rs` operator and field parsing, `search.rs` | Unicode and quoting edge cases are described from the parser, not exercised |
| [user/assistant.md](user/assistant.md) | `chat_tools.rs` tool list and annotations, `chat.rs` loop, `CONFIRM_TIMEOUT` | Provider-side limits and model names change outside this repository |
| [user/settings.md](user/settings.md) | `Config::default`, `settings_catalog.rs`, `settings.js` | Only the four in-app pages are covered; other keys are TOML-only |
| [user/troubleshooting.md](user/troubleshooting.md) | Hotkey supervisor in `hotkey.rs`, `SCHTASK_NAME`, discovery backend selection, app-directory helpers in `config.rs` | Symptoms were not reproduced; each entry follows from the code path |
| [development/onboarding.md](development/onboarding.md) | `Cargo.toml` for all three packages, CI workflow, git hooks, `build.rs` | Toolchain workarounds are the author's machine, not a general rule |
| [development/architecture.md](development/architecture.md) | Process spawn paths, `PROTOCOL_VERSION`, `INDEXER_ADDR`, singleton mutex | Startup ordering is read from code, not traced under a debugger |
| [development/index-and-search.md](development/index-and-search.md) | `mft_discovery.rs`, `index_store.rs`, `tantivy_search.rs`, `search.rs` scoring constants | MFT timing figures are from one machine and one drive pair |
| [development/overlay-and-glass.md](development/overlay-and-glass.md) | `refraction.rs`, `glass_ps.rs`, `glass_ps.hlsl`, `host.rs`, `icons.rs` | The material itself cannot be captured, so appearance claims rest on the shader, not on looking at it |
| [development/chat-and-tools.md](development/chat-and-tools.md) | `chat.rs` loop and provider match, `chat_tools.rs` predicate and annotations | Streaming behaviour per provider was not exercised against a live endpoint |
| [development/ipc-and-privileges.md](development/ipc-and-privileges.md) | Pipe names and creators in `hotkey.rs`, `helper`, `mft_protocol.rs`; integrity expectations | The UIPI rules are documented behaviour; the failures were not re-induced this pass |
| [development/configuration.md](development/configuration.md) | `config.rs` migrations, template writer, validation ranges | The migration list is a summary, not a key-by-key diff of all 27 versions |
| [development/testing.md](development/testing.md) | CI workflow steps, test file inventory, opt-in environment variables | Test counts change constantly and are deliberately not quoted |
| [development/releasing.md](development/releasing.md) | `.github/workflows/release.yml`, `build.rs`, version fields in three crates | The packaging scripts were read, not run |
| [development/validation-checklist.md](development/validation-checklist.md) | Release workflow, existing checklist structure | The manual scenarios are a maintained list; nobody re-ran all of them this pass |
| [adr/*.md](adr/) | Code for the decision's current state; release notes and commit history for the date and the alternatives | Alternatives are recorded as they were argued at the time, which is a reconstruction from commit messages and release notes |
| [history/timeline.md](history/timeline.md) | Tag dates, release-note headlines, commit messages for abandoned work | Deliberately a summary; the per-version detail is in `releases/` |
| [reference/](reference/) | Copied verbatim, not verified | External material; nothing here is a statement about Nex |

## What no document here can tell you

Four things are outside what reading the code can establish. Each is named so that
nobody spends an afternoon trying.

- **What the glass looks like.** Both windows exclude themselves from screen
  capture. A screenshot returns the desktop behind the panel. A GPU readback probe
  or a photograph of the screen is the only evidence.
- **Whether the elevated hotkey path works.** It needs High integrity. A Medium
  process cannot observe or simulate it, and both keyboard hooks discard injected
  input, so `SendInput` cannot open the panel either.
- **Whether the helper starts.** `cargo test -p nex-helper` cannot run, because the
  helper embeds a `requireAdministrator` manifest and the harness exits with
  "The requested operation requires elevation".
- **What search feels like on a large real drive.** Item caps, MFT availability, and
  disk speed all change the answer, and none of them are in the repository.

## Keeping this honest

Re-check a document when you change the code it describes, and update its row here
if the gap changed. A document whose row still claims a date from 2026-11-22 while
the code has moved is not covered by this page any more; it is covered by nothing.

## Corrections made in this pass

The previous documentation was not merely out of date. Some of it was wrong in ways
that would have actively misled a new contributor, so the corrections are recorded
rather than silently applied.

| Claim that was wrong | Reality |
|---|---|
| `cargo run -p nex`, and a `nex.exe` binary | There is no `nex.exe`. `apps/core` is a library crate; the binaries are `NexUIProcess`, `NexIndexerProcess`, and `NexHelper` |
| The overlay is built with Iced | The overlay is WebView2 via `tao` and `wry`. The Iced migration was abandoned in June 2026 |
| `CURRENT_CONFIG_VERSION = 13` | It is 27 |
| Config key `kind:` filters search results | It is offered by the generated template but never parsed |
| The helper is started with `runas` | It is started through the `NexHelperV2` scheduled task |
| A whole-volume index is walked per root | MFT discovery is per volume; roots are matched against the result |
| Release notes exist in the tree for every tag | They exist for 94 of 111 tags, up to `v3.0.0` |
