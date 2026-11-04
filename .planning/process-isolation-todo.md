# Process Isolation TODO

- [x] Create NexUIProcess and NexIndexerProcess binaries
- [x] Add IPC module (ping, index_status, search)
- [x] NexUIProcess spawns indexer
- [x] UI skips providers/watchers in indexer mode
- [x] UI search can use indexer with fallback
- [x] Indexer health-check/auto-restart
- [x] Remove local CoreService discovery from UI entirely
- [x] Make indexer the sole Tantivy/SQLite owner
- [x] Add UI cache reload from indexer updates
- [x] Update release/packaging scripts for new binaries
- [x] Add end-to-end recovery tests
- [x] Verify parity (local vs indexer search)
