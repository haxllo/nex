[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$Pattern = '(?i)sample|documentation|tools for|uwp|desktop apps'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Write-Host "== Nex Start App Source Inspector =="
Write-Host "Pattern: $Pattern"
Write-Host ""

Write-Host "[1/2] Get-StartApps matches"
$startApps = @(Get-StartApps | Where-Object { $_.Name -match $Pattern } | Sort-Object Name)
if ($startApps.Count -eq 0) {
    Write-Host "No Get-StartApps matches found."
} else {
    $startApps | Format-Table Name, AppID -AutoSize
}

Write-Host ""
Write-Host "[2/2] Start Menu .lnk matches"
$roots = @(
    "$env:ProgramData\Microsoft\Windows\Start Menu\Programs",
    "$env:APPDATA\Microsoft\Windows\Start Menu\Programs"
)

$shell = New-Object -ComObject WScript.Shell
$shortcutRows = @()

foreach ($root in $roots) {
    if (-not (Test-Path -LiteralPath $root)) {
        continue
    }

    Get-ChildItem -LiteralPath $root -Recurse -File -Filter *.lnk -ErrorAction SilentlyContinue |
        Where-Object { $_.BaseName -match $Pattern } |
        ForEach-Object {
            $targetPath = ''
            $arguments = ''
            $iconLocation = ''
            try {
                $sc = $shell.CreateShortcut($_.FullName)
                $targetPath = [string]$sc.TargetPath
                $arguments = [string]$sc.Arguments
                $iconLocation = [string]$sc.IconLocation
            } catch {
                $targetPath = '<failed-to-resolve>'
                $arguments = ''
                $iconLocation = ''
            }

            $shortcutRows += [PSCustomObject]@{
                Name       = $_.BaseName
                LnkPath    = $_.FullName
                TargetPath = $targetPath
                Arguments  = $arguments
                Icon       = $iconLocation
            }
        }
}

if ($shortcutRows.Count -eq 0) {
    Write-Host "No Start Menu .lnk matches found."
} else {
    $shortcutRows |
        Sort-Object Name |
        Format-List Name, LnkPath, TargetPath, Arguments, Icon
}
