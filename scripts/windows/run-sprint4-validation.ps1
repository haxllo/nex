param(
  [switch]$SkipSmoke = $false,
  [switch]$SkipRust = $false
)

$ErrorActionPreference = 'Stop'

Write-Host '== Sprint 4 Windows Validation Preflight ==' -ForegroundColor Cyan

if (-not $SkipRust) {
  $env:NEX_WINDOWS_RUNTIME_SMOKE = '1'
  Write-Host '[1/3] Running Windows runtime smoke harness...' -ForegroundColor Yellow
  cargo test -p nex --test windows_runtime_smoke_test -- --exact windows_runtime_smoke_registers_hotkey_and_transport_roundtrip
}

if (-not $SkipSmoke) {
  Write-Host '[2/3] Running repository smoke tests...' -ForegroundColor Yellow
  ./node_modules/.bin/vitest --run tests/smoke/scaffold.test.ts
}

Write-Host '[3/3] Checklist location:' -ForegroundColor Yellow
Write-Host 'docs/engineering/windows-runtime-validation-checklist.md' -ForegroundColor Green
Write-Host 'Run scripts/windows/record-manual-e2e.ps1 to capture manual pass/fail evidence.' -ForegroundColor Green
