# Testing and quality gates

Nex has five gates. CI runs all of them on Windows; running the same five locally
before a pull request is what keeps a red build from being a surprise.

## The gates, in order

| Gate | Command |
|---|---|
| JavaScript assets | `pnpm exec vitest --run` |
| Release build | `cargo build --locked --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess` |
| Rust tests | `cargo test --locked -p nex` |
| Query latency | `cargo test --locked -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms` |
| Runtime smoke | `cargo test --locked -p nex --test windows_runtime_smoke_test` |

CI also verifies the three binaries exist before running the test gates, and, on a
pull request, checks that a `CIQ-<n>` id appears in the title or body if any file
under `apps/`, `scripts/`, or `manifests/` changed.

## What each gate covers

**JavaScript assets.** Vitest runs the TypeScript tests under `tests/js/`, which
cover the parts of the page that can be exercised without a browser: chat history
and action rendering, provider configuration, the media clock, and What's New
rendering. These are unit tests over extracted logic, not a page test.

**Release build.** A release build is the gate, not a debug build. Two reasons:
the release profile is what ships, and several modules only compile under it.

**Rust tests.** `cargo test -p nex` covers unit tests inside the crate and the
integration tests under `apps/core/tests/`. That includes config parsing and
migration, the search contract, the index store, discovery, hotkey runtime, the
transport contract, settings, startup, and search reliability.

For a single module while iterating:

```bash
cargo test -p nex --lib chat
cargo test -p nex --lib config
```

**Query latency.** A performance gate rather than a correctness test: it builds a
realistic corpus, runs warm queries, and asserts the median batch p95 stays under
15 ms. It runs by exact name because running the whole perf file would include
benchmarks that are not gates.

**Runtime smoke.** Registers a hotkey and round-trips the transport. It is skipped
unless `NEX_WINDOWS_RUNTIME_SMOKE=1` is set, which CI sets and a local shell does
not.

## Cross-process tests

The process isolation test starts a real indexer, talks to it over its socket, and
verifies snapshot publishing, restart recovery, protocol version rejection, and
shutdown. It is opt-in:

```bash
NEX_RUN_PROCESS_ISOLATION_TEST=1 cargo test -p nex-indexer --test process_isolation_test
```

It lives in the indexer's package because it needs that binary.

## What cannot be tested normally

**The helper.** `NexHelper.exe` embeds a `requireAdministrator` manifest, so the
test harness exits with `The requested operation requires elevation`. There is no
way to run `cargo test -p nex-helper`. Verify helper behaviour by hand.

**The glass material.** The panel calls `SetWindowDisplayAffinity` with
`WDA_EXCLUDEFROMCAPTURE`, so every screenshot method returns the desktop behind the
window. There is also no synthetic input path: the hotkey hooks drop injected
events.

Two techniques replace screenshots here:

```bash
cargo test -p nex --lib refraction -- --ignored
```

The ignored tests read back the real effect graph on the GPU at panel size, which
is the only automated evidence about the material. For anything the probes do not
cover, mirror the maths in a separate harness, or ask a person to look at the
screen and say what they see. A phone photograph of the display is real evidence;
a screenshot is not.

**Page layout.** Layout and contrast claims should be measured, not read off the
stylesheet. Render the page outside the app, drive it with a browser automation
tool, and assert on `getBoundingClientRect` values. This works because the page is
ordinary HTML and CSS; only the native material behind it is out of reach.

## Test-writing notes

- Test services must own a throwaway index. A service built from `Config::default()`
  opens the developer's real index and makes the suite interfere with itself.
- Tantivy tests contend for a shared writer under load. A commit followed by a
  garbage collection can exceed a reader deadline in the full suite while passing
  alone. If a Tantivy test fails only in a full run, rerun it alone before
  assuming a real failure.
- Anything asserting a process-global fact should skip when that fact does not hold
  on the machine running the tests, rather than fail.

## Next

- [validation-checklist.md](validation-checklist.md) for the manual pass before a release
- [onboarding.md](onboarding.md) for build setup and common build failures
- [overlay-and-glass.md](overlay-and-glass.md) for why capture and synthetic input fail
- [releasing.md](releasing.md) for what happens after the gates pass
