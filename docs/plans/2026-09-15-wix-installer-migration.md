# WiX Installer Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace current Inno Setup installer with a WiX-built Windows installer while preserving Nex's staged files, startup behavior, upgrade/uninstall behavior, release artifact names, and user data.

**Architecture:** Keep `package-windows-artifact.ps1` as source-of-truth for staged release contents. Replace `nex.iss` compilation with a WiX MSI project and a Burn bootstrapper that produces existing `nex-<version>-windows-x64-setup.exe` output. First implementation targets current-user installation on the developer machine, then adds migration handling for existing Inno installs before release use.

**Tech Stack:** WiX Toolset v4, MSI, Burn bootstrapper, PowerShell 7, Cargo release builds, existing staged artifact and manifest format.

---

## Constraints And Decisions

- Do not modify current Inno packaging until WiX output passes local install/upgrade/uninstall tests.
- Preserve `nex-<version>-windows-x64-setup.exe` filename so updater and release scripts continue working.
- Preserve `%APPDATA%\Nex` and `%APPDATA%\SwiftFind` by never deleting them during normal uninstall or upgrade.
- Keep scheduled-task creation owned by Nex runtime; installer only removes stale `NexHelperV2` during uninstall/migration.
- Start with per-user MSI/Bundle behavior for local testing. Do not claim all-users parity until explicitly tested.
- Use stable WiX `UpgradeCode`; versioned `ProductCode` behavior must support major upgrades.

## Task 1: Add WiX Toolchain Skeleton

**Files:**
- Create: `installer/wix/NexMsi.wixproj`
- Create: `installer/wix/NexBundle.wixproj`
- Create: `installer/wix/Product.wxs`
- Create: `installer/wix/Bundle.wxs`
- Create: `installer/wix/README.md`

**Steps:**

1. Pin WiX Toolset v4 package references in both projects.
2. Define x64 Release configuration and deterministic output directory properties.
3. Define bundle version property passed from command line as `AppVersion`.
4. Define stable bundle/Msi upgrade identifiers; document generated IDs and ownership.
5. Add README commands for local prerequisite installation and project build.
6. Run `dotnet build installer/wix/NexMsi.wixproj -c Release -p:AppVersion=2.19.0`.
7. Run `dotnet build installer/wix/NexBundle.wixproj -c Release -p:AppVersion=2.19.0`.
8. Commit: `build: add WiX installer projects`.

## Task 2: Author MSI File Layout

**Files:**
- Modify: `installer/wix/Product.wxs`
- Create: `installer/wix/Files.wxs`
- Create: `installer/wix/Registry.wxs`

**Steps:**

1. Author x64 package metadata for Nex.
2. Install staged files from `StageDir` into a per-user install directory.
3. Include:
   - `bin/Nex.exe`
   - `bin/NexHelper.exe`
   - `bin/Everything64.dll`
   - `assets/**`
   - `scripts/update-nex.ps1`
4. Add Start Menu shortcut to `Nex.exe --background`.
5. Add optional desktop shortcut through MSI feature/property.
6. Add HKCU startup value matching Inno behavior:
   - Value name: `Nex`
   - Value data: installed `Nex.exe --background`
7. Add cleanup for legacy files `nex-core.exe` and `swiftfind-core.exe`.
8. Add uninstall metadata and ARP display icon.
9. Build MSI from a temporary stage directory containing known fixture files.
10. Commit: `feat: author Nex WiX MSI layout`.

## Task 3: Add Runtime Shutdown Custom Action

**Files:**
- Create: `installer/wix/StopRuntime.ps1`
- Modify: `installer/wix/Product.wxs`
- Create: `installer/wix/CustomActions.wxs`

**Steps:**

1. Implement bounded shutdown request for staged/current Nex executable using `--quit`.
2. Wait briefly for process exit.
3. Remove `NexHelperV2` task if present.
4. Fall back to targeted process termination only when the process remains alive.
5. Ensure custom action does not terminate unrelated executables by image name alone.
6. Schedule action before file replacement and during uninstall.
7. Ensure silent install/uninstall does not display PowerShell windows.
8. Test upgrade while Nex and NexHelper are running.
9. Commit: `feat: stop Nex during WiX install lifecycle`.

## Task 4: Add Inno Installation Migration

**Files:**
- Create: `installer/wix/MigrateInno.ps1`
- Modify: `installer/wix/Bundle.wxs`
- Modify: `installer/wix/README.md`

**Steps:**

