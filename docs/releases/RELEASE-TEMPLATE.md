{1-2 sentence summary of release}

**Note:** Do NOT include an H1 title/version heading here. GitHub uses `--title` separately — body starts with summary text. Avoid "v{N} — {title}" duplication.

**Pre-release checklist (BEFORE writing notes):**
0. Tag last: create tag `v{VER}` only AFTER the release-notes commit is on master — otherwise the tag lags master by commits and GitHub shows a phantom "1 commit" gap.
1. `git log v{PREV}..HEAD --oneline` — every commit must appear in the Commit Log; no silent omissions.
2. List every user-reported issue since v{PREV} (session reports, GitHub issues, logs) — each must be in the changelog below, linked to its fix, or explicitly deferred with reason.
3. Work the changelog from the commit list + user issue list; if a commit fixes a user-visible bug, it belongs under Bug Fixes, not only in the Commit Log.
4. Binary links must match `{VER}` exactly — build artifacts first, verify names exist in `artifacts/windows/`.

## User-Reported Issues (checklist — completes step 2)

- [ ] {issue reported} → {fixed in this release / deferred: reason}
- [ ] {issue reported} → {fixed in this release / deferred: reason}

## Changelog ({N} commits since v{PREV})

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
