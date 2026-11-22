#Requires -Version 7.0
<#
.SYNOPSIS
    Publish a Nex release to winget.

.DESCRIPTION
    Regenerates the winget manifests for a published release, verifies the
    installer hash against the release's own recorded hash, and submits them to
    microsoft/winget-pkgs.

    Deliberately not part of release.yml. Submission needs a PAT that can open
    pull requests on microsoft/winget-pkgs, and an expired token must not turn a
    successful release red.

    If an open winget-pkgs pull request already exists for this package, the new
    version replaces the old one on the same branch instead of opening a second
    pull request. winget validation allows exactly one package version per pull
    request, and moderators ask for the newest version to supersede the older.

.PARAMETER Version
    Release version without a leading v, for example 3.3.0.

.PARAMETER DryRun
    Generate and validate the manifests, then stop before touching GitHub.

.EXAMPLE
    ./scripts/windows/publish-winget.ps1 -Version 3.3.0 -DryRun

.EXAMPLE
    $env:WINGETCREATE_TOKEN = $env:GH_TOKEN
    ./scripts/windows/publish-winget.ps1 -Version 3.4.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Version,

    [switch] $DryRun
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$PackageId    = 'Haxllo.Nex'
$GitHubRepo   = 'haxllo/nex'
$WingetRepo   = 'microsoft/winget-pkgs'
$ManifestRoot = 'manifests/h/Haxllo/Nex'

$Version = $Version.TrimStart('v', 'V')
$Tag = "v$Version"

