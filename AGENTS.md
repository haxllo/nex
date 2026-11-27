# Nex — Agent Reference

## Non-negotiables (read before editing)

1. **Find the existing issue before touching code.** For any user-reported bug,
   feature request, or implementation task, run the lookup in "Finding the issue
   you are about to fix" *before* editing. Reuse the issue you find, widen it if
   the symptom differs, and create one only when nothing covers it. Never create
   a near-duplicate of an issue that is already open. Questions, planning and
   status requests need no issue. Details: "Linear Project Tracking" below.
2. **Never close an issue yourself.** Finished work moves to **In Review**; only
   a merged PR moves it to **Done**. Setting Done before the PR merges is not
   allowed, no matter how complete the implementation looks.
3. **Verify, then report.** Use the release profile and `G:\nex-target`, run the
   relevant gate, and never claim a check passed unless it was run.
4. **Commit identity.** Author is the human responsible for the change — the
   repo owner here, an outside contributor on their own pull request — using
   their own real address. Never a placeholder like `<your-email@example.com>`:
   GitHub credits a commit to whoever owns the author email, so a placeholder
   silently credits a stranger. For the owner's own commits, no bot/AI name as
   author and no AI/bot trailers in the body; a contributor's commit message is
   their own business. Do not commit unless asked.
5. **Process boundaries.** UI (Medium IL) never indexes; indexer (Medium IL)
   never renders; helper (High IL) is a broker, not a supervisor. See
   "Process Model & Privilege Boundaries" below.
6. **Branch first.** Implementation work for a recorded issue goes on its own
   branch; never commit it straight to `master`. Work on `master` only when the
   user explicitly asks. The `commit-msg` hook refuses an issue commit on
   `master`/`main`. The branch is created from `master` before the first commit,
   named after the issue (`ciq-42-overlay-focus`).

## Build & Test

**Cargo target location**: Use `G:\nex-target` for Cargo commands when available. Set `$env:CARGO_TARGET_DIR='G:\nex-target'` before `cargo build`, `cargo check`, or `cargo test` to avoid C: drive pressure.

**Toolchain location (this machine)**: the default `C:\Users\<user>\.rustup`
`rustc` **crashes** on this crate (`STATUS_STACK_BUFFER_OVERRUN`, exit
`0xc0000409`) when emit-warnings runs out of memory — the machine has 7.9 GB
RAM and a pagefile capped by a full C: drive. A working 1.93.0 toolchain and
`CARGO_HOME` live on G:. Build with all three paths on G:, and **one job**
(`CARGO_BUILD_JOBS=1`); parallel jobs hit `LNK1102: out of memory` and leave
corrupt rlibs behind, which then fail as bogus `E0786` invalid-metadata errors
— if that happens, `rm -rf G:\nex-target\release` before retrying.

```powershell
$env:RUSTUP_HOME='G:\rustup-home'
$env:CARGO_HOME='G:\cargo-home'
$env:CARGO_TARGET_DIR='G:\nex-target'
$env:CARGO_BUILD_JOBS=1
cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess
```

```bash
# same thing from bash (Git Bash)
export RUSTUP_HOME='G:\rustup-home' CARGO_HOME='G:\cargo-home' \
       CARGO_TARGET_DIR='G:\nex-target' CARGO_BUILD_JOBS=1
cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess
```

A cold release build takes ~10 minutes here; the output binary is at
`G:\nex-target\release\`. `NexUIProcess.exe` locks itself while running, so
`taskkill //F //IM NexUIProcess.exe` before rebuilding. Web assets are
`include_str!`-embedded at compile time, so **any change to `style.css`,
`app.js` or `index.html` needs a rebuild** to appear in the app — editing them
alone changes nothing at runtime.

Long builds get killed by the tool timeout, so run them detached with a log
(`nohup … > /g/build.log 2>&1 &`) and poll, rather than blocking on one call.

```bash
cargo build --bin NexUIProcess         # debug build (UI only)
cargo build --release -p nex-helper      # release build (helper only)
cargo test -p nex --lib chat             # chat/tool-loop tests (fast, reliable)
cargo test -p nex                        # all unit tests (Windows)
cargo test -p nex-indexer --test process_isolation_test  # IPC/process-isolation e2e
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
cargo test -p nex --test windows_runtime_smoke_test  # CI-only smoke test
```

