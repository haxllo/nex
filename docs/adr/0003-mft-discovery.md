# ADR-0003: Read the NTFS master file table instead of walking directories

## Status

Accepted.

## Date

2026-11-05

## Context

A launcher that indexes a whole drive has to enumerate every file. Directory
enumeration asks the filesystem for one folder at a time, which turns a whole-drive
index into tens of thousands of round trips. The result is a scan measured in
minutes, which users experience as an index that never finishes.

Nex had already tried a third-party index service and a change-journal approach
before this decision.

An elevated process is available (see [0002](0002-three-process-split.md)), and
reading the master file table requires the privilege it has.

## Decision

On NTFS volumes, read the NTFS master file table directly and rebuild paths from
file-reference and parent-reference pairs. Apply the configured roots, exclusions,
depth limit, and item caps to the resulting records. Keep directory walking as the
fallback, and use the USN change journal to apply deltas rather than rescanning.

Selection is per root and automatic: MFT when the root's item cap is large enough
for the scan to pay off, walkdir otherwise. `file_discovery_backend` can force
either.

## Alternatives considered

### Everything's SDK

Use the Everything index service as the file source.

Rejected. It made the launcher depend on a third-party service being installed and
running, it had no incremental story of its own, and it duplicated the store that
Nex needed anyway. The configuration string still loads for old configs and is
treated as `auto`; the backend itself is gone.

### USN journal as the primary source

Read the change journal alone and reconstruct the file system from change records.

Rejected as a primary source. The journal is a change log, not a snapshot: it does
not describe files that already existed before the journal's window, so a fresh
install would have to walk the disk first anyway. It is used where it fits, as the
delta path on top of an MFT snapshot.

### Directory walking only

Keep the simple, portable approach.

Retained as the fallback rather than the primary path. It remains correct for
non-NTFS volumes, locked BitLocker volumes, and systems where the elevated helper
is not available.

## Consequences

- A whole-drive scan becomes a sequential metadata read. Measured on the
  development machine at a 40,000 item per-root cap: 1.13 million MFT records from
  one volume in about 16 seconds, and 718,000 records from a slower disk in about
  45 seconds.
- **The elevated helper is on the critical path for indexing.** This is the largest
  cost of the decision. When the helper is unavailable, discovery waits briefly and
  then falls back to a directory walk for the cycle. The failure mode is a slower
  scan, not an empty index.
- The scan runs at background priority so a startup scan cannot starve the desktop
  or the first panel show.
- Paths must be reconstructed from parent references, which needs a bounded walk up
  the parent chain and a rule for records whose parent is unreachable. Deleted and
  renamed subtrees need reconciliation.
- Only NTFS benefits. The fallback is not dead code and must keep working.
