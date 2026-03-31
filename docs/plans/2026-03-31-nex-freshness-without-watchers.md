# Nex Freshness Without Watchers Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Improve freshness observability and self-healing without adding a filesystem watcher service.

**Architecture:** Build on the existing incremental provider stamps, stale-entry pruning, and config-driven background reindexing. Add provider freshness and stale-prune telemetry, and debounce queued reindex requests that arrive while an index refresh is already running.

**Tech Stack:** Rust, existing `CoreService` incremental indexing flow, runtime status/log parsing, PowerShell profiling script, cargo test

---

### Task 1: Add provider freshness diagnostics

**Files:**
- Modify: `apps/core/src/core_service.rs`
- Modify: `apps/core/src/runtime.rs`

**Step 1: Write the failing tests**

- Add runtime parser/status-json tests for `provider_freshness` lines.

**Step 2: Run targeted tests to verify failure**

Run: `cargo test -p nex status_diagnostics`

Expected: FAIL because `provider_freshness` is not parsed yet.

**Step 3: Write minimal implementation**

- Log provider freshness after incremental provider decisions with:
  - provider name
  - skipped flag
  - last scan age
  - reconcile interval
  - stamp presence
- Surface the latest line in `--status` and `--status-json`.

**Step 4: Run targeted tests to verify pass**

Run: `cargo test -p nex status_diagnostics`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/core/src/core_service.rs apps/core/src/runtime.rs
git commit -m "add provider freshness diagnostics"
```

### Task 2: Add stale-prune telemetry

**Files:**
- Modify: `apps/core/src/core_service.rs`
- Modify: `apps/core/src/runtime.rs`

**Step 1: Write the failing tests**

- Extend runtime diagnostics tests for `stale_prune` log lines.

**Step 2: Run targeted tests to verify failure**

Run: `cargo test -p nex status_diagnostics`

Expected: FAIL because `stale_prune` is not parsed yet.

**Step 3: Write minimal implementation**

- Log stale-prune activity when stale entries are removed:
  - scanned count
  - removed count
  - cache remaining
- Surface the latest line in status diagnostics.

**Step 4: Run targeted tests to verify pass**

Run: `cargo test -p nex status_diagnostics`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/core/src/core_service.rs apps/core/src/runtime.rs
git commit -m "log stale prune freshness telemetry"
```

### Task 3: Debounce queued discovery reindex while indexing is active

**Files:**
- Modify: `apps/core/src/runtime.rs`

**Step 1: Write the failing test**

- Add a pure helper test for queued reindex debounce timing.

**Step 2: Run targeted tests to verify failure**

Run: `cargo test -p nex queued_reindex`

Expected: FAIL because debounce helpers do not exist yet.

**Step 3: Write minimal implementation**

- Replace the boolean-only pending reindex state with:
  - queued-at / due-at timing
  - request count
- Keep immediate reindex for idle cases.
- Debounce only the “index already active” path so repeated config saves do not trigger chained reindexes.

**Step 4: Run targeted tests to verify pass**

Run: `cargo test -p nex queued_reindex`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/core/src/runtime.rs
git commit -m "debounce queued discovery reindex"
```

### Task 4: Update operator tooling and run full suite

**Files:**
- Modify: `scripts/windows/profile-memory-and-icons.ps1`
- Modify: `docs/engineering/windows-runtime-validation-checklist.md`

**Step 1: Update tooling/docs**

- Include `cache_compaction`, `provider_freshness`, `stale_prune`, and `startup_phase` in the profiling script output.
- Document the new freshness telemetry and debounced reindex behavior.

**Step 2: Run full suite**

Run: `cargo test -p nex`

Expected: PASS

**Step 3: Commit**

```bash
git add scripts/windows/profile-memory-and-icons.ps1 docs/engineering/windows-runtime-validation-checklist.md
git commit -m "document freshness diagnostics"
```