**Verifying UI work**: the chat/overlay HTML+CSS+JS can be rendered and
measured outside the app with Playwright driving the Chrome already installed
(`playwright-core` in `G:\nex-test`, `executablePath:
C:\Program Files\Google\Chrome\Application\chrome.exe`). Stub the panel at
760x520, then assert on `getBoundingClientRect()` values and take screenshots.
This is how layout and contrast claims should be made — measuring, not reading
the stylesheet. Keep scratch files out of the repo and delete them before
committing.

**The glass theme cannot be screenshotted or recorded, and cannot be driven by
synthetic input.** Both windows call `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`
(`overlay/refraction.rs`, `set_capture_exclusion`), so any capture — Snipping
Tool, OBS, `CopyFromScreen`, desktop duplication, Playwright — returns whatever
is *behind* the window where the glass and page are. The same applies to
mirroring the panel into a Python/JS harness: that renders the page, not the
material. Screenshotting the live overlay leaves you looking at the desktop
underneath it and is not evidence about the glass. Both keyboard hooks also drop
injected input (`kb.flags & LLKHF_INJECTED`), so `keybd_event`/`SendInput` can
never open the panel — a synthetic `Win` only opens the Start menu. Verify glass
work with a GPU readback probe of the real effect graph at panel size
(`refraction.rs` tests, run with `--ignored`), or mirror the maths in Python.
Ask the user to describe or approve the look. A **phone photo of the screen is
the one way to capture the glass**: hardware bypasses the affinity flag, so a
photo is real evidence, a screenshot is not.


`vitest --run` for the JS scaffold gate (just checks file existence). Server-side only, no browser.

**Note**: `cargo test -p nex --lib` now passes reliably (567 passed, 4 ignored, verified
over six consecutive full runs). It previously failed 1-2 tests per run on a rotating
victim, and this line used to say "broken for now" rather than describing the flakiness,
which hid it. The causes were real and are fixed:

- Tests built `CoreService` from `Config::default()`, whose `index_db_path` resolves to the
  live `%APPDATA%\Nex\index.sqlite3`. `CoreService` opens the Tantivy index beside it, so
  those tests read and wrote the developer's **production index** - the shared resource
  behind the rotating failures. Test services now own a throwaway index.
- Tantivy tests compete for the shared writer under full-suite CPU load, so a commit
  followed by `collect_garbage` can take longer than any reader deadline. The module
  serialises itself with a local mutex.
- `instance_signal_handles_absent_window` asserted a process-global fact and failed when any
  Nex window existed on the machine; it now skips instead.

`cargo test -p nex-helper` still cannot run: the helper embeds a `requireAdministrator`
manifest, so the harness exits with "The requested operation requires elevation" (os error 740).

**CI order**: two policy gates run first, before anything is compiled, and
they are the ones an agent skips when it follows the build-and-test part of
this file only. Neither is reachable from the `commit-msg` hook, which checks
only the Linear id and the branch, so a green hook says nothing about them.

1. `Require Linear issue for code changes` — a `CIQ-<n>` in the PR title or
   body.
2. `Require docs update for behaviour changes` — `scripts/check-docs-currency.sh`
   maps a source file to the document it invalidates (`config.rs` →
   `docs/user/settings.md` and so on; `scripts/check-docs-currency.sh --list`
   prints the table). If the source changed, the document changes in the same
   push. When the change genuinely cannot affect the document — a test module,
   a private helper — set `NEX_ALLOW_STALE_DOCS=1` and state the reason in the
   PR body, where the bypass is allowed only with one.

Run both locally before opening the PR; the docs gate takes a second and reads
the same changed-file list CI does:

```bash
git diff --name-only origin/master... | scripts/check-docs-currency.sh
git diff --name-only origin/master... | NEX_ALLOW_STALE_DOCS=1 scripts/check-docs-currency.sh
```

Then the build gates: `vitest --run` → `cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess` → `cargo test -p nex` → perf gate → smoke gate.

