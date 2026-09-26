param(
  [ValidateSet("stable", "beta")]
  [string]$Channel = "stable",
  [string]$Version,
  [string]$Repo = "haxllo/nex",
  [switch]$StartAfterUpdate = $true,
  [switch]$KeepBackup,
  [switch]$Force,
  [switch]$CheckOnly,
  [string]$InstallRoot = "$env:LOCALAPPDATA\Programs\Nex",
  [string]$CacheRoot = "$env:LOCALAPPDATA\Nex\updates"
)

$UninstallSubkey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}_is1'

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Normalize-Version([string]$TagOrVersion) {
  if (-not $TagOrVersion) {
    return ""
  }
  $value = $TagOrVersion.Trim()
  if ($value.StartsWith("v")) {
    return $value.Substring(1)
  }
  if ($value -match '^(\d+)\.(\d+)([-+].*)?$') {
    $suffix = $Matches[3]
    if (-not $suffix) {
      $suffix = ''
    }
    return "$($Matches[1]).$($Matches[2]).0$suffix"
  }
  return $value
}

function Get-ArtifactBaseCandidates([string]$TagOrVersion) {
  $value = [string]$TagOrVersion
  if (-not $value) {
    return @()
  }

  $trimmed = $value.Trim()
  if ($trimmed.StartsWith("v")) {
    $trimmed = $trimmed.Substring(1)
  }

  $candidates = @()
  $normalized = Normalize-Version $trimmed
  if ($normalized) {
    $candidates += "nex-$normalized-windows-x64"
  }
  if ($trimmed -and $trimmed -ne $normalized) {
    $candidates += "nex-$trimmed-windows-x64"
  }
  return $candidates | Select-Object -Unique
}

function Is-BetaRelease($release) {
  if ($release.prerelease) {
    return $true
  }
  $tag = [string]$release.tag_name
  return $tag -match "-beta(\.|-|$)"
}

function Resolve-TargetRelease {
  param(
    [array]$Releases,
    [string]$ChannelName,
    [string]$RequestedVersion
  )

  if ($RequestedVersion -and $RequestedVersion.Trim().Length -gt 0) {
    $normalized = Normalize-Version $RequestedVersion
    $release = $Releases | Where-Object {
      $tag = Normalize-Version ([string]$_.tag_name)
      $tag -eq $normalized
    } | Select-Object -First 1
    if (-not $release) {
      throw "Version '$RequestedVersion' was not found in repo '$Repo' releases."
    }

    $isBeta = Is-BetaRelease $release
    if ($ChannelName -eq "stable" -and $isBeta) {
      throw "Requested version '$RequestedVersion' is a beta release but channel is stable."
    }
    if ($ChannelName -eq "beta" -and -not $isBeta) {
      throw "Requested version '$RequestedVersion' is not a beta release."
    }
    return $release
  }

  $filtered = $Releases | Where-Object {
    if ($ChannelName -eq "stable") {
      return -not (Is-BetaRelease $_)
    }
    return (Is-BetaRelease $_)
  }

  $selected = $filtered | Sort-Object -Property @{ Expression = {
      try { [version](Normalize-Version ([string]$_.tag_name)) }
      catch { [version]'0.0.0' }
    }; Descending = $true } | Select-Object -First 1
  if (-not $selected) {
    throw "No '$ChannelName' release found in repo '$Repo'."
  }
  return $selected
}

function Resolve-ReleaseAsset {
  param(
    $Release,
    [string[]]$AssetNames
  )

  $asset = $Release.assets | Where-Object { $AssetNames -contains $_.name } | Select-Object -First 1
  if (-not $asset) {
    throw "Release '$($Release.tag_name)' is missing assets: $($AssetNames -join ', ')."
  }
  return $asset
}

