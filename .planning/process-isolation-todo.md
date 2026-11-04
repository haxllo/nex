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
- [x] Add end-to-end recovery tests (`apps/core/tests/process_isolation_test.rs`)
- [x] Verify parity (local vs indexer snapshot search)

## Verified

- `cargo build --release --bin Nex --bin NexUIProcess --bin NexIndexerProcess` exits 0.
- `NEX_RUN_PROCESS_ISOLATION_TEST=1 cargo test -p nex --test process_isolation_test`
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

## Known limits

- Every store change ships a full snapshot to the UI (2 s poll, debounced by
  generation). Move to deltas if watcher churn ever costs noticeable CPU.
- `Nex.exe --restart` leaves the old indexer to exit through its parent-PID
  watchdog; the new UI respawns it within ~5 s.
- Not measured here: keystroke latency on a full (uncapped) 100k-row store,
  since the UI searches its snapshot without Tantivy. `perf_query_latency_test`
  is the gate once it runs on Windows.
