# Release validation checklist

Run this on a real Windows host before publishing a release. It covers the
behaviour the automated gates cannot reach: the elevated hotkey path, the glass
material, and the update flow.

Automated gates are listed in [testing.md](testing.md). Passing them is a
precondition, not a substitute.

## Preconditions

- A release build of all three binaries exists.
- The `NexHelperV2` scheduled task either exists or is expected to be created on
  first run.
- No Nex process is running: `NexUIProcess.exe --status` should report not running.

## Automated gates

```bash
pnpm exec vitest --run
cargo build --locked --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess
cargo test --locked -p nex
cargo test --locked -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
cargo test --locked -p nex --test windows_runtime_smoke_test
```

Cross-process behaviour:

```bash
NEX_RUN_PROCESS_ISOLATION_TEST=1 cargo test -p nex-indexer --test process_isolation_test
```

## First-run flow

1. Remove `%APPDATA%\Nex` and delete the `NexHelperV2` task if it exists.
2. Start `NexUIProcess.exe`. A single elevation prompt should appear.
3. Accept it. The tray icon should appear.
4. Press the hotkey. The panel should open under the cursor.

## Hotkey matrix

Test each of these with the panel closed and with the panel open.

| Scenario | Expected |
|---|---|
| No window focused | Panel toggles |
| Task Manager in front, elevated | Panel toggles, this is the helper path |
| Windows Settings in front | Panel toggles |
| A fullscreen game with Game Mode on | Hotkey suppressed, tray tooltip reads `Nex (Game Mode)` |
| Panel visible but not focused | Panel takes focus rather than toggling twice |
| Ten presses in quick succession | Every press produces exactly one toggle |

The last row is the regression that matters most. Two detection paths observing one
press produces a double toggle, and it is invisible in a single slow test.

## Panel behaviour

| Check | Expected |
|---|---|
| Empty box on open | Quick Launch list, or a bare field in Compact view |
| Typing | Results update within a keystroke, no stutter |
| Arrow keys | Selection moves, list scrolls to keep it visible |
| Enter | Selected item launches, panel hides |
| Escape | Panel hides |
| Right-click a row | Context menu with the actions valid for that row |
| Resize and DPI change | Panel stays inside its frame at 100, 125, and 150 percent |

## Glass material

The panel cannot be captured, so this section cannot be automated end to end. See
[overlay-and-glass.md](overlay-and-glass.md) for why.

| Check | Expected |
|---|---|
| Transparency on, panel over text | Backdrop bends toward the edge, centre stays readable |
| Panel over a photo | No visible banding in large flat areas |
| Panel over a dark window | Pane reads deeper than over a light one |
| Corners | Fully round, no straight step where the curve meets the edge |
| Transparency off | Opaque surface, no blank panel |
| Moving the panel | Glass follows without a stale frozen backdrop |

Automated evidence, when a GPU is available:

```bash
cargo test -p nex --lib refraction -- --ignored
```

For anything the probes do not cover, look at the screen and describe it. A phone
photograph is real evidence; a screenshot is not.

## Indexing and search

| Check | Expected |
|---|---|
| First run on a large drive | Progress window appears, indexing completes |
| Search during indexing | Results appear, typing does not stall |
| Install a new program, restart Nex | The program appears in results |
| Delete the index, restart | Index rebuilds, nothing else is lost |
| Change a discovery root | Index refreshes without a restart |

## Assistant

| Check | Expected |
|---|---|
| Ask a question with no tool use | Answer appears, no card |
| Ask it to create a file | Approval card naming the file, action runs only on approval |
| Ask it to run a command | Card shows the exact command line |
| Approve the rest of the turn | Later actions in the same turn do not prompt |
| Ask for something under `System32` | Refused without a card |
| Leave a card open for 90 seconds | Card lapses, action does not run |

## Quit and update

| Check | Expected |
|---|---|
| `NexUIProcess.exe --quit` | Nex stops, no indexer or helper process remains |
| Tray Quit | Same as above |
| `NexUIProcess.exe --restart` | New instance starts, hotkey works |
| Install an update over the running copy | Nex updates, What's New pill appears once |
| Click the What's New pill | Page opens, and does not reappear after a restart |

## Release blockers

Any of these fails the release:

- A press that produces two toggles.
- The hotkey not working over an elevated window while the tray icon reports
  healthy.
- A blank or black panel.
- Quit leaving an indexer or helper process behind.
- The index failing to rebuild after deletion.
- An approval card appearing after the action it guards has already run.