1. Detect current-user and machine Inno uninstall keys for the existing Nex AppId.
2. Read `InstallLocation`, `DisplayIcon`, and `UninstallString`.
3. Resolve old runtime path without trusting arbitrary command arguments.
4. Request old Nex shutdown.
5. Run old uninstaller silently with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`.
6. Fail migration clearly if uninstall returns nonzero; do not continue with ambiguous duplicate installs.
7. Preserve user data directories.
8. Add Burn detection condition so migration runs only when old Inno install exists and WiX install is not already present.
9. Test migration from both HKCU and HKLM registry layouts.
10. Commit: `feat: migrate existing Inno installations`.

## Task 5: Author Burn Bootstrapper

**Files:**
- Modify: `installer/wix/Bundle.wxs`
- Create: `installer/wix/BundleBootstrapperApplication.wxs` only if needed

**Steps:**

1. Chain migration prerequisite, MSI package, and optional launch action.
2. Pass `AppVersion`, `StageDir`, and install-location properties into MSI.
3. Preserve setup EXE output name `nex-<version>-windows-x64-setup.exe`.
4. Add per-user install default and explicit scope behavior only if WiX authoring supports it without unsafe mixed-context behavior.
5. Add downgrade blocking.
6. Add uninstall/repair/modify support.
7. Build bundle locally and inspect generated version metadata.
8. Commit: `feat: build Nex WiX bootstrapper`.

## Task 6: Replace Packaging Script Behind Feature Flag

**Files:**
- Modify: `scripts/windows/package-windows-installer.ps1`
- Create: `scripts/windows/package-windows-installer-inno.ps1`
- Modify: `AGENTS.md`
- Modify: `docs/guides/development.md`
- Modify: `docs/engineering/windows-packaging-readiness.md`

**Steps:**

1. Move current Inno implementation unchanged into compatibility script.
2. Add WiX script parameters:
   - `Version`
   - `Channel`
   - `OutputRoot`
   - `StageDir`
   - `WixConfiguration`
   - `WixToolsetPath` if required
3. Build artifact stage if absent using existing artifact script.
4. Build MSI and Burn bundle from stage.
5. Copy/rename bundle to expected setup filename.
6. Update existing manifest setup name, size, and SHA256 fields.
7. Fail if output version or architecture does not match requested values.
8. Add `-InstallerEngine inno|wix` with default `wix` only after local validation is complete.
9. Keep `-InstallerEngine inno` rollback path during migration period.
10. Commit: `build: route Windows installer packaging through WiX`.

## Task 7: Update Release And Updater Contracts

**Files:**
- Modify: `scripts/windows/update-nex.ps1`
- Modify: `docs/engineering/windows-update-rollout-strategy.md`
- Modify: `docs/engineering/windows-security-release-checklist.md`
- Modify: `manifests/h/Haxllo/Nex/*/Haxllo.Nex.installer.yaml` only for future versions

**Steps:**

1. Confirm updater accepts Burn setup EXE silently.
2. Preserve installer checksum verification against manifest.
3. Confirm updater waits for setup process completion.
4. Confirm rollback logic restores the previous installation if bundle fails.
5. Update silent install switches from Inno-specific switches to Burn switches where needed.
6. Document WiX/Burn installer type and commands.
7. Commit: `docs: document WiX release flow`.

## Task 8: Add Local Installer Validation Script

**Files:**
- Create: `scripts/windows/test-wix-installer.ps1`
- Create: `docs/engineering/windows-wix-validation.md`

**Steps:**

1. Validate bundle file exists and reports expected version.
2. Install into isolated local test directory/profile.
3. Verify Nex.exe, NexHelper.exe, Everything64.dll, assets, scripts, shortcuts, and HKCU startup value.
4. Start Nex, verify `--status`, then uninstall/upgrade while runtime is active.
5. Verify runtime process and `NexHelperV2` task are removed.
6. Verify `%APPDATA%\Nex` survives uninstall.
7. Verify old Inno install migration path using a disposable registry/install fixture.
8. Add `-KeepInstall` and `-SkipRuntime` controls for safe developer iteration.
9. Commit: `test: add WiX installer validation workflow`.

## Task 9: Final Cutover

**Files:**
- Modify: `scripts/windows/package-windows-installer.ps1`
- Delete or retain: `scripts/windows/nex.iss` based on rollback policy
- Modify: `AGENTS.md`
- Modify: `docs/guides/development.md`

**Steps:**

1. Run full local WiX validation on clean install.
2. Run upgrade from latest Inno release.
3. Run upgrade from previous WiX build.
4. Run uninstall with data preservation.
5. Run silent install and silent uninstall.
6. Run artifact packaging and verify manifest hashes.
7. Run release build and package workflow.
8. Remove Inno default path only after validation passes.
9. Keep Inno script in repository for one release cycle if rollback is required.
10. Commit: `release: cut over Windows installer to WiX`.

## Required Validation Matrix

- Fresh per-user install.
- Fresh all-users install, if supported by final authoring.
- Upgrade from Inno current release.
- Upgrade from previous WiX build.
- Downgrade rejection.
- Repair install.
- Silent install.
- Silent uninstall.
- Nex running during install.
- NexHelper running during install.
- Existing `NexHelperV2` scheduled task.
- Existing startup registry value.
- Existing `%APPDATA%\Nex` config and SQLite index.
- Missing WebView2 Runtime behavior.
- Failed install rollback.
- Manifest setup hash and release asset names.

## Rollback

Until Task 9 completes, invoke the old path explicitly:

```powershell
pwsh -ExecutionPolicy Bypass -File scripts/windows/package-windows-installer.ps1 -InstallerEngine inno -Channel stable
```

Do not delete `scripts/windows/nex.iss` until Inno-to-WiX upgrade and uninstall tests pass on a disposable machine/profile.
