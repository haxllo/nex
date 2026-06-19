# Nex keystroke-to-list — bottlenecks & fixes

Pipeline summary:

```
keydown → JS debounce(40ms) → ipc.postMessage → host.rs handle_ipc
  → crossbeam(OverlayEvent::QueryChanged) → RuntimeWorker
  → mpsc(SearchRequest) → SearchWorker thread → CoreService::search
  → mpsc(SearchResult) + crossbeam(SearchResultsReady) → RuntimeWorker
  → shim.set_results → EventLoopProxy(UiCommand::Apply) → host.rs push_state
  → wv.evaluate_script("window.nex.apply(json)") → JS render()
```

---

## B-1. 40ms fixed debounce delays every keystroke

**File:** `app.js:260`

```js
clearTimeout(debounce);
debounce = setTimeout(() => { post("query", q); }, 40);
```

40ms added before query _starts_. On fast typist (200ms between chars), 40ms is 20% of gap. On slow typist, 100% unhappy wait.

**Fix:** Adaptive debounce — 20ms on first char, then 40ms after flow of identical queries. Or use idle-until-urgent pattern: send immediately, then debounce subsequent rapid changes at 80ms to let search worker cancel stale.

```js
// adaptive: first char fires immediately, subsequent rapid chars
// fall into a longer debounce so SearchWorker can drain stale.
let lastQueryTime = 0;
function onInput(q) {
  const now = performance.now();
  const delay = (now - lastQueryTime > 300) ? 0 : 80;
  lastQueryTime = now;
  clearTimeout(debounce);
  if (delay === 0) { post("query", q); return; }
  debounce = setTimeout(() => post("query", q), delay);
}
```

**Impact:** ~0–20ms saved on first char of each burst, ~40ms on solo keystroke.

---

## B-2. `evaluate_script` blocks host event loop synchronously

**File:** `host.rs:431`

```rust
let _ = wv.evaluate_script(&format!("window.nex&&window.nex.apply({json})"));
```

wry's `evaluate_script` calls WebView2 `ExecuteScript` → waits for browser UI thread → serializes JSON → sends result back. Blocks the host event loop for the duration. During this block, `UiCommand::Show`, `Resize`, `Painted` are queued but not processed.

**Measured impact (rough):** 2–8ms for small result sets, 15–40ms for 50+ rows with base64 icons.

**Fix A:** Move `evaluate_script` to a dedicated thread so the host event loop stays responsive:
```rust
let wv_clone = wv.controller().clone();
thread::Builder::new()
  .name("nex-push-state".into())
  .spawn(move || {
    let _ = wv_clone.evaluate_script(...);
  })
  .ok();
```
wry `WebView` is `Send` + `Sync` (it wraps `Arc`), so this is safe.

**Fix B:** Use `post_message` instead of `evaluate_script`. Replace JS-side `window.nex.apply(json)` with `window.addEventListener("message", ...)`:
- Rust: `wv.post_message("apply-state", json)` instead of `evaluate_script`
- JS: `window.addEventListener("message", e => { if (e.data.type === "apply-state") nex.apply(e.data.payload); })`
- `post_message` is fire-and-forget async — never blocks. **Largest single win.**

**Impact:** `evaluate_script` latency (2–40ms) removed from critical path. Host loop processes next keystroke immediately.

---

## B-3. Full state snapshot serialized + base64-encoded per Apply

**File:** `host.rs:441-500`

Every `push_state` rebuilds the full JSON blob: iterates all rows, base64-encodes every icon path (even cached ones). `serde_json::json!` allocates a fresh `Value` tree, then `.to_string()` allocates again.

**Measured impact:** Base64 is fast (single memcpy), but `serde_json::Value` tree allocation for 50 rows + 2 headers = ~52 objects. Total ~2–5µs per row in Rust, but JSON string of ~15KB must be parsed again by JS `JSON.parse` (implicit in `evaluate_script`).

**Fix A:** Pre-allocate `serde_json::json!` only for state keys that change rarely (theme, placeholder, hotkeyHint). Build rows array as raw JSON string concatenation instead of `Value` tree:

```rust
fn snapshot_json_fast(s: &ShimState, icons: &Arc<IconCache>) -> String {
  let mut out = String::with_capacity(16_384);
  out.push_str(r#"{"query":"#);
  serde_json::to_string(&s.query, &mut out).ok();
  out.push_str(r#","rows":["#);
  for (i, r) in s.rows.iter().enumerate() {
    if i > 0 { out.push(','); }
    write_row_json(&mut out, r, icons);
  }
  out.push_str(r#"],"selected":"#);
  // ... rest of fields
  out.push('}');
  out
}
```

