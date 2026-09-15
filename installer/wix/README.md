# Nex WiX Installer

WiX Toolset v7 builds Nex MSI and Burn bootstrapper packages from the existing staged release layout.

WiX 7 binary use requires accepting its Open Source Maintenance Fee EULA once per developer/CI environment:

```powershell
wix eula accept wix7
```

Run this only after reviewing `https://github.com/wixtoolset/wix/blob/main/OSMFEULA.txt`.

Build MSI:

```powershell
dotnet build installer/wix/NexMsi.wixproj -c Release -p:AppVersion=2.19.0 -p:StageDir=$pwd/artifacts/windows/nex-2.19.0-windows-x64-stage
```

Build bundle after MSI exists:

```powershell
dotnet build installer/wix/NexBundle.wixproj -c Release -p:AppVersion=2.19.0 -p:MsiPath=$pwd/artifacts/windows/nex-2.19.0-windows-x64.msi
```

Migration remains incomplete until existing Inno installations are detected and removed safely. Do not cut over release packaging until upgrade, uninstall, and user-data preservation tests pass.

Inspect existing Inno installation without changing the machine:

```powershell
pwsh -File installer/wix/Migrate-InnoInstall.ps1 -WhatIf
```

The migration script preserves `%APPDATA%\Nex` and `%APPDATA%\SwiftFind`; it only invokes the old uninstaller.
