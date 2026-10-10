param(
  [ValidateSet("stable", "beta")]
  [string]$Channel = "stable",
  [string]$Version,
  [string]$Repo = "haxllo/nex",
  [switch]$StartAfterUpdate = $true,
  [switch]$KeepBackup,
  [switch]$Force,
  [switch]$CheckOnly,
  [string]$RunningVersion,
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
  # Windows PowerShell 5.1 renders a progress bar for every Invoke-WebRequest
  # chunk, which throttles multi-MB downloads to a crawl. Silencing progress
  # (or WebClient below) is an order of magnitude faster.
  $previousProgressPreference = $ProgressPreference
  $ProgressPreference = 'SilentlyContinue'
  try {
    Invoke-WebRequest `
      -Uri $Asset.browser_download_url `
      -Headers @{ "User-Agent" = "Nex-Updater"; "Accept" = "application/octet-stream" } `
      -OutFile $OutFile
  }
  finally {
    $ProgressPreference = $previousProgressPreference
  }
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

function Test-IsUnderProgramFiles([string]$Path) {
  if (-not $Path) {
    return $false
  }
  $full = $Path.Trim().TrimEnd('\', '/')
  if ($full.Length -eq 0) {
    return $false
  }
  foreach ($pf in @($env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramW6432)) {
    if ($pf) {
      $base = ([string]$pf).Trim().TrimEnd('\', '/')
      if ($base -and ($full.StartsWith($base, [System.StringComparison]::OrdinalIgnoreCase))) {
        return $true
      }
    }
  }
  return $false
}

function Get-RunningInstallRoot {
  # The installed updater lives at <install_root>\scripts\update-nex.ps1,
  # so its own directory reveals which scope copy is actually running.
  # Returns $null for repo/dev runs (scripts\windows\... has no bin\Nex.exe).
  $dir = $PSScriptRoot
  if (-not $dir) {
    return $null
  }
  $parent = Split-Path -Parent $dir
  if ($parent -and (Test-Path -LiteralPath (Join-Path $parent "bin\Nex.exe"))) {
    return $parent
  }
  return $null
}

function Get-RegistryInstallLocation {
  param([string]$Hive)

  $key = "$Hive`:\$UninstallSubkey"
  if (-not (Test-Path $key)) {
    return $null
  }
  $props = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
  if ($null -eq $props) {
    return $null
  }
  if ($null -eq $props.InstallLocation) {
    return $null
  }
  $candidate = ([string]$props.InstallLocation).Trim()
  if ($candidate.Length -eq 0) {
    return $null
  }
  if (-not (Test-Path -LiteralPath (Join-Path $candidate "bin\Nex.exe"))) {
    return $null
  }
  return $candidate
}

function Resolve-InstallRoot {
  param([string]$DefaultRoot)

  $runningRoot = Get-RunningInstallRoot
  $hkcuLoc = Get-RegistryInstallLocation -Hive 'HKCU'
  $hklmLoc = Get-RegistryInstallLocation -Hive 'HKLM'
  # A machine-wide entry outside Program Files is never a legitimate
  # all-users install — it is the stale hybrid left by the older updater
  # bug (HKLM pointing at a per-user %LOCALAPPDATA% path). It must not
  # decide scope; it only triggers one-time elevated cleanup.
  $legitHklmLoc = $null
  if ($hklmLoc -and (Test-IsUnderProgramFiles $hklmLoc)) {
    $legitHklmLoc = $hklmLoc
  }

  # Prefer the copy that is actually running — this is what disambiguates
  # dual-scope machines (both hives registered). Scope follows location:
  # under Program Files means all-users, otherwise current user.
  if ($runningRoot) {
    return [pscustomobject]@{
      Root = $runningRoot
      NeedsElevation = (Test-IsUnderProgramFiles $runningRoot)
    }
  }

  if ($hkcuLoc) {
    return [pscustomobject]@{
      Root = $hkcuLoc
      NeedsElevation = $false
    }
  }

  if ($legitHklmLoc) {
    return [pscustomobject]@{
      Root = $legitHklmLoc
      NeedsElevation = $true
    }
  }

  # Stale hybrid only (HKCU missing, HKLM points per-user): update that
  # per-user path instead of treating it as all-users.
  if ($hklmLoc) {
    return [pscustomobject]@{
      Root = $hklmLoc
      NeedsElevation = $false
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
    [ValidateSet("up-to-date", "updated", "failed", "version-skew")]
    [string]$Status,
    [string]$Version,
    [string]$Message
  )

  $obj = @{ status = $Status }
  if ($Version) { $obj.version = $Version }
  if ($Message) { $obj.message = $Message }
  $line = "NEX_UPDATE_RESULT: $(ConvertTo-Json -Compress $obj)"
  # The parent process may already be gone (it is killed as part of the
  # update), in which case console output throws under Stop preference.
  # The marker must still reach the log file, so never let it be fatal.
  try { Write-Host $line } catch {}
  if ($script:UpdateLogPath) {
    try { Add-Content -LiteralPath $script:UpdateLogPath -Value $line -ErrorAction SilentlyContinue } catch {}
  }
}

$script:UpdateLogPath = $null

function Write-UpdateLog {
  param([string]$Message, [string]$Color)
  # Progress output must never be fatal: once Nex.exe is stopped, stdout
  # may have no reader, and a throwing Write-Host would skip rollback.
  if ($script:UpdateLogPath) {
    try { Add-Content -LiteralPath $script:UpdateLogPath -Value $Message -ErrorAction SilentlyContinue } catch {}
  }
  try {
    if ($Color) { Write-Host $Message -ForegroundColor $Color } else { Write-Host $Message }
  } catch {}
}

function Stop-Runtime {
  param([string]$InstalledExePath)

  # The elevated helper cannot be killed by taskkill from this
  # medium-integrity process — end it through its own scheduled task
  # first. Otherwise it keeps bin\NexHelper.exe locked and every later
  # file operation on the install tree fails after Nex is already dead.
  cmd /c "schtasks /end /tn NexHelperV2 >NUL 2>&1" | Out-Null

  if (Test-Path -LiteralPath $InstalledExePath) {
    try {
      & $InstalledExePath --quit | Out-Null
    }
    catch {
      Write-UpdateLog "Warning: graceful quit failed; using hard stop fallback." "Yellow"
    }
    Start-Sleep -Milliseconds 400
  }

  # Do not use taskkill /T here: the updater is a child of Nex.exe when launched
  # from the update button, so terminating the whole process tree would kill
  # the updater before it can install or restart the new version.
  foreach ($imageName in @("Nex.exe", "NexHelper.exe", "nex-core.exe", "swiftfind-core.exe")) {
    cmd /c "taskkill /IM $imageName /F >NUL 2>&1" | Out-Null
  }

  # Verify everything actually died before touching the install tree.
  # Aborting here is safe (nothing moved yet); proceeding with a live
  # process holding files is what bricks the install with no rollback.
  for ($attempt = 0; $attempt -lt 10; $attempt++) {
    $remaining = @(Get-Process -Name "Nex", "NexHelper", "nex-core", "swiftfind-core" -ErrorAction SilentlyContinue)
    if ($remaining.Count -eq 0) { break }
    Start-Sleep -Milliseconds 500
  }
  $remaining = @(Get-Process -Name "Nex", "NexHelper", "nex-core", "swiftfind-core" -ErrorAction SilentlyContinue)
  if ($remaining.Count -gt 0) {
    throw "Could not stop running processes: $($remaining.Name -join ', '). Aborting update before touching the install."
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

if (-not ('NexRestartManager' -as [type])) {
  # Restart Manager is Windows' own "which process holds this path" API.
  # Used purely for diagnostics: name the locker when the backup move
  # is blocked, so updater.log contains the culprit instead of a bare
  # sharing violation. All signatures use int (not uint) so PowerShell
  # [ref] marshaling stays simple.
  Add-Type @'
using System;
using System.Runtime.InteropServices;
public enum NexRmAppType { RmUnknownApp = 0, RmMainWindow = 1, RmOtherWindow = 2, RmService = 3, RmExplorer = 4, RmConsole = 5, RmCritical = 1000 }
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct NexRmProcessInfo {
  public int dwProcessId;
  public long processStartTime;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string strAppName;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string strServiceShortName;
  public NexRmAppType ApplicationType;
  public uint AppStatus;
  public uint TSSessionId;
  [MarshalAs(UnmanagedType.Bool)] public bool bRestartable;
}
public static class NexRestartManager {
  [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
  public static extern int RmStartSession(out int pSessionHandle, int dwSessionFlags, string strSessionKey);
  [DllImport("rstrtmgr.dll")]
  public static extern int RmEndSession(int pSessionHandle);
  [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
  public static extern int RmRegisterResources(int pSessionHandle, int nFiles, string[] rgsFilenames, int nApplications, IntPtr rgApplications, int nServices, string[] rgsServiceNames);
  [DllImport("rstrtmgr.dll")]
  public static extern int RmGetList(int dwSessionHandle, out int pnProcInfoNeeded, ref int pnProcInfo, [In, Out] NexRmProcessInfo[] rgAffectedApps, ref int lpdwRebootReasons);
}
'@
}

function Get-LockingProcesses {
  # Names every process Restart Manager sees holding $Path, e.g.
  # "MsMpEng.exe [pid 1234, type RmService]". Restart Manager only
  # tracks files, so directories are expanded to the files inside
  # (capped — the first entries already name the locker). Never
  # throws: returns @() when the API is unavailable or nothing holds
  # the path.
  param([string]$Path)
  $found = @()
  try {
    $files = @()
    if (Test-Path -LiteralPath $Path -PathType Container) {
      # ponytail: 128-file cap; lockers show up in the first entries.
      $files = @(Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue |
        Select-Object -First 128 -ExpandProperty FullName)
    }
    elseif (Test-Path -LiteralPath $Path -PathType Leaf) {
      $files = @($Path)
    }
    if (-not $files.Count) {
      return $found
    }
    $session = 0
    if ([NexRestartManager]::RmStartSession([ref]$session, 0, [guid]::NewGuid().ToString()) -ne 0) {
      return $found
    }
    try {
      if ([NexRestartManager]::RmRegisterResources($session, $files.Count, $files, 0, [IntPtr]::Zero, 0, $null) -ne 0) {
        return $found
      }
      $needed = 0
      $count = 16
      $reasons = 0
      $list = New-Object NexRmProcessInfo[] $count
      $rc = [NexRestartManager]::RmGetList($session, [ref]$needed, [ref]$count, $list, [ref]$reasons)
      if ($rc -eq 234 -and $needed -gt $count) {
        # 234 = ERROR_MORE_DATA: resize and ask once more.
        $count = $needed
        $list = New-Object NexRmProcessInfo[] $count
        $rc = [NexRestartManager]::RmGetList($session, [ref]$needed, [ref]$count, $list, [ref]$reasons)
      }
      if ($rc -eq 0) {
        for ($i = 0; $i -lt $count; $i++) {
          $entry = $list[$i]
          $found += "$($entry.strAppName) [pid $($entry.dwProcessId), type $($entry.ApplicationType)]"
        }
      }
    }
    finally {
      [void][NexRestartManager]::RmEndSession($session)
    }
  }
  catch {}
  return $found
}

function Write-PreMoveDiagnostics {
  # Snapshot of everything that could plausibly hold the install tree,
  # taken after Stop-Runtime and before the backup move. Every probe is
  # independent and silent on failure — diagnostics must never break
  # the update they observe.
  param([string]$Root)
  try {
    $procs = @(Get-Process -Name "Nex", "NexHelper", "nex-core", "swiftfind-core", "msedgewebview2" -ErrorAction SilentlyContinue |
      ForEach-Object { "$($_.ProcessName) [pid $($_.Id)]" })
    Write-UpdateLog "Processes at backup time: $(if ($procs.Count) { $procs -join ', ' } else { '(none of Nex/NexHelper/msedgewebview2)' })"
  }
  catch {}
  try {
    $webviews = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction Stop |
      Where-Object { $_.CommandLine -match 'nex' } |
      ForEach-Object { "pid $($_.ProcessId): $($_.CommandLine)" })
    if ($webviews.Count) {
      Write-UpdateLog "Nex-owned WebView2 children still alive: $($webviews -join ' | ')" "Yellow"
    }
  }
  catch {}
  try {
    $av = @(Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntivirusProduct -ErrorAction Stop |
      ForEach-Object { $_.displayName })
    if ($av.Count) {
      Write-UpdateLog "Registered antivirus: $($av -join ', ')"
    }
  }
  catch {}
  try {
    $lockers = @(Get-LockingProcesses -Path $Root)
    Write-UpdateLog "Restart Manager lockers on install root: $(if ($lockers.Count) { $lockers -join ', ' } else { '(none)' })"
  }
  catch {}
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

# A legacy orphan outside the registered install (e.g. a pre-AppId copy under
# Program Files) can only be removed by an elevated installer — a per-user
# installer physically cannot touch Program Files. Elevate just the installer
# launch (scope stays per-user via /CURRENTUSER); the UAC prompt is
# unavoidable exactly once for this cleanup.
$legacyOrphan = $false
foreach ($programFiles in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
  if ($programFiles -and (Test-Path -LiteralPath (Join-Path $programFiles 'Nex\bin\Nex.exe'))) {
    $legacyOrphan = $true
    break
  }
}
# Hybrid registration from an older updater bug: a machine-wide entry pointing
# at a per-user directory. There is no legitimate install to preserve there,
# but removing it still needs elevation.
if (-not $legacyOrphan) {
  # Get-ItemPropertyValue (not Get-ItemProperty) — accessing .InstallLocation
  # on a $null result throws under StrictMode when the HKLM key is absent,
  # which is the normal state of a pure per-user install. That one crash
  # killed the whole script: no update button, no updates, no log.
  $hklmLocation = Get-ItemPropertyValue -LiteralPath "HKLM:\$UninstallSubkey" -Name InstallLocation -ErrorAction SilentlyContinue
  if ($hklmLocation) {
    $underProgramFiles = $false
    foreach ($programFiles in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
      if ($programFiles -and ($hklmLocation -like "$programFiles*")) {
        $underProgramFiles = $true
        break
      }
    }
    if (-not $underProgramFiles) {
      $legacyOrphan = $true
    }
  }
}
if ($legacyOrphan -and -not $needsElevation) {
  Write-Host "Legacy Nex copy found outside the registered install; elevating the installer once so it can remove it (install scope stays per-user)." -ForegroundColor Yellow
  $needsElevation = $true
}

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

# The updater resolves the *registered* install, but the process that
# launched it may be an orphaned copy elsewhere. Updating the registered
# install while a stale binary holds the single-instance slot only adds
# confusion — tell the user to restart from the installed location.
if ($RunningVersion -and $installedVersion -and (Compare-Versions $RunningVersion $installedVersion) -lt 0) {
  $skewMessage = "running v$RunningVersion but v$installedVersion is installed; restart Nex from the installed location instead of updating"
  Write-Host "Version skew detected ($skewMessage)." -ForegroundColor Yellow
  Write-UpdateResult -Status "version-skew" -Version $installedVersion -Message $skewMessage
  exit 0
}

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
$script:UpdateLogPath = Join-Path $workDir "updater.log"

# Leave whatever directory we were launched from: if our own working
# directory sits inside the install tree (inherited from Nex), the later
# Move-Item of that tree fails with "in use" — held open by ourselves.
try {
  Set-Location -LiteralPath $workDir -ErrorAction Stop
  Write-UpdateLog "Working directory: $workDir"
}
catch {
  Set-Location -LiteralPath ([System.IO.Path]::GetTempPath())
}

$setupPath = Join-Path $workDir $setupAsset.name
$manifestPath = Join-Path $workDir $manifestAsset.name

Write-UpdateLog "[1/5] Downloading manifest and installer..." "Yellow"
Download-ReleaseAsset -Asset $manifestAsset -OutFile $manifestPath
Download-ReleaseAsset -Asset $setupAsset -OutFile $setupPath

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
Write-UpdateLog "[2/5] Verifying integrity..." "Yellow"
Verify-ManifestAndInstaller `
  -Manifest $manifest `
  -ExpectedVersion $resolvedVersion `
  -ExpectedArtifact $artifactBase `
  -ExpectedChannel $Channel `
  -SetupPath $setupPath

$installedExe = Resolve-InstalledRuntimePath -Root $InstallRoot
$backupDir = $null

try {
  Write-UpdateLog "[3/5] Stopping active runtime and preparing rollback snapshot..." "Yellow"
  Stop-Runtime -InstalledExePath $installedExe
  Write-PreMoveDiagnostics -Root $InstallRoot

  if (Test-Path -LiteralPath $InstallRoot) {
    $backupRoot = Join-Path $CacheRoot "backups"
    New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
    $backupDir = Join-Path $backupRoot "nex-backup-$stamp"
    # Retry transient locks (AV scans, dying processes releasing handles).
    $moved = $false
    for ($attempt = 1; $attempt -le 3 -and -not $moved; $attempt++) {
      try {
        Move-Item -LiteralPath $InstallRoot -Destination $backupDir -ErrorAction Stop
        $moved = $true
      }
      catch {
        if ($attempt -eq 3) { throw }
        $lockers = @(Get-LockingProcesses -Path $InstallRoot)
        $lockerText = if ($lockers.Count) { " Lockers: $($lockers -join ', '). " } else { " No lockers reported. " }
        Write-UpdateLog "Backup move blocked, retrying ($attempt/3)...$lockerText" "Yellow"
        Start-Sleep -Seconds 1
      }
    }
    Write-UpdateLog "Backup created: $backupDir"
  }

  Write-UpdateLog "[4/5] Installing update..." "Yellow"
  $logPath = Join-Path $workDir "setup.log"
  # Scope follows the registered install and NEVER changes with elevation:
  # elevation is only about permission (e.g. removing a legacy orphan),
  # while /ALLUSERS vs /CURRENTUSER decides registry hives and folders.
  # Coupling them once registered a per-user install machine-wide.
  $scopeArg = if ($installInfo.NeedsElevation) { "/ALLUSERS" } else { "/CURRENTUSER" }
  # Fully automatic install: silent progress UI with no wizard prompts, no
  # restart, an installer log for diagnostics, and the explicit scope and
  # target directory. /NEXUPDATER tells the installer not to launch Nex
  # itself — the updater restarts it after verification.
  $setupArgs = "/SILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL /SP- /LOG=`"$logPath`" /DIR=`"$InstallRoot`" $scopeArg /NEXUPDATER"
  if ($needsElevation) {
    $proc = Start-Process -FilePath $setupPath -ArgumentList $setupArgs -Verb RunAs -PassThru -WindowStyle Normal
  }
  else {
    $proc = Start-Process -FilePath $setupPath -ArgumentList $setupArgs -PassThru -WindowStyle Normal
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

  Write-UpdateLog "[5/5] Update complete." "Green"
  Write-UpdateLog "Installed version: $resolvedVersion"
  if ($backupDir) {
    Write-UpdateLog "Rollback snapshot retained: $backupDir"
  }
  Write-UpdateResult -Status "updated" -Version $resolvedVersion
}
catch {
  # Capture first: every diagnostic below must survive a dead parent pipe.
  $failure = $_.Exception.Message
  try {
    $lockers = @(Get-LockingProcesses -Path $InstallRoot)
    if ($lockers.Count) {
      $failure = "$failure Lockers at failure time: $($lockers -join ', ')."
    }
  }
  catch {}
  try {
    Write-UpdateLog "Update failed: $failure" "Red"
    Write-UpdateLog "Attempting rollback..." "Yellow"
    try {
      Stop-Runtime -InstalledExePath (Resolve-InstalledRuntimePath -Root $InstallRoot)
    }
    catch {
      Write-UpdateLog "Warning: stop during rollback failed: $($_.Exception.Message)" "Yellow"
    }
    if ($backupDir -and (Test-Path -LiteralPath $backupDir)) {
      if (Test-Path -LiteralPath $InstallRoot) {
        Remove-Item -LiteralPath $InstallRoot -Recurse -Force
      }
      Move-Item -LiteralPath $backupDir -Destination $InstallRoot
      $restoredExe = Resolve-InstalledRuntimePath -Root $InstallRoot
      if ($StartAfterUpdate -and (Test-Path -LiteralPath $restoredExe)) {
        Start-Process -FilePath $restoredExe -ArgumentList "--foreground" -WindowStyle Hidden
      }
      Write-UpdateLog "Rollback complete: restored previous installation." "Green"
    }
    elseif (Test-Path -LiteralPath $installedExe) {
      # Aborted before anything moved: the original install is untouched,
      # so restart it instead of leaving the user with nothing running.
      if ($StartAfterUpdate) {
        Start-Process -FilePath $installedExe -ArgumentList "--foreground" -WindowStyle Hidden
      }
      Write-UpdateLog "Update aborted before any files moved; restarted previous version." "Yellow"
    }
    else {
      Write-UpdateLog "No backup snapshot available for rollback." "Yellow"
    }
  }
  catch {
    Write-UpdateLog "Rollback failed: $($_.Exception.Message)" "Red"
  }
  Write-UpdateResult -Status "failed" -Message $failure

  throw
}
