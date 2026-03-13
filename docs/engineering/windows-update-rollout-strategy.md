# Windows Update and Rollout Strategy

This document defines Nex Windows update behavior for Phase 3.

## Scope

- Applies to packaged Windows releases (`setup.exe` + zip).
- Keeps runtime lightweight: no always-on auto-update service.
- Uses release-channel policy and operator-driven rollout.

## Current Update Model

Current behavior is operator-driven update (no always-on updater service):

1. Run `scripts/windows/update-nex.ps1` with channel `stable` or `beta`.
2. Script resolves the target GitHub release and downloads:
   - `nex-<version>-windows-x64-manifest.json`
   - `nex-<version>-windows-x64-setup.exe`
3. Script verifies installer SHA256 against manifest before installation.
4. Script snapshots current install directory for rollback safety.
5. Installer applies update in place under `%LOCALAPPDATA%\Programs\Nex`.
6. On failure, script restores previous snapshot automatically.

No background update polling is performed by `nex.exe`.

## Release Channels

Two channels are supported at release-management level:

1. `stable`
- General users.
- Must pass full automated gates and full Windows manual lifecycle validation.

2. `beta`
- Early testers.
- Can ship faster, but still requires core runtime and lifecycle checks.

Channel is communicated in release tags/notes and consumed by `update-nex.ps1`.

## Versioning and Publishing

Publishing baseline:

1. Build artifacts:
- `artifacts/windows/nex-<version>-windows-x64.zip`
- `artifacts/windows/nex-<version>-windows-x64-manifest.json`
- `artifacts/windows/nex-<version>-windows-x64-setup.exe`
2. Ensure manifest contains:
- `channel`
- `artifacts.zip.sha256`
- `artifacts.setup.sha256`
3. Publish GitHub release with clear channel label.
4. Attach validation evidence summary and known limitations.

Recommended tag style:

- Stable: `vX.Y.Z`
- Beta: `vX.Y.Z-beta.N`

## Upgrade and Rollback Policy

Upgrade:

- Supported path is `update-nex.ps1` channel/version update.
- Update must preserve `%APPDATA%\Nex` config/index/logs.
- Installer apply is blocked if manifest checksum verification fails.

Rollback:

- Automatic rollback occurs when update apply/verify fails.
- Manual rollback remains available by reinstalling previous known-good setup.
- Verify runtime process, hotkey, query/launch/close flow immediately after rollback.

Rollback validation is required for every release candidate before broad stable rollout.

## Future Auto-Update Direction (Not Implemented Yet)

Future background auto-update can be considered only if all constraints are met:

- no persistent always-on updater process in idle runtime path
- explicit user consent and clear channel selection
- signed payload verification before install
- rollback-safe failure behavior

Until then, script-driven, user-triggered updates remain the official update path.