function Download-ReleaseAsset {
  param(
    $Asset,
    [string]$OutFile
  )
  Invoke-WebRequest `
    -Uri $Asset.browser_download_url `
    -Headers @{ "User-Agent" = "Nex-Updater"; "Accept" = "application/octet-stream" } `
    -OutFile $OutFile
}

function Get-Sha256([string]$Path) {
  if (Get-Command Get-FileHash -ErrorAction SilentlyContinue) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
  }

  try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
      $stream = [System.IO.File]::OpenRead($Path)
      try {
        $bytes = $sha256.ComputeHash($stream)
      }
      finally {
        $stream.Dispose()
      }
    }
    finally {
      $sha256.Dispose()
    }
    return ([System.BitConverter]::ToString($bytes)).Replace('-', '').ToLowerInvariant()
  }
  catch {
    $lines = @(certutil.exe -hashfile $Path SHA256 2>$null)
    if ($LASTEXITCODE -ne 0) {
      throw "Unable to calculate SHA-256 hash for '$Path': $($_.Exception.Message)"
    }
    $hash = ($lines | Where-Object { $_ -match '^[0-9a-fA-F ]{64,}$' } | Select-Object -First 1)
    if (-not $hash) {
      throw "Unable to parse SHA-256 hash for '$Path'."
    }
    return ([string]$hash).Trim().Replace(' ', '').ToLowerInvariant()
  }
}

function Get-RuntimeExecutableCandidates {
  param([string]$Root)

  return @(
    (Join-Path $Root "bin\Nex.exe"),
    (Join-Path $Root "bin\NexHelper.exe"),
    (Join-Path $Root "bin\nex-core.exe"),
    (Join-Path $Root "bin\swiftfind-core.exe")
  )
}

function Resolve-InstalledRuntimePath {
  param([string]$Root)

  foreach ($candidate in (Get-RuntimeExecutableCandidates -Root $Root)) {
    if (Test-Path -LiteralPath $candidate) {
      return $candidate
    }
  }

  return (Join-Path $Root "bin\Nex.exe")
}

function Resolve-InstallRoot {
  param([string]$DefaultRoot)

  foreach ($hive in @('HKCU', 'HKLM')) {
    $key = "$hive`:\$UninstallSubkey"
    if (Test-Path $key) {
      $props = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
      if ($null -ne $props.InstallLocation -and [string]$props.InstallLocation.Trim().Length -gt 0) {
        $candidate = [string]$props.InstallLocation
        if (Test-Path -LiteralPath (Join-Path $candidate "bin\Nex.exe")) {
          return [pscustomobject]@{
            Root = $candidate
            NeedsElevation = ($hive -eq 'HKLM')
          }
        }
      }
    }
  }

  return [pscustomobject]@{
    Root = $DefaultRoot
    NeedsElevation = $false
  }
}

function Resolve-InstalledVersion {
  param([string]$Root)

  $exe = Resolve-InstalledRuntimePath -Root $Root
  if (-not (Test-Path -LiteralPath $exe)) {
    return ""
  }
  try {
    return [string](Get-Item -LiteralPath $exe).VersionInfo.FileVersion
  }
  catch {
    return ""
  }
}

function Compare-Versions {
  param([string]$A, [string]$B)

  $aParts = @(($A -split '[-+]')[0] -split '\.' | ForEach-Object { [int]$_ })
  $bParts = @(($B -split '[-+]')[0] -split '\.' | ForEach-Object { [int]$_ })
  $count = [Math]::Max($aParts.Count, $bParts.Count)
  for ($i = 0; $i -lt $count; $i++) {
    $ap = if ($i -lt $aParts.Count) { $aParts[$i] } else { 0 }
    $bp = if ($i -lt $bParts.Count) { $bParts[$i] } else { 0 }
    if ($ap -gt $bp) { return 1 }
    if ($ap -lt $bp) { return -1 }
  }
  return 0
}

function Write-UpdateResult {
  param(
    [ValidateSet("up-to-date", "updated", "failed")]
    [string]$Status,
    [string]$Version,
    [string]$Message
  )

  $obj = @{ status = $Status }
  if ($Version) { $obj.version = $Version }
  if ($Message) { $obj.message = $Message }
  Write-Host "NEX_UPDATE_RESULT: $(ConvertTo-Json -Compress $obj)"
}

function Stop-Runtime {
  param([string]$InstalledExePath)

  if (Test-Path -LiteralPath $InstalledExePath) {
    try {
      & $InstalledExePath --quit | Out-Null
    }
    catch {
      Write-Host "Warning: graceful quit failed; using hard stop fallback." -ForegroundColor Yellow
    }
    Start-Sleep -Milliseconds 400
  }

  foreach ($imageName in @("Nex.exe", "NexHelper.exe", "nex-core.exe", "swiftfind-core.exe")) {
    cmd /c "taskkill /IM $imageName /F /T >NUL 2>&1" | Out-Null
  }
  Start-Sleep -Milliseconds 200
}

