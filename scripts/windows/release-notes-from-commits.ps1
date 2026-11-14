param(
  [string]$Version,
  [string]$PreviousTag,
  [string]$OutputPath
)

# Drafts docs/releases/v<ver>-notes.md from the commit log since the previous
# tag, using the section order that RELEASE-TEMPLATE.md and the What's New page
# expect. The What's New view (apps/core/src/whats_new.rs) renders the release
# body up to the "## Commit Log" marker, so that marker must stay last.
# The generated prose is a starting point: rewrite the summary and the
# per-item lines in the user's words before tagging.

$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Push-Location $repoRoot
try {
  if (-not $Version -or $Version.Trim().Length -eq 0) {
    $match = Select-String -Path "apps/core/Cargo.toml" -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = $match.Matches[0].Groups[1].Value.Trim()
  }

  if (-not $PreviousTag -or $PreviousTag.Trim().Length -eq 0) {
    $PreviousTag = (git describe --tags --abbrev=0 HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $PreviousTag) {
      throw "No previous tag found. Pass -PreviousTag explicitly."
    }
    $PreviousTag = $PreviousTag.Trim()
  }

  $range = "$PreviousTag..HEAD"
  $commits = git log $range --pretty=format:"%h%x09%s"
  if ($LASTEXITCODE -ne 0) {
    throw "git log $range failed."
  }

  $features = New-Object System.Collections.Generic.List[string]
  $fixes = New-Object System.Collections.Generic.List[string]
  $performance = New-Object System.Collections.Generic.List[string]
  $log = New-Object System.Collections.Generic.List[string]

  foreach ($line in $commits) {
    if (-not $line) { continue }
    $parts = $line -split "`t", 2
    $sha = $parts[0]
    $subject = if ($parts.Count -gt 1) { $parts[1] } else { "" }

    # "type(scope): summary" -> scope + summary; anything else stays whole.
    $item = $subject
    $bucket = $log
    if ($subject -match '^([a-zA-Z]+)(\(([^)]*)\))?!?:\s*(.+)$') {
      $type = $matches[1].ToLowerInvariant()
      $scope = $matches[3]
      $summary = $matches[4]
      if ($scope) { $item = "- **$scope**: $summary" } else { $item = "- $summary" }
      switch ($type) {
        'feat' { $bucket = $features }
        'fix' { $bucket = $fixes }
        'perf' { $bucket = $performance }
        default { $bucket = $log }
      }
    }
    else {
      $item = "- $subject"
    }
    $bucket.Add($item)
    $log.Add("- [``$sha``](https://github.com/haxllo/nex/commit/$sha) $subject")
  }

  $counts = @()
  if ($features.Count) { $counts += "$($features.Count) new capabilities" }
  if ($fixes.Count) { $counts += "$($fixes.Count) fixes" }
  if ($performance.Count) { $counts += "$($performance.Count) performance changes" }
  $summary = if ($counts.Count) { ($counts -join ", ") } else { "$($log.Count) changes" }

  $out = New-Object System.Collections.Generic.List[string]
  $out.Add("## What's new in v$Version")
  $out.Add("")
  $out.Add("Changes since $PreviousTag — $summary.")
  $out.Add("")
  $out.Add("## What changed")
  $out.Add("")

  if ($features.Count) {
    $out.Add("### Features")
    $out.Add("")
    $out.AddRange($features)
    $out.Add("")
  }
  if ($fixes.Count) {
    $out.Add("### Bug Fixes")
    $out.Add("")
    $out.AddRange($fixes)
    $out.Add("")
  }
  if ($performance.Count) {
    $out.Add("### Performance")
    $out.Add("")
    $out.AddRange($performance)
    $out.Add("")
  }

  $out.Add("### Notes")
  $out.Add("")
  $out.Add("- Limits worth knowing, and anything that could still look wrong.")
  $out.Add("")
  $out.Add("## Commit Log")
  $out.Add("")
  $out.AddRange($log)

  $text = ($out -join "`n") + "`n"

  if ($OutputPath) {
    $dir = Split-Path -Parent $OutputPath
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $target = if ([System.IO.Path]::IsPathRooted($OutputPath)) {
      [System.IO.Path]::GetFullPath($OutputPath)
    } else {
      [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputPath))
    }
    [System.IO.File]::WriteAllText(
      $target,
      $text,
      (New-Object System.Text.UTF8Encoding $false)
    )
    Write-Host "Wrote $OutputPath"
  }
  else {
    $text
  }
}
finally {
  Pop-Location
}
