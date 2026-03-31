param(
  [ValidateSet("stable", "beta")]
  [string]$Channel = "stable",
  [string]$Version,
  [string]$Repo = "haxllo/sch",
  [switch]$StartAfterUpdate = $true,
  [switch]$KeepBackup,
  [string]$InstallRoot = "$env:LOCALAPPDATA\Programs\Nex",
  [string]$CacheRoot = "$env:LOCALAPPDATA\Nex\updates"
)

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

  $selected = $filtered | Select-Object -First 1
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

function Get-RuntimeExecutableCandidates {
  param([string]$Root)

  return @(
    (Join-Path $Root "bin\nex.exe"),
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

  return (Join-Path $Root "bin\nex.exe")
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

  foreach ($imageName in @("nex.exe", "nex-core.exe", "swiftfind-core.exe")) {
    cmd /c "taskkill /IM $imageName /F /T >NUL 2>&1" | Out-Null
  }
  Start-Sleep -Milliseconds 200
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

  $actualSha = (Get-FileHash -LiteralPath $SetupPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actualSha -ne $setupSha.ToLowerInvariant()) {
    throw "Installer checksum mismatch. Expected '$setupSha', got '$actualSha'."
  }
}

Write-Host "== Nex Update ==" -ForegroundColor Cyan
Write-Host "Channel: $Channel"
if ($Version) {
  Write-Host "Requested version: $Version"
}
Write-Host "Repo: $Repo"
Write-Host "Install root: $InstallRoot"

$apiUrl = "https://api.github.com/repos/$Repo/releases?per_page=40"
$releases = @(Invoke-RestMethod -Uri $apiUrl -Headers @{ "User-Agent" = "Nex-Updater" })
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

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -Depth 12
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
  $args = @("/VERYSILENT", "/NORESTART", "/SUPPRESSMSGBOXES", "/SP-")
  $proc = Start-Process -FilePath $setupPath -ArgumentList $args -PassThru -Wait
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
    Start-Process -FilePath $newExe -ArgumentList "--background" -WindowStyle Hidden
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
}
catch {
  Write-Host "Update failed: $($_.Exception.Message)" -ForegroundColor Red
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
        Start-Process -FilePath $restoredExe -ArgumentList "--background" -WindowStyle Hidden
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
