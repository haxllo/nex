## {Descriptive release title}

{1-2 sentence summary of release — plain words, what it feels like for the user}

<!--
  PUBLISHING RULES (never ship this block — it is an HTML comment):
  - GitHub release title must be exactly `v{VER}` (version only, no
    descriptive suffix — the descriptive title lives in the body heading).
  - Tag last: create tag `v{VER}` only AFTER the release-notes commit is
    on master — otherwise the tag lags master and GitHub shows a phantom gap.
  - `git log v{PREV}..HEAD --oneline` — every commit must appear in the
    Commit Log; no silent omissions. Work user-visible fixes into the
    changelog, not only the log.
  - Binary links must match `{VER}` exactly — build artifacts first,
    verify names exist in `artifacts/windows/`.
  - Voice: explain what happened and why it mattered, plain words for
    readers (future us, contributors, curious people). No "user" wording,
    no corporate changelog tone. Technical terms only when unavoidable.
-->

## What changed

- **{thing}**: {what it was like before → what it's like now}

### Performance

- **{label}**: {description}

### Features

- **{feature}**: {description}

### Bug Fixes

- **{issue}**: {fix}

### Architecture

- **{change}**: {reason}

## Commit Log

{list all commits since last tag, each linked to GitHub}
- [`{abbrev}`](https://github.com/haxllo/nex/commit/{abbrev}) {message}

## Binary

- [Download `nex-{VER}-windows-x64.zip`](https://github.com/haxllo/nex/releases/download/v{VER}/nex-{VER}-windows-x64.zip)
- [Install `nex-{VER}-windows-x64-setup.exe`](https://github.com/haxllo/nex/releases/download/v{VER}/nex-{VER}-windows-x64-setup.exe)
- [Manifest `nex-{VER}-windows-x64-manifest.json`](https://github.com/haxllo/nex/releases/download/v{VER}/nex-{VER}-windows-x64-manifest.json)
