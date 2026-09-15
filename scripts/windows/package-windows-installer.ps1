param(
  [string]$Version,
  [ValidateSet("stable", "beta")]
  [string]$Channel = "stable",
  [string]$OutputRoot = "artifacts/windows",
  [ValidateSet("inno", "wix")]
  [string]$InstallerEngine = "inno",
  [string]$InnoCompiler = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
  [string]$WixConfiguration = "Release"
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
$wixMsiProject = Join-Path $repoRoot "installer/wix/NexMsi.wixproj"
$wixBundleProject = Join-Path $repoRoot "installer/wix/NexBundle.wixproj"

Write-Host "== Building Nex Setup.exe for $Version ==" -ForegroundColor Cyan

if ($InstallerEngine -eq "wix") {
  if (-not (Test-Path $wixMsiProject) -or -not (Test-Path $wixBundleProject)) {
    throw "WiX projects not found under '$repoRoot/installer/wix'."
  }
}
elseif (-not (Test-Path $InnoCompiler)) {
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

if ($InstallerEngine -eq "inno" -and -not (Test-Path $issPath)) {
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

if (-not (Test-Path (Join-Path $stageDir "bin/Nex.exe"))) {
  throw "Missing staged executable at '$stageDir/bin/Nex.exe'."
}

New-Item -ItemType Directory -Force -Path $outputRootAbs | Out-Null
if ($InstallerEngine -eq "wix") {
  $msiPath = Join-Path $outputRootAbs "$artifactName.msi"
  & dotnet build $wixMsiProject -c $WixConfiguration `
    "-p:AppVersion=$Version" "-p:StageDir=$stageDir" "-p:OutputPath=$outputRootAbs"
  if ($LASTEXITCODE -ne 0) {
    throw "WiX MSI compilation failed with exit code $LASTEXITCODE."
  }

  $builtMsi = Join-Path $outputRootAbs "NexMsi.msi"
  if (-not (Test-Path $builtMsi)) {
    throw "WiX MSI was not generated at '$builtMsi'."
  }
  Copy-Item $builtMsi $msiPath -Force

  & dotnet build $wixBundleProject -c $WixConfiguration `
    "-p:AppVersion=$Version" "-p:MsiPath=$msiPath" "-p:OutputPath=$outputRootAbs"
  if ($LASTEXITCODE -ne 0) {
    throw "WiX Burn compilation failed with exit code $LASTEXITCODE."
  }

  $builtBundle = Join-Path $outputRootAbs "NexBundle.exe"
  if (-not (Test-Path $builtBundle)) {
    throw "WiX Burn bundle was not generated at '$builtBundle'."
  }
  $setupPath = Join-Path $outputRootAbs "nex-$Version-windows-x64-setup.exe"
  Copy-Item $builtBundle $setupPath -Force
  Write-Host "Created WiX MSI: $msiPath" -ForegroundColor Green
}
else {
  & $InnoCompiler "/DAppVersion=$Version" "/DStageDir=$stageDir" "/DSetupIconPath=$setupIconPath" "/O$outputRootAbs" $issPath
  if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup compilation failed with exit code $LASTEXITCODE."
  }
}

$setupPath = Join-Path $outputRootAbs "nex-$Version-windows-x64-setup.exe"
if (-not (Test-Path $setupPath)) {
  throw "Expected installer was not generated at '$setupPath'."
}

$manifestPath = Join-Path $outputRootAbs "$artifactName-manifest.json"
if (Test-Path $manifestPath) {
  $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
  if (-not $manifest.artifacts) {
    $manifest | Add-Member -NotePropertyName artifacts -NotePropertyValue ([PSCustomObject]@{})
  }
  if (-not $manifest.artifacts.setup) {
    $manifest.artifacts | Add-Member -NotePropertyName setup -NotePropertyValue ([PSCustomObject]@{})
  }

  $manifest.channel = $Channel
  $manifest.artifacts.setup.name = [System.IO.Path]::GetFileName($setupPath)
  $manifest.artifacts.setup.size_bytes = (Get-Item -LiteralPath $setupPath).Length
  # SHA256 with fallback for older PowerShell
  try {
    $manifest.artifacts.setup.sha256 = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash.ToLowerInvariant()
  } catch {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($setupPath)
    $hashBytes = $sha256.ComputeHash($stream)
    $stream.Close()
    $manifest.artifacts.setup.sha256 = [System.BitConverter]::ToString($hashBytes).Replace('-', '').ToLowerInvariant()
  }
  $manifest | ConvertTo-Json -Depth 10 | Set-Content -Encoding UTF8 -LiteralPath $manifestPath
  Write-Host "Updated manifest with setup hash: $manifestPath" -ForegroundColor Green
}

Write-Host "Created installer: $setupPath" -ForegroundColor Green
