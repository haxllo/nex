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

- [ ] Docs impact: did this change make any document wrong? If so, that document was updated in this PR ([docs/development/maintaining-docs.md](docs/development/maintaining-docs.md) maps each type of change to its document)
- [ ] If CI's docs gate was bypassed, this body contains `NEX_ALLOW_STALE_DOCS=1:` and the reason
- [ ] Process roles preserved (UI never indexes, indexer never renders, helper is a broker)
- [ ] Commits use the repo-owner identity and contain no AI/bot names or trailers