**Helper binary**: `NexHelper.exe` is a separate workspace member (`apps/helper`) built via `--bin NexHelper`. It runs elevated (High IL) via a scheduled task to handle hotkey detection when elevated windows (Task Manager, Settings) are foreground. The scheduled task is created automatically on first run (one-time UAC prompt). The helper lives beside `NexUIProcess.exe` and is found automatically.

## Running

`Nex.exe` is not a runtime; the UI entry point is `NexUIProcess.exe`.

```bash
.\target\release\NexUIProcess.exe --foreground   # dev mode, keeps terminal attached
.\target\release\NexUIProcess.exe --quit         # stop running instance
.\target\release\NexUIProcess.exe --status       # check if running
.\target\release\NexUIProcess.exe --restart      # restart
.\target\release\NexUIProcess.exe --diagnostics-bundle
.\target\release\NexUIProcess.exe                # normal: background hotkey runtime
```

Config created at `%APPDATA%\Nex\config.toml` on first launch. Index at `%APPDATA%\Nex\index.sqlite3`.

**Helper**: On first run nex creates a scheduled task (`NexHelperV2`) to launch `NexHelper.exe` at High IL. This enables hotkey detection and overlay foreground when elevated windows (Task Manager, Settings) are active. The scheduled task creation triggers a one-time UAC prompt.

## Project Structure

- Workspace members: `apps/core` (crate `nex`, lib `nex_core`), `apps/ui` (bin `NexUIProcess`), `apps/indexer` (bin `NexIndexerProcess`), `apps/helper` (bin `NexHelper`)
- Binary entry: `apps/ui/src/main.rs` → `nex_core::runtime::run_with_options`
- Library entry: `apps/core/src/lib.rs`
- **Two-process split**: `NexUIProcess.exe` owns the overlay, hotkeys, tray, and search over the indexer's published snapshot. `NexIndexerProcess.exe` owns discovery, SQLite writes, file/app watchers, and Tantivy. There is no in-process indexing fallback.
- **One binary per package on purpose**: a build script's Windows resource is linked into all of its package's binaries and inherited by dependent packages, so sharing a package would give every process the same `FileDescription` (Task Manager name).
- **Windows-only modules**: `runtime_loop`, `runtime_overlay_rows`
- Tests in `apps/core/src/runtime.rs:tests` and `apps/core/tests/`, `tests/perf/`, plus process-isolation e2e in `apps/indexer/tests/`

## Process Model & Privilege Boundaries

| Process | Integrity | Owns |
|---|---|---|
| `NexUIProcess.exe` | Medium | Overlay/WebView2, hotkeys, tray, settings, search over the published snapshot. Session owner: spawns, supervises, and stops the indexer and helper. |
| `NexIndexerProcess.exe` | Medium | Discovery, SQLite writes, file/app watchers, Tantivy. Parent-PID watchdog; exits when the UI disappears. |
| `NexHelper.exe` | High (elevated) | Narrow privileged broker: elevated hotkey detection, foreground grant, input simulation. **Never owns process lifecycle.** |

**Pipe integrity rule (UIPI):** a Medium-IL process cannot write into a
High-IL pipe (`Access is denied`). Therefore:
- only the log pipe the UI *reads* is created by the helper (`NexHelperLog`);
  a Medium-IL reader is allowed to read *down* from a High-IL server;
- the detection pipe (`nex-hotkey`) is created by the UI and read by the UI,
  with the helper connecting as the client that writes. It cannot be
  helper-created: that made the UI the reader of a High-IL pipe, which UIPI
  refuses, so the helper's writes succeeded into its own server end while nex
  received nothing;
- the command pipe the UI *writes* must be created by the UI (`NexHelperCmd`),
  with the helper as the read-only client;
- the MFT command channel (`NexMftCmd\<ui pid>`) is created by the indexer, which
  writes requests; the helper connects as a client and streams framed snapshots
  back (`apps/core/src/mft_protocol.rs`, shared with the helper via `#[path]`).
Do not invert that ownership.

**Low-level hooks are UIPI-gated too:** `WH_KEYBOARD_LL`/`WH_MOUSE_LL` in the
Medium-IL UI never fire for input routed to an elevated window (Task Manager,
Settings). Keep that detection in the helper and forward it over the detection
pipe (`HOTKEY`, `MOUSEDOWN`); the UI re-checks overlay visibility and the
panel rect.

