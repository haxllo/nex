# Windows Security Release Checklist

Use this checklist before publishing any Windows release (`stable` or `beta`).

## 1. Build Provenance

- Build from clean `master` state (no uncommitted local changes).
- Tag/version in artifact names matches release notes.
- Artifact set is complete:
  - `nex-<version>-windows-x64.zip`
  - `nex-<version>-windows-x64-manifest.json`
  - `nex-<version>-windows-x64-setup.exe`
- Manifest includes integrity data:
  - `artifacts.zip.sha256`
  - `artifacts.setup.sha256`

## 2. Runtime Safety Verification

- Launch target validation still enforced (no raw shell command concatenation path introduced).
- Missing/stale launch targets show user-visible error and do not crash runtime.
- Runtime remains single-instance and does not spawn unmanaged background duplicates.
- Uninstall flow terminates runtime and removes startup registration.

## 3. Data and Privacy Boundaries

- No telemetry enabled by default.
- No raw query text exfiltration path introduced.
- Logs remain local under `%APPDATA%\Nex\logs`.
- Logs do not include secrets/tokens/passwords.
- Config remains local under `%APPDATA%\Nex\config.toml`.

## 4. Dependency and Test Gates

Required pass gates:

```powershell
cargo check -p nex
cargo test -p nex
pnpm vitest --run
cargo test -p nex --test perf_query_latency_test -- --exact warm_query_p95_under_15ms
```

Security-focused checks:

- Review dependency updates in lockfiles for suspicious/unreviewed changes.
- Confirm no new high-risk crates/packages were added without justification.
- Re-run manual launcher flow to validate:
  - open/focus
  - query + launch
  - click-outside close
  - uninstall cleanup

## 5. Signing Posture

Current posture:

- Unsigned binaries are allowed for current phase, with expected SmartScreen warnings.
- Integrity minimum bar still applies: updater must verify manifest checksum before apply.

If signing is enabled for a release:

- Signature verification succeeds (`signtool verify` / `Get-AuthenticodeSignature`).
- Timestamping is present.
- Signing failures block release publication.

## 6. Release Decision

Release is blocked if any item above fails.

If a non-blocking known limitation remains, it must be listed in release notes with:

- impact
- workaround
- planned fix milestone
