# Nex Memory Envelope Tightening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Nex memory behavior more predictable on broad discovery roots by tightening file/folder cache compaction and exposing the active cache policy in logs and status diagnostics.

**Architecture:** Keep app caching untouched and reduce only file/folder cache pressure when discovery scope is broad. Reuse the existing cache compaction and overlay tuning paths instead of adding a background watcher or a new config surface. Expose the selected policy through runtime logs and status parsing so Windows validation can verify the behavior.

**Tech Stack:** Rust, existing `CoreService` cache compaction, Windows overlay diagnostics/logging, cargo test

---

### Task 1: Add compaction-policy tests

**Files:**
- Modify: `apps/core/src/core_service.rs`

**Step 1: Write the failing tests**

- Add unit tests for:
  - broad-root detection for `C:\` style roots
  - default roots not triggering broad-root mode
  - compacted file/folder cap becoming stricter in broad-root mode while app items remain untouched

**Step 2: Run targeted tests to verify failure**

Run: `cargo test -p nex core_service::tests::broad_root`

Expected: FAIL because the helpers do not exist yet.

**Step 3: Write minimal implementation**

- Add helper logic for:
  - broad-root discovery detection
  - effective file/folder cache cap selection
  - compaction summary generation

**Step 4: Run targeted tests to verify pass**

Run: `cargo test -p nex core_service::tests::broad_root`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/core/src/core_service.rs
git commit -m "tighten broad-root cache compaction"
```

### Task 2: Expose cache policy in runtime logs and status diagnostics

**Files:**
- Modify: `apps/core/src/core_service.rs`
- Modify: `apps/core/src/runtime.rs`

**Step 1: Write the failing test**

- Add runtime parser/status-json tests for `cache_compaction` diagnostics.

**Step 2: Run targeted tests to verify failure**

Run: `cargo test -p nex status_diagnostics`

Expected: FAIL because cache-compaction lines are not parsed yet.

**Step 3: Write minimal implementation**

- Expand `cache_compaction` logging to include:
  - total input count
  - retained count
  - dropped count
  - app retained count
  - file/folder retained count
  - effective file/folder cap
  - broad-root mode flag
  - active memory target
- Extend status snapshot parsing and `--status-json` to surface the latest cache compaction line.

**Step 4: Run targeted tests to verify pass**

Run: `cargo test -p nex status_diagnostics`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/core/src/core_service.rs apps/core/src/runtime.rs
git commit -m "surface cache compaction diagnostics"
```

### Task 3: Improve overlay-side memory diagnostics

**Files:**
- Modify: `apps/core/src/windows_overlay.rs`
- Modify: `apps/core/src/runtime.rs`

**Step 1: Write the failing test or parser assertion**

- Extend runtime diagnostics tests to verify extra icon-cache telemetry fields are preserved in status parsing.

**Step 2: Run targeted tests to verify failure**

Run: `cargo test -p nex status_diagnostics`

Expected: FAIL because the new icon-cache tokens are not logged yet.

**Step 3: Write minimal implementation**

- Add live/max icon-cache entry counts to overlay icon-cache logs.
- Keep existing tuning behavior; do not add a new config key.

**Step 4: Run targeted tests to verify pass**

Run: `cargo test -p nex status_diagnostics`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/core/src/windows_overlay.rs apps/core/src/runtime.rs
git commit -m "expand overlay memory telemetry"
```

### Task 4: Update validation docs and run full suite

**Files:**
- Modify: `docs/engineering/windows-runtime-validation-checklist.md`

**Step 1: Update docs**

- Document the new `cache_compaction` and icon-cache telemetry expectations in `--status` / `--status-json`.
- Add validation notes for broad-root scenarios.

**Step 2: Run full test suite**

Run: `cargo test -p nex`

Expected: PASS

**Step 3: Commit**

```bash
git add docs/engineering/windows-runtime-validation-checklist.md
git commit -m "document memory envelope diagnostics"
```

