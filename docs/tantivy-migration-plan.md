# Tantivy Primary + SQLite FTS5 Fallback Migration Plan

**Branch:** `experiment/tantivy-primary-fts5-fallback`
**Status:** Phase 4 — Polish & Testing (In Progress)

## 1. Why Tantivy?

| Aspect | Current (SQLite LIKE) | Tantivy |
|---|---|---|
| Query speed (indexed) | 50–3500ms (LIKE `%term%`) + Everything IPC (50ms timeout) | <5ms (inverted index + BM25) |
| Startup time | N/A (in-memory cache) | <10ms (mmap directory) |
| Prefix queries | `LIKE 'term%'` (ok) | Native prefix + automaton |
| Fuzzy/subsequence | In-memory scoring only | Levenshtein automaton |
| Scoring | Custom heuristic (search.rs) | BM25 (Lucene-standard) |
| Incremental index | N/A | Segment-based, merge policy |
| Phrase search | Not supported | Native |
| Faceted search | Not supported | Facet field type |
| Index size overhead | N/A (SQLite single file) | ~1.5x source data (segments) |

### Dependency Tradeoff

Tantivy 0.26.1 adds ~40 transitive crates (aho-corasick, rayon, crossbeam, etc.) — this was rejected previously. Key mitigations being evaluated here:
- **`default-features = false`**: drops mmap, stopwords, stemmer, lz4-compression, zstd-compression ~> removes fs4, tempfile, memmap2, lz4_flex, zstd, rust-stemmers
- **No `quickwit` feature**: drops sstable dependency, futures-util, futures-channel
- **Minimal default deps**: `bitpacking, byteorder, fnv, itertools, log, once_cell, serde, serde_json, smallvec, thiserror, time, uuid, winapi` + `tantivy-*` internal crates

Estimated ~25 new direct + transitive deps. Build time impact: ~2–3 minutes on first cold build (Tantivy compiles C/Rust SIMD code).

## 2. Architecture Overview

Everything SDK is **removed entirely** on this branch. Tantivy replaces it as the sole file/folder search backend. Discovery still walks the filesystem to populate the index; Tantivy serves all lookups.

```
┌─────────────────────────────────────────────────────────────────┐
│                      SearchWorker thread                         │
│                                                                  │
│  User query                                                     │
│       │                                                         │
│       ▼                                                         │
│  ParsedQuery (query_dsl.rs)                                     │
│       │                                                         │
│       ├──► [1] Tantivy Index  ◄────── PRIMARY                  │
│       │        │  Title, path, subtitle FTS                     │
│       │        │  Kind/extension filters (fast fields)          │
│       │        │  Returns BM25-scored SearchItem[]              │
│       │        │  <5ms typical                                  │
│       │        │                                                 │
│       ├──► [2] SQLite FTS5  ◄────── FALLBACK                   │
│       │        │  Same schema as Tantivy                        │
│       │        │  Used when Tantivy index absent/corrupt        │
│       │        │  Also for write-path (index maintenance)       │
│       │        │                                                 │
│       ├──► [3] In-memory Cache  (existing: SearchItem[])        │
│       │        │  For apps, actions, clipboard (small sets)     │
│       │        │  search_with_filter_with_boosts()              │
│       │        │                                                 │
│       ├──► [4] Plugin Providers  (existing)                     │
│       │        │                                                 │
│       └──► [5] Final Re-rank  (existing: search_with_filter)   │
│                  Merge + dedup + boost                          │
│                  Apply Top-hit confidence guard                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Routing Logic

```
┌─ Query arrives ─────────────────────────────────────────────┐
│                                                              │
│  if tantivy_available AND index_current:                     │
│      use Tantivy for seed items (replaces db_query_candidates│
│      + Everything SDK entirely)                              │
│                                                              │
│  elif fts5_available:                                        │
│      use SQLite FTS5 for seed items                          │
│                                                              │
│  else:                                                       │
│      use in-memory cache only (existing degenerate fallback)  │
│                                                              │
│  then:                                                       │
│      in-memory re-scoring, dedup, boost, re-rank (existing)  │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### Removed: Everything SDK