**Supervision:** the UI's hotkey supervisor keeps the helper connected and
reconnects with 2s→30s backoff when the pipe closes; the in-process hook stays
active meanwhile. The snapshot-sync loop respawns a dead indexer.

## Agent Tooling

- `scripts/git-hooks/commit-msg` enforces the non-negotiables: it refuses a
  placeholder or domainless author identity for anyone (GitHub would credit a
  stranger), and for the owner's own commits it also strips AI trailers and
  assistant signatures and refuses a bot author. A contributor's identity and
  message are never rejected. It then rejects a commit that changes `apps/`,
  `scripts/`, or `manifests/` without a `CIQ-<n>` id, and rejects an issue commit
  made on `master`/`main` (staged changes survive, so the commit is redone on a
  branch). Enable it per clone with `git config core.hooksPath scripts/git-hooks`.
  Exceptional commits can bypass once via `NEX_ALLOW_NO_LINEAR=1` (no id) or
  `NEX_ALLOW_MASTER_COMMIT=1` (issue commit on master).
- Hooks are opt-in per clone and `--no-verify` skips them, so the branch rule is
  a guard, not a guarantee; CI and review are the backstop.
- `CLAUDE.md` and `.github/copilot-instructions.md`
  repeat the non-negotiables for agents that do not load this file.
- CI rejects pull requests that change code paths without a Linear id in the
  title or body.

## Toolchain

**Local (Windows)**: `stable-x86_64-pc-windows-gnu` with MinGW-w64 at `C:\Users\Admin\AppData\Local\Microsoft\WinGet\...\mingw64\bin`.
**CI**: `dtolnay/rust-toolchain@stable` (auto-detected native target).

## Environment Variables

| Variable | Purpose |
|---|---|
| `NEX_SUPPRESS_STDIO=1` | Suppress stdout logging (also `SWIFTFIND_SUPPRESS_STDIO`) |
| `NEX_WINDOWS_RUNTIME_SMOKE=1` | Enable the windows runtime smoke test (otherwise skips) |
| `NEX_DISCOVERY_BACKEND` | `mft` forces the NTFS MFT snapshot backend, `walkdir` disables it; unset = auto (MFT when a per-root cap is >= 100k) |

## Overlay Architecture

**WebView2** (tao + wry). Overlay is a `WS_POPUP` window hosting a WebView2
control. No GDI/GDI+/D2D — all rendering is HTML/CSS/JS.

- Panel built from embedded HTML (`INDEX_HTML`, `STYLE_CSS`, `APP_JS` in
  `overlay/host.rs`), served via `nexasset://` custom protocol.
- State pushed to JS via `ICoreWebView2::PostWebMessageAsJson` (fire-and-forget,
  non-blocking). No synchronous `evaluate_script` on the critical path.
- Icons decoded to PNG, embedded as base64 data URIs in the state snapshot
  (`overlay/icons.rs`). LRU cache keyed by file path, backed by a bounded
  disk cache (`overlay/icon_disk_cache.rs`, `%APPDATA%\Nex\icon-cache\v<N>`)
  validated by source size/mtime fingerprints. "Clear Icon Cache" action
  empties both tiers.
- Window positioning, DPI handling, acrylic backdrop in `overlay/host.rs`.
- Hotkey listener on dedicated thread (`overlay/hotkey.rs`). System tray icon
  with context menu (`overlay/tray.rs`).
- Single warm-release timer thread (crossbeam channel, re-arm on Hide); trims the in-memory icon cache only (disk tier kept) — WebView stays warm.
- Theme detection: Windows registry `AppsUseLightTheme` (`overlay/platform.rs`).
- Indexing progress window (`overlay/indexing_progress.rs`): a second, non-activating window in the overlay event loop, shown bottom-right only during large full discovery passes. The indexer reports `full_pass` counts in its `sync` response; `indexing_progress::ProgressWindowGate` decides show/update/finish.
- Mica backdrop via `DWMWA_SYSTEMBACKDROP_TYPE` planned but not yet implemented.

## Config

