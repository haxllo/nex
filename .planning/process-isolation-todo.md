# Process Isolation TODO

- [x] Create NexUIProcess and NexIndexerProcess binaries
- [x] Add IPC module (ping, status, snapshot, search)
- [x] NexUIProcess spawns indexer
- [x] UI skips providers/watchers in indexer mode
- [x] UI search uses the indexer snapshot (no per-query IPC search)
- [x] Indexer health-check/auto-restart
- [x] Remove local CoreService discovery from UI entirely
- [x] Make indexer the sole Tantivy owner (SQLite stays shared: the UI writes launch history)
- [x] Add UI cache reload from indexer updates
- [x] Update release/packaging/install/uninstall scripts for new binaries
- [x] Add end-to-end recovery tests (`apps/indexer/tests/process_isolation_test.rs`)
- [x] Verify parity (local vs indexer snapshot search)
- [x] Remove `Nex.exe`; `NexUIProcess.exe` is the only UI entry point
- [x] Delete the in-process indexing fallback so the UI can never index

## Verified

- `cargo build --release --bin NexUIProcess --bin NexIndexerProcess` exits 0.
- `NEX_RUN_PROCESS_ISOLATION_TEST=1 cargo test -p nex-indexer --test process_isolation_test`
  passed repeatedly (~26 s): snapshot publishes the seeded file, IPC search finds
  it, protocol-version mismatch is rejected, UI snapshot search finds it, a
  restarted indexer serves again, and a `shutdown` request stops the indexer.
- The indexer binds and serves before its first scan finishes, so readiness is
  "listening" instead of "index built" (this was the restart-recovery failure).
- IPC `search` in the test polls up to 120 s: the store snapshot lands before
  Tantivy syncs, so a single-shot search raced the index and flaked once.
- `nex --quit` no longer leaves NexIndexerProcess running: new IPC `shutdown`
  method, UI calls it on graceful exit, `stop_runtime_instance` stops the
  indexer too (graceful request, taskkill fallback). The parent-PID watchdog
  remains only as the crash backstop.
- Binaries are one-per-package (`apps/ui`, `apps/indexer`) because a
  build script's Windows resource is linked into every binary of its package
  and inherited by dependents. Sharing the package made Task Manager show
  "Nex" for both processes.
- Indexer-path modules no longer log through `runtime`; they call
  `crate::logging` directly, so nothing on the indexing path reaches overlay
  or UI state.

## Known limits

- Every store change ships a full snapshot to the UI (2 s poll, debounced by
  generation). Move to deltas if watcher churn ever costs noticeable CPU.
- `NexUIProcess.exe --restart` leaves the old indexer to exit through its
  parent-PID watchdog; the new UI respawns it within ~5 s.
- Not measured here: keystroke latency on a full (uncapped) 100k-row store,
  since the UI searches its snapshot without Tantivy. `perf_query_latency_test`
  is the gate once it runs on Windows.
- Source-level split still pending: `nex_core` contains both UI and indexing
  modules. The linker already drops UI code from the indexer binary
  (8.8 MB vs 15.4 MB), but a physical crate split needs the item-generator
  modules (`settings_catalog`, `control_panel`, `uninstall_registry`,
  `recent_files`, `bookmarks`, `action_executor`) promoted to `pub` and the
  UI modules' `crate::` paths rewritten to `nex_core::`.
- Helper supervision gap: if `NexHelper.exe` dies, the UI clears
  `HELPER_ACTIVE` and falls back to the in-process hook, but it does not
  reconnect. Elevated-window detection stays degraded until the UI restarts.