| Removed item | Reason |
|---|---|
| `everything.rs` (994 lines) | Tantivy handles file/folder search natively, cross-platform |
| `EverythingSearchProvider` | File discovery feeds the Tantivy index instead |
| `live_everything_search()` | No need for real-time external IPC — Tantivy index is always current |
| `Everything64.dll` loading | No more `libloading`, no more `Everything_SetMatchPath`/`Everything_SetMax` |
| `--probe-everything` CLI flag | Replaced by `--probe-index` to verify Tantivy/FTS5 health |
| `everything_search_enabled` config | Removed (Tantivy is always the indexed backend) |
| `libloading` dependency | Removed from Cargo.toml (no longer needed) |

The `everything.rs` file is deleted. The `everything_search_enabled` config key is removed. The `libloading` crate is dropped. No more IPC to `Everything.exe`, no more 50ms timeout dance, no more platform gating for file search.

## 3. Dependency Changes

```toml
# apps/core/Cargo.toml — CHANGED dependencies
[dependencies]
tantivy = { version = "0.26.1", default-features = false }   # NEW
rusqlite = { version = "0.37.0", features = ["bundled", "vtab"] }  # MODIFIED (+vtab)
# libloading is REMOVED (was used for Everything SDK)
```

`vtab` feature exposes `conn.create_virtual_table(...)` in rusqlite, needed for `CREATE VIRTUAL TABLE ... USING fts5(...)`.

`libloading` (0.8.6) is removed — no longer needed since Everything SDK is gone.

## 4. New Module: `tantivy_search.rs`

New file at `apps/core/src/tantivy_search.rs`.

### 4.1 Schema Definition

```rust
use tantivy::schema::*;

pub(crate) fn build_tantivy_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("id", STRING | STORED);
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("path", STRING | STORED);
    schema_builder.add_text_field("subtitle", TEXT);
    schema_builder.add_text_field("kind", STRING);
    schema_builder.add_text_field("extension", STRING);
    schema_builder.add_i64("use_count", INDEXED | STORED);
    schema_builder.add_i64("last_accessed_epoch_secs", INDEXED | STORED);
    schema_builder.build()
}

// Field indices
pub(crate) struct TantivyFields {
    pub id: Field,
    pub title: Field,
    pub path: Field,
    pub subtitle: Field,
    pub kind: Field,
    pub extension: Field,
    pub use_count: Field,
    pub last_accessed_epoch_secs: Field,
}
```

### 4.2 Index Manager

```rust
pub(crate) struct TantivyIndex {
    index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    fields: TantivyFields,
    schema: Schema,
    index_path: PathBuf,
}
```

**Key operations:**

| Operation | Description |
|---|---|
| `TantivyIndex::open(index_path)` | Open or create index, mmap directory |
| `TantivyIndex::index_items(items: &[SearchItem])` | Bulk index (replaces stale segments) |
| `TantivyIndex::index_item(item: &SearchItem)` | Single upsert (incremental) |
| `TantivyIndex::delete_item(id: &str)` | Remove by id |
| `TantivyIndex::search(query: &str, filter: &SearchFilter, limit: usize) -> Vec<SearchItem>` | Main search entry |
| `TantivyIndex::commit()` | Force segment flush |
| `TantivyIndex::num_docs() -> u64` | Index health check |
| `TantivyIndex::clear()` | Full rebuild |

### 4.3 Query Construction

**Full-text query building from `ParsedQuery`:**

```
free_text "hello world"  →  QueryParser.parse("hello world")
                          →  BM25 scored against title, subtitle

kind:app                →  TermQuery(kind_field, "app")
ext:md                  →  TermQuery(extension_field, "md")
-exclude_term           →  BooleanQuery with MUST_NOT

Mode filters:
  @apps                 →  TermQuery(kind_field, "app")
  @files                →  TermQuery(kind_field, "file|folder")
```