TOML format (primary), with JSON/JSON5 backward compatibility. **Never add new keys to the JSON template** — only TOML template (`apps/core/src/config.rs::write_user_template_toml`). Config is versioned (`CURRENT_CONFIG_VERSION = 13`); migrations in `apply_migrations()`.

## Release

**IMPORTANT**: Release workflows must be run directly by the main agent (me), NOT delegated to subagents. Use subagents only for review/verification of release output if needed.

**Trigger**: When the user asks to run, create, or publish a Nex release, the
work is: bump the version, rewrite `docs/whats-new.md`, commit, and
push the tag. Pushing a `v*` tag runs `.github/workflows/release.yml`, which
verifies the tag matches all three crates, builds, packs the zip and installer,
and opens the GitHub release. **The agent never writes
`docs/releases/v<ver>-notes.md`** — it is generated, not hand-maintained, and
the workflow drafts it from the commit log at tag time. Do not run the packaging
scripts by hand either. Do not bump the version or push a tag unless
the user asked for a release.

**Two separate release documents.** Both ship; neither substitutes for the other.

| File | Audience | Who reads it |
|---|---|---|
| `docs/releases/v<ver>-notes.md` | GitHub release page | people browsing releases; includes the commit log |
| `docs/whats-new.md` | in-app What's New view | the person using Nex, nothing else |

They are not views of one file. `docs/whats-new.md` is a single document edited
in place every release — it carries its own format instructions in HTML
comments, which `apps/core/build.rs` strips before baking it into the binary.
Never put tickets, commits, file or function names in it.

`apps/core/build.rs` bakes `docs/whats-new.md` at compile time, so it must
exist before the build. A build without it emits a `cargo:warning` and bakes an
empty string, which ships the tips fallback.

**Rewriting the What's New document every release is required, not optional.**
Write the prose for this release's user-visible changes and bump its `release:`
marker in the same commit as the version bump. The marker is how a stale
document is detected; leaving it alone ships last release's text. The file is
committed, so its absence means a broken tree and fails the release; so does a
file with no text outside its HTML comments. `scripts/windows/release-notes-from-commits.ps1` drafts
`docs/releases/v<ver>-notes.md` from the commit log when that file is absent,
and the workflow runs it automatically; it is a drafting tool for the release
page only, never a source for What's New.

```bash
# 1. bump version in apps/core, apps/ui, and apps/indexer (all three build
#    scripts stamp FileVersion from their own package)
# 2. rewrite docs/whats-new.md for this release, and bump its `release:` marker
#    (both required; skipping either ships last release's text)
#    — do 1-2 in one commit, because the What's New document is compiled in

# NO local release build is needed. CI builds the shipped binaries; a local
# build only proves the tree compiles and costs ten minutes. Do not run one
# as part of releasing. Do not write docs/releases/v<ver>-notes.md either.

# 3. tag only after those commits are on master, then push
git tag v<ver>
git push origin master --tags
# .github/workflows/release.yml does everything else: verifies the tag against
# all three crates, drafts missing notes, builds, packages the zip and the
# installer, and opens the release.
```

Publishing happens from the tag. The release page gets the release notes file;
the app shows the What's New document.

Artifacts land in `artifacts/windows/nex-<ver>-windows-x64.{zip,setup.exe}` + manifest.

## Style & Conventions

- `cargo build` warnings: ~12 dead-code warnings (pre-existing, mostly unused overlay/misc functions)
- No clippy, no formatter enforced in CI
- Most platform gating: `#[cfg(target_os = "windows")]` on Windows-specific modules/functions
- Legacy name "SwiftFind" still appears in some env var/constant names — don't rename unless explicitly asked

## Git Commit Author

All commits MUST be attributed to the human who is responsible for the change,
under their own real email address. In this repo that is the owner; on a
contributor's pull request it is that contributor. Never a placeholder. For the
owner's own commits: never a bot/AI name, and the body carries no assistant
trailer; that part is personal, not a rule imposed on contributors.

```bash
git commit --author="Habeeb <mshabeeburrahman786@gmail.com>" -m "message"
```

