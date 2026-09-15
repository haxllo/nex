param(
  [switch]$WhatIf
)

$ErrorActionPreference = "Stop"
$innoAppId = "{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}_is1"
$uninstallRoots = @(
  "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$innoAppId",
  "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$innoAppId",
  "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\$innoAppId"
)

function Get-OldInstall {
  foreach ($root in $uninstallRoots) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    $entry = Get-ItemProperty -LiteralPath $root
    $uninstaller = $entry.UninstallString
    if (-not $uninstaller) { continue }
    $uninstaller = $uninstaller.Trim()
    if ($uninstaller.StartsWith('"')) {
      $end = $uninstaller.IndexOf('"', 1)
      if ($end -gt 1) { $uninstaller = $uninstaller.Substring(1, $end - 1) }
    } else {
      $space = $uninstaller.IndexOf(' ')
      if ($space -gt 0) { $uninstaller = $uninstaller.Substring(0, $space) }
    }
    if (Test-Path -LiteralPath $uninstaller) {
      return [PSCustomObject]@{ RegistryPath = $root; Uninstaller = $uninstaller }
    }
  }
  return $null
}

$old = Get-OldInstall
if (-not $old) {
  Write-Output "No existing Inno installation found."
  exit 0
}

Write-Output "Existing Inno installation found at $($old.RegistryPath)."
if ($WhatIf) { exit 0 }

$args = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"
$process = Start-Process -FilePath $old.Uninstaller -ArgumentList $args -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0) {
  throw "Existing Inno uninstall failed with exit code $($process.ExitCode)."
}
