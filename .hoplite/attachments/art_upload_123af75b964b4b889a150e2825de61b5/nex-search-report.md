# Nex — Why Search Misses Files, and How to Fix It

**A design review of the discovery → index → query pipeline in [`haxllo/nex`](https://github.com/haxllo/nex) (Rust, Windows-only).**

Nex indexes configured roots into SQLite (source of truth), mirrors that into Tantivy, and ranks queries against a compacted in-memory cache. Users report that most files are missing from results, deep files never appear, and new files show up late or not at all.

**Headline finding: this is not primarily an index-capacity problem.** Discovery is mostly indexing what it promises. The files go missing *after* indexing, in three independent places:

1. **The candidate cap is applied before ranking, not after.** The top-K seed budget (`index_max_items_per_query_seed`, default 5 000) is passed straight into Tantivy's `TopDocs::with_limit(...)`, so the ranker never sees documents outside the top 5 000 BM25 hits — by BM25, not by launcher relevance.
2. **Broad-root mode silently shrinks the searchable surface to 5 000 file/folder rows and keeps the *oldest* ones.** Cache compaction keeps SQLite `ORDER BY id`, folders first, and breaks at the cap — so every newly indexed file (highest id) is the first thing evicted. That is the reported "compaction drops 45k+ entries".
3. **Lifecycle and watcher bugs make the index stale or unrecoverable:** `wait_merging_threads()` is never called, GC results are discarded, a corrupt index is `remove_dir_all`'d and reopened *without* reseeding from SQLite, and a watcher thread that hits `GetOverlappedResult` failure dies silently with no supervisor.

**Recommended path (ranked in §3):** make SQLite the *recall* engine and Tantivy the *ranking* engine, delete the truncated corpus cache, then split the indexer into its own process. The P0 subset is ~1–2 days of work and removes the majority of misses without any architectural change.

> **Scope note:** this document is a research and design deliverable, not an execution artifact. The sandbox for this project is gated behind a pending security migration, so nothing here was compiled or benchmarked against Nex; every code claim is a static read of `master` @ `dee0d68` (2026-11-03), and every latency/memory figure is an engineering estimate flagged as such.

<!-- cell -->

## 1. Evidence base

| | |
|---|---|
| Code | `haxllo/nex` @ `master` (`dee0d68`, pushed 2026-11-03), crate `apps/core` (+ `apps/helper`) [^13] |
| Files audited | `discovery.rs`, `everything_bridge.rs`, `config.rs`, `settings_catalog.rs`, `tantivy_search.rs`, `index_store.rs`, `runtime_index.rs`, `core_service.rs`, `search.rs`, `search_worker.rs`, `runtime_search_session.rs`, `file_watcher.rs`, `file_watcher_consumer.rs`, `application_watcher.rs` |
| Method | Static read of raw sources, four parallel passes. Line anchors are from `master` @ `dee0d68` and are accurate to ±1; anchors marked *(fn)* are cited by function name where the file exceeded extraction limits |
| Not done | Nothing was compiled, profiled, or run. No latency number below is measured |
| Vendor docs | Tantivy (reader/writer/GC) [^9] [^18] [^7] [^34], Win32 `ReadDirectoryChangesW` [^4], NTFS change journals [^23], SQLite FTS5 [^40], Everything SDK [^28] |

---

## 2. Failure model — the eight ways a file becomes invisible

Ranked by how much of the observed "most files are missing" they explain.

| # | Symptom | Verified mechanism | Anchor | Confidence |
|---|---|---|---|---|
| 1 | Files are indexed but never ranked; results feel arbitrary | Seed budget is a **Tantivy `TopDocs` limit applied before any launcher scoring**. Only the top 5 000 BM25 hits can ever reach `search.rs`; the rest of the candidate pool is invisible to the ranker | `tantivy_search.rs:142`, `runtime_search_session.rs:100,148` | **Verified** |
| 2 | "Cache compaction drops 45k+ entries" | `retain_items` walks `ORDER BY id` output, keeps **folders first**, then files, and `break`s at `effective_file_seed_cap`. Newly indexed files have the **highest** ids, so they are evicted first | `core_service.rs` `compact_cached_items` / `retain_items` *(fn)* | **Verified** |
| 3 | One drive root collapses everything | `broad_root_discovery_enabled` fires on `C:\` / `\`, and `effective_file_folder_cache_cap` then pins the in-memory file/folder snapshot to 5 000 rows. Discovery still walks 40 k rows that search can never see | `core_service.rs` *(fn)*, regression test asserts `effective_file_seed_cap == 5_000`, `retained_file_folders == 700` | **Verified** |
| 4 | Deep files missing beyond 5 levels | `max_depth` is **hardcoded to 5** at provider construction; there is no `Config` field for it, so it cannot be raised without a code change | `core_service.rs` `runtime_providers_from_config` → `FileSystemDiscoveryProvider::from_config(config, 5)`; `discovery.rs:573-576` | **Verified** |
| 5 | Corrupt index → *empty* index, not rebuilt | Recovery path is `remove_dir_all` + reopen; the SQLite re-seed exists only on the *open-succeeded-but-empty* branch. A `.term`-file error therefore yields a valid, empty index | `core_service.rs:1075-1150` | **Verified** |
| 6 | Missing segment files (`.term`, `.idx`) after restart/rebuild | No `wait_merging_threads()` before shutdown; GC result discarded via `let _ = …wait()`; tantivy documents that GC can still be running **after the writer is dropped** [^10]. Multiple `Index` construction sites exist with no writer-lock discipline [^34] | `tantivy_search.rs:221-261`, `core_service.rs:1079,1086,1146,185-189` | **High** |
| 7 | New files appear late or never | Watcher: 200 ms debounce **re-armed on every notification** (starves under churn); unbounded channel with a 4 096 pending-event cap that **drops**; `GetOverlappedResult` failure exits the thread silently; batches arriving during a resync are discarded | `file_watcher.rs:31,125,266-275`, `file_watcher_consumer.rs:171` | **Verified** |
| 8 | Deleted/moved files still return; stale rows persist | 30-min stamp skip short-circuits the provider, and no prune-on-scan step was found in the audited path; `incremental_sync_items` re-adds **all** docs each cycle instead | `discovery.rs:496-521,772`, `core_service.rs` `should_skip_provider_discovery` *(fn)* | **Medium** (prune may exist outside audited files) |

Two secondary mechanics worth knowing, because they bound what a "fix the caps" patch can achieve:

- **Mid-filename tokens cannot match on the Tantivy side.** `path` is indexed as a single `STRING` term and queries are prefix-only (`FuzzyTermQuery::new_prefix`), so `report` never matches `Q3-report.pdf` in the index. Mid-string matching exists only in `search.rs:372` — over candidates that already survived the caps.
- **`Folders` and `Actions` modes never consult Tantivy at all** (`should_use_db_query_seed` is gated to `All | Files`), so folder search is 100% dependent on the truncated cache — which is exactly the tier that drops rows by id order.

<!-- cell -->

## 3. Architecture options, ranked

All latency/RAM figures are **engineering estimates for a 120 k-item corpus**, to be validated by the test plan in §10 — not measurements.

| Option | Shape | Recall guarantee | p95 query (est.) | RAM (est.) | Blast radius of a bug | Effort | Verdict |
|---|---|---|---|---|---|---|---|
| **B. Hybrid: SQLite recall + Tantivy ranking** | One process, two roles: SQLite answers "which items *can* match", Tantivy + `search.rs` decide *order* | ✅ Any indexed row is reachable | 2–8 ms prefix, 10–30 ms substring | ~30–60 MB | Ranking degrades, results still appear | 3–5 d | **Recommended** |
| **A. Harden the current Tantivy-gated design** | Keep Tantivy as gatekeeper; raise/remove caps, fix cache order, fix GC/reload | ⚠️ Only if every cap is removed *and* stays removed | 1–5 ms | 40–70 MB | Any cap regression re-hides files | 1–2 d | **Do first as P0, not as the destination** |
| **C. Separate indexer process** | `nex-indexd.exe` owns writer + watchers + SQLite writes; UI is a read-only client | ✅ inherits B | +IPC 0.3–1 ms | −20–30 MB in UI | Contained; UI survives indexer crash | 2–3 w | **Next, after B** |
| **D. USN/MFT engine (Everything-class)** | Read `$MFT` / `FSCTL_READ_USN_JOURNAL` directly per NTFS volume | ✅✅ plus sub-second cold start | <1 ms (in-RAM) | 60–150 MB | Highest: corruption, elevation, ACLs | 4–8 w | Later, only if >500 k items or 1 M+ target |
| **E. Delegate to Everything SDK** | Forward queries to a running Everything instance | ✅ (if installed) | 1–3 ms + IPC | ~0 | Hard dependency on third-party app | 1–2 d | Optional accelerator, never the only path |

### Why B wins on this evidence

- The audit shows the corpus is **already complete enough** (40 k/root, 120 k total) — the losses are at the *surface*: `TopDocs::with_limit`, id-ordered cache truncation, broad-root clamping. B removes the surface instead of enlarging the budget.
- **Completeness becomes a property of the schema, not of a tunable.** A `WHERE name_lower LIKE 'q%'` over an indexed column cannot "forget" a row because a different query scored higher.
- **Tantivy stops being a single point of failure.** Today a reload error degrades to *empty results*; under B it degrades to *worse ordering*.
- The in-memory corpus cache — the component with the 45 k-drop bug — **disappears entirely**, replaced by a small result-LRU plus a hot set (apps/actions/recent). That also removes the entire class of "compaction dropped my files" bugs.
- Cost is bounded: one migration adding generated/normalized columns plus indexes, one new query function, one scoring adapter. No new process, no elevation, no third-party dependency.

### Why not D yet

The USN journal is the right *resync* primitive ([^23], and it is what Everything is reported to build on [^45]) but it is the wrong *first* move: it requires a volume handle (`\\.\C:`), typically elevation, and it replaces the one part of Nex that currently works. Adopt it as the **delta/resync mechanism** inside C (§7), not as the initial indexer.

### Why not A as the destination

A is a set of bug fixes, and every one of them is re-breakable by a config value: set `index_max_items_per_query_seed` back to 5 000, or add a drive root, and the symptom returns. It is still the correct **first week** of work because each fix is independently shippable, testable, and reversible.

<!-- cell -->

## 4. Tantivy lifecycle: what "missing `.term` file" actually means

Tantivy keeps two manifests: `meta.json` lists the **segments** that constitute the index, and `managed.json` lists the **files** it owns. A segment's files are protected from deletion only while a `SegmentMeta` for it is alive in the process; GC computes "alive segments → their files" and deletes everything else in `managed.json` [^7]. So a missing `.term` file is never spontaneous — it means *something else deleted files this reader still expects*.

The three realistic triggers in a launcher like Nex, in order of likelihood:

1. **GC outliving the writer.** Tantivy's GC runs on the segment-updater thread. Dropping `IndexWriter` does not guarantee the GC task has finished; the upstream issue describing exactly this was resolved by calling `wait_merging_threads()` [^10]. Nex never calls it, and discards the GC result (`let _ = …wait()`), so a restart or a rebuild can race a live GC.
2. **A second `Index`/`IndexWriter` for the same directory.** Tantivy binds a `DirectoryLock` to the writer's lifetime with `INDEX_WRITER_LOCK` and `META_LOCK` [^34]. Nex has three construction sites plus a boot path and no lock arbitration; its own comments already flag multi-writer corruption risk (`core_service.rs:185-189`).
3. **`remove_dir_all` on the live index.** The failure path deletes the directory and reopens. Any surviving reader holding mmaps, or any in-flight merge, now writes into a directory that no longer matches its manifests.

### Doctrine to adopt (each rule is independently shippable)

| Rule | Implementation | Why |
|---|---|---|
| **One owner per index directory** | Acquire the writer at startup only. If `LockBusy`, do **not** construct a second writer — surface "another instance is indexing" and run read-only | Prevents interleaved GC/merge [^34] |
| **Never drop the writer bare** | `writer.commit()?; writer.wait_merging_threads()?;` on shutdown, on rebuild, and on config change | Documented GC-after-drop hazard [^10] [^9] |
| **Manual reload, explicit** | `ReloadPolicy::Manual`, then `reader.reload()?` immediately after every successful `commit()`, and propagate the error | Automatic `OnCommitWithDelay` is not synchronized with `commit()` and can lag tens of ms [^18] |
| **Commit cadence tied to visibility** | Commit on a timer (≤300 ms) *or* every N ops, whichever first — not on an opaque threshold whose units are unverified in the current code | A new file must be searchable in <2 s |
| **Check the GC result** | `garbage_collect_files()?.wait()?` on the owner thread only, after commit | Silent GC failure is how you discover corruption a week later |
| **Never rebuild in place** | Build `index.next/` beside the live dir, `wait_merging_threads`, `fs::rename` swap, then delete the old dir | Rename is atomic; `remove_dir_all` is not |
| **Quarantine, don't delete** | On open/reload failure: rename the dir to `index.corrupt.<ts>/`, then rebuild from SQLite | Today a corrupt index silently becomes an **empty** index (`core_service.rs:1075-1150`) |
| **Reconcile on boot** | Compare `SELECT COUNT(*) FROM item WHERE deleted=0` against `searcher.num_docs()`; mismatch beyond a threshold ⇒ rebuild | The only way to detect drift without a user bug report |
| **Delete on the term, not the doc** | `writer.delete_term(Term::from_field_text(id_field, &id))` per removed row | `delete_all_documents` + re-add (the current rebuild) is O(corpus) per change |

Two Windows-specific constraints worth encoding while you are in here: keep the index under `%LOCALAPPDATA%` (never a roaming/OneDrive-synced path, where a sync agent can rewrite files under the writer), and wrap index I/O in a bounded retry — AV real-time scanning produces transient `os error 32/5` on `rename` and `delete`.

<!-- cell -->

## 5. Search path: SQLite for recall, Tantivy for ranking

### The three candidate designs, judged

| Design | Verdict | Reason |
|---|---|---|
| **Tantivy-only** (today) | ❌ Reject | A scoring engine used as a *filter*. Any cap, analyzer gap, reload failure, or BM25 quirk silently removes rows. `path` is a single `STRING` term, so mid-filename tokens cannot match at all |
| **Tantivy + in-memory cache fallback** (today) | ❌ Reject | The fallback is lossy *and* ordered by SQLite `id`, so it answers with the oldest rows. It also only runs when the index returns **zero** results — a short, wrong result set never triggers it |
| **Dual index** (Tantivy + a second external inverted index) | ❌ Reject | Two stores to keep transactional. Every consistency bug you already have is doubled |
| **SQLite recall + Tantivy ranking** (recommended) | ✅ Adopt | One transactional store for truth; FTS5 lives *inside* that same SQLite file, so "dual" costs no second consistency domain [^40] |

The important distinction: this is **not** "add a fallback". A fallback that only fires on empty results cannot fix phantom incompleteness. Recall has to be the **primary** path, with relevance layered on top.

### Schema additions (additive migration, no table rewrite)

```sql
ALTER TABLE item ADD COLUMN name_lower TEXT;      -- leaf name, lowercased, stored
ALTER TABLE item ADD COLUMN ext_lower  TEXT;      -- extension without dot
ALTER TABLE item ADD COLUMN root_id    INTEGER;   -- which configured root owns it
ALTER TABLE item ADD COLUMN deleted    INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_item_name_prefix ON item(name_lower, deleted);
CREATE INDEX idx_item_root_kind   ON item(root_id, kind, deleted);

-- substring tier, same database, no second consistency domain
CREATE VIRTUAL TABLE item_fts USING fts5(
  name_lower, path_lower,
  content='item', content_rowid='id',
  tokenize="trigram"
);
```

`GLOB 'q*'` on a lowercased column is a case-exact, index-usable range scan (unlike `LIKE`, which needs NOCASE collation matching to be optimized). The trigram tokenizer is what finally makes mid-filename matching a schema property rather than a lucky cache hit [^40].

### Query plan (replaces `search_indexed_candidates` + seed cap)

```
t0 = now()
q  = normalize(input)                      # lowercased, diacritics folded
T1  exact:      name_lower = q                              LIMIT 200
T2  prefix:     name_lower GLOB q||'*'   [AND root/kind/ext] LIMIT 2000
T3  substring:  len(q) >= 3 ? item_fts MATCH q              LIMIT 2000
                            : (skip — see note)
T4  fuzzy:      Tantivy FuzzyTermQuery (typo tolerance)     LIMIT 500   # optional booster
pool = dedupe(T1..T4) by id
rank(pool) using existing search.rs lexical scorer + Tantivy pre_score if present
return top(result_limit), with diagnostics
```

Note the one honest limitation: trigram substrings shorter than 3 characters cannot match through FTS5 [^40]. For 1–2 char queries the prefix tier is the correct answer anyway, and it must be **index-backed**, not cache-backed — which is precisely what the current hot-prefix cache gets wrong.

### What gets deleted

- `index_max_items_per_query_seed` as a *pre-ranking* limit — gone. Replace with per-tier `recall_limit` values and a wall-clock budget (target 40 ms, hard stop).
- `compact_cached_items` / `retain_items` / `effective_file_seed_cap` — gone. `cached_items: RwLock<Vec<SearchItem>>` becomes a bounded result-LRU keyed by `(query, filter, limit)` plus a small hot set of apps/actions/recent items. Memory stops scaling with corpus size, and "compaction dropped 45 k entries" becomes structurally impossible.
- `broad_root_discovery_enabled` — gone as a *search* concept. A broad root is now only a scanning-policy question (§6), never a reason to shrink what search can see.

### Latency budget (estimates, to be confirmed in §10)

| Tier | 120 k rows | 1 M rows | Notes |
|---|---|---|---|
| Exact + prefix (index scan) | 0.3–1.5 ms | 1–4 ms | Scales with `log n`, not `n` |
| Trigram substring | 5–20 ms | 30–120 ms | Degrades gracefully; cap with `LIMIT` + time budget |
| Ranking 2–4 k candidates in Rust | 1–3 ms | 1–3 ms | Already the existing `search.rs` cost |
| **Total p95** | **< 25 ms** | **< 150 ms** | Add an early-exit budget above 250 ms |

### Diagnostics are part of the design, not an afterthought

`SearchResponse { results }` currently carries **no** truncation signal at all — the only evidence is a log line. Add:

```rust
struct SearchDiagnostics {
    recall_source: RecallSource,     // Sql | Tantivy | Both
    tiers: Vec<TierStat>,            // {name, hits, took_ms}
    truncated: bool,                 // any limit or budget was hit
    budget_exhausted: bool,
    index_state: IndexState,         // Healthy | Degraded(reason) | Rebuilding
}
```

This single struct is what turns "my file is missing" from a week of guessing into a one-line answer.

<!-- cell -->

## 6. Caps and depth: replace silent truncation with observable budgets

The current policy has one fatal property: **when a cap is hit, nothing in the product records that the answer set is incomplete.** `discovery_cap … reached=true` goes to a log the user never reads, and the search layer happily serves the truncated corpus as if it were whole.

### Principle

> A cap may limit *how long we work now*. It may never limit *what search can eventually see*, and it must always be visible in diagnostics.

### Depth

| Root class | Depth policy | Rationale |
|---|---|---|
| `C:\Users\<user>` (profile) | **Unlimited** (`0`) | This is where user documents live; the exclusions already block `.git`, `node_modules`, `target`, `.venv`, `AppData` at any depth (`discovery.rs:21-50`) |
| Additional user-added roots | Configurable, default `0` (unlimited) | User explicitly opted in |
| Volume roots (`C:\`, `G:\`) | `12` + known-giant pruning, or USN (§7) | Unbounded walk of a system volume is not a depth problem, it is a time problem |

Depth 5 is not a tuning miss — it is below the median depth of a normal project tree (`~/projects/app/src/components/ui/…`), which is why *deep files are exactly the ones that are missing*, as reported.

```rust
// config.rs — new fields (defaults shown)
pub discovery_max_depth_profile: u32,   // 0
pub discovery_max_depth_volume:  u32,   // 12
pub discovery_scan_budget_items: u64,   // 250_000 per root
pub discovery_scan_budget_secs:  u64,   // 90 per root
pub discovery_defer_partial:     bool,  // true — queue a continuation pass
```

### Item budgets instead of hard stops

Replace the `break` at `discovery.rs:585` with a three-state outcome per root:

| State | Meaning | Pruning allowed? | Search impact |
|---|---|---|---|
| `Complete` | Walk finished inside budget | ✅ prune unseen rows for this root | Full |
| `Partial(time)` | Budget exhausted, continuation queued with a path cursor | ❌ never prune | Full — stale rows are safer than missing rows |
| `Failed(io)` | Permission/IO error | ❌ never prune | Full + diagnostic banner |

The continuation pass resumes from the last visited path (`start_after` cursor) rather than restarting, so a 400 k-item root converges over a few passes instead of re-truncating at the same 40 k boundary every time.

### Memory-aware sizing (replaces `effective_file_seed_cap`)

The current formula (`budget_bytes / 400`, clamped to the seed cap) sizes a **truncated RAM mirror of the corpus** — the wrong object to size. Under the hybrid design, RAM holds only results and a hot set, so the corpus no longer needs a RAM-derived cap:

| Corpus | SQLite on disk (est.) | Tantivy on disk (est.) | RAM, hybrid design (est.) | RAM, today (est.) |
|---|---|---|---|---|
| 120 k | ~40–55 MB | ~12–20 MB | ~20–40 MB (page cache + hot set) | 40–70 MB (full mirror) |
| 1 M | ~350–450 MB | ~100–150 MB | ~60–100 MB | infeasible — truncation today |

Per-item constants worth putting in one place and measuring once: leaf name ≈ 2 × chars, full path ≈ 2 × chars, SQLite row overhead ≈ 120 B, FTS5 trigram ≈ 3–4 × name bytes (the real cost — budget for it deliberately [^40]).

### Per-root bookkeeping table

```sql
CREATE TABLE root_state (
  root_id        INTEGER PRIMARY KEY,
  path           TEXT NOT NULL,
  depth          INTEGER NOT NULL,
  scan_state     TEXT NOT NULL,      -- complete | partial | failed
  scan_generation INTEGER NOT NULL,
  items          INTEGER NOT NULL,
  truncated      INTEGER NOT NULL,
  last_full_scan INTEGER,            -- epoch secs
  last_usn       INTEGER,            -- NTFS USN cursor for delta scans
  last_error     TEXT
);
```

`item.scan_generation` makes pruning safe: `DELETE FROM item WHERE root_id=? AND scan_generation < ?` **only** when `scan_state='complete'`. This is what turns the 30-minute stamp skip from a correctness hazard into a cache.

<!-- cell -->

## 7. Real-time updates: `ReadDirectoryChangesW` can silently return *nothing*

The single most important platform fact for this design, straight from Microsoft:

> "If the buffer overflows, `ReadDirectoryChangesW` will still return **true**, but the entire contents of the buffer are discarded and the `lpBytesReturned` parameter will be zero, which indicates that your buffer was too small to hold all of the changes that occurred." … "In this case, you should compute the changes by enumerating the directory or subtree." [^4]

Nex already keys on the zero-byte signal and routes to `OverflowResync` — that part is right, and it is better than most implementations. The losses come from everything *around* it.

### Failure modes → detection → action

| Failure mode | Detected by | Required action |
|---|---|---|
| Buffer overflow (burst writes, e.g. `npm install`, unzip) | 0 bytes returned; also `ERROR_NOTIFY_ENUM_DIR` on the overlapped completion [^4] | Mark root dirty → scoped rescan of the *reported* path prefix, not a global rescan |
| Event backpressure | bounded channel full | **Never drop silently** — increment `dropped_events`, then escalate to the same dirty-root resync |
| Watcher thread death | `GetOverlappedResult` false, or `queued == 0` | Supervisor restart with exponential backoff (100 ms → 5 s), max 10 attempts, then mark `Failed` and fall back to polling |
| Change during in-flight resync | batches arriving while resync holds the state lock | Buffer them into a dirty-set; apply after the resync completes. Today they are **discarded** (`file_watcher_consumer.rs:171`) |
| Debounce starvation | 200 ms re-armed on every event | Leading-edge emit + fixed trailing timer with a hard 1 s max latency |
| Network / removable volume | `ERROR_INVALID_FUNCTION` [^4] | Drop to polling (60 s) and record the reason; never retry the handle in a loop |
| Directory recursively deleted | `REMOVED` on a subtree root | Delete by path prefix in one statement, not per descendant event |

### Startup ordering (the "new file never appears" bug class)

The current ordering creates a gap: a file created *during* the initial scan can be missed by both the scan (already passed that directory) and the watcher (not yet registered). Fix with:

```
1. record  t0 = now()            (and per-volume USN cursor u0)
2. register watchers on all roots      ← BEFORE scanning
3. run initial scan
4. drain queued events with mtime >= t0   (or USN delta from u0)
5. commit index, reload reader, mark roots Complete
```

Step 4 is cheap because step 1 makes it *precise*: you only need to reconcile items whose `mtime >= t0`, not the whole tree.

### The durable resync primitive: the NTFS USN change journal

`ReadDirectoryChangesW` requires the process to be running at all times and carries no memory across restarts; the NTFS change journal exists precisely to avoid full rescans, and is designed to "recover file system indexing" after a crash or restart [^23]. Everything is reported to build its index on this mechanism [^45].

Adopt it as the **delta channel**, incrementally, not as a rewrite:

| Step | Cost | Payoff |
|---|---|---|
| Persist `root_state.last_usn` after each successful scan | trivial | Enables delta recovery |
| On startup, open `\\.\<vol>` and `FSCTL_READ_USN_JOURNAL` from `last_usn` | low | Replaces the 30-min stamp guess with an exact delta |
| If `last_usn < LowestValidUsn` (journal wrapped/cleared) | low | Fall back to a full scoped rescan — the *only* case that needs one |
| Keep `ReadDirectoryChangesW` for live latency | none | USN is the safety net, not the hot path |

Caveat to plan for: opening a volume handle typically requires elevation. If Nex is not elevated, USN must be an **optional enhancement** (`if available`) with the scoped-rescan path as the guaranteed fallback — not a hard dependency.

<!-- cell -->

## 8. Recommended plan: P0 fixes, then the hybrid

### P0 — "stop hiding files" (1–2 days, ~6 files, no architecture change)

Each item is independently shippable and independently revertible. Together they should remove most of the observed misses.

| # | Change | Where | Effort | Risk | Effect |
|---|---|---|---|---|---|
| P0-1 | Replace hardcoded `5` with `discovery_max_depth` (default `0` for profile roots, `12` for volume roots) | `core_service.rs::runtime_providers_from_config`, `config.rs:463-508`, `settings_catalog.rs` | S | Low | Deep files become discoverable |
| P0-2 | Stop passing the seed budget into `TopDocs::with_limit`; fetch `max(seed_cap, 25_000)` (or unbounded under a time budget), apply `candidate_limit` **after** scoring | `tantivy_search.rs:142`, `runtime_search_session.rs:100` | S | Low | Ranker sees the real candidate pool |
| P0-3 | Retention must not be id-ordered: sort by `(mtime DESC, id DESC)` and/or drop the corpus cache entirely | `core_service.rs::retain_items` *(fn)* | S | Low | Newest files stop being evicted first |
| P0-4 | `broad_root` no longer clamps the file/folder cache to 5 000 | `core_service.rs::effective_file_folder_cache_cap` *(fn)* | S | Low | Drive-root configs stop collapsing to ~700 folders |
| P0-5 | Rebuild path must **re-seed from SQLite** after `remove_dir_all`, and quarantine instead of delete | `core_service.rs:1075-1150` | M | Med | Corrupt index recovers instead of going empty |
| P0-6 | `wait_merging_threads()` before drop; check GC results; acquire writer lock and fail fast on `LockBusy` | `tantivy_search.rs:61,221-261`, `core_service.rs:1079,1086,1146` | M | Med | Eliminates the missing-`.term` class |
| P0-7 | `ReloadPolicy::Manual` + explicit `reload()?` after every commit | `tantivy_search.rs:132` | S | Low | Deterministic visibility of new rows |
| P0-8 | Watcher supervisor + bounded channel with drop counter → resync; debounce fixed (leading + trailing, 1 s max); don't discard events during resync | `file_watcher.rs:31,125,266-275`, `file_watcher_consumer.rs:171` | M | Med | New files appear; churn doesn't blind the watcher |
| P0-9 | Add `SearchDiagnostics` + a `nex doctor` command (counts per root: DB vs `num_docs` vs cache, stale rows, watcher state, overflow/drop counters, last reload error) | `contract.rs`, `search_worker.rs`, `runtime_diagnostics.rs` | M | Low | Every future "file missing" is one command away |
| P0-10 | Prune only on `scan_state='complete'`, guarded by `scan_generation` | `discovery.rs`, `index_store.rs` | M | Med | Stale rows disappear without risking deletion of unscanned rows |

**Expected effect of P0 alone (estimate):** depth + cache-order + seed-cap fixes address the three mechanisms that explain the majority of misses; GC/rebuild/reload fixes remove the intermittent "everything disappeared" failures. Query latency unchanged (±1 ms). Memory unchanged.

### P1 — hybrid recall (3–5 days, ~8 files)

1. Migration: `name_lower`, `ext_lower`, `root_id`, `deleted`, `scan_generation`, two indexes, `item_fts` trigram table.
2. New `recall_candidates()` in `tantivy_search.rs` (or a new `recall.rs`) implementing T1–T4 from §5.
3. Delete `compact_cached_items` / `retain_items`; shrink `cached_items` to a result-LRU + hot set.
4. Keep `search.rs` ranking as-is — it is the least-broken part of the pipeline (verified lexical scoring, no path-depth penalty).
5. Wire `SearchDiagnostics` through `SearchResponse`.

**Expected effect (estimate):** every indexed row is reachable; p95 query 2–8 ms for prefix at 120 k rows; RAM *drops* because the corpus mirror is gone.

### P2 — process isolation (2–3 weeks)

Move writer + watchers + discovery into `nex-indexd.exe`; UI becomes a read-only client with an IPC query API and a change-subscription stream. This is what makes P0-6 and P0-8 permanently true rather than true-until-the-next-refactor, and it contains crashes. It is **not** required to fix the reported bugs.

### P3 — USN/MFT (4–8 weeks, only if needed)

Adopt `FSCTL_READ_USN_JOURNAL` as the delta channel (§7), then optionally `$MFT` enumeration for sub-second cold starts at 1 M+ items. Only worth it once P1 + P2 are in place.

### Minimal vs refactor — the honest tradeoff

| | Minimal (P0) | Hybrid (P0+P1) | Full (P0–P2) |
|---|---|---|---|
| Time | 1–2 d | ~1 w | ~4 w |
| Files touched | 6 | 9 | 14 + new crate |
| Fixes "most files missing" | ✅ mostly | ✅ completely | ✅ completely |
| Fixes "new files not appearing" | ✅ mostly | ✅ | ✅ |
| Survives a future cap/config change | ❌ re-breakable | ✅ structural | ✅ structural |
| Memory at 120 k | unchanged | −10 to −30 MB | −20 to −30 MB |
| Risk of regression | Low | Medium | Medium-High |

**Recommendation:** ship P0 this week behind existing settings (no migration), then P1 as a single PR with the FTS5 migration. Skip P2 unless crash isolation or a 1 M-item target becomes a requirement — from the code audit, the correctness problems are all in-process bugs, not process-boundary problems.

<!-- cell -->

## 9. Files and functions to change

Anchors are `master` @ `dee0d68`; verify ±1 before editing. `(fn)` = cited by function name where the file body was too large to pin a line.

| File | Symbol / anchor | Change | Phase |
|---|---|---|---|
| `core_service.rs` | `runtime_providers_from_config` → `from_config(config, 5)` | take depth from config; per-root class | P0-1 |
| `core_service.rs` | `search_indexed_candidates` (`indexed_seed_limit`) | fetch `max(seed, 25k)`; apply display limit post-rank | P0-2 |
| `core_service.rs` | `effective_file_folder_cache_cap` *(fn)* | delete the broad-root clamp | P0-4 |
| `core_service.rs` | `compact_cached_items` / `retain_items` *(fn)* | `mtime DESC, id DESC`; later delete entirely | P0-3 / P1 |
| `core_service.rs` | `refresh_cache_from_store` *(fn)* | stop materialising the corpus in RAM | P1 |
| `core_service.rs` | `open_tantivy_index` `:1075-1127`, `reset_tantivy_index` `:1131-1150` | quarantine + re-seed from SQLite; add single-owner lock | P0-5 |
| `core_service.rs` | `search_with_filter_internal` *(fn)* | fallback fires only on *empty*; make recall primary | P1 |
| `core_service.rs` | `should_skip_provider_discovery` *(fn)* | keep as cache only when `scan_state='complete'` | P0-10 |
| `tantivy_search.rs` | `:61` `Index::open_or_create` | acquire `INDEX_WRITER_LOCK`; fail fast on busy | P0-6 |
| `tantivy_search.rs` | `:142` `TopDocs::with_limit(limit)` | **the** pre-ranking truncation | P0-2 |
| `tantivy_search.rs` | `:132` `reader.reload()` | `ReloadPolicy::Manual` + explicit reload after commit | P0-7 |
| `tantivy_search.rs` | `:221-261` rebuild (commit → `delete_all_documents` → re-add → commit → GC) | build `index.next/`, `wait_merging_threads`, atomic rename | P0-5/6 |
| `tantivy_search.rs` | `build_prefix_query` (`FuzzyTermQuery::new_prefix`) | demote to an optional fuzzy tier | P1 |
| `discovery.rs` | `:573-576` `WalkDir::new(root).max_depth(max_depth)` | source depth from config | P0-1 |
| `discovery.rs` | `:555-586` cap `break` | three-state budget + continuation cursor | P0-10 |
| `discovery.rs` | `:496-521`, `:772` stamp | stamp only invalidates cache; add `last_usn` | P0-10 |
| `discovery.rs` | `:13-56` exclusion tables | keep; make user-extensible (they already work correctly) | — |
| `index_store.rs` | `:433-441` `item` table (PK only) | migration: `name_lower`, `ext_lower`, `root_id`, `deleted`, `scan_generation`, 2 indexes, `item_fts` | P1 |
| `runtime_search_session.rs` | `:100`, `:148` seed cap / `.take(candidate_limit)` | rework with the new recall path | P0-2 / P1 |
| `runtime_search_session.rs` | `:286` `ranked.truncate(result_limit)` | emit `truncated` into diagnostics | P0-9 |
| `runtime_search_session.rs` | `:89-130` result + prefix caches | LRU keyed on (query, filter, limit); drop seeded-prefix cache | P1 |
| `search.rs` | `:9-12` score tiers, `:372` substring position | **leave as-is** — good lexical scoring, no path-depth bias | — |
| `file_watcher.rs` | `:31` 64 KiB buffer, `:125` channel, `:266-275` overlapped loop | supervisor, bounded channel + drop counter, debounce fix | P0-8 |
| `file_watcher_consumer.rs` | `:171` resync discard | dirty-set, apply after resync | P0-8 |
| `contract.rs`, `search_worker.rs` | `SearchResponse`, `SearchResult` | add `SearchDiagnostics` | P0-9 |
| `everything_bridge.rs` | provider query path | mirror the same `root_state` accounting so both backends report identically | P1 |

Deliberately **not** changed: `search.rs` scoring (it is the healthiest subsystem), the exclusion tables (component-name matching plus explicit-root override is correct), and the 64 KiB watcher buffer (raising it delays overflow rather than fixing the cause — overflow handling is the fix).

<!-- cell -->

## 10. Test plan

### Acceptance criteria (make these the merge gate, not vibes)

| # | Criterion | Target |
|---|---|---|
| A1 | A file created at depth 3 / 8 / 15 under `C:\Users\<u>` is searchable by full name | p99 ≤ 2 s |
| A2 | Every row in `item` for a `complete` root is reachable by its exact name | 100 % (assert `count(matches) ≥ 1` for a 5 000-item sample) |
| A3 | DB count vs `searcher.num_docs()` after a scan | equal (0 drift) |
| A4 | Prefix query latency at 120 k items | p95 ≤ 10 ms |
| A5 | Substring (trigram) at 120 k items | p95 ≤ 30 ms |
| A6 | Restart with a 120 k index | ready-to-search ≤ 1.5 s, zero reload errors |
| A7 | Kill the process mid-commit (3×), reopen | index opens or rebuilds from SQLite; never empty |
| A8 | 100 k events fired in 10 s into one root | zero permanent misses after resync settles |
| A9 | RSS with 120 k items indexed and no query | ≤ 200 MB |

### Test matrix

| Layer | Test | Asserts |
|---|---|---|
| Unit | Cap/budget state machine | `Partial` never prunes; `Complete` prunes only older `scan_generation` |
| Unit | Retention ordering | most-recent items survive; folders and files each get their share |
| Unit | Stamp invalidation | changing depth/roots/exclusions invalidates; changing mtimes does not |
| Unit | Debounce | 1 000 events in 200 ms produce exactly one batch within 1 s |
| Integration (Windows CI) | `create → search` for depth 3/8/15, extensions, unicode names, 260+ char paths | A1 |
| Integration | Rename pair (`OLD`+`NEW`) leaves exactly one row, correct path | no duplicates, no orphans |
| Integration | Delete subtree → all descendant rows gone in one pass | prefix delete |
| Integration | `Folders` / `Actions` search modes | reach the same recall path as `Files` (today they never touch Tantivy) |
| Fault injection | Manually delete a `.term` file while running | reload error surfaces in diagnostics, index quarantined + rebuilt, search recovers |
| Fault injection | Two `nex.exe` instances | second acquires read-only, no second writer, no corruption |
| Fault injection | `LockBusy` on startup | clear UI state, no crash loop |
| Fault injection | Simulated overflow (`OverflowResync` forced) | scoped rescan only; unrelated roots not re-walked |
| Scale | Synthetic tree: 60 k files in one root, then 300 k across 3 roots | A2, A3, no silent truncation; `root_state.scan_state` reported honestly |
| Perf | `criterion` benches: exact / prefix / substring / fuzzy at 120 k, 500 k, 1 M | A4, A5 |
| Perf | Cold-start scan throughput per root | ≥ 5 k items/s on an SSD; time-to-first-searchable ≤ 3 s |
| Soak | 24 h with a churn generator (create/rename/delete 10/s) | memory flat (±10 %), zero dropped-event drift, A8 |

### Two harnesses worth building once

1. **`nex doctor --json`** — the same struct the tests assert on: per-root `{items, scan_state, last_full_scan, last_usn, truncated}`, `{db_count, tantivy_num_docs, cache_count}`, watcher `{state, overflows, dropped_events, restarts}`, `{last_reload_error}`. Every integration test should call this rather than poking internals.
2. **`xtask gen-tree --root X --files N --depth D --unicode`** — deterministic synthetic trees so scale tests are reproducible and CI-cheap.

### The regression test that would have caught this

A single test named `every_indexed_item_is_reachable`: insert 10 000 rows (mixed depths, mixed ages, folders first by id), run a query for each row's exact leaf name, assert `≥1` hit. Under today's code it fails for the rows evicted by `retain_items` and for anything below the `TopDocs` cut — which is the whole bug class in one assertion.

<!-- cell -->

## 11. Risks, open questions, sources

### Risks of the recommended path

| Risk | Mitigation |
|---|---|
| Removing the pre-ranking cap increases candidate-set cost at 1 M+ items | Hard wall-clock budget (40 ms soft / 250 ms hard) with `truncated=true` in diagnostics — bounded latency *and* honest about it |
| FTS5 trigram index inflates the DB (≈ 3–4× name bytes) [^40] | Accept at 120 k (~15–25 MB); at 1 M gate the substring tier behind a setting |
| Migration on an existing user DB can be slow or interleave with the watcher | Version-gated migration in one transaction, watchers paused, `PRAGMA journal_mode=WAL`, resumable on crash |
| USN deltas need a volume handle | Optional enhancement only; scoped rescan is the guaranteed fallback (§7) |
| Watcher supervisor can loop on a permanently bad root | Exponential backoff cap + `Failed` state + poll fallback; surface it in `nex doctor` rather than retrying forever |
| Paths > 260 chars | Use `\\?\` prefixed paths in the index layer; assert in tests (A1 covers a 280-char path) |
| Deleting the corpus cache may slow the "instant" first paint | Keep a small hot set (apps/actions/recent) hydrated from DB; measured target is ≤10 ms to first result |

### Open questions to confirm before writing code

1. **`commit_threshold 500` units** — the writer is configured with a 16 MB arena and this threshold; if it is *documents*, a new file can wait a long time before it is searchable. Verify in `tantivy_search.rs` before relying on P0-7 alone.
2. **Is there a prune-on-scan path** anywhere in `core_service.rs`? The file exceeded extraction limits; stale rows are the one symptom I could not fully attribute.
3. **Does `everything_bridge.rs` apply its own item cap**, and does it report `root_state` differently from the walkdir provider? If so, diagnostics must normalise both.
4. **Is the 4 096 pending-event cap per root or global?** A global cap would explain cross-root starvation.
5. **Does Nex ship elevated?** It determines whether USN can be a default rather than an opt-in.
6. **What is the user's real corpus size** per root? If the 40 k per-root cap is genuinely being hit, budget policy (§6) matters as much as the query-surface fixes.

### Bottom line

- The index is not the problem; **the surface between the index and the ranker is**. Fix the pre-ranking cap, the id-ordered cache, and the hardcoded depth first — that is a 1–2 day change touching six files.
- Then make **SQLite the recall engine** and demote Tantivy to ranking. Completeness becomes a schema property, the 45 k-drop class of bug disappears, and a Tantivy failure degrades ordering instead of emptying results.
- Treat the `.term` errors as a **lifecycle** bug (GC outliving the writer, no `wait_merging_threads`, in-place `remove_dir_all`), not as disk corruption [^10] [^9] [^7].
- Assume `ReadDirectoryChangesW` will overflow and return **zero events successfully**; the only correct response is enumeration/resync [^4].

---

### Sources

[^4] Microsoft Learn — *ReadDirectoryChangesW function (winbase.h)*. https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw
[^7] quickwit-oss/tantivy wiki — *File Lifecycle in Tantivy & Garbage Collection*. https://github.com/quickwit-oss/tantivy/wiki/File-Lifecycle-in-Tantivy-&-Garbage-Collection
[^9] docs.rs — *tantivy::indexer::IndexWriter* (`garbage_collect_files`, `wait_merging_threads`). https://docs.rs/tantivy/latest/tantivy/indexer/struct.IndexWriter.html
[^10] quickwit-oss/tantivy issue #2262 — *After the IndexWriter is destructed, garbage_collect_files may still continue to execute*. https://github.com/quickwit-oss/tantivy/issues/2262
[^13] GitHub API — `haxllo/nex` repository metadata (`master`, Rust, MIT). https://api.github.com/repos/haxllo/nex
[^18] docs.rs — *tantivy::IndexReader* (`reload`, `ReloadPolicy::OnCommitWithDelay` timing). https://docs.rs/tantivy/latest/tantivy/struct.IndexReader.html
[^23] Microsoft Learn — *Change Journals* (NTFS USN journal, indexing recovery). https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals
[^28] voidtools — *Everything SDK* (IPC wrapper; requires the Everything client running). https://www.voidtools.com/support/everything/sdk/
[^34] docs.rs — *tantivy `directory_lock.rs` source* (`INDEX_WRITER_LOCK`, `META_LOCK`). https://docs.rs/tantivy/latest/src/tantivy/directory/directory_lock.rs.html
[^40] SQLite — *FTS5 Extension*, §4.3.4 The Trigram Tokenizer. https://sqlite.org/fts5.html
[^45] voidtools forum — *why everything so quick?* (Everything's use of the USN journal; community source, treat as weak evidence). https://www.voidtools.com/forum/viewtopic.php?t=12371
