# Phases 7-11 — Context & Decisions

**Date:** 2026-06-08
**Status:** Discussed & Decided
**Prior Context:** PROJECT.md, REQUIREMENTS.md, STATE.md, ROADMAP.md
**Codebase Scout:** Complete (indexing, search quality, UI, snippets, testing, architecture)

## Dependency Graph (Revised)

```
Phase 6 (Stability — DONE)
    │
    ├──► Phase 7 (Perf: Indexing) ──► Phase 8 (Perf: Search)   ← parallel
    │
    ├──► Phase 7 ──► Phase 9 (UI Polish) ──► Phase 10 (Snippets) ──► Phase 11 (Testing)
```

- **Phase 7 & 8**: run in parallel (different code paths — indexing vs. search quality)
- **Phase 9**: depends on Phase 7 only (indexing must be stable before UI polish that affects responsiveness perception)
- **Phase 10**: depends on Phase 9 (snippets render inside the polished overlay UI)
- **Phase 11**: depends on Phase 10 (must test snippet system)

---

## Phase 7: Performance — Indexing & Memory

### Requirements: R3.1, R3.2

### Decided

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Incremental vs batch rebuild** | Incremental add/remove per-item | Faster sync, lower resource usage. File watchers already call `upsert_item`/`delete_item_by_id` per-item — extend this to Tantivy. Initial full-build stays for first run. |
| **Index compaction** | Periodic auto-compaction | Run `commit()` + merge policy tuning periodically (every N writes or M minutes). Prevents index fragmentation over time. |
| **Memory profiling approach** | Both runtime logging + WPR profiling | Add periodic memory logging (every 30s) to the existing runtime stats. Use Windows Performance Recorder/Analyzer for deep-dive on hotspots. |
| **Performance targets** | Requirements targets | Initial index <30s for 100K items. Incremental updates <500ms. Idle memory <50MB. Active search <100MB. Existing p95 query latency <15ms unchanged. |
| **Architecture cleanup** | Full audit of all production `.unwrap()` calls | Replace `.unwrap()` on Mutex locks with graceful error handling throughout the codebase (not just runtime_loop.rs). |

### Implementation Notes
- **Tantivy per-item API**: `tantivy::IndexWriter` supports `add_document()` and `delete_term()`. Replace the current `index_items()` batch loop with incremental writer calls.
- **File watcher integration**: `file_watcher_consumer.rs` already calls `service.upsert_item()`/`delete_item_by_id()` — extend these to also call Tantivy/FTS5 incremental updates.
- **FTS5**: Enable individual INSERT/DELETE on the FTS5 virtual table instead of full rebuild.
- **Compaction trigger**: After every 500 writes or every 5 minutes, commit the Tantivy writer with merge policy (`LogMergePolicy` with tuned segment sizes).
- **Memory logging**: Use `windows-sys` `GetProcessMemoryInfo` or `std::process::id()` + WMI. Log working set, private bytes, and Tantivy arena usage.
- **Discovery bottlenecks**: `walkdir` max_depth=5 with broad roots produces many entries. The caps (`index_max_items_*`) already gate this — optimize the walk to respect limits mid-walk.

### Scope Creep Guard (NOT in Phase 7)
- Cross-platform indexing (Windows only)
- Switching index engines (Tantivy stays)
- Adding new file discovery backends

---

## Phase 8: Performance — Search Quality

### Requirements: R3.3

### Decided

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Fuzzy matching algorithm** | Keep subsequence matching | Current approach already handles missing chars and char ordering. Adding Levenshtein/Damerau-Levenshtein adds complexity without proven UX gain for a launcher's search. |
| **Result deduplication** | App deduplication only | Deduplicate the same app appearing from multiple sources (Start Menu .lnk + uninstall registry + .exe path). Keep existing top-hit confidence guard. Do NOT do full result clustering. |
| **Parallel with Phase 7** | Yes, run in parallel | Different code paths — Phase 7 touches `search.rs`/`discovery.rs`/`core_service.rs`, Phase 8 touches `runtime_search_session.rs`/`query_dsl.rs`/scoring logic. |

### Implementation Notes
- **App deduplication**: When multiple `SearchItem`s share the same `title` (case-insensitive) and similar `path` (same basename), keep the highest-scored one. Implement in `search_with_filter_internal()` or as a post-processing pass.
- **Subsequence matching**: Already implemented. No changes needed — this decision means "don't add Levenshtein."
- **Scoring audit**: Document the current scoring formula constants. Consider making `source_bonus` and `mode_bonus` configurable.
- **Typo tolerance**: Current subsequence handles missing chars (e.g., "ntepd" matches "Notepad"). Add adjacent-char swap tolerance if user feedback demands it — out of scope for Phase 8.

