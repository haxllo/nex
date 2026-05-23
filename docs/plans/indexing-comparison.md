# Indexing Approach Comparison & Recommendation

> **Purpose:** Compare file indexing strategies for Nex and recommend the best approach.

---

## 1. Current Approach

Nex uses **walkdir-based crawling** into a **SQLite database** (via `FileSystemDiscoveryProvider`).

```
FileSystemDiscoveryProvider
  ├── Option A: Windows Search API (via PowerShell COM → ADODB)
  └── Option B (fallback): walkdir recursive crawl
```

### How it works:
- On startup, crawl configured roots with `walkdir::WalkDir`
- Store results in SQLite `item` table
- On each query, load items from SQLite → score in-memory
- Change stamps track when reindex is needed

### Current Limitations:
- **Full crawl at startup** — can take 5-30 seconds for large drives
- **No real-time change tracking** — always potentially stale until next crawl
- **Memory pressure** — loads all items into memory for scoring
- **Windows Search COM dependency** is fragile (PowerShell invocation)
- **No USN Journal integration** — the only Windows-native real-time change API

---

## 2. Available Approaches Compared

| Feature | **Walkdir (Current)** | **Everything SDK** | **USN Journal** | **Windows Search API** | **ReadDirectoryChangesW** |
|---------|----------------------|-------------------|-----------------|----------------------|--------------------------|
| **Speed** | 🟡 Medium (crawl) | ✅ Instant | ✅ Near-instant | 🟡 Medium | ✅ Instant (for changes) |
| **Real-time updates** | ❌ No | ✅ Yes | ✅ Yes | 🟡 Partial | ✅ Yes |
| **CPU usage** | 🟡 Spiky during crawl | ✅ Low | ✅ Low | 🟡 Moderate | 🟡 Moderate |
| **Memory** | ✅ SQLite persisted | ✅ External process | ✅ Low | 🟡 High | ✅ Low |
| **Setup complexity** | ✅ Simple | 🟡 Requires Everything installed | 🔴 Complex | 🟡 Moderate | 🟡 Moderate |
| **External dependency** | ❌ None | 🔴 Everything.exe (separate tool) | ❌ None | ❌ None (OS built-in) | ❌ None |
| **Win32 managed?** | ✅ Pure Rust | ⚠️ C SDK, need FFI | ✅ Win32 API | ✅ Win32 API | ✅ Win32 API |
| **Cross-platform?** | ✅ Yes | ❌ Windows only | ❌ Windows only | ❌ Windows only | ❌ Windows only |
| **File metadata** | ✅ Full | ✅ Full (Everything index) | 🟡 Limited (MFT-based) | ✅ Full | ✅ Full (on change) |
| **Index all drives?** | ✅ Yes | ✅ Yes (Everything index) | ✅ Yes (per volume) | ✅ Yes | ✅ Yes (per directory) |

---

## 3. Detailed Analysis

### Option A: Everything SDK (Recommended for v1.1)

