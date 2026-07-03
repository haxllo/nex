# Nex — Full Codebase Audit & Improvement Roadmap

> **Generated:** May 2026
> **Last reviewed:** July 2026 (v1.3.0)
> **Scope:** Full codebase review, competitive analysis, and prioritized improvement plan
> **Status:** AI features deferred (out of scope). P0 Everything SDK integration **complete** (v1.1.0+). Overlay and runtime refactors **complete** (pre-v1.2). Tray icon path, freeze-on-Everything-down, and Tantivy memory regressions all fixed in v1.2/v1.3.
> **See also:**
>   - [Overlay Refactor Plan](./overlay-refactor-plan.md) — `windows_overlay.rs` → 11 modules, **complete** (legacy code removed, now WebView2-based `overlay/`)
>   - [Indexing Comparison](./indexing-comparison.md) — Everything SDK vs USN Journal vs walkdir, **decision made** (Everything SDK primary, walkdir fallback, USN rejected)
>   - [Project Charter](../product/project-charter.md) — original product requirements

---

## 1. Executive Summary

Nex is a **keyboard-first Windows launcher** built in Rust (~22K lines). It provides global hotkey-activated search, file indexing, clipboard history, and a plugin system. The code compiles cleanly and the architecture is sound.

**The core finding:** Nex has a strong technical foundation (Rust, proper Win32 APIs, DirectComposition overlay, SQLite-backed search) but is **not yet competitive** with Raycast or Flow Launcher. Gaps in plugin ecosystem, AI, window management, and polish features are significant but all addressable.

---

## 2. Current Architecture

| Component | Lines | Assessment |
|-----------|-------|------------|
| `overlay/` | 9 files | WebView2-based overlay (tao + wry). HTML/CSS/JS rendering via `nexasset://` protocol. Acrylic backdrop, state pushed via `PostWebMessageAsJson`. |
| `runtime.rs` | 1,123 | Thin orchestration entrypoint after module extraction |
| `runtime_*.rs` | 9 files | Runtime split across commands, actions, loop, diagnostics, hotkey, indexing, process, rows, and search session |
| `discovery.rs` | 1,767 | Reasonable, could split |
| `core_service.rs` | 1,464 | OK |
| `config.rs` | 1,379 | OK |
| `uninstall_registry.rs` | 1,007 | OK |
| `search.rs` | 759 | Well-structured |
| `clipboard_history.rs` | 322 | OK |
| `index_store.rs` | 296 | OK |
| `plugin_sdk.rs` | 249 | **Too small** — needs major expansion |
| Other files | < 300 each | Generally fine |

### Strengths
- **Rust** — native perf, no GC, small binary, no runtime dependency
- **DirectComposition/Dwm overlay** — correct modern Windows approach
- **Clean workspace layout** — cargo-managed, well-structured
- **Win32 API usage** — proper feature-gated dependencies
- **Tests exist** for core modules
- **MIT license** — fully open source

### Weaknesses
- **No async runtime** — everything is sync threads
- **Plugin system is skeletal** — no distribution mechanism
- **No AI integration** at all
- **No window management**
- **No text expansion / snippets**
- **No emoji picker, color picker**
- **No extension store**
- **Test coverage ~25%** — insufficient (target 30%+)
- **Documentation outdated**

---

## 3. Competitive Landscape

