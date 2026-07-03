<!-- generated-by: gsd-doc-writer -->
# Testing

Nex uses **Cargo's built-in test framework** for all Rust tests (unit and integration) and **Vitest** for a lightweight JavaScript scaffold gate. Tests are organized by scope into unit tests, integration tests, performance tests, and a smoke test.

---

## Test Framework and Setup

| Layer | Framework | Config |
|---|---|---|
| Rust (all tests) | `#[test]` via Cargo | `Cargo.toml` — dev-dependencies: `tempfile` |
| JS scaffold gate | Vitest | `vitest.config.ts` at project root |
| Performance | `#[test]` via Cargo | Shared logic in `tests/perf/` directory |

No global setup step is required beyond `cargo test` or `vitest --run`. The Rust test suite uses `tempfile` for filesystem isolation and `nex_core::config::Config::default()` for reproducible test configurations.

---

## Running Tests

### Full Rust Test Suite

```bash
cargo test -p nex
```

Runs all unit tests (inline `#[cfg(test)]` modules in `apps/core/src/`) and integration tests (files in `apps/core/tests/`).

### Single Integration Test File

```bash
cargo test -p nex --test config_test
```

Replace `config_test` with any integration test name (without `.rs` extension).

### Single Test Function

```bash
cargo test -p nex -- accepts_default_config
```

### Performance Test (exact gate)

```bash
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
```

Validates that warm fuzzy-search latency stays under 15ms P95 with a 10,000-item index. This is the most resource-intensive test and runs as its own CI step.

### Windows Runtime Smoke Test

```bash
NEX_WINDOWS_RUNTIME_SMOKE=1 cargo test -p nex --test windows_runtime_smoke_test
```

Requires the `NEX_WINDOWS_RUNTIME_SMOKE=1` environment variable. On non-Windows platforms, it runs a fallback test that validates the code compiles and the transport layer round-trips without native hotkey registration.

### JS Scaffold Gate

```bash
pnpm install && vitest --run
```

Runs a single test (`tests/smoke/scaffold.test.ts`) that verifies critical Rust entry points and bundled font assets exist. This is a fast file-existence check, not a functional test.

---

## Test Categories

### Unit Tests (Inline)

Found in `#[cfg(test)] mod tests { ... }` blocks throughout `apps/core/src/`. Each module tests its own functions in isolation. Key modules with inline tests:

| Module | File | Scope |
|---|---|---|
| `action_registry` | `src/action_registry.rs` | Action registration and lookup |
| `calculator` | `src/calculator.rs` | Expression evaluation, precedence, functions |
| `clipboard_history` | `src/clipboard_history.rs` | Clipboard history storage and retrieval |
| `config` | `src/config.rs` | Default values, validation, file loading |
| `core_service` | `src/core_service.rs` | Core service CRUD and search orchestration |
| `everything_bridge` | `src/everything_bridge.rs` | Everything SDK IPC protocol |
| `file_watcher` | `src/file_watcher.rs` | File system change detection |
| `file_watcher_consumer` | `src/file_watcher_consumer.rs` | File event processing queue |
| `logging` | `src/logging.rs` | Log initialization and format |
| `overlay_state` | `src/overlay_state.rs` | UI state snapshot construction |
| `plugin_sdk` | `src/plugin_sdk.rs` | Plugin execution sandbox |
| `query_dsl` | `src/query_dsl.rs` | Search query parsing |
| `runtime` | `src/runtime.rs` | Runtime option parsing |

### Integration Tests (`tests/` directory)

14 test files in `apps/core/tests/` that exercise cross-module behavior:

| Test file | What it covers |
|---|---|
| `action_executor_test.rs` | Action dispatch and execution |
| `config_test.rs` | Config load, save, validation, migration |
| `contract_test.rs` | Request/response serialization round-trips |
| `core_service_test.rs` | Full index lifecycle via CoreService |
| `discovery_test.rs` | Search item discovery from filesystem |
| `hotkey_test.rs` | Hotkey string parsing and validation |
| `hotkey_runtime_test.rs` | Hotkey registration lifecycle |
| `index_store_test.rs` | SQLite index operations |
| `search_test.rs` | Search ranking and result ordering |
| `settings_test.rs` | Settings persistence |
| `startup_test.rs` | Launch-at-startup configuration |
| `transport_test.rs` | JSON transport layer |
| `perf_query_latency_test.rs` | Performance gate (includes `tests/perf/query_latency_test.rs`) |
| `windows_runtime_smoke_test.rs` | End-to-end runtime smoke (requires env var) |

