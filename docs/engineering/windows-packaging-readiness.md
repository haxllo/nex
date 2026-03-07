# Windows Packaging Readiness

## Artifact Convention

- Output root: `artifacts/windows/`
- Staging directory: `artifacts/windows/swiftfind-<version>-windows-x64-stage/`
- Zip artifact: `artifacts/windows/swiftfind-<version>-windows-x64.zip`
- Build manifest: `artifacts/windows/swiftfind-<version>-windows-x64-manifest.json`

## Version Stamping Policy

- Preferred: explicit `-Version` argument in packaging command.
- Fallback: `git describe --tags --always`.
- If git metadata is unavailable: `0.0.0-local`.

## Packaging Command

Run on Windows PowerShell:

```powershell
scripts/windows/package-windows-artifact.ps1 -Version "0.5.0" -Channel "stable"
```

Build Windows `Setup.exe` (Inno Setup) from the staged artifact:

```powershell
scripts/windows/package-windows-installer.ps1 -Version "0.5.0" -Channel "stable"
```

Signed packaging (PFX):

```powershell
scripts/windows/package-windows-artifact.ps1 `
  -Version "0.5.0" `
  -Sign `
  -CertPath "C:\secure\certs\swiftfind-signing.pfx" `
  -CertPassword "<PFX_PASSWORD>"
```

Expected outputs:

- `artifacts/windows/swiftfind-0.5.0-windows-x64.zip`
- `artifacts/windows/swiftfind-0.5.0-windows-x64-manifest.json`
- `artifacts/windows/swiftfind-0.5.0-windows-x64-stage/`
- `artifacts/windows/swiftfind-0.5.0-windows-x64-setup.exe`

## Included Payload

Paths below are inside the staged artifact layout:

- `bin/swiftfind-core.exe`
- `assets/swiftfinder.svg`
- `docs/windows-runtime-validation-checklist.md`
- `docs/release-notes-template.md`
- `scripts/install-swiftfind.ps1`
- `scripts/uninstall-swiftfind.ps1`
- `scripts/update-swiftfind.ps1`

## Signing Requirements

- `signtool.exe` available in `PATH` (Windows SDK).
- Valid Authenticode certificate (`.pfx`).
- Timestamp server reachable (default: `http://timestamp.digicert.com`).

If `-Sign` is enabled, packaging script will:

1. Sign `target/release/swiftfind-core.exe`
2. Verify signature via `signtool verify`
3. Validate status via `Get-AuthenticodeSignature`

If any of these fail, packaging fails.

## Installer-Prep Notes

- This milestone prepares a deterministic zip payload and manifest.
- Manifest includes channel and SHA256 integrity data for zip/setup artifacts.
- Inno Setup consumes the staging directory directly via `scripts/windows/swiftfind.iss`.
- Keep staging layout stable to avoid installer script churn.
- Inno `[UninstallRun]` entries include `RunOnceId` values to prevent duplicate execution/warnings.
- Uninstall path performs graceful quit, startup key cleanup, and hard-stop fallback for leftover runtime process.

## Inno Setup Requirements

- Install Inno Setup 6:

```powershell
winget install JRSoftware.InnoSetup -e
```

- Default compiler path used by wrapper:
  - `C:\Program Files (x86)\Inno Setup 6\ISCC.exe`
- Override compiler path if needed:

```powershell
scripts/windows/package-windows-installer.ps1 `
  -Version "0.5.0" `
  -InnoCompiler "D:\Tools\Inno Setup 6\ISCC.exe"
```

## GitHub Release Update

After packaging, upload installer to the existing tag/release:

```powershell
gh release upload v0.5.0 `
  artifacts/windows/swiftfind-0.5.0-windows-x64-setup.exe `
  --clobber
```

Channel and rollout policy:

- See `docs/engineering/windows-update-rollout-strategy.md` for `stable`/`beta` policy, upgrade expectations, and rollback rules.

## Local Install / Uninstall Commands

Run on Windows PowerShell:

```powershell
scripts/windows/install-swiftfind.ps1
```

End-user note:

- Rust/Cargo is **not required** when installing from the packaged release zip.
- The installer script uses the prebuilt `bin/swiftfind-core.exe` from the zip.

Optional flags:

- `-BuildFromSource` (developer mode; builds with Cargo if prebuilt exe is not present)
- `-SourceExe "<path>"` (explicit exe path override)
- `-StartAfterInstall:$false` (install only)
- `-InstallScope CurrentUser|AllUsers` (default `CurrentUser`; `AllUsers` requires elevated PowerShell)
- `-InstallRoot "<path>"` (explicit install path override)

Uninstall:

```powershell
scripts/windows/uninstall-swiftfind.ps1
```

Optional:

- `-PurgeUserData` (also removes `%APPDATA%\SwiftFind`)
- `-InstallScope CurrentUser|AllUsers` (default `CurrentUser`; `AllUsers` requires elevated PowerShell)
- `-InstallRoot "<path>"` (explicit install path override)