| Feature | **Raycast** | **Flow Launcher** | **PowerToys Run** | **Nex** |
|---------|------------|-------------------|-------------------|---------|
| **Language** | TypeScript/React + native | C# (.NET) | C# (.NET) | **Rust** ✅ |
| **Plugin SDK** | React/TS, 1000+ extensions | Multi-language (C#, Python, JS, TS) | Limited native | Rust trait, no distribution ❌ |
| **AI Features** | Built-in Quick AI, Pro models | Community LLM plugins | None | **None** ❌ |
| **File Indexing** | Custom proprietary indexer | Everything integration | Windows Search | Custom SQLite + **Everything SDK** ✅ |
| **Window Management** | Built-in | Community plugins | None | **None** ❌ |
| **Clipboard Manager** | Yes + snippets | Community plugin | Native Win+V | Basic |
| **Text Expansion** | Yes (Snippets) | Community plugin | None | **None** ❌ |
| **Extension Store** | Yes, 1000+ extensions | Yes, Plugin Store | N/A | **None** ❌ |
| **Open Source** | No (proprietary) | Yes (MIT) | Yes (MIT) | Yes (MIT) ✅ |
| **Search Speed** | Near-instant | Instant (w/ Everything) | Fast | Fast (SQLite)
| **UI Quality** | Premium, cohesive | Good, customizable | Basic | Custom overlay (WIP)
| **RAM** | ~150-300 MB | ~80-150 MB | ~50-100 MB | **Low (Rust advantage)**

### Positioning Opportunity

> *"Raycast-like experience with native Rust performance, fully open source, and privacy-first (local AI)."*

---

## 4. Improvement Tasks — Prioritized

### P0 — Ship-blocking (v1.1)

| # | Task | Est. Effort | Impact | Status |
|---|------|------------|--------|--------|
| 1 | **Plugin SDK + distribution** — WASM-based plugin format, `nx install` command | 3-4 weeks | 🔴 Critical | ⏳ Planned (still pending — no public store yet) |
| ~~2~~ | ~~**AI integration** — Ollama + OpenAI options, "ask anything" query mode~~ | ~~2-3 weeks~~ | ~~🔴 Critical~~ | ❌ **Deferred** (out of scope, privacy-first posture) |
| 3 | **Window management** — 6-8 tile layouts, monitor movement, sizing | 2-3 weeks | 🔴 Critical | ⏳ Planned (no code yet; was claimed complete in v1.1.0 release notes but not shipped) |
| 4 | **File indexing upgrade** — Everything SDK integration for instant search | 2-3 weeks | 🔴 Critical | ✅ **Complete** (v1.1.0; v1.3.0 added service-liveness probe + walkdir fallback) |

### P1 — Competitive Parity (v1.2)

| # | Task | Est. Effort | Impact | Status |
|---|------|------------|--------|--------|
| 5 | **Calculator + unit converter** | 1 week | 🟡 High | ✅ **Complete** (custom shunting-yard in `apps/core/src/calculator.rs` — supports `+`, `-`, `*`, `/`, `%`, `^`, parens, `sqrt`, `abs`, `ln`, `round`, `floor`, `ceil`, `pi`, `e`. `meval` dependency rejected per risk note.) |
| 6 | **Snippets/text expansion** | 1-2 weeks | 🟡 High | ⏳ Planned |
| 7 | **Async runtime (tokio)** | 2-3 weeks | 🟡 High | ❌ **Rejected** — sync `std::thread` is sufficient; tokio would add dependency weight without a current need. Re-evaluate if file_watcher or HTTP-based providers land. |
| 8 | **Refactor large files** — split `windows_overlay.rs` and `runtime.rs` | 1-2 weeks | 🟡 Medium | ✅ **Complete** (9-module `overlay/` WebView2 migration, 9-module `runtime_*.rs`) |

### P2 — Quality of Life (v1.3+)

| # | Task | Est. Effort | Impact | Status |
|---|------|------------|--------|--------|
| 9 | **Expand test coverage** to 30%+ and add E2E tests | Ongoing | 🟢 Medium | ⏳ In progress (current ~25%, growing per release) |
| 10 | **Update documentation** | 1 week | 🟢 Medium | ⏳ In progress (this file is part of that effort) |
| 11 | **Emoji picker, color picker, web search shortcuts** | 1-2 weeks | 🟢 Medium | ⏳ Planned |
| 12 | **Performance benchmarking suite** | 1 week | 🟢 Low | ⏳ Planned |
| 13 | **DirectoryWatcher** (real-time file change events) | 1-2 weeks | 🟡 High | ✅ **Complete** — `apps/core/src/file_watcher.rs` implemented and wired via `core_service.rs:743` (`start_file_watchers`), called from `runtime_loop.rs:586`. Uses `ReadDirectoryChangesW` with debounce. |
| 14 | **GDI RAII wrappers** (`GdiBrush`, `GdiFont`, `GdiIcon`) | 0.5 week | 🟢 Low | ⏳ Deferred — manual `DeleteObject` cleanup is correct, RAII is nice-to-have |

---

## 5. File-by-File Code Review Notes

### `overlay/` module (WebView2)
- **✅ WebView2 migration complete:** tao window + wry WebView, HTML/CSS/JS rendering via `nexasset://` protocol
- **✅ Acrylic backdrop** via `window-vibrancy` crate (Mica dropped — not implemented)
- **✅ State pushed to JS** via `PostWebMessageAsJson` (non-blocking, fire-and-forget)
- **✅ Icons** decoded to PNG, embedded as base64 data URIs in state snapshot
- **✅ DPI handling** and window positioning on cursor monitor

### `runtime.rs` + `runtime_*.rs`
- **✅ Refactored:** Runtime orchestration is split into dedicated modules for commands, actions, event loop, diagnostics, hotkey, indexing, process control, overlay rows, and search session behavior
- **🟡 `runtime.rs` is still a central entrypoint:** It remains the top-level dispatcher, but no longer owns the full runtime implementation
- **🟡 Good logging/diagnostics** infrastructure
- **🟡 Query profiling** is well-implemented

### `discovery.rs` (1,767 lines)
- **🟡 Windows Search integration** via PowerShell COM is fragile
- **🟡 Good exclusion policy** and change stamp tracking
- **✅ Well-factored provider trait**

### `search.rs` (759 lines)
- **✅ Clean scoring system** with good test coverage
- **✅ Top hit confidence guard** is clever
- **🟡 Could benefit from SIMD scoring** for large datasets

### `plugin_sdk.rs` (249 lines)
- **🔴 Far too minimal** — needs WASM runtime, manifest parsing, store protocol
- **🟡 Trait design is sound** but incomplete

### `index_store.rs` (296 lines)
- **✅ Clean migration system** with versioned schema
- **✅ Proper query memory table** for personalization

---

## 6. Architecture Decision Records

The following decisions have been made or are pending:

1. **Plugin runtime** — Pending (WASM vs Lua vs embedded Python)
2. **AI provider strategy** — Deferred (out of scope, privacy-first posture)
3. **Window management API** — Pending (no code yet)
4. **Async runtime** — **Decided:** sync `std::thread` (tokio rejected — unnecessary dependency weight)
5. **Search index** — **Decided:** Everything SDK primary, walkdir fallback, USN Journal rejected
6. **Overlay rendering** — **Decided:** WebView2 (tao + wry), HTML/CSS/JS via custom protocol
7. **Backdrop material** — **Decided:** Acrylic via `window-vibrancy` crate (Mica dropped)