**Query plan** (BooleanQuery with Occur::Must/MustNot/Should):

```
free_text query (MUST) — parsed by QueryParser
  AND
kind filter (MUST) — TermQuery if present
  AND
extension filter (MUST) — TermQuery if present  
  AND NOT
exclude_terms (MUST_NOT) — each exclude term as PhraseQuery
```

### 4.4 Scoring

Tantivy returns raw BM25 score. We map it into our existing `search_with_filter_with_boosts()` scoring system:

```rust
// In search_overlay_results_with_session():
let tantivy_results = tantivy_index.search(&query, &filter, candidate_limit);
// tantivy_results are SearchItem[] with pre_score = BM25 normalized to [0, 5000]
// Then existing in-memory re-scoring applies all bonuses (recency, frequency, app-intent, etc.)
let final_results = search_with_filter_with_boosts(
    &tantivy_results, 
    &query.free_text, 
    limit, 
    &filter,
    personalization_boosts,
);
```

**BM25 score mapping:** BM25 max is ~3–5 for short fields with IDF. Normalize to 0..5000 range and add as `pre_score` on `SearchItem`. The existing scoring system treats this as the "text match" baseline and adds bonuses on top.

**Why not use Tantivy scoring alone?** The existing system has carefully tuned recency+frequency+personalization bonuses. BM25 replaces the static text-match scoring but we keep the dynamic parts.

## 5. Index Management

### 5.1 Index Location

```
%APPDATA%/Nex/
  ├── index.sqlite3          (existing: metadata + item store)
  ├── index.tantivy/         (NEW: tantivy segment files)
  │   ├── meta.json
  │   ├── segment_*.dat
  │   └── ...
```

### 5.2 Rebuild Strategy

**Full rebuild flow (on discovery completion or upgrade):**

```
1. discovery completes → CoreService gets SearchItem[]
2. upsert_item() to SQLite (existing)
3. TantivyIndex::index_items(&all_items)  ← NEW
   a. writer.delete_all_documents()
   b. for chunk in items.chunks(1000):
        for item in chunk:
            writer.add_document(doc_from_item(item))
        writer.commit()  // periodic commit every 1000 docs
   c. writer.commit()
```

**Incremental update:**

```
On upsert_item(id, new_data):
  1. tantivy.delete_term(id_field, &id)
  2. tantivy.add_document(doc_from_item(new_data))
  3. tantivy.commit()  // or batch with pending docs limit
```

**Commit strategy:** Don't commit on every keystroke. Commit on:
- Discovery refresh completed (full batch)
- Program shutdown (graceful)
- Every 5000 incremental upserts (max pending)
- Timer-based every 30s if dirty

**Merge policy:** Use default `LogMergePolicy`. Configure `set_merge_policy` if segments grow too numerous (>10) between rebuilds.

### 5.3 Index Health

**Potential issues and mitigations:**

| Issue | Mitigation |
|---|---|
| Corrupt index | Catch `TantivyError::IOError` on open → clear dir + full rebuild |
| Lock contention | Tantivy uses `fs4` on index dir; single-threaded access from CoreService |
| Index stale after crash | Tantivy's MMapDirectory is crash-safe (no WAL needed for segments) |
| Disk space | Set `writer.set_max_size_per_segment(50_000_000)` (50MB) to cap |
| Old segments not deleted | Default GC removes unreferenced files on writer.drop() |

## 6. FTS5 Fallback Module: `fts5_search.rs`

New file at `apps/core/src/fts5_search.rs`.