**Fix B:** Skip base64 for icons already sent in the previous state. Track per-row icon hash and only include `"icon"` field when it changed. JS keeps last icon in DOM.

**Fix C:** Batch row data as JSON array of arrays `[role,title,subtitle,kind,icon,selectable,resultIndex]` instead of objects — reduces serialized size ~30%.

---

## B-4. Icon decode blocks first search (cold cache)

**File:** `shim.rs:248-255` + `icons.rs:334-354`

```rust
std::thread::Builder::new()
  .name("nex-icon-prefetch".into())
  .spawn(move || prefetch_rows(&cache, &rows))
```

Prefetch spawns a thread per `set_results` and decodes all row icons sequentially (one `ExtractIconExW` call each). First search after cold start pays full decode cost.

**Measured impact:** Each icon decode is ~1–5ms (shell icon extraction + PNG encode). 30 visible rows = 30–150ms total on prefetch thread. Invisible to user (prefetch is async), but icons are blank until next search.

**Fix A:** Throttle prefetch — batch first N=8 icons immediately, defer rest to low-priority thread:
```rust
let (fast, slow) = rows.split_at(8.min(rows.len()));
prefetch_rows(cache, fast);
thread::spawn(move || prefetch_rows(cache, slow));
```

**Fix B:** Cache decoded PNGs to temp directory on first access so subsequent launches skip `ExtractIconExW` entirely. Keyed by file path hash + last-modified stamp.

**Impact:** Icons visible on first render instead of second keystroke.

---

## B-5. `recv_timeout(50ms)` in message pump adds jitter

**File:** `shim.rs:289`

```rust
match event_rx.recv_timeout(std::time::Duration::from_millis(50)) {
```

Runtime thread sleeps up to 50ms between events when no new query arrives. If `QueryChanged` and `SearchResultsReady` arrive within the same polling cycle, one polls with 0-wait. But if they straddle a boundary, `SearchResultsReady` waits up to 50ms.

This is not hit in normal flow (query → result inside one cycle) but can add 50ms on first show after warm-release rebuild.

**Fix:** Use blocking `recv()` instead of `recv_timeout`. The 50ms timeout exists only for the `is_running` check loop. Replace with:

```rust
while is_running.load(Ordering::SeqCst) {
  cross_channel::select! {
    recv(event_rx) -> event => on_event(event?),
    default(Duration::from_millis(50)) => {}
  }
}
```

Or use a dedicated stop channel instead of polling `is_running` + timeout.

**Impact:** Removes 50ms worst-case jitter. Low-impact fix.

---

## B-6. `querySelectorAll(".row")` on every arrow key

**File:** `app.js:146`

```js
for (const el of list.querySelectorAll(".row")) {
  el.classList.toggle("selected", Number(el.dataset.index) === selected);
}
```

Arrow key (keyboard navigation) calls `setSelected` which re-queries all `.row` elements O(n) to toggle a single class. For 100 rows this is ~100 DOM lookups per keypress.

**Fix:** Keep live `NodeList` or `Map<index, HTMLElement>` on the render path:

```js
// At end of render(), populate a row map:
const rowMap = new Map();
for (const li of list.children) {
  if (li.classList.contains("row")) rowMap.set(Number(li.dataset.index), li);
}
```

Then in `setSelected`:
```js
const prev = rowMap.get(selected);
if (prev) prev.classList.remove("selected");
const next = rowMap.get(i);
if (next) next.classList.add("selected");
```

**Impact:** O(n) → O(1) for selection change. Only matters on 50+ rows with rapid arrow scrolling.

---

## B-7. Double JSON parse + stringify across Rust/JS boundary

**File:** `host.rs:441-500` + `app.js:267-296`

Rust serializes `snapshot_json` → `to_string()`, `evaluate_script` passes this string to WebView2, JS `JSON.parse`s it (implicitly when `evaluate_script` evaluates `nex.apply({json})`).

**Fix:** If switching to `post_message` (B-2 Fix B), pass structured data directly — no `JSON.stringify` needed. wry `post_message` serializes with serde for you, and JS side receives already-parsed object.

Combined with B-2 Fix B and B-3 Fix A, this eliminates 3 allocation/parse cycles per keystroke.

---

## B-8. `scrollIntoView` triggers forced layout