### Performance Test

Defined in `tests/perf/query_latency_test.rs` and included by `apps/core/tests/perf_query_latency_test.rs` via `include!()`. Benchmarks fuzzy search by:

1. Seeding an index with 10,000 items + 1 target item
2. Running 30 warm-up searches
3. Collecting 5 batches of 80 samples each
4. Computing the P95 latency for each batch
5. Asserting the median P95 is ≤ 15ms

### WS_POPUP: Windows Runtime Smoke Test

`windows_runtime_smoke_test.rs` validates:
- Hotkey registration/unregistration via OS APIs
- Transport layer JSON round-trip through `handle_json`
- CoreService initialization and search in a realistic pipeline

Guarded by `NEX_WINDOWS_RUNTIME_SMOKE=1` to prevent accidental execution on CI agents without a real desktop session.

---

## CI Pipeline

Defined in `.github/workflows/ci.yml`. Runs on `push` and `pull_request` against `windows-latest`.

**Execution order (sequential gates):**

1. **Vitest scaffold gate** — `./node_modules/.bin/vitest --run`
   - Fast file-existence check (~2s). Quick failure if build structure is broken.
2. **Rust test gate** — `cargo test -p nex`
   - All unit and integration tests except perf and smoke gates.
3. **Rust perf gate** — `cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms`
   - Isolated latency check with 15ms P95 budget.
4. **Windows runtime smoke gate** — `cargo test -p nex --test windows_runtime_smoke_test`
   - Runs with `NEX_WINDOWS_RUNTIME_SMOKE=1` set in CI env.

All gates run on the same `windows-latest` runner. The `dtolnay/rust-toolchain@stable` action automatically detects the native target.

---

## Adding New Tests

### Unit Test (inline)

Add a `#[cfg(test)] mod tests { ... }` block at the end of the source file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn my_new_feature_works() {
        let result = my_function("test input");
        assert!(result.is_ok());
    }
}
```

### Integration Test

Create a new file in `apps/core/tests/` with the naming convention `<name>_test.rs`. The file should `use nex_core::...` to access library internals:

```rust
use nex_core::some_module::some_function;

#[test]
fn my_integration_scenario() {
    // test logic here
}
```

### Performance Test

Add the test logic to `tests/perf/` and create a thin integration test file in `apps/core/tests/` that includes it via `include!()`:

```rust
// apps/core/tests/my_perf_test.rs
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/perf/my_perf_test.rs"
));
```

This pattern keeps the perf test source outside the crate tree so it does not affect normal `cargo test` compilation.

---

## Coverage Requirements

No coverage thresholds are configured. The project does not use `tarpaulin`, `grcov`, or any code coverage tooling in CI. Tests rely on functional correctness rather than coverage metrics.

---

## Known Limitations

- **Windows-only modules** (`runtime_loop.rs`, `everything_bridge.rs`, `runtime_overlay_rows.rs`) have limited test coverage because their functionality depends on OS APIs. The Everything bridge tests validate IPC protocol parsing with mock data but do not require an Everything installation.
- **Smoke test guard**: `windows_runtime_smoke_test` skips silently when `NEX_WINDOWS_RUNTIME_SMOKE` is not set. CI sets it globally, so local run failures may go unnoticed if the env var is missing.
- **Performance test variability**: The 15ms P95 budget in `warm_query_p95_under_15ms` is calibrated for `windows-latest` GitHub runners. Local machines with different hardware may see flaky results near the threshold.
- **Single-threaded test execution**: `cargo test -p nex` runs tests in parallel by default. Performance-sensitive tests may need `--test-threads=1` for stable timing.
