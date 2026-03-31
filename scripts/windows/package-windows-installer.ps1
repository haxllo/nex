param(
  [string]$Version,
  [ValidateSet("stable", "beta")]
  [string]$Channel = "stable",
  [string]$OutputRoot = "artifacts/windows",
  [string]$InnoCompiler = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
)

$ErrorActionPreference = "Stop"

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

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "..\.."))
$outputRootAbs = if ([System.IO.Path]::IsPathRooted($OutputRoot)) {
  [System.IO.Path]::GetFullPath($OutputRoot)
} else {
  [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputRoot))
}

$artifactName = "nex-$Version-windows-x64"
$stageDir = Join-Path $outputRootAbs "$artifactName-stage"
$issPath = Join-Path $repoRoot "scripts/windows/nex.iss"
$setupIconPath = Join-Path $repoRoot "apps/assets/nex.ico"

Write-Host "== Building Nex Setup.exe for $Version ==" -ForegroundColor Cyan

if (-not (Test-Path $InnoCompiler)) {
  $resolvedInno = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
  if (-not $resolvedInno) {
    $candidates = @(
      "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
      "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
      "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) }
    $resolvedInno = $candidates | Select-Object -First 1
  }

  if ($resolvedInno) {
    $InnoCompiler = $resolvedInno
    Write-Host "Resolved Inno Setup compiler: $InnoCompiler" -ForegroundColor Yellow
  }
  else {
    throw "Inno Setup compiler not found. Install Inno Setup or pass -InnoCompiler with the full ISCC.exe path."
  }
}

if (-not (Test-Path $issPath)) {
  throw "Installer spec not found at '$issPath'."
}

if (-not (Test-Path $setupIconPath)) {
  throw "Setup icon not found at '$setupIconPath'. Expected apps/assets/nex.ico."
}

if (-not (Test-Path $stageDir)) {
  Write-Host "Staged artifact not found. Building package stage first..." -ForegroundColor Yellow
  & (Join-Path $repoRoot "scripts/windows/package-windows-artifact.ps1") -Version $Version -Channel $Channel -OutputRoot $outputRootAbs
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to build staged artifact."
  }
}

if (-not (Test-Path (Join-Path $stageDir "bin/nex.exe"))) {
  throw "Missing staged executable at '$stageDir/bin/nex.exe'."
}

New-Item -ItemType Directory -Force -Path $outputRootAbs | Out-Null
& $InnoCompiler "/DAppVersion=$Version" "/DStageDir=$stageDir" "/DSetupIconPath=$setupIconPath" "/O$outputRootAbs" $issPath
if ($LASTEXITCODE -ne 0) {
  throw "Inno Setup compilation failed with exit code $LASTEXITCODE."
}

$setupPath = Join-Path $outputRootAbs "nex-$Version-windows-x64-setup.exe"
if (-not (Test-Path $setupPath)) {
  throw "Expected installer was not generated at '$setupPath'."
}

$manifestPath = Join-Path $outputRootAbs "$artifactName-manifest.json"
if (Test-Path $manifestPath) {
  $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -Depth 10
  if (-not $manifest.artifacts) {
    $manifest | Add-Member -NotePropertyName artifacts -NotePropertyValue ([PSCustomObject]@{})
  }
  if (-not $manifest.artifacts.setup) {
    $manifest.artifacts | Add-Member -NotePropertyName setup -NotePropertyValue ([PSCustomObject]@{})
  }

  $manifest.channel = $Channel
  $manifest.artifacts.setup.name = [System.IO.Path]::GetFileName($setupPath)
  $manifest.artifacts.setup.size_bytes = (Get-Item -LiteralPath $setupPath).Length
  $manifest.artifacts.setup.sha256 = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash.ToLowerInvariant()
  $manifest | ConvertTo-Json -Depth 10 | Set-Content -Encoding UTF8 -LiteralPath $manifestPath
  Write-Host "Updated manifest with setup hash: $manifestPath" -ForegroundColor Green
}

Write-Host "Created installer: $setupPath" -ForegroundColor Green