Why the email matters: GitHub credits a commit to the account that owns the
author *email*, not the name. A commit made as `<your-email@example.com>` — git's
unconfigured default — is credited to whichever stranger registered that address;
this repo has exactly one such commit and it put an unrelated person on the
contributors list. `scripts/git-hooks/commit-msg` refuses placeholder, test and
domainless addresses for everyone, and prints a note when the author is not the
owner (an outside contributor is not blocked). The AI-trailer stripping and the
refusal of a bot *author* apply only in the owner's own clone, keyed off the
configured identity — a contributor using Claude may keep its trailer.
`NEX_OWNER_EMAIL` changes whose address counts as the owner's.

An agent working for someone commits under **that person's** identity, not its
own: the human is accountable for the change, and attribution must point at a
person. That is not licence to borrow a name — use the address of the person who
owner the work and can answer for it.

**Note**: Use this identity for any git operation that requires an author (commits, amends, rebases, etc.).

## Commit Message Content

The rules below are **the owner's, for the owner's own commits**. A contributor's
commit message is theirs: an outside contributor may commit through an assistant
and keep whatever trailer that tool appends. Nothing in this section is used to
reject someone else's commit.

Commit message bodies MUST contain NO AI agent embedded names. This means absolutely NO `Co-authored-by:` trailers referencing any bot or AI, NO bot signatures, NO mentions of AI assistants, and NO references to agent systems. This applies to commit messages, tag messages, branch descriptions, and any other git-related text.

### Finding the issue you are about to fix

A duplicate has already been created here by skipping the step below, so the order
is fixed. Each step is cheaper than the one after it, and the first one that
answers ends the search.

1. **Did the task arrive as a GitHub issue?** Read its backlink comment; it names
   the Linear issue outright and nothing else needs searching:

   ```bash
   gh issue view <n> --json comments --jq '.comments[].body' | grep -o 'CIQ-[0-9]*'
   ```

   That is the id to work. Do not create a second Linear issue for the same
   report.
2. **No backlink, and Linear is reachable?** Search P-CIQ-2 with **at least three
   different vocabularies** before concluding nothing exists: the symptom in the
   reporter's words, the symbol or config key involved, and the surface (installer,
   overlay, search, discovery). Titles are a weak match; open the plausible hits
   and compare their "Observed" section against the report in front of you.
3. **No Linear access (an outside contributor)?** Search the GitHub issues instead
   and use that issue as the record. See "Contributors without Linear" below.
4. **Found the same problem under a different symptom?** Do not open a twin.
   Comment what you are about to change on the existing issue and widen its body
   so it still describes the whole problem. Two symptoms, one cause, one issue.
5. **Found an issue that only partly covers this?** Create a new issue only when
   the work genuinely splits, link it with `relatedTo`, name the older issue in
   the new one, and use *Related to CIQ-n* in the PR. If the older issue also
   still needs work, say so in a comment before you start.
6. **Realised later that a duplicate exists anyway?** Mark it with
   `save_issue({ id, duplicateOf: "<the other id>" })`, comment naming the issue
   that survives, and close it as **Duplicate** (never Done). Close the mirrored
   GitHub issue by hand with a pointer, because the mirror is create-only.

