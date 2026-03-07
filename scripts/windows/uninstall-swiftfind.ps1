param(
  [switch]$PurgeUserData,
  [ValidateSet("CurrentUser", "AllUsers")]
  [string]$InstallScope = "CurrentUser",
  [string]$InstallRoot = ""
)

$ErrorActionPreference = "Continue"

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
  throw "AllUsers uninstall requires an elevated PowerShell session (Run as administrator)."
}

Write-Host "== SwiftFind Uninstall ==" -ForegroundColor Cyan
Write-Host "Install scope: $InstallScope"
Write-Host "Install root: $InstallRoot"

$installedExe = Join-Path $InstallRoot "bin\swiftfind-core.exe"

if (Test-Path $installedExe) {
  Write-Host "[1/5] Signaling running instance to quit..."
  & $installedExe --quit | Out-Null
  Start-Sleep -Milliseconds 800
}
else {
  Write-Host "[1/5] Installed executable not found; skipping quit signal."
}

Write-Host "[2/5] Hard-stopping any leftover SwiftFind process..."
taskkill /IM swiftfind-core.exe /F /T | Out-Null
Start-Sleep -Milliseconds 200

Write-Host "[3/5] Removing startup registration..."
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v SwiftFind /f | Out-Null
reg delete "HKLM\Software\Microsoft\Windows\CurrentVersion\Run" /v SwiftFind /f | Out-Null

Write-Host "[4/5] Removing installed files..."
if (Test-Path $InstallRoot) {
  for ($attempt = 1; $attempt -le 3; $attempt++) {
    try {
      Remove-Item -Recurse -Force $InstallRoot
      break
    }
    catch {
      if ($attempt -eq 3) {
        throw
      }
      Start-Sleep -Milliseconds 300
    }
  }
}

if ($PurgeUserData) {
  Write-Host "[5/5] Removing user data (%APPDATA%\\SwiftFind)..."
  $userData = Join-Path $env:APPDATA "SwiftFind"
  if (Test-Path $userData) {
    Remove-Item -Recurse -Force $userData
  }
}
else {
  Write-Host "[5/5] Keeping user data. Use -PurgeUserData to remove."
}

Write-Host "Uninstall complete." -ForegroundColor Green