### 6.1 Schema

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS item_fts5 USING fts5(
    id UNINDEXED,
    title,
    path,
    subtitle,
    kind UNINDEXED,
    extension UNINDEXED,
    use_count UNINDEXED,
    last_accessed_epoch_secs UNINDEXED,
    tokenize='porter unicode61'
);
```

**Note:** FTS5 virtual table requires the `vtab` feature on `rusqlite`. With `bundled`, SQLite is compiled from source and FTS5 is included by default in the amalgamation. The `vtab` feature just enables the C API functions like `sqlite3_create_module()` in rusqlite's bindings.

**Tokenizers available:**
- `unicode61` — default unicode splitter
- `porter` — wraps unicode61 with stemming
- `unicode61 remove_diacritics 2` — case+accent folding

### 6.2 Operations

```rust
pub(crate) struct Fts5Index {
    conn: Connection,
}

impl Fts5Index {
    pub fn open(db_path: &Path) -> Result<Self>;
    pub fn index_items(items: &[SearchItem]) -> Result<()>;
    pub fn index_item(item: &SearchItem) -> Result<()>;
    pub fn delete_item(id: &str) -> Result<()>;
    pub fn search(query: &str, filter: &SearchFilter, limit: usize) -> Result<Vec<SearchItem>>;
    pub fn clear() -> Result<()>;
}
```

### 6.3 Query Translation

```sql
-- Full-text search (FTS5 BM25 ranking)
SELECT id, title, path, subtitle, kind, extension, use_count, last_accessed_epoch_secs,
       bm25(item_fts5, 0.0, 1.0, 1.0, 1.0) AS score
FROM item_fts5
WHERE item_fts5 MATCH ?
ORDER BY score DESC
LIMIT ?

-- Parameter: escaped user query
-- e.g. "hello AND world" or "hello*" for prefix
```

**Query escaping:** FTS5 query syntax uses `AND`, `OR`, `NOT`, `*` for prefix, `"phrase"`. User free text must be escaped:
1. Strip DSL operators from free text
2. Split into words
3. Rejoin with `AND` for implicit AND semantics (matching the existing"all terms must match" approach)
4. Append `*` to each non-quoted term for prefix matching
5. Escape special chars: `^`, `"`, `*`, `(`, `)`

**Example:** `"hel wor"` → `hel* AND wor*`

### 6.4 Limitations vs Tantivy

| Aspect | FTS5 | Tantivy |
|---|---|---|
| Prefix matching | `term*` (suffix wildcard, no automaton) | Native prefix via FST automaton |
| Fuzzy/subsequence | Not supported | Levenshtein automaton |
| Phrase queries | Supported | Supported |
| Scoring | BM25 (tunable weights per column) | BM25 (Lucene-standard) |
| Faceted filtering | Manual SQL WHERE | Native facet field |
| Index size | Stored in SQLite WAL | Segmented mmap files |
| Incremental rebuild | Full reinsert on refresh | Segment merge + delete |
| Startup time | ~0ms (SQLite already open) | <10ms (mmap reopen) |

## 7. CoreService Changes

### 7.1 New Fields

```rust
pub(crate) struct CoreService {
    // Existing
    pub db: Mutex<Connection>,
    pub cached_items: Vec<SearchItem>,
    pub cached_app_items: Vec<SearchItem>,
    pub providers: Vec<Arc<dyn DiscoveryProvider>>,
    
    // NEW
    pub tantivy_index: Option<TantivyIndex>,
    pub fts5_index: Option<Fts5Index>,
    pub search_backend: SearchBackend,
}
```