function Write-Step([string] $Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

# --- 1. Confirm the release exists, and that it carries the asset winget needs.

Write-Step "Resolving release $Tag"

$releaseJson = gh release view $Tag --repo $GitHubRepo --json tagName,assets,publishedAt
if ($LASTEXITCODE -ne 0) {
    throw "No release for tag $Tag. Publish the release before submitting to winget."
}

$release = $releaseJson | ConvertFrom-Json
$assetName = "nex-$Version-windows-x64-setup.exe"
$asset = $release.assets | Where-Object { $_.name -eq $assetName }
if (-not $asset) {
    throw "Release $Tag has no asset named $assetName. Nothing to submit."
}

$installerUrl = "https://github.com/$GitHubRepo/releases/download/$Tag/$assetName"
$manifestAssetName = "nex-$Version-windows-x64-manifest.json"

# --- 2. Hash the real artefact and check it against the release's own record.
# A winget submission that pins the wrong hash fails validation late and looks
# like a winget problem. Catching it here keeps the failure where the cause is.

Write-Step "Downloading $assetName and hashing it"

$work = Join-Path ([IO.Path]::GetTempPath()) "nex-winget-$Version"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$installerPath = Join-Path $work $assetName

gh release download $Tag --repo $GitHubRepo --pattern $assetName --dir $work --clobber
if ($LASTEXITCODE -ne 0) { throw "Failed to download $assetName from $Tag." }

$actualSha = (Get-FileHash -Algorithm SHA256 -Path $installerPath).Hash.ToUpperInvariant()
Write-Host "    sha256 $actualSha"

$recordedSha = $null
if ($release.assets.name -contains $manifestAssetName) {
    gh release download $Tag --repo $GitHubRepo --pattern $manifestAssetName --dir $work --clobber
    if ($LASTEXITCODE -eq 0) {
        $recorded = Get-Content (Join-Path $work $manifestAssetName) -Raw | ConvertFrom-Json
        $recordedSha = $recorded.artifacts.setup.sha256.ToUpperInvariant()
    }
}

if ($recordedSha) {
    if ($recordedSha -ne $actualSha) {
        throw "Hash mismatch. The release records $recordedSha but the asset hashes to $actualSha."
    }
    Write-Host "    matches the hash recorded in $manifestAssetName" -ForegroundColor Green
} else {
    Write-Warning "$manifestAssetName is missing from the release; the hash could not be cross-checked."
}

# --- 3. Build the manifests from the newest set already in the repository.
# Copying the previous version and patching the fields that a release changes
# keeps the schema, switches and dependency pins consistent across versions.

Write-Step "Generating manifests for $Version"

$existing = Get-ChildItem -Path $ManifestRoot -Directory |
    Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' }

if (-not $existing) {
    throw "No existing manifest set under $ManifestRoot to use as a template."
}

# Prefer a different version as the template so a regeneration starts from a
# complete, previously validated set. Falling back to the same version lets a
# release be regenerated in place to refresh its date or hash.
$template = $existing |
    Where-Object { $_.Name -ne $Version } |
    Sort-Object { [version] $_.Name } -Descending |
    Select-Object -First 1
if (-not $template) {
    $template = $existing | Sort-Object { [version] $_.Name } -Descending | Select-Object -First 1
}
Write-Host "    template: $($template.Name)"

$target = Join-Path $ManifestRoot $Version
New-Item -ItemType Directory -Force -Path $target | Out-Null

$releaseDate = ([datetime] $release.publishedAt).ToUniversalTime().ToString('yyyy-MM-dd')
$releaseNotesUrl = "https://github.com/$GitHubRepo/releases/tag/$Tag"

foreach ($file in Get-ChildItem -Path $template.FullName -File) {
    $text = Get-Content $file.FullName -Raw

    $text = $text -replace '(?m)^PackageVersion:.*$', "PackageVersion: $Version"
    $text = $text -replace '(?m)^(\s+)DisplayVersion:.*$', "`$1DisplayVersion: $Version"
    $text = $text -replace '(?m)^ReleaseDate:.*$', "ReleaseDate: $releaseDate"
    $text = $text -replace '(?m)^ReleaseNotesUrl:.*$', "ReleaseNotesUrl: $releaseNotesUrl"

    if ($file.Name -eq "$PackageId.installer.yaml") {
        $text = $text -replace '(?m)^(\s+)InstallerUrl:.*$', "`$1InstallerUrl: $installerUrl"
        $text = $text -replace '(?m)^(\s+)InstallerSha256:.*$', "`$1InstallerSha256: $actualSha"
    }

    # ManifestType and ManifestVersion are left as the template had them.
    # WriteAllText rather than Set-Content: Set-Content appends its own trailing
    # newline, which would add a blank line on every regeneration.
    [IO.File]::WriteAllText((Join-Path $target $file.Name), $text.TrimEnd() + "`n")
    Write-Host "    wrote $($file.Name)"
}

# --- 4. Let winget validate before anything reaches GitHub.

Write-Step "Validating"

$validateOutput = winget validate --manifest $target 2>&1
$validateExit = $LASTEXITCODE

# Block on errors, not on warnings. Newer winget builds exit non-zero when they
# emit warnings while still reporting "validation succeeded with warnings", so
# the exit code alone cannot decide. Read the verdict out of the output, and fall
# back to the exit code only when there is no verdict to read.
$verdict = ($validateOutput | Out-String)
Write-Host $verdict.Trim()

if ($verdict -match 'Manifest validation failed' -or $verdict -match 'Manifest Error:') {
    throw "winget validate rejected the generated manifests."
}
if ($validateExit -ne 0 -and $verdict -notmatch 'validation succeeded') {
    throw "winget validate exited $validateExit without a verdict."
}
if ($verdict -match 'Manifest Warning:') {
    Write-Warning "winget validate reported warnings; they do not block submission."
}

if ($DryRun) {
    Write-Step "Dry run: manifests are in $target and validated. Nothing submitted."
    # exit, not return: the process exit code is still whatever the last native
    # command left behind, and winget validate exits non-zero for warnings. A
    # plain return would fail the calling CI step despite the clean run.
    exit 0
}

# --- 5. Supersede an open pull request, or submit a new one.

$token = $env:WINGETCREATE_TOKEN
if (-not $token) {
    $token = $env:GH_TOKEN
}
if (-not $token) {
    throw "Set WINGETCREATE_TOKEN or GH_TOKEN to a token that can open pull requests on $WingetRepo."
}

# winget-pkgs carries thousands of open pull requests, so an unfiltered
# `gh pr list --limit N` will not include this package's pull request. Search by
# package name and restrict to pull requests this token's account opened, which
# is the only account wingetcreate submits as.
$openPrs = gh pr list --repo $WingetRepo --state open --search $PackageId --limit 20 `
    --json number, title, headRefName, headRepositoryOwner, files |
    ConvertFrom-Json |
    Where-Object {
        $_.files.path -match [regex]::Escape($ManifestRoot) -and
        $_.headRepositoryOwner.login -ne 'microsoft'
    }

if ($openPrs) {
    $pr = @($openPrs)[0]
    Write-Step "Open pull request #$($pr.number) already carries this package; superseding it"

    $forkOwner = $pr.headRepositoryOwner.login
    $branch = $pr.headRefName

    $stale = Get-ChildItem -Path $ManifestRoot -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' -and $_.Name -ne $Version }
    foreach ($dir in $stale) {
        foreach ($f in Get-ChildItem -Path $dir.FullName -File) {
            $path = "$ManifestRoot/$($dir.Name)/$($f.Name)"
            $sha = gh api "repos/$forkOwner/winget-pkgs/contents/$path?ref=$branch" --jq '.sha' 2>$null
            if ($LASTEXITCODE -eq 0 -and $sha) {
                gh api -X DELETE "repos/$forkOwner/winget-pkgs/contents/$path" `
                    -f message="Remove $($dir.Name) in favour of $Version" `
                    -f sha=$sha -f branch=$branch | Out-Null
                Write-Host "    removed $path"
            }
        }
    }

    foreach ($f in Get-ChildItem -Path $target -File) {
        $path = "$ManifestRoot/$Version/$($f.Name)"
        $content = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes((Get-Content $f.FullName -Raw)))
        $payload = @{
            message = "Update $PackageId to $Version"
            content = $content
            branch  = $branch
        } | ConvertTo-Json -Compress
        $payload | gh api -X PUT "repos/$forkOwner/winget-pkgs/contents/$path" --input - | Out-Null
        Write-Host "    wrote $path"
    }

    Write-Host "Superseded: https://github.com/$WingetRepo/pull/$($pr.number)" -ForegroundColor Green
    exit 0
}

Write-Step "Submitting to $WingetRepo"

wingetcreate token --store --token $token | Out-Null
wingetcreate submit --prtitle "New version: $PackageId version $Version" --no-open $target
if ($LASTEXITCODE -ne 0) { throw "wingetcreate submit failed." }

Write-Step "Submitted. Check https://github.com/$WingetRepo/pulls?q=is%3Apr+$PackageId"
exit 0
