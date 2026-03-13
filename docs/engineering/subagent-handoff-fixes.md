# Subagent Handoff: Review Fixes and Guardrails

Apply these standards in all follow-up work:

1. Do not use filtered cargo commands that can return `0 tests`.
- Use `cargo test -p nex --test <file> -- --exact <test_name>`.

2. Do not hardcode performance values in tests.
- Perf tests must measure runtime and assert budget.

3. Avoid test-environment-specific attribute hacks.
- Test behavior directly instead of forcing implementation-specific attributes.

4. Keep smoke tests meaningful.
- Smoke tests should assert rendered UI state or behavior, not placeholder string checks.

## Required Commands (must pass)

```bash
./node_modules/.bin/vitest --run
cargo test -p nex
```

## Verified Targets

```bash
cargo test -p nex --test hotkey_test -- --exact parses_default_hotkey
cargo test -p nex --test index_store_test -- --exact inserts_and_reads_search_item
cargo test -p nex --test search_test -- --exact typo_query_returns_expected_match
cargo test -p nex --test action_executor_test -- --exact rejects_empty_launch_path
cargo test -p nex --test config_test -- --exact rejects_max_results_out_of_range
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
```
