# Developer onboarding

This page gets you from a fresh clone to a running build with passing tests. It
assumes Windows, because Nex does not build anywhere else.

If you are here to change behaviour, read [architecture.md](architecture.md) after
this page. If you are here to fix a bug, `AGENTS.md` at the repository root states
the process rules, including the Linear issue requirement.

## Prerequisites

| Tool | Why |
|---|---|
| Rust stable | The crate uses edition 2024, so 1.85 or newer |
| A linker | Either the MSVC or the MinGW-w64 toolchain works |
| Node 20+ and pnpm | Only for the JavaScript test gate |
| WebView2 Runtime | Ships with Windows 11 and current Windows 10 |

Nex targets `x86_64-pc-windows-msvc` or `x86_64-pc-windows-gnu`. Nothing else is
supported and there are no non-Windows fallbacks for the overlay, the hotkey, or
the MFT discovery path.

## Build

```bash
git clone https://github.com/haxllo/nex.git
cd nex
pnpm install --frozen-lockfile
cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess
```

All three binaries are required. `NexUIProcess.exe` starts the other two itself,
so a build with only the UI does not run.

Run it attached to your terminal:

```bash
./target/release/NexUIProcess.exe --foreground
```

## Builds that keep running out of memory

The release build of this crate needs more memory than some machines can give it.
Two settings help:

```bash
export CARGO_TARGET_DIR=G:/nex-target
export CARGO_BUILD_JOBS=1
```

Building with one job avoids the parallel linker passes that exhaust memory. If a
build fails with `LNK1102` or with `E0786` invalid-metadata errors, the target
directory holds corrupt artifacts from an earlier attempt: delete the `release`
directory under your target path and build again.

A cold release build takes around ten minutes on a modest machine. Run it in the
background with a log rather than waiting on it:

```bash
nohup cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess > /tmp/build.log 2>&1 &
```

## The web assets are compiled in

`index.html`, `style.css`, `app.js`, `settings.html`, and `settings.js` under
`apps/core/assets/` are embedded in the binary with `include_str!`, and
`docs/whats-new.md` is baked the same way by `apps/core/build.rs`.

Editing any of them without rebuilding changes nothing at runtime. This is the
single most common reason a UI change appears not to work.

## Tests

```bash
pnpm exec vitest --run                                     # JavaScript assets
cargo test -p nex                                          # Rust unit and integration tests
cargo test -p nex --lib chat                               # one module, fast
cargo test -p nex-indexer --test process_isolation_test     # cross-process IPC
```

`cargo test -p nex-helper` cannot run. The helper embeds a
`requireAdministrator` manifest, so the test harness exits with
`The requested operation requires elevation`.

[testing.md](testing.md) covers what each layer is for and how to verify UI work.

## Git hooks

The repository ships hooks that enforce the contribution rules: an issue id on
any commit that touches `apps/`, `scripts/`, or `manifests/`, no AI attribution
trailers, and no issue commit on `master`.

Enable them once per clone:

```bash
git config core.hooksPath scripts/git-hooks
```

## Where things live

```
apps/
  core/      nex_core, the library every binary links
    src/
      overlay/     WebView2 host, hotkey, tray, icons, glass renderer
      discovery/   MFT and walkdir scanning (discovery.rs, mft_discovery.rs)
      chat/        assistant loop, tool catalog, approval predicate
      chat.rs, chat_tools.rs, chat_eval.rs
      ...          search, config, calculator, clipboard, media, updater
    assets/    index.html, style.css, app.js, settings.html, settings.js
  ui/        NexUIProcess, the user-facing entry point
  indexer/   NexIndexerProcess, discovery and index writer
  helper/    NexHelper, the elevated input broker
scripts/     packaging, installer, release notes, git hooks
docs/        this tree
```

## Your first change

1. Find or create the Linear issue. `AGENTS.md` explains the search-first rule.
2. Branch from `master`. Name it after the issue: `ciq-237-docs-reorg`.
3. Make the change.
4. Run the gates that match what you touched. At minimum
   `cargo test -p nex`; add `pnpm exec vitest --run` if you touched a web asset.
5. Open a pull request with the issue id in the title.

## Common friction

| Symptom | Cause |
|---|---|
| My CSS change does nothing | Assets are embedded. Rebuild. |
| `E0786` invalid metadata | Corrupt artifacts from an interrupted build. Delete the `release` directory. |
| `LNK1102` out of memory | Build with `CARGO_BUILD_JOBS=1`. |
| UI change is invisible in the running app | `NexUIProcess.exe` locks itself while running. Terminate it before rebuilding. |
| Helper tests will not start | The helper requires elevation. Test it by hand instead. |
| Test fails only in the full suite | Tantivy tests contend for a shared writer. Run the module alone to confirm it is contention, not a real failure. |

## Next

- [architecture.md](architecture.md): the three processes and why
- [testing.md](testing.md): the gate order and how to verify UI work
- `AGENTS.md`: the process rules that bind every change
