param(
  [string]$Version,
  [ValidateSet("stable", "beta")]
  [string]$Channel = "stable",
  [string]$OutputRoot = "artifacts/windows",
  [switch]$Sign,
  [string]$CertPath,
  [string]$CertPassword,
  [string]$TimestampUrl = "http://timestamp.digicert.com",
  [string]$SignTool = "signtool.exe"
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Everything SDK download URL
# ---------------------------------------------------------------------------
$EverythingSdkUrl = "https://www.voidtools.com/Everything-SDK.zip"
$EverythingDllName = "Everything64.dll"

function Resolve-VersionFromCargo {
  $cargoToml = Join-Path $PSScriptRoot "..\..\apps\core\Cargo.toml"
  if (-not (Test-Path $cargoToml)) {
    return $null
  }

  $match = Select-String -Path $cargoToml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
  if (-not $match) {
    return $null
  }

  return $match.Matches[0].Groups[1].Value.Trim()
}

if (-not $Version -or $Version.Trim().Length -eq 0) {
  try {
    $Version = Resolve-VersionFromCargo
    if (-not $Version -or $Version.Trim().Length -eq 0) {
      $Version = (git describe --tags --always).Trim()
    }
  }
  catch {
    $Version = "0.0.0-local"
  }
}

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$artifactName = "nex-$Version-windows-x64"
$stageDir = Join-Path $OutputRoot "$artifactName-stage"
$zipPath = Join-Path $OutputRoot "$artifactName.zip"
$manifestPath = Join-Path $OutputRoot "$artifactName-manifest.json"

Write-Host "== Packaging $artifactName ==" -ForegroundColor Cyan

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null
if (Test-Path $stageDir) { Remove-Item -Recurse -Force $stageDir }
if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stageDir "bin") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stageDir "assets") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stageDir "docs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stageDir "scripts") | Out-Null

cargo build -p nex-cli --release --quiet

$coreExe = "target/release/nex.exe"
if (-not (Test-Path $coreExe)) {
  throw "Expected core executable not found at $coreExe"
}

if ($Sign) {
  Write-Host "Signing enabled. Signing $coreExe ..." -ForegroundColor Cyan

  if (-not $CertPath -or $CertPath.Trim().Length -eq 0) {
    throw "Signing requested but -CertPath was not provided."
  }
  if (-not (Test-Path $CertPath)) {
    throw "Signing requested but certificate file was not found: $CertPath"
  }

  $signtoolCmd = Get-Command $SignTool -ErrorAction SilentlyContinue
  if (-not $signtoolCmd) {
    throw "Signing requested but signtool was not found. Install Windows SDK SignTool or pass -SignTool with full path."
  }

  $signArgs = @(
    "sign",
    "/fd", "SHA256",
    "/tr", $TimestampUrl,
    "/td", "SHA256",
    "/f", $CertPath
  )
  if ($CertPassword -and $CertPassword.Length -gt 0) {
    $signArgs += @("/p", $CertPassword)
  }
  $signArgs += $coreExe

  & $signtoolCmd.Source @signArgs
  if ($LASTEXITCODE -ne 0) {
    throw "signtool sign failed with exit code $LASTEXITCODE"
  }

  & $signtoolCmd.Source verify /pa /v $coreExe
  if ($LASTEXITCODE -ne 0) {
    throw "signtool verify failed with exit code $LASTEXITCODE"
  }

  $signature = Get-AuthenticodeSignature $coreExe
  if ($signature.Status -ne "Valid") {
    throw "Authenticode signature is not valid: $($signature.Status)"
  }
  Write-Host "Signature verified: $($signature.SignerCertificate.Subject)" -ForegroundColor Green
}
else {
  Write-Host "Signing skipped (unsigned artifact)." -ForegroundColor Yellow
}

Copy-Item $coreExe (Join-Path $stageDir "bin/nex.exe") -Force