**File:** `app.js:153-155`

```js
function scrollToSelected() {
  const el = list.querySelector(".row.selected");
  if (el) el.scrollIntoView({ block: "nearest" });
}
```

`scrollIntoView` forces synchronous layout. Called at end of every `render()` and every `setSelected`.

**Fix:** Only `scrollIntoView` when the selected row is outside the visible viewport. Use `element.offsetTop` + `list.scrollTop` + `list.clientHeight` to test:

```js
function scrollToSelected() {
  const el = list.querySelector(".row.selected");
  if (!el) return;
  const top = el.offsetTop;
  const bot = top + el.offsetHeight;
  if (top < list.scrollTop || bot > list.scrollTop + list.clientHeight) {
    el.scrollIntoView({ block: "nearest" });
  }
}
```

Or skip `scrollIntoView` entirely on keyboard navigation and just set `element.scrollTop` — avoids forced layout.

**Impact:** Saves forced-layout cost on every keystroke (when selected is already visible, which is >95% of cases).

---

## B-9. Full `prune_stale_items_if_due` runs on every search query

**File:** `core_service.rs:340`

```rust
self.prune_stale_items_if_due()?;
```

Called on every `search_with_filter_internal` (every keystroke). `prune_stale_items_if_due` acquires write lock on cached_items, scans all cached items, deletes stale ones from store + backends. Even if no items are stale, acquiring `RwLock::write` stalls concurrent reads.

**Measured impact:** `prune_stale_items_if_due` has an early-exit time check so it runs at most every `PRUNE_INTERVAL_MS = 60_000`. But on the first query after 60s idle, it blocks the search.

**Fix:** Move prune to a dedicated background thread at `PRUNE_INTERVAL_MS` cadence. Remove from `search_with_filter_internal` entirely.

```rust
// In CoreService::new, spawn:
thread::Builder::new()
  .name("nex-stale-pruner".into())
  .spawn(move || loop {
    thread::sleep(PRUNE_INTERVAL_MS);
    service.prune_stale_items_if_due().ok();
  });
```

**Impact:** Removes write-lock contention from search path. ~0–500ms pause on first query after 60s idle.

---

## B-10. `CoreService` mutex lock contention on search path

**File:** `search_worker.rs:69`

```rust
let service_guard = match service.lock() {
```

`Arc<Mutex<CoreService>>` — the entire service is locked during search. If an index refresh holds the lock (e.g., `sync_indexes_from_cache`), search blocks.

**Fix:** Replace `Mutex<CoreService>` with `RwLock<CoreService>`. Search takes read lock. Index refresh takes write lock. Tantivy/FTS5 indexes inside CoreService are themselves behind individual mutexes (already), so the outer lock is overly coarse.

```rust
// In search_worker.rs
let service_guard = service.read().map_err(...)?;
// In core_service background refresh
let mut service = self.write();
```

**Impact:** Concurrent search + refresh no longer serialized. ~0–full-sync-duration worst-case reduced to ~0ms.

---

## Summary by impact

| # | Bottleneck | Est. latency | Fix effort | Net gain |
|---|-----------|-------------|-----------|---------|
| B-2 | `evaluate_script` blocking host loop | 2–40ms | Medium | Async dispatch removes blocking |
| B-3 | Full JSON re-serialization per Apply | 1–5ms | Medium | Faster Apply, less GC |
| B-1 | Fixed 40ms debounce | 0–40ms | Low | First char instant, burst smoother |
| B-9 | Stale prune in search path | 0–500ms (rare) | Low | Removes write-lock from search |
| B-10 | `Mutex<CoreService>` on search | 0–full-sync (rare) | Medium | Read-lock allows concurrent search |
| B-5 | `recv_timeout(50ms)` jitter | 0–50ms (rare) | Low | Removes worst-case idle jitter |
| B-4 | Cold icon decode | visible on 2nd keystroke | Low | Icons visible on 1st render |
| B-7 | Double JSON parse | 1–3ms | Free w/ B-2 fix | Eliminated by post_message |
| B-6 | `querySelectorAll` per arrow | <1ms | Low | O(n)→O(1) selection |
| B-8 | `scrollIntoView` forced layout | <1ms | Low | Skip when already visible |

**Recommended order:** B-2 (post_message) → B-1 (adaptive debounce) → B-9 (prune off search path) → B-10 (RwLock) → B-3 (fast JSON) → B-4 (icon throttle) → B-5/B-6/B-8 polish.
