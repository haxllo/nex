param(
  [switch]$BuildFromSource,
  [switch]$SkipBuild,
  [string]$SourceExe,
  [switch]$StartAfterInstall = $true,
  [ValidateSet("Ask", "True", "False")]
  [string]$LaunchAtStartup = "Ask",
  [ValidateSet("CurrentUser", "AllUsers")]
  [string]$InstallScope = "CurrentUser",
  [string]$InstallRoot = ""
)

$ErrorActionPreference = "Stop"

function Test-IsAdministrator {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not $InstallRoot -or $InstallRoot.Trim().Length -eq 0) {
  if ($InstallScope -eq "AllUsers") {
    $InstallRoot = Join-Path $env:ProgramFiles "SwiftFind"
  }
  else {
    $InstallRoot = Join-Path $env:LOCALAPPDATA "Programs\SwiftFind"
  }
}

if ($InstallScope -eq "AllUsers" -and -not (Test-IsAdministrator)) {
  throw "AllUsers install requires an elevated PowerShell session (Run as administrator)."
}

Write-Host "== SwiftFind Install ==" -ForegroundColor Cyan
Write-Host "Install scope: $InstallScope"
Write-Host "Install root: $InstallRoot"

if ($SkipBuild) {
  # Backward-compatible behavior for older usage; skip build when explicitly set.
  $BuildFromSource = $false
}

if (-not $SourceExe -or $SourceExe.Trim().Length -eq 0) {
  $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
  $candidates = @(
    (Join-Path $scriptDir "..\bin\swiftfind-core.exe"),              # packaged zip layout
    (Join-Path $scriptDir "..\..\target\release\swiftfind-core.exe"), # repo layout
    (Join-Path (Get-Location) "bin\swiftfind-core.exe"),
    (Join-Path (Get-Location) "target\release\swiftfind-core.exe")
  )

  foreach ($candidate in $candidates) {
    if (Test-Path $candidate) {
      $SourceExe = $candidate
      break
    }
  }
}

if ((-not $SourceExe -or -not (Test-Path $SourceExe)) -and $BuildFromSource) {
  $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
  $repoRoot = Resolve-Path (Join-Path $scriptDir "..\..")
  Write-Host "[1/5] Building release binary from source..." -ForegroundColor Yellow
  Push-Location $repoRoot
  try {
    cargo build -p swiftfind-core --release --quiet
    $built = Join-Path $repoRoot "target\release\swiftfind-core.exe"
    if (Test-Path $built) {
      $SourceExe = $built
    }
  }
  finally {
    Pop-Location
  }
}

if (-not $SourceExe -or -not (Test-Path $SourceExe)) {
  throw @"
Could not find swiftfind-core.exe to install.

For end users:
- Extract the release zip and run this script from that extracted folder.
- The zip should contain bin\swiftfind-core.exe.

For developers:
- Re-run with -BuildFromSource, or pass -SourceExe "<full path to swiftfind-core.exe>".
"@
}

$binDir = Join-Path $InstallRoot "bin"
$assetsDir = Join-Path $InstallRoot "assets"
$docsDir = Join-Path $InstallRoot "docs"

Write-Host "[2/5] Preparing install directories..."
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
New-Item -ItemType Directory -Force -Path $assetsDir | Out-Null
New-Item -ItemType Directory -Force -Path $docsDir | Out-Null

Write-Host "[3/5] Copying runtime files..."
Copy-Item $SourceExe (Join-Path $binDir "swiftfind-core.exe") -Force

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$assetCandidates = @(
  (Join-Path $scriptDir "..\assets\swiftfinder.svg"),              # packaged zip layout
  (Join-Path $scriptDir "..\..\apps\assets\swiftfinder.svg")       # repo layout
)
foreach ($asset in $assetCandidates) {
  if (Test-Path $asset) {
    Copy-Item $asset (Join-Path $assetsDir "swiftfinder.svg") -Force
    break
  }
}

$runbookCandidates = @(
  (Join-Path $scriptDir "..\docs\windows-operator-runbook.md"),                # packaged zip layout
  (Join-Path $scriptDir "..\..\docs\engineering\windows-operator-runbook.md")  # repo layout
)
foreach ($runbook in $runbookCandidates) {
  if (Test-Path $runbook) {
    Copy-Item $runbook (Join-Path $docsDir "windows-operator-runbook.md") -Force
    break
  }
}

$installedExe = Join-Path $binDir "swiftfind-core.exe"

Write-Host "[4/5] Preparing config and startup sync..."
& $installedExe --ensure-config

$enableLaunchAtStartup = $false
switch ($LaunchAtStartup) {
  "True" {
    $enableLaunchAtStartup = $true
  }
  "False" {
    $enableLaunchAtStartup = $false
  }
  default {
    if ([Environment]::UserInteractive) {
      $answer = Read-Host "Launch SwiftFind automatically at Windows sign-in? (y/N)"
      if ($answer -match '^(y|yes)$') {
        $enableLaunchAtStartup = $true
      }
    }
    else {
      Write-Host "Non-interactive install detected; defaulting launch-at-startup to false."
    }
  }
}

if ($enableLaunchAtStartup) {
  & $installedExe --set-launch-at-startup=true
}
else {
  & $installedExe --set-launch-at-startup=false
}

Write-Host "Note: launch-at-startup can be changed later in $env:APPDATA\SwiftFind\config.toml"

if ($StartAfterInstall) {
  Write-Host "[5/5] Starting SwiftFind in background..."
  Start-Process -FilePath $installedExe -ArgumentList "--background" -WindowStyle Hidden
}
else {
  Write-Host "[5/5] Start skipped (--StartAfterInstall:\$false)."
}

Write-Host "Install complete." -ForegroundColor Green
Write-Host "Executable: $installedExe"
Write-Host "Config: $env:APPDATA\SwiftFind\config.toml"
