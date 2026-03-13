# Sprint 4 Verification Record

## Required Gates

Executed on current `master` head:

- `./node_modules/.bin/vitest --run`
- `cargo test -p nex`
- `cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms`

Status: PASS

## Added Windows Runtime Check

Executed on non-Windows host:

- `cargo test -p nex --test windows_runtime_smoke_test -- --exact non_windows_fallback_smoke_still_roundtrips`

Status: PASS (fallback path)

## Windows Host Confirmation Still Required

- `cargo test -p nex --test windows_runtime_smoke_test` on `windows-latest` and local Windows host with `NEX_WINDOWS_RUNTIME_SMOKE=1`.
- Manual checklist in `docs/engineering/windows-runtime-validation-checklist.md`.
- Packaging script dry-run on Windows: `scripts/windows/package-windows-artifact.ps1`.
