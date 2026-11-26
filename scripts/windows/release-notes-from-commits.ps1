param(
  [string]$Version,
  [string]$PreviousTag,
  [string]$OutputPath,
  [string]$Notes
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

  # The workflow runs after the tag is pushed, so HEAD *is* the tag being
  # released and describing it returns that same tag: the range came out
  # empty and the first real release note read "Changes since v3.0.0 - 0
  # changes". Step back over any tag that already points at HEAD.
  $headTags = @(git tag --points-at HEAD)
  $guard = 0
  while ($PreviousTag -and ($headTags -contains $PreviousTag)) {
    $guard++
    if ($guard -gt 20) { throw "Could not find a tag older than $PreviousTag." }
    $PreviousTag = (git describe --tags --abbrev=0 "$PreviousTag^" 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $PreviousTag) {
      throw "No previous tag found before the released one. Pass -PreviousTag explicitly."
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
  $untyped = New-Object System.Collections.Generic.List[string]
  $log = New-Object System.Collections.Generic.List[string]

  # Commits that describe how the work landed rather than what changed. The
  # commit-msg hook already exempts the same subjects, so a revert or a merge
  # does not appear in the notes a user reads.
  $skipPattern = '^(Merge |Revert |fixup!|squash!|docs:|chore:|test:)'

  # A commit subject is free text and can contain an "@name" - "call the winget
  # script explicitly instead of splatting @args" was published as a live
  # GitHub mention, so a stranger with that username was notified about our
  # release. The sigil becomes an HTML entity: it renders as "@args" exactly as
  # written, and the mention parser never sees an account to link or ping.
  function ConvertTo-MentionSafe([string]$text) {
    if (-not $text) { return $text }
    return ($text -replace '@', '&#64;')
  }

  foreach ($line in $commits) {
    if (-not $line) { continue }
    $parts = $line -split "`t", 2
    $sha = $parts[0]
    $subject = if ($parts.Count -gt 1) { $parts[1] } else { "" }
    if ($subject -match $skipPattern) { continue }

    # "type(scope): summary" -> scope + summary; anything else stays whole.
    # The Linear id is stripped before the type is read: without that,
    # "CIQ-201: read the ..." parses as a "CIQ" type and the summary loses its
    # opening words. The id identifies the issue, not the change, and a
    # release note is read by people who do not have Linear access.
    $clean = ($subject -replace '^CIQ-[0-9]+:\s*', '').Trim()
    $clean = ($clean -replace '\s*\(?CIQ-[0-9]+\)?\s*', ' ').Trim()
    # Stripping an id that sat inside a list leaves the punctuation that
    # separated it: "(CIQ-179, CIQ-180)" became "delete to the Recycle Bin ,".
    $clean = ($clean -replace '\s+,', ',') -replace '\s{2,}', ' '
    $clean = $clean.TrimEnd(' ', ',', ';', ':')
    $item = $clean
    $bucket = $null
    if ($clean -match '^([a-zA-Z]+)(\(([^)]*)\))?!?:\s*(.+)$') {
      $type = $matches[1].ToLowerInvariant()
      $scope = $matches[3]
      $summary = $matches[4]
      if ($scope) { $item = "- **$scope**: $summary" } else { $item = "- $summary" }
      switch ($type) {
        'feat' { $bucket = $features }
        'fix' { $bucket = $fixes }
        'perf' { $bucket = $performance }
        default { $bucket = $null }
      }
    }
    else {
      # No type prefix: still a change worth listing, so it goes in the flat
      # list rather than being dropped. Only that list is rendered - the commit
      # log below carries the same text with its link, which is why this used
      # to appear twice.
      $item = "- $clean"
    }
    $item = ConvertTo-MentionSafe $item
    # `$null -ne` rather than truthiness: an empty List[string] evaluates to
    # $false in PowerShell, so `if ($bucket)` sent every typed commit to the
    # untyped list until each bucket already had an entry.
    if ($null -ne $bucket) { $bucket.Add($item) } else { $untyped.Add($item) }
    $log.Add("- [``$sha``](https://github.com/haxllo/nex/commit/$sha) $(ConvertTo-MentionSafe $subject)")
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

  # Only when something landed without a type prefix. An empty heading reads
  # worse than none at all.
  if ($untyped.Count) {
    $out.Add("### Changes")
    $out.Add("")
    $out.AddRange($untyped)
    $out.Add("")
  }

  # No "Notes" section by default. It used to emit a placeholder bullet
  # ("Limits worth knowing...") that shipped in every drafted release and
  # described nothing, so the section now appears only when -Notes is passed.
  if ($Notes -and $Notes.Trim().Length -gt 0) {
    $out.Add("### Notes")
    $out.Add("")
    foreach ($noteLine in ($Notes -split "`n")) {
      if ($noteLine.Trim().Length -gt 0) { $out.Add("- " + $noteLine.Trim()) }
    }
    $out.Add("")
  }
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