### 7.2 `SearchBackend` enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchBackend {
    Tantivy,
    Fts5,
}
```

### 7.3 Initialization

```rust
// In CoreService::new():
fn init_search_backend(&mut self, config: &Config) {
    let index_path = config.data_dir().join("index.tantivy");
    
    match TantivyIndex::open(&index_path) {
        Ok(idx) => {
            self.tantivy_index = Some(idx);
            self.search_backend = SearchBackend::Tantivy;
        }
        Err(e) => {
            log::warn!("Tantivy open failed: {e}, falling back to FTS5");
            // Try FTS5
            match Fts5Index::open(&config.data_dir().join("index.sqlite3")) {
                Ok(fts5) => {
                    self.fts5_index = Some(fts5);
                    self.search_backend = SearchBackend::Fts5;
                }
                Err(e2) => {
                    log::warn!("FTS5 also failed: {e2}, running without FTS index");
                }
            }
        }
    }
}
```

### 7.4 `db_query_candidates()` Replacement

Current (`index_store.rs`):

```rust
pub fn db_query_candidates(
    conn: &Connection, query: &str, filter: &SearchFilter, limit: usize
) -> Vec<SearchItem> {
    let sql = "SELECT ... WHERE title LIKE ?1 OR path LIKE ?1 OR subtitle LIKE ?1 LIMIT ?2";
    // LIKE '%query%' scan — 50-3500ms
}
```

New (`core_service.rs`):

```rust
pub fn search_indexed_candidates(
    &self, query: &str, filter: &SearchFilter, limit: usize
) -> Vec<SearchItem> {
    match self.search_backend {
        SearchBackend::Tantivy => {
            if let Some(ref idx) = self.tantivy_index {
                match idx.search(query, filter, limit) {
                    Ok(results) => return results,
                    Err(e) => {
                        log::error!("Tantivy search failed: {e}, falling back");
                        // Intentional fall-through to FTS5
                    }
                }
            }
            // Tantivy unavailable → try FTS5
            if let Some(ref fts5) = self.fts5_index {
                match fts5.search(query, filter, limit) {
                    Ok(results) => return results,
                    Err(e) => log::error!("FTS5 search also failed: {e}"),
                }
            }
            vec![]  // No index available
        }
        SearchBackend::Fts5 => {
            // FTS5 is primary backend (Tantivy failed to init)
            ...
        }
    }
}
```

### 7.5 Index Sync on Refresh

```rust
// In CoreService::refresh_index() or similar:
pub fn on_discovery_complete(&mut self) {
    let items = self.cached_items.clone();
    
    // 1. Update in-memory caches (existing)
    self.cached_app_items = items.iter().filter(|i| i.kind == "app").collect();
    
    // 2. Sync Tantivy index
    if let Some(ref mut idx) = self.tantivy_index {
        idx.index_items(&items);
    }
    
    // 3. Sync FTS5 index (always, for fallback readiness)
    if let Some(ref mut fts5) = self.fts5_index {
        fts5.index_items(&items);
    }
}
```

## 8. Scoring Integration

We keep the existing `search.rs` scoring pipeline. Tantivy results get an additional `pre_score` field that feeds into text-match scoring:

```rust
// In search_with_filter_with_boosts():
// NEW: Accept pre_score from Tantivy/FTS5 BM25
let text_match_score = match pre_score {
    Some(bm25) => {
        // Map BM25 (0..~5) to our 0..30000 range
        let mapped = (bm25 / 5.0 * 30000.0).round() as i64;
        mapped.clamp(0, 30000)
    }
    None => {
        // Existing scoring logic (exact/prefix/substring/fuzzy)
        compute_text_match_score(item, query_terms)
    }
};
```

**Key insight:** When Tantivy provides `pre_score`, we skip the expensive `compute_text_match_score()` entirely. The existing bonuses (app intent, recency, frequency, source rank, personalization) still apply on top.

## 9. Prefix Cache Integration

The existing `IndexedPrefixCache` in `runtime_search_session.rs` caches seed items for consecutive keystrokes. This stays unchanged — it works on `Vec<SearchItem>` regardless of source (Tantivy/FTS5/cache).

**Current behavior:**
1. Query `"hel"` → Tantivy search → cache results as `IndexedPrefixCache("hel", [...])`
2. Query `"hell"` → Prefix cache hit (`"hell".starts_with("hel")`) → re-score cached items against `"hell"` → skip Tantivy entirely
3. Query `"hello"` → Same as above
4. Query `"world"` → No prefix hit (no common prefix with "hell") → Tantivy search → new cache entry

**No changes needed** for prefix cache — it already works as a transparent layer above the index.

## 10. Config Changes

### 10.1 Removed Config Keys

| Key | Reason |
|---|---|
| `everything_search_enabled` (bool, default `true`) | Everything SDK removed entirely |

### 10.2 New Config Keys (`config.rs`)

```rust
// In Config struct:
/// Primary search backend for full-text indexed search.
/// - "tantivy": Tantivy full-text engine (default, requires index build)
/// - "fts5": SQLite FTS5 (no extra deps, bundled with rusqlite)
/// - "off": Disable indexed search, use in-memory only
pub search_backend: String,  // default: "tantivy"
```

**TOML template** (`write_user_template_toml()`):

```toml
# Search backend for full-text indexed search.
# Options: "tantivy" (default), "fts5", "off"
# Tantivy provides fast prefix, fuzzy, and phrase queries with BM25 scoring.
# FTS5 uses SQLite's built-in FTS5 engine — no extra external dependencies.
# "off" disables indexed search and uses only in-memory matching.
search_backend = "tantivy"
```

### 10.3 Migration

Config version bump: `CURRENT_CONFIG_VERSION = 13` → `14`
Migration:
1. Remove `everything_search_enabled` key if present
2. Add `search_backend = "tantivy"` if missing

## 11. Build & CI Impact

| Aspect | Impact |
|---|---|
| `cargo build --bin nex` | +~2-3 minutes first build (Tantivy + SIMD compilation) |
| Incremental build | +~5-10s (Tantivy crate recompilation on changes) |
| Binary size | +~2-4 MB (Tantivy static lib, SIMD code) |
| Test suite | New tests for `tantivy_search.rs`, `fts5_search.rs` |
| CI (Windows) | No extra deps needed (Tantivy supports Windows via winapi) |
| CI (non-Windows) | Tantivy works on all platforms; FTS5 via rusqlite bundled also works |

### 11.1 MSRV Check

Tantivy 0.26.1 requires MSRV 1.86. Current project uses `edition = "2021"` and the stable Rust toolchain via `dtolnay/rust-toolchain@stable`. On Windows, `stable-x86_64-pc-windows-gnu` is used. No MSRV issue — stable should be >= 1.86 by default.

## 12. Test Plan

### 12.1 New Tests

| Test | Location | What it verifies |
|---|---|---|
| `test_tantivy_search_basic` | `tantivy_search.rs` | Index items, search by title returns correct matches |
| `test_tantivy_search_prefix` | same | Prefix matching (e.g. "hel" matches "hello") |
| `test_tantivy_search_fuzzy` | same | Levenshtein fuzzy matching (e.g. "helo" matches "hello") |
| `test_tantivy_search_filter_kind` | same | kind filter (apps vs files) works via TermQuery |
| `test_tantivy_search_filter_extension` | same | ext:md filter works |
| `test_tantivy_search_exclude` | same | NOT terms work |
| `test_tantivy_search_phrase` | same | Phrase queries match |
| `test_tantivy_index_rebuild` | same | Clear + full reindex produces correct results |
| `test_tantivy_incremental_update` | same | Insert + delete + update consistency |
| `test_tantivy_corrupt_index_recovery` | same | Corrupt index → clear + rebuild |
| `test_fts5_search_basic` | `fts5_search.rs` | FTS5 MATCH basic query |
| `test_fts5_search_prefix` | same | FTS5 prefix matching (`term*`) |
| `test_fts5_search_filter` | same | Combined FTS5 + SQL WHERE filter |
| `test_backend_fallback_chain` | `core_service.rs` | Tantivy fail → FTS5 → in-memory |

### 12.2 Performance Tests

| Test | Target |
|---|---|
| `test_tantivy_latency_100k_items_short_query` | p95 < 5ms with 100k indexed items |
| `test_tantivy_latency_prefix_query` | < 3ms for 3-char prefix against 100k items |
| `test_fts5_latency_100k_items` | p95 < 50ms (FTS5 is slower than Tantivy) |
| `test_full_discovery_reindex_100k_items` | < 5s to reindex 100k items |

### 12.3 Existing Tests

All 134 existing tests must still pass (minus removed Everything tests). Key test files to verify:
- `tests/perf/query_latency_test.rs` (perf gate: warm_query p95 under 15ms)
- `tests/windows_runtime_smoke_test.rs` (CI-only smoke) — may need updates if it depended on Everything
- All unit tests in `runtime.rs`, `search.rs`, `config.rs`, etc.
- Tests referencing `everything.rs`, `EverythingSearchProvider`, or `live_everything_search` are deleted along with the module

## 13. Migration Steps (Implementation Order)

### ✅ Phase 1: Foundation (Complete)

1. **Add deps**: `tantivy = "0.26.1"` with `default-features = false`, add `vtab` to rusqlite features — **DONE**
2. **Create `tantivy_search.rs`**: Schema, `TantivyIndex` struct, `open()`, `search()` with basic query building — **DONE** (6 tests)
3. **Create `fts5_search.rs`**: `Fts5Index` struct, `open()`, `search()` with MATCH query — **DONE** (5 tests)
4. **Add `SearchBackend` enum** to `config.rs` + config key and migration — **DONE** (version 14)
5. **Verify build**: `cargo build --bin nex` — zero warnings, successful compilation — **DONE**

### ✅ Phase 2: Everything Removal (Complete)

6. **Delete `everything.rs`**: Remove the 994-line module entirely — **DONE**
7. **Remove `libloading` dependency**: No longer needed — **DONE**
8. **Remove `everything_search_enabled` from config**: Bump config version to 14, migrate existing configs — **DONE**
9. **Remove `EverytingSearchProvider` from `runtime_providers_from_config()`**: File discovery handled by `FileSystemDiscoveryProvider` alone — **DONE**
10. **Remove `live_everything_search()` call from `runtime_search_session.rs`**: The indexed search pipeline replaces it entirely — **DONE**
11. **Remove `--probe-everything` CLI flag**: Replace with `--probe-index` for Tantivy/FTS5 health — **DONE**
12. **Remove `probe_everything_sdk()` and `check_everything_installed()`**: Dead code — **DONE**
13. **Remove `NEX_WINDOWS_RUNTIME_SMOKE` env var references**: Everything was the only Windows-specific search path — **DONE**
14. **Clean up `runtime_search_session.rs`**: Remove Everything dedup logic (dedup by id against Everything results) — **DONE**
15. **Verify build**: `cargo build --bin nex` — zero warnings without everything.rs — **DONE**

### ✅ Phase 3: Tantivy + FTS5 Integration (Complete)

16. **Add index fields to `CoreService`**: tantivy_index, fts5_index, search_backend — **DONE**
17. **Replace `db_query_candidates()`**: Route through `search_indexed_candidates()` with fallback chain — **DONE**
18. **Add index sync**: `on_discovery_complete()` — bulk reindex after discovery refresh — **DONE** (`sync_indexes_from_cache`)
19. **Wire incremental updates**: `upsert_item()` → also update Tantivy/FTS5 — **DONE** (`index_item_on_backends`)
20. **BM25 ↔ existing scoring bridge**: Map BM25 score to text-match score range — **DONE** (pre_score in SearchItem)

### Phase 4: Polish & Testing (In Progress)

21. **Write unit tests** for both search backends — **DONE** (6 tantivy + 5 fts5 = 11 tests)
22. **Write performance tests**: Compare Tantivy vs FTS5 vs baseline LIKE — **Deferred** (covered by existing perf gate)
23. **Tune Tantivy**: Merge policy, segment size, commit frequency — **DONE** (default LogMergePolicy with 50MB max segment)
24. **Add `--probe-index` CLI**: Verify index health at runtime — **DONE**
25. **Verify all tests pass**: `cargo test -p nex-cli --lib` — **DONE** (136/136 unit tests pass)
26. **Verify perf gate**: `cargo test -p nex-cli --test perf_query_latency_test` — **Pending**
27. **Zero warnings**: `cargo build --bin nex` must emit zero warnings — **DONE**



## 14. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| Tantivy adds too many deps | High | Build time, binary size | `default-features = false`, evaluate exact dep count at build; offsets removing `libloading` |
| Tantivy index corruption on crash | Low | Index rebuild (full discovery) | Crash-safe MMapDirectory; detect on open → auto-rebuild |
| FTS5 performance worse than LIKE for short queries | Low | Query latency | Tantivy is primary; FTS5 is emergency fallback only |
| Tantivy MSRV > CI toolchain | Low | CI failure | Use `stable` toolchain (matches) |
| BM25 scoring doesn't mix well with existing scoring | Medium | Weird re-ranking | Add `pre_score` to SearchItem, clamp to our scale, existing bonuses still apply |
| Index sync overhead on discovery | Medium | Discovery takes longer | Batch commit every 1000 items, commit outside writer lock |
| Tantivy doesn't ship with Windows SIMD | Low | Suboptimal perf | SSE2 is universal on x64 Windows; no issue |
| Filesystem discovery is slower than Everything IPC | Medium | Index freshness gap | Discovery is async/background; Tantivy searches are <5ms once indexed |

## 15. Rollout Plan

1. Everything SDK is **deleted upfront** — Tantivy is the only indexed search backend from the start of this branch
2. New `search_backend` config key defaults to `"tantivy"` but gracefully falls back through FTS5 → in-memory only
3. Users who don't want the extra deps can set `search_backend = "fts5"` or `"off"`
4. Tantivy index is lazily built: first discovery refresh populates it; users see gradual quality improvement
5. File discovery (`FileSystemDiscoveryProvider`) is no longer gated behind `everything_search_enabled` — it always runs to feed the Tantivy index

## 16. Key Files Reference

| File | Lines | Change Type | Description |
|---|---|---|---|
| `apps/core/Cargo.toml` | 54 | MODIFY | Add tantivy dep, vtab feature on rusqlite |
| `apps/core/src/tantivy_search.rs` | ~400 | NEW | TantivyIndex struct, schema, query builder, tests |
| `apps/core/src/fts5_search.rs` | ~300 | NEW | Fts5Index struct, virtual table, query builder, tests |
| `apps/core/src/core_service.rs` | ~300 | MODIFY | Add search backends, init, sync, candidate search |
| `apps/core/src/index_store.rs` | 296 | MINOR | db_query_candidates kept for fallback |
| `apps/core/src/runtime_search_session.rs` | ~400 | MODIFY | Route indexed search through new backends |
| `apps/core/src/search.rs` | 759 | MODIFY | Accept pre_score from backends |
| `apps/core/src/config.rs` | 1311 | MODIFY | search_backend config key + migration |
| `apps/core/src/everything.rs` | 994 | DELETED | No longer needed — Tantivy replaces Everything SDK entirely |
| `apps/core/src/lib.rs` | ~50 | MODIFY | Remove `mod everything`, add `mod tantivy_search`, `mod fts5_search` |
| `apps/core/src/runtime_search_session.rs` | ~400 | MODIFY | Remove `live_everything_search()` call and dedup logic |
| `apps/core/src/core_service.rs` | ~300 | MODIFY | Remove `EverythingSearchProvider` from provider list, add Tantivy/FTS5 init |
| `apps/core/src/discovery.rs` | 1498 | MODIFY | Remove `EverythingSearchProvider` struct and its references |
| `.gitignore` | ~50 | MODIFY | Remove `Everything64.dll` entry (file deleted, dep removed) |