### Scope Creep Guard (NOT in Phase 8)
- Trigram index
- Full Levenshtein/Damerau-Levenshtein
- Configurable scoring weights (can be Phase 8 stretch goal)
- Search analytics/telemetry

---

## Phase 9: UI Polish — Mica, Corners, Typography

### Requirements: R5.1, R5.2, R5.3, R5.4

### Decided

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Animation strategy** | Iced built-in `animate()` API | Use `Container::animate()` for opacity on show/hide (150ms fade), `Column::animate()` for height expansion (110ms). Iced 0.14 supports widget-level animation natively. |
| **Mica backdrop** | Win11 detection gate only | Detect Windows 11 22H2+ via `RtlGetVersion`. If detected and version >= 10.0.22621, attempt `DWMWA_SYSTEMBACKDROP_TYPE = 2` (Mica). On Win10 or pre-22H2 Win11, keep current transparent background. No silent fallback — explicit OS check. |
| **DWM rounded corners** | DWM on Win11, Iced border on Win10 | `DWMWA_WINDOW_CORNER_PREFERENCE = 2` (rounded) on Win11. `Border { radius: 10.0 }` as fallback on Win10. Iced's border radius provides software-rounded corners already — DWM just makes them native on Win11. |
| **Font** | Bundle Inter | Bundle Inter as an embedded font. Consistent cross-version rendering. Inter is modern, highly readable at small sizes (used by Raycast, Vercel, Figma). Embed via `include_bytes!` + Iced font API at startup. |
| **Light theme** | Complete and ship now | Wire `Theme::Light` into the auto-detection system (`platform.rs` reads registry). The palette values exist — need to construct Light Palette with appropriate colors and enable the theme toggle. |
| **Keycap rendering** | Custom widget with rounded border + monospace text | Footer hints show keycap-style boxes (Enter, ↑↓, Esc, Tab). Build as a simple `Row` of `Container` widgets with `Border { radius: 4.0 }`, muted text color, monospace font. |