**The one time this was got wrong here**, for calibration: a report that
`panel_background = "acrylic"` stopped Nex from starting was filed as a new issue
(#443) two days after #388, "A typo in config.toml makes Nex not start at all,
with no message anywhere a user would look". Same root cause — an unreadable
config ends the process — so the correct move was a comment on #388 naming the enum
case being fixed and leaving its other half (the silence, and the five backups
never offered) open. Instead #388 still sits open and #443 shipped separately.

## Linear Project Tracking

The Nex Linear project is [P-CIQ-2](https://linear.app/ciqada/project/nex-838bd9949214).

- For Nex bug reports, feature requests, and coding tasks that require implementation, search the Nex project for a matching issue before editing code. Reuse the matching issue when one exists; otherwise create an issue in the Nex project. Do not create issues for questions, planning, or status requests that do not ask for implementation.
- Track a release-only request with a Linear Release record, following the configured Nex release pipeline. Do not create a regular issue solely to build, package, tag, or publish a release. Link related implementation issues to the release when available. If no release pipeline is configured or accessible, tell the user and do not create a regular issue as a substitute.
- Keep the issue current as work progresses. Before finishing, record the implemented behavior and the build/test checks that were or were not run.
- **State policy:** agents may move issues to In Progress or In Review. Only a
  merged PR closes an issue (moves it to Done). Do not set Done directly, even
  for retroactive or verification-only work.
- When work is delegated, agents should update the same issue and avoid creating duplicate issues for subtasks.
- **Agents working for the owner** need Linear: if the connection is missing, ask for it before starting. Outside contributors are the exception, below.
- Use project status updates for periodic progress, health, and risks; use milestones for major deliverables with clear outcomes. Do not create a milestone for every issue or a health update without evidence.
- If Linear is unavailable to an agent working for the owner, ask for it before
  starting implementation.

### Contributors without Linear

The Linear project is not shared with outside contributors, and nothing about
contributing should depend on it. Someone cloning this repo and opening a pull
request works in GitHub only:

- **The record is the GitHub issue.** Search the open issues (several wordings,
  same as step 2 above) and reuse or open one there. Do not create Linear issues,
  and never invent a `CIQ-<n>` id.
- **The id is the maintainer's job.** A PR from a contributor may go up without a
  `CIQ-<n>` in its body; the owner links it to a Linear issue and adds the id
  before merge. The contributor is not asked to wait for access.
- **The local hooks are the owner's rules.** A contributor's commit message is
  their own (see "Commit Message Content"); `NEX_ALLOW_NO_LINEAR=1` exists for
  maintenance commits and is not something a contributor has to reach for.
- If an agent is doing the work for a contributor, it follows this section, not
  the Linear steps above.

### Use the whole issue, not just the title

Linear is the record of the work, not a to-do list. Fill in the fields that
carry meaning, and skip the ones that would be invented:

- **Description** — the observed behaviour, the cause if known, and the
  intended fix. Write it so someone who was not in the session can act on it.
- **Labels** — reuse the existing set (`Bug`, `Performance`, `Tech Debt`,
  `Discovery`, `Hotkey`, `UX`, `Search`, `IPC`, `Compatibility`). Do not
  invent a label for a single issue; if nothing fits, leave it empty rather
  than creating noise.
- **Priority** — 1 Urgent, 2 High, 3 Medium, 4 Low. Set it from impact, not
  from how interesting the work is.
- **Milestone** — attach to an existing milestone when the work clearly
  belongs to one (e.g. "Hotkey stack robustness", "Reliable file/folder
  search"). Do not create a milestone for a single issue.
- **Relations** — link issues that genuinely depend on each other with
  `blocks` / `blockedBy`, and use `relatedTo` for shared-cause work. This is
  how a fix that unblocks another gets found later.
- **Comments** — post the PR link, what was verified, and what was not. When
  splitting a ticket, comment on the parent naming the new issues so the
  split is traceable.
- **Estimate** — only when the user asks for one. Do not guess.
- **Assignee** — set it when the user is the one who will act on it.

Do not create labels, milestones, cycles, or documents speculatively. Check
what already exists first (`list_project_labels`, `list_milestones`,
`list_cycles`) and reuse it.

### Closing and splitting

- A PR that only *partly* addresses an issue must say `Related to CIQ-n`, not
  `Closes CIQ-n`. Closing a ticket with work still on it silently drops the
  remainder.
- When an issue carries unrelated items, split them into their own issues
  before the PR merges, comment on the parent naming them, and leave the
  parent tracking only what the PR actually delivers.

- Only a merged PR moves an issue to Done. If a PR is merged and the issue is
  still open, pull master, confirm the change is present, then close it.

### GitHub issues mirror Linear — keep both in step

Every Linear issue in P-CIQ-2 has a GitHub issue with the same title. GitHub
`#126` is not Linear `CIQ-126`; the numbers drift apart, so never assume they
match. Title is the join key.

**Creation is automatic.** A Linear integration creates the GitHub issue from
the Linear one, usually within seconds and with the full description. Do **not**
run `gh issue create` for a Linear issue — that produces a duplicate. Create the
Linear issue and wait (or re-list `gh issue list --state open`) to confirm the
GitHub side appeared.

**Closing is manual.** Nothing closes the GitHub issue for you, and `#NNN` in a
PR body closes the *GitHub* issue while doing nothing to Linear. Handle both:

1. Put the GH reference in the PR body (`Closes #128`) so GitHub closes itself.
2. Put the Linear id in the PR title/body (`CIQ-127`) so the branch and commit
   pass CI, and set the Linear issue to In Review.
3. After merge, close both: the Linear issue to Done, and the GitHub issue
   with `gh issue close <n> --reason completed --comment "Fixed in #<pr> (<commit>)"`.

Caveat on step 1: a plain `Related to #NNN` reference is **also** auto-closed by
a merged PR (observed on #120), so a partial PR can close a GitHub issue you
meant to leave open. Prefer naming the number in prose, not as a `Related to`
keyword, when the work is partial.

When closing a batch, match by title — do not trust the number. Audit drift
with `gh issue list --state open` against `list_issues` on P-CIQ-2; a burst of
GitHub issues that are Done in Linear means the sync was skipped.

### GitHub comments are for the reporter, Linear comments are for us

A GitHub issue may be filed by a stranger. Treat its comments as a message to
that person, not as your working notes. The internal record belongs in Linear,
which is where the owner reads it.

Write to the GitHub reporter with:

- a thank-you and an explicit acknowledgement of what they got right;
- what was wrong, in one or two sentences, in their terms;
- what changed for them, not which functions moved;
- an honest caveat when something could not be verified;
- a concrete ask ("rescan on the next build and tell us").

Do not put in a GitHub comment:

- commit SHAs, PR numbers or branch names — they assert state that may change,
  and a SHA on master can be reverted before the fix ships. Name the merge
  once it has happened, not while it is open;
- test counts, internal type or function names, or file paths;
- how the fix was verified, or the reasoning that got there;
- first-person review language ("checked rather than assumed", "your other
  point was right too") that only makes sense to the owner.

The same facts, in full, go on the Linear issue. Keeping the two audiences
apart is the whole point: a reporter wants to know it is fixed and whether to
retest, and nothing else in the comment should compete with that.

### Other Linear surfaces worth using

The workspace has one team (`CIQ`, "Ciqada") and one project (`Nex`, `P-CIQ-2`).
There are **no** release pipelines and **no** project or issue templates, so
do not reach for those. Features that do exist and are under-used:

- **Documents** (`list_documents`, `get_document`, `save_document`) — the
  established pattern for work that outlives an issue: investigation write-ups,
  multi-phase plans, audit roadmaps. `CIQ-78 plan` and the search/scalability
  audit are both documents. When an issue is really a design question, write a
  document and link it from the issue instead of a very long description.
- **Development log documents** (`Nex Development Log — <Month> <Year>`) — a
  per-month commit ledger maintained by the repo owner. Do not edit these from
  an agent session; they are generated. Read them when you need history.
- **Project status updates** (`get_status_updates`, `save_status_update`) — for
  periodic progress, health (`onTrack` / `atRisk` / `offTrack`) and risks. Use
  these to report on a body of work; use comments to report on one issue. Post
  an update only when there is evidence, never as a ritual.
- **Notifications** (`get_notifications`, `mark_notification`) — check with
  `get_notifications` at the start of a session to see whether the owner has
  replied on an issue. Two caveats: the list is mostly historical (status
  changes and old comments, not unread replies), and some bodies are errors
  from Linear's own AI features (e.g. missing AI credits) that are not
  addressed to the agent. Filter by `issueNewComment` near the top, and ignore
  the rest.
- **Search** (`list_issues` with `query`, `search_documentation`) — `query`
  searches titles and descriptions, which beats guessing an identifier. Use
  `search_documentation` for how Linear itself works.
- **Statuses** (`list_issue_statuses`) — the workflow is Triage, Backlog, Todo,
  In Progress, In Review, Done, Canceled, Duplicate. "In Review" is a
  *started* state, which is why a merged PR still has to move an issue to Done
  explicitly.
- **Users** (`list_users`, `get_user`, `whoami`) — resolve a name to an id when
  setting an assignee.

Deliberately unused in this workspace: release pipelines and releases (none
configured), issue/project templates (none defined), customers and customer
needs (not used), agent skills, and the diff-review tools (reviews happen on
GitHub PRs, not Linear diffs). Do not introduce them without the owner asking.