**How it works:**
- [Everything](https://www.voidtools.com/) by David Carpenter is a free Windows tool that indexes all files using NTFS MFT (Master File Table) directly
- It provides a C SDK (`Everything64.dll`) with IPC to query the Everything process
- Every launcher on Windows (Flow Launcher, uTools, Keypirinha) integrates with it

**Pros:**
- **Instant search** — Everything already has every file indexed before you query
- **Zero indexing overhead** — no crawl, no DB maintenance
- **Real-time** — Everything watches USN Journal for changes automatically
- **Extremely stable** — battle-tested for 15+ years
- **User expectation** — power users already have Everything installed

**Cons:**
- **Requires Everything to be installed** (~5 MB, free, open source MIT)
- **C SDK FFI** — need Rust bindings (`everything-sys` or manual)
- **No control over index scope** — Everything indexes everything by default
- **Extra process** — Everything runs as a background service

**Verdict:** Should be the **primary** indexing backend. Offer to install Everything if not present.

### Option B: USN Journal (Best for v1.2+)

**How it works:**
- NTFS maintains a USN (Update Sequence Number) journal per volume
- Applications can query the journal for file creates, deletes, renames since a given USN
- Used by Windows Search, Everything, and most antivirus software

**Pros:**
- **No external dependency** — pure Win32 API
- **Real-time** — poll the journal for changes every few seconds
- **Efficient** — only processes changes, not full scans

**Cons:**
- **Complex implementation** — need to manage USN cursors per volume
- **Initial crawl still needed** — journal only has deltas since it was enabled
- **Volume-specific** — must handle per-volume USN tracking
- **Limited metadata** — just filenames, timestamps, and sizes from MFT
- **No content indexing** — just file system metadata

**Verdict:** Ideal as a **replacement for walkdir** for change tracking, but not for initial index building.

### Option C: Windows Search API (Current Approach)

**How it works:**
- Windows maintains its own index via `SearchIndexer.exe`
- Query via OLE DB (`Provider=Search.CollatorDSO`) or Windows Search COM API
- Nex currently invokes it via PowerShell script → ADODB COM

**Pros:**
- **Built into Windows** — no extra install needed
- **Full-text indexing** — can search file contents too
- **Property store** — rich metadata access

**Cons:**
- **Slow** — Windows Search index is notoriously slow and inconsistent
- **Fragile COM invocation** — PowerShell-based ADODB is unreliable
- **Index doesn't cover all locations** — only indexed paths
- **High memory usage** — SearchIndexer can use 200MB+

**Verdict:** Keep as a **supplemental** option only. Do not rely on it as primary.

### Option D: ReadDirectoryChangesW (Not Recommended)

**How it works:**
- Win32 API that watches directories for file system changes
- Sends notifications on create, modify, delete, rename

**Pros:**
- **No external dependency**
- **Real-time notifications** (not polling)

**Cons:**
- **Requires watching each directory** — not recursive by default
- **Buffer overflow under load** — can miss events
- **High overhead for large directory trees**
- **Not suitable as primary indexer** — needs initial crawl anyway

**Verdict:** Useful as a **supplement** to watch specific paths (e.g., Start Menu) but not as primary indexer.

---

## 4. Recommended Architecture

### Tier 1 (Primary): Everything SDK

```
User types query
  → Check if Everything is running
    → Yes: Query Everything SDK → return results
    → No: Fall back to SQLite index
```

### Tier 2 (Fallback): USN Journal + SQLite

```
Startup:
  → Read last USN cursor per volume from SQLite meta
  → Query journal for changes since last cursor
  
Periodic:
  → Poll journal every 30s
  → Update SQLite with creates/deletes/renames
  
Full rebuild:
  → walkdir crawl (only when schema changes or user requests)
```

### Tier 3 (Edge case): Walkdir

```
Only when Everything is not installed and USN Journal is unavailable
(current walkdir approach - retained as ultimate fallback)
```

---

## 5. Implementation Plan

> **Head start:** Nex already has a `DiscoveryProvider` trait in `discovery.rs` and `CoreService` manages a list of providers. Adding an Everything provider is just implementing the trait and registering it — no framework changes needed.

### Phase 1 — Everything SDK Integration (P0, v1.1)

1. **Create FFI bindings** for `Everything64.dll`:
   - `Everything_SetSearchW`, `Everything_GetResult*`
   - `Everything_QueryW`, `Everything_IsDBLoaded`
   - `Everything_SetRequestFlags` (filename, path, size, date modified)
   - Track `everything-sys` crate or hand-roll

2. **Create `EverythingSearchProvider`** in `discovery.rs` (or a new file):
   - Implements the existing `DiscoveryProvider` trait
   - Wraps Everything SDK calls
   - Returns `SearchItem` results with `SearchItem::new()`

3. **Register provider** in `runtime.rs`:
   - Add `EverythingSearchProvider` to the provider list alongside `StartMenuAppDiscoveryProvider` and `FileSystemDiscoveryProvider`
   - Use `change_stamp()` to control reindex timing (since Everything always has latest data, stamp can be empty/time-based)

4. **Add configuration option**:
   - `search_everything_enabled = true` in config
   - Detect Everything installation path from registry (`HKCU\Software\Everything`)

### Phase 2 — USN Journal Watcher (P1, v1.2)

1. **Create `UsnJournalWatcher`** module:
   - `open_journal(volume_path: &str) -> Result<UsnJournalWatcher>`
   - `read_changes(&mut self) -> Result<Vec<FileChange>>`
   - `delete_journal()` on shutdown

2. **Store USN cursor** in SQLite meta table

3. **Polling loop** in background thread every 30s

4. **Apply changes** to SQLite index store

### Phase 3 — Remove Walkdir Dependence (P2, v1.3)

1. Keep walkdir as final fallback only
2. Remove Windows Search COM invocation code
3. Simplify `FileSystemDiscoveryProvider`

---

## 6. Recommendations Summary

| Priority | Approach | When | Effort |
|----------|----------|------|--------|
| **P0** | Everything SDK integration | v1.1 | 2-3 weeks |
| **P1** | USN Journal change tracking | v1.2 | 2-3 weeks |
| **P2** | Deprecate walkdir as primary | v1.3 | 1 week |
| **P3** | Deprecate Windows Search COM | v1.3 | 0.5 week |

**Bottom line:** Integrate Everything SDK first. It's what every competitive launcher uses. Offer to auto-install Everything if not present. This single change gives Nex **instant file search** parity with Flow Launcher.
