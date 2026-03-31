[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$ConfigPath = "$env:APPDATA\Nex\config.toml",
    [Parameter(Mandatory = $false)]
    [string]$RepoRoot = (Resolve-Path ".").Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Write-Host "== Nex Memory + App Discovery Profile =="
Write-Host "Repo: $RepoRoot"
Write-Host "Config: $ConfigPath"
Write-Host ""

if (-not (Test-Path -LiteralPath $ConfigPath)) {
    throw "Config file not found: $ConfigPath"
}

$raw = Get-Content -LiteralPath $ConfigPath -Raw
$updated = $raw
$updated = [regex]::Replace($updated, '(?m)^\s*discovery_roots\s*=.*$', 'discovery_roots = ["C:\\"]')
$updated = [regex]::Replace($updated, '(?m)^\s*windows_search_enabled\s*=.*$', 'windows_search_enabled = true')
$updated = [regex]::Replace($updated, '(?m)^\s*windows_search_fallback_filesystem\s*=.*$', 'windows_search_fallback_filesystem = true')
if ($updated -eq $raw) {
    Write-Host "No config substitutions were needed."
} else {
    Set-Content -LiteralPath $ConfigPath -Value $updated -NoNewline
    Write-Host "Updated config for C:\\ profiling."
}

Push-Location $RepoRoot
try {
    Write-Host ""
    Write-Host "[1/5] Restart runtime"
    cargo run -p nex -- --quit | Out-Host
    Start-Sleep -Milliseconds 250
    Start-Process -FilePath "cargo" -ArgumentList "run -p nex -- --background" -WindowStyle Hidden | Out-Null
    Start-Sleep -Seconds 2

    Write-Host ""
    Write-Host "[2/5] Allow background indexing to run"
    Start-Sleep -Seconds 6

    Write-Host ""
    Write-Host "[3/5] Capture status-json"
    cargo run -p nex -- --status-json | Out-Host

    Write-Host ""
    Write-Host "[4/5] Capture human status"
    cargo run -p nex -- --status | Out-Host

    Write-Host ""
    Write-Host "[5/5] Show recent query/memory lines"
    $logPath = "$env:APPDATA\Nex\logs\nex.log"
    if (Test-Path -LiteralPath $logPath) {
        Get-Content $logPath | Select-String -Pattern "query_profile|memory_snapshot|overlay_icon_cache|cache_compaction|provider_freshness|stale_prune|startup_phase|config reloaded|index_provider" | Select-Object -Last 150 | Out-Host
    } else {
        Write-Host "Log file not found: $logPath"
    }

    Write-Host ""
    Write-Host "Done. Search manually for: notepad, wezterm"
} finally {
    Pop-Location
}
