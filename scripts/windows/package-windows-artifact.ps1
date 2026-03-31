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

cargo build -p nex --release --quiet

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

$zipHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$zipSize = (Get-Item -LiteralPath $zipPath).Length
$exePath = Join-Path $stageDir "bin/nex.exe"
$exeHash = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256).Hash.ToLowerInvariant()
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
    sha256 = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
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
