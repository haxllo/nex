## Summary

<!-- What changed and why (one or two sentences). -->

## Linked Linear issue

<!-- Required when this PR changes apps/, scripts/, or manifests/. -->

CIQ-

## Verification

- [ ] Release build: `cargo build --release --bin NexUIProcess --bin NexHelper --bin NexIndexerProcess`
- [ ] Relevant gate run (list which, or why not): <!-- e.g. `cargo test -p nex-indexer --test process_isolation_test` -->
- [ ] Not claiming any check that was not actually run

## Checklist

- [ ] Process roles preserved (UI never indexes, indexer never renders, helper is a broker)
- [ ] Commits use the repo-owner identity and contain no AI/bot names or trailers