### Implementation Notes
- **Inter font bundle**: Download Inter variable or static .ttf from Google Fonts. Embed via `include_bytes!("../assets/Inter-Regular.ttf")`. Load at startup via `iced::Font::with_name("Inter")`.
- **Mica detection**: Use `windows-sys` `RtlGetVersion` to get build number >= 22621. Store as `bool` in `State` (not Model — it's read-only). Apply `DwmSetWindowAttribute` on the raw window handle after creation.
- **Animations**: Replace `SW_SHOWNOACTIVATE`/`SW_HIDE` instant toggle with Iced model-based animation. The model gets an `animation_state` enum: Hidden → FadingIn(progress) → Visible → FadingOut(progress). Drive via `iced::time::every()` subscription while animating.
- **Light theme**: Current dark palette has 15 color slots. Light palette needs similar slots with inverted/soft values. Auto-detect on startup and on `WM_SETTINGCHANGE` (registry change broadcast).

### Scope Creep Guard (NOT in Phase 9)
- Full theme customization UI
- Acrylic/blur backdrop (Mica only)
- Custom widget system
- Font size customization by user

---

## Phase 10: Snippets / Text Expansion

### Requirements: R4.1, R4.2, R4.3, R4.4

### Decided

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Storage format** | Dedicated `snippets.toml` file | `%APPDATA%\Nex\snippets.toml`. Simple, user-editable, consistent with config TOML format. Not part of config.toml (keeps config clean). |
| **Trigger model** | Search-integrated matching | Snippet triggers appear in overlay search results alongside apps/files/actions. User types trigger keyword, sees snippet result, presses Enter to expand. More discoverable than prefix-only. |
| **Keystroke simulation** | `SendInput` (Win32) | Modern API, supports Unicode + virtual keys + hardware scan codes. Can target foreground window. |
| **Snippet count** | Prepare for 1000+ snippets | Use a trie (prefix tree) for O(k) trigger lookup where k = query length. Avoid linear scan. |
| **Dynamic placeholders** | Date/time/clipboard only | `{{date}}`, `{{time}}`, `{{datetime}}`, `{{clipboard}}`. Simple string replacement before expansion. |
| **Snippet management UI** | File-only (no UI in Phase 10) | Users edit `snippets.toml` directly. Focus Phase 10 on the engine + overlay integration only. UI deferred. |

### Implementation Notes
- **snippets.toml format**:
  ```toml
  [[snippets]]
  trigger = ";sig"
  text = "Best regards,\nJohn Doe"
  case_sensitive = false

  [[snippets]]
  trigger = ";now"
  text = "{{datetime}}"
  ```
- **Trie data structure**: Implement as `HashMap<char, TrieNode>` with leaf nodes holding `Vec<Snippet>` (multiple snippets can share a prefix). Rebuild trie on file change (watch `snippets.toml` for changes).
- **Search integration**: Add a snippet search provider to the search pipeline (like clipboard history). Snippets appear with a distinct icon and `kind = "snippet"`. When selected + Enter pressed: hide overlay, simulate keystrokes.
- **SendInput expansion**: Build an array of `INPUT` structs. For regular text: `KEYBDINPUT` with `KEYEVENTF_UNICODE`. For special keys (Tab, Enter): virtual key codes with press/release pairs.
- **Clipboard placeholder**: Use existing `clipboard_history` infrastructure to read current clipboard. Only expand on demand (user presses Enter), not on every search.
- **Date/time formatting**: Use `chrono` crate (check if already in dependencies). If not, use `std::time::SystemTime` + manual formatting or add `chrono`.

### Scope Creep Guard (NOT in Phase 10)
- Snippet management UI (deferred)
- Full template engine (conditionals, loops, math)
- Snippet import/export
- Snippet sync/cloud backup
- Cursor/selection reading for placeholders

---

## Phase 11: Testing — Coverage & CI Hardening

### Requirements: R6.1, R6.2, R6.3

### Decided

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Coverage strategy** | Targeted: key logic areas | Focus tests on config migrations (NOT property-based), model update logic, search ranking correctness, snippet matching. No numeric coverage threshold. |
| **Config testing** | Skip deep migration testing | Existing 5 config tests are sufficient. No roundtrip testing from every version. |
| **Architecture cleanup scope** | Full audit of `.unwrap()` in production | Replace all `.unwrap()` on Mutex locks across the entire codebase with graceful error handling (not just runtime_loop.rs). Audit all ~9 production `.unwrap()` calls. |

### Implementation Notes
- **Model update tests**: Test `Model::update()` for query changes, selection movement, submit logic, escape handling. The `#[cfg(test)]` module already exists in `model.rs` with 10 tests.
- **Search ranking tests**: Create deterministic test scenarios with known SearchItems and verify the ranking order. Test app deduplication, source bonus, mode bonus, personalization boost.
- **Snippet tests**: Test trie construction, trigger matching, dynamic placeholder expansion, SendInput payload construction.
- **Theme palette tests**: Verify palette colors are valid (no zeros, sufficient contrast ratios). Existing 3 theme tests cover basic structure.
- **CI: Identify flaky tests**: Review CI history. Common flaky patterns: timing-dependent tests, file system state leakage, parallel test interference.
- **`.unwrap()` audit** (from R3.4 scope):
  - `runtime_loop.rs`: 9 instances → replace with `.ok()` + `log_warn!` + early return/skip
  - `everything_bridge.rs`: 1 instance → safe unwrap_or
  - Other files: audit and fix

### Scope Creep Guard (NOT in Phase 11)
- Property-based testing for config migrations
- Screenshot comparison / visual regression testing
- Mutation testing
- 100% coverage target
- Browser-based UI tests

---

## Cross-Cutting Decisions

| Decision | Choice | Applies To |
|----------|--------|------------|
| **.unwrap() cleanup** | Full production audit | All phases (R3.4) |
| **Phase 7+8 parallelism** | Parallel execution | Phases 7, 8 |
| **Font bundling** | Inter, embedded via `include_bytes!` | Phase 9 |
| **No UI for snippets** | File-only snippet management | Phase 10 |

## Deferred Ideas (Scope Creep Guard)

These were discussed and explicitly deferred:

| Idea | Reason | Could Return In |
|------|--------|-----------------|
| Trigram index for fuzzy search | Overkill for launcher use case | Phase 12+ |
| Full result clustering | App deduplication sufficient | Phase 12+ |
| Levenshtein/Damerau-Levenshtein | Subsequence matching works well | Phase 12+ |
| Snippet management UI | Focus Phase 10 on engine only | Phase 12+ |
| Full template engine for snippets | Date/time/clipboard enough | Phase 12+ |
| Property-based config migration tests | Existing tests sufficient | Phase 12+ |
| Numeric coverage threshold | Targeted testing preferred | Never |
| Cursor/selection reading in snippets | Adds significant complexity | Phase 12+ |
| Configurable scoring weights | Premature optimization | Phase 12+ | 

## Next Steps

1. `/gsd-plan-phase 7` — Plan Phase 7 (Perf: Indexing & Memory)
2. `/gsd-plan-phase 8` — Plan Phase 8 (Perf: Search Quality) — can run in parallel with Phase 7
3. After Phase 7 completes: `/gsd-plan-phase 9` — Plan Phase 9 (UI Polish)
4. After Phase 9 completes: `/gsd-plan-phase 10` — Plan Phase 10 (Snippets)
5. After Phase 10 completes: `/gsd-plan-phase 11` — Plan Phase 11 (Testing)