# -----------------------------------------------------------------------
# Download and bundle Everything SDK DLL
# -----------------------------------------------------------------------
$everythingDllDir = Join-Path $stageDir "bin"
$everythingDllPath = Join-Path $everythingDllDir $EverythingDllName
$everythingDllBundled = $false
try {
  Write-Host "Downloading Everything SDK from $EverythingSdkUrl ..." -ForegroundColor Yellow
  $sdkZip = Join-Path $env:TEMP "Everything-SDK.zip"
  # Ensure TLS 1.2 for older Windows PowerShell versions
  if (-not [Net.ServicePointManager]::SecurityProtocol.ToString().Contains('Tls12')) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
  }
  Invoke-WebRequest -Uri $EverythingSdkUrl -OutFile $sdkZip -UseBasicParsing -ErrorAction Stop
  # Try multiple extraction methods for PowerShell version compatibility
  $extracted = $false
  # Method 1: .NET ZipFile (PS 5.0+ with Add-Type)
  if (-not $extracted) {
    try {
      Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction Stop
      $zip = [System.IO.Compression.ZipFile]::OpenRead($sdkZip)
      $entry = $zip.Entries | Where-Object { $_.Name -eq $EverythingDllName } | Select-Object -First 1
      if ($entry) {
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $everythingDllPath, $true)
        $extracted = $true
      }
      $zip.Dispose()
    } catch {
      Write-Host "Method 1 (ZipFile) failed, trying fallback..." -ForegroundColor DarkGray
    }
  }
  # Method 2: Expand-Archive (PS 5.0+)
  if (-not $extracted) {
    try {
      $extractDir = Join-Path $env:TEMP "everything-sdk-extract"
      Remove-Item -Recurse -Force $extractDir -ErrorAction SilentlyContinue
      Expand-Archive -Path $sdkZip -DestinationPath $extractDir -ErrorAction Stop
      $dllPath = Get-ChildItem -Recurse -Path $extractDir -Filter $EverythingDllName | Select-Object -First 1
      if ($dllPath) {
        Copy-Item $dllPath.FullName $everythingDllPath -Force
        $extracted = $true
      }
      Remove-Item -Recurse -Force $extractDir -ErrorAction SilentlyContinue
    } catch {
      Write-Host "Method 2 (Expand-Archive) failed, trying fallback..." -ForegroundColor DarkGray
    }
  }
  # Method 3: Shell.Application COM (legacy PS)
  if (-not $extracted) {
    try {
      $extractDir = Join-Path $env:TEMP "everything-sdk-extract"
      Remove-Item -Recurse -Force $extractDir -ErrorAction SilentlyContinue
      New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
      $shell = New-Object -ComObject Shell.Application
      $zipObj = $shell.NameSpace($sdkZip)
      $dest = $shell.NameSpace($extractDir)
      $dest.CopyHere($zipObj.Items(), 0x14)
      $dllPath = Get-ChildItem -Recurse -Path $extractDir -Filter $EverythingDllName | Select-Object -First 1
      if ($dllPath) {
        Copy-Item $dllPath.FullName $everythingDllPath -Force
        $extracted = $true
      }
      Remove-Item -Recurse -Force $extractDir -ErrorAction SilentlyContinue
    } catch {
      Write-Host "Method 3 (Shell.Application) failed." -ForegroundColor DarkGray
    }
  }
  if ($extracted) {
    Write-Host "Bundled $EverythingDllName next to nex.exe" -ForegroundColor Green
    $everythingDllBundled = $true
  } else {
    Write-Host "WARNING: Could not extract $EverythingDllName from Everything-SDK.zip" -ForegroundColor Yellow
  }
  Remove-Item $sdkZip -Force -ErrorAction SilentlyContinue
}
catch {
  Write-Host "WARNING: Failed to download Everything SDK: $_" -ForegroundColor Yellow
  Write-Host "Nex will still work but Everything search will be unavailable until the DLL is placed manually." -ForegroundColor Yellow
}
if (Test-Path "apps/assets/nex.svg") {
  Copy-Item "apps/assets/nex.svg" (Join-Path $stageDir "assets/nex.svg") -Force
}
if (Test-Path "apps/assets/fonts/Geist") {
  New-Item -ItemType Directory -Force -Path (Join-Path $stageDir "assets/fonts") | Out-Null
  Copy-Item "apps/assets/fonts/Geist" (Join-Path $stageDir "assets/fonts/Geist") -Recurse -Force
}
Copy-Item "docs/engineering/windows-runtime-validation-checklist.md" (Join-Path $stageDir "docs/windows-runtime-validation-checklist.md") -Force
Copy-Item "docs/releases/windows-milestone-release-notes-template.md" (Join-Path $stageDir "docs/release-notes-template.md") -Force
Copy-Item "scripts/windows/install-nex.ps1" (Join-Path $stageDir "scripts/install-nex.ps1") -Force
Copy-Item "scripts/windows/uninstall-nex.ps1" (Join-Path $stageDir "scripts/uninstall-nex.ps1") -Force
Copy-Item "scripts/windows/update-nex.ps1" (Join-Path $stageDir "scripts/update-nex.ps1") -Force

Compress-Archive -Path (Join-Path $stageDir "*") -DestinationPath $zipPath

function Compute-Sha256Hash($path) {
  if (Get-Command Get-FileHash -ErrorAction SilentlyContinue) {
    return (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
  }
  # Fallback for older PowerShell / .NET
  try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($path)
    $hashBytes = $sha256.ComputeHash($stream)
    $stream.Close()
    return [System.BitConverter]::ToString($hashBytes).Replace('-', '').ToLowerInvariant()
  } catch {
    throw "Unable to compute SHA256: $_"
  }
}

$zipHash = Compute-Sha256Hash $zipPath
$zipSize = (Get-Item -LiteralPath $zipPath).Length
$exePath = Join-Path $stageDir "bin/nex.exe"
$exeHash = Compute-Sha256Hash $exePath
$exeSize = (Get-Item -LiteralPath $exePath).Length

$stageDirPrefix = (Resolve-Path -LiteralPath $stageDir).Path
if (-not $stageDirPrefix.EndsWith("\") -and -not $stageDirPrefix.EndsWith("/")) {
  $stageDirPrefix = "$stageDirPrefix\"
}

$stageFiles = Get-ChildItem -LiteralPath $stageDir -Recurse -File | ForEach-Object {
  $fullPath = $_.FullName
  $relative = $fullPath.Substring($stageDirPrefix.Length).Replace('\', '/')
  [ordered]@{
    path = $relative
    size_bytes = $_.Length
    sha256 = (Compute-Sha256Hash $fullPath)
  }
}

$manifest = [ordered]@{
  artifact = $artifactName
  version = $Version
  channel = $Channel
  built_utc = (Get-Date).ToUniversalTime().ToString('o')
  build_stamp = $stamp
  os = "windows-x64"
  signed = [bool]$Sign
  artifacts = [ordered]@{
    zip = [ordered]@{
      name = "$artifactName.zip"
      size_bytes = $zipSize
      sha256 = $zipHash
    }
    setup = [ordered]@{
      name = "$artifactName-setup.exe"
      size_bytes = $null
      sha256 = $null
    }
    core_exe = [ordered]@{
      path = "bin/nex.exe"
      size_bytes = $exeSize
      sha256 = $exeHash
    }
  }
  files = $stageFiles
}

$manifest | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 $manifestPath

Write-Host "Created artifact: $zipPath" -ForegroundColor Green
Write-Host "Created manifest: $manifestPath" -ForegroundColor Green
Write-Host "Staging dir retained: $stageDir" -ForegroundColor Green