function Bring-ProcessToFront($Process) {
  if (-not ('NexWindow' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NexWindow {
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
}
'@
  }
  $shell = New-Object -ComObject WScript.Shell
  for ($attempt = 0; $attempt -lt 20; $attempt++) {
    $Process.Refresh()
    if ($Process.HasExited) {
      return
    }
    if ($Process.MainWindowHandle -ne 0) {
      $handle = [IntPtr]$Process.MainWindowHandle
      [NexWindow]::ShowWindowAsync($handle, 9) | Out-Null
      [NexWindow]::SetForegroundWindow($handle) | Out-Null
      if (-not $shell.AppActivate($Process.Id)) {
        Write-Host "Warning: could not activate installer window." -ForegroundColor Yellow
      }
      return
    }
    Start-Sleep -Milliseconds 100
  }
  Write-Host "Warning: installer window was not ready for foreground activation." -ForegroundColor Yellow
}

function Verify-ManifestAndInstaller {
  param(
    $Manifest,
    [string]$ExpectedVersion,
    [string]$ExpectedArtifact,
    [string]$ExpectedChannel,
    [string]$SetupPath
  )

  if (-not $Manifest.artifact) {
    throw "Manifest is missing 'artifact'."
  }
  if (-not $Manifest.version) {
    throw "Manifest is missing 'version'."
  }

  if ([string]$Manifest.artifact -ne $ExpectedArtifact) {
    throw "Manifest artifact mismatch. Expected '$ExpectedArtifact', got '$($Manifest.artifact)'."
  }

  $manifestVersion = Normalize-Version ([string]$Manifest.version)
  if ($manifestVersion -ne $ExpectedVersion) {
    throw "Manifest version mismatch. Expected '$ExpectedVersion', got '$manifestVersion'."
  }

  if ($Manifest.channel) {
    $manifestChannel = ([string]$Manifest.channel).ToLowerInvariant()
    if ($manifestChannel -ne $ExpectedChannel.ToLowerInvariant()) {
      throw "Manifest channel mismatch. Expected '$ExpectedChannel', got '$manifestChannel'."
    }
  }

  if (-not $Manifest.artifacts -or -not $Manifest.artifacts.setup) {
    throw "Manifest is missing artifacts.setup integrity data."
  }

  $setupSha = [string]$Manifest.artifacts.setup.sha256
  if (-not $setupSha -or $setupSha.Trim().Length -eq 0) {
    throw "Manifest artifacts.setup.sha256 is missing."
  }

  $actualSha = Get-Sha256 -Path $SetupPath
  if ($actualSha -ne $setupSha.ToLowerInvariant()) {
    throw "Installer checksum mismatch. Expected '$setupSha', got '$actualSha'."
  }
}

$installInfo = Resolve-InstallRoot -DefaultRoot $InstallRoot
$InstallRoot = $installInfo.Root
$needsElevation = $installInfo.NeedsElevation

Write-Host "== Nex Update ==" -ForegroundColor Cyan
Write-Host "Channel: $Channel"
if ($Version) {
  Write-Host "Requested version: $Version"
}
Write-Host "Repo: $Repo"
Write-Host "Install root: $InstallRoot"

$apiUrl = "https://api.github.com/repos/$Repo/releases?per_page=40"
$releasesResponse = Invoke-RestMethod -Uri $apiUrl -Headers @{ "User-Agent" = "Nex-Updater" }
$releases = @($releasesResponse)
if ($releases.Count -eq 0) {
  throw "No releases were returned for '$Repo'."
}

$targetRelease = Resolve-TargetRelease -Releases $releases -ChannelName $Channel -RequestedVersion $Version
$resolvedVersion = Normalize-Version ([string]$targetRelease.tag_name)
$artifactBaseCandidates = Get-ArtifactBaseCandidates ([string]$targetRelease.tag_name)
$artifactBase = $artifactBaseCandidates | Select-Object -First 1
$setupNames = $artifactBaseCandidates | ForEach-Object { "$_-setup.exe" }
$manifestNames = $artifactBaseCandidates | ForEach-Object { "$_-manifest.json" }

Write-Host "Target release: $($targetRelease.tag_name)" -ForegroundColor Green

$installedVersion = Resolve-InstalledVersion -Root $InstallRoot
if (-not $Force -and $installedVersion -and (Compare-Versions $installedVersion $resolvedVersion) -ge 0) {
  Write-Host "Already up to date (installed $installedVersion, latest $resolvedVersion)." -ForegroundColor Green
  Write-UpdateResult -Status "up-to-date" -Version $installedVersion
  exit 0
}

if ($CheckOnly) {
  Write-Host "Update available (installed $installedVersion, latest $resolvedVersion)." -ForegroundColor Yellow
  Write-Host "NEX_UPDATE_RESULT: $(ConvertTo-Json -Compress @{ status = 'update-available'; version = $resolvedVersion })"
  exit 0
}

$setupAsset = Resolve-ReleaseAsset -Release $targetRelease -AssetNames $setupNames
$manifestAsset = Resolve-ReleaseAsset -Release $targetRelease -AssetNames $manifestNames

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$workDir = Join-Path $CacheRoot "$artifactBase-update-$stamp"
New-Item -ItemType Directory -Force -Path $workDir | Out-Null

$setupPath = Join-Path $workDir $setupAsset.name
$manifestPath = Join-Path $workDir $manifestAsset.name

Write-Host "[1/5] Downloading manifest and installer..." -ForegroundColor Yellow
Download-ReleaseAsset -Asset $manifestAsset -OutFile $manifestPath
Download-ReleaseAsset -Asset $setupAsset -OutFile $setupPath

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
Write-Host "[2/5] Verifying integrity..." -ForegroundColor Yellow
Verify-ManifestAndInstaller `
  -Manifest $manifest `
  -ExpectedVersion $resolvedVersion `
  -ExpectedArtifact $artifactBase `
  -ExpectedChannel $Channel `
  -SetupPath $setupPath

$installedExe = Resolve-InstalledRuntimePath -Root $InstallRoot
$backupDir = $null

try {
  Write-Host "[3/5] Stopping active runtime and preparing rollback snapshot..." -ForegroundColor Yellow
  Stop-Runtime -InstalledExePath $installedExe

  if (Test-Path -LiteralPath $InstallRoot) {
    $backupRoot = Join-Path $CacheRoot "backups"
    New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
    $backupDir = Join-Path $backupRoot "nex-backup-$stamp"
    Move-Item -LiteralPath $InstallRoot -Destination $backupDir
    Write-Host "Backup created: $backupDir"
  }

  Write-Host "[4/5] Installing update..." -ForegroundColor Yellow
  if ($needsElevation) {
    $proc = Start-Process -FilePath $setupPath -Verb RunAs -PassThru -WindowStyle Normal
  }
  else {
    $proc = Start-Process -FilePath $setupPath -PassThru -WindowStyle Normal
  }
  Bring-ProcessToFront -Process $proc
  $proc.WaitForExit()
  if ($proc.ExitCode -ne 0) {
    throw "Installer exited with code $($proc.ExitCode)."
  }

  $newExe = Resolve-InstalledRuntimePath -Root $InstallRoot
  if (-not (Test-Path -LiteralPath $newExe)) {
    throw "Updated runtime executable not found at '$newExe'."
  }

  & $newExe --ensure-config | Out-Null
  & $newExe --sync-startup | Out-Null

  if ($StartAfterUpdate) {
    # Runtime already runs detached from this updater process. Starting the
    # foreground runtime directly avoids the background->foreground respawn
    # race during WebView initialization and first overlay positioning.
    Start-Process -FilePath $newExe -ArgumentList "--foreground" -WindowStyle Hidden
  }

  if ($backupDir -and (Test-Path -LiteralPath $backupDir) -and -not $KeepBackup) {
    Remove-Item -LiteralPath $backupDir -Recurse -Force
    $backupDir = $null
  }

  Write-Host "[5/5] Update complete." -ForegroundColor Green
  Write-Host "Installed version: $resolvedVersion"
  if ($backupDir) {
    Write-Host "Rollback snapshot retained: $backupDir"
  }
  Write-UpdateResult -Status "updated" -Version $resolvedVersion
}
catch {
  Write-Host "Update failed: $($_.Exception.Message)" -ForegroundColor Red
  Write-UpdateResult -Status "failed" -Message $_.Exception.Message
  Write-Host "Attempting rollback..." -ForegroundColor Yellow

  try {
    Stop-Runtime -InstalledExePath (Resolve-InstalledRuntimePath -Root $InstallRoot)
    if (Test-Path -LiteralPath $InstallRoot) {
      Remove-Item -LiteralPath $InstallRoot -Recurse -Force
    }
    if ($backupDir -and (Test-Path -LiteralPath $backupDir)) {
      Move-Item -LiteralPath $backupDir -Destination $InstallRoot
      $restoredExe = Resolve-InstalledRuntimePath -Root $InstallRoot
      if ($StartAfterUpdate -and (Test-Path -LiteralPath $restoredExe)) {
        Start-Process -FilePath $restoredExe -ArgumentList "--foreground" -WindowStyle Hidden
      }
      Write-Host "Rollback complete: restored previous installation." -ForegroundColor Green
    }
    else {
      Write-Host "No backup snapshot available for rollback." -ForegroundColor Yellow
    }
  }
  catch {
    Write-Host "Rollback failed: $($_.Exception.Message)" -ForegroundColor Red
  }

  throw
}
