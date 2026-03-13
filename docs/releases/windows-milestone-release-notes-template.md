# Nex Windows Milestone Release Notes

## Release Metadata

- Version: `<version>`
- Build date (UTC): `<timestamp>`
- Artifact: `artifacts/windows/nex-<version>-windows-x64.zip`
- Manifest: `artifacts/windows/nex-<version>-windows-x64-manifest.json`

## Highlights

- Windows hotkey runtime support behind `cfg(windows)`.
- Runtime discovery providers for Start Menu apps and configured file roots.
- Typed request/response transport boundary with stable serializable errors.
- Launcher UI wiring for search, keyboard selection, launch, and visible error states.
- Config load/save on stable app-data path with migration-safe defaults.

## Validation Summary

- Automated gates passed:
  - `./node_modules/.bin/vitest --run`
  - `cargo test -p nex`
  - `cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms`
- Windows runtime smoke harness status: `<pass/fail>`
- Manual E2E checklist status: `<pass/fail>`
- Manual evidence file: `artifacts/windows/manual-e2e-result.json`

## Known Limitations

- `<list open items requiring post-milestone follow-up>`

## Rollback Plan

1. Stop running instance.
2. Restore previous zip artifact.
3. Re-run runtime smoke harness and minimal launcher flow checks.
