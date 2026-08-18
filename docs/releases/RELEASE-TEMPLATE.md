{1-2 sentence summary of release — plain words, what it feels like for the user}

**Note:** Do NOT include an H1 title/version heading here. GitHub uses `--title` separately — body starts with summary text. Avoid "v{N} — {title}" duplication.

**Pre-release checklist (BEFORE writing notes):**
0. Tag last: create tag `v{VER}` only AFTER the release-notes commit is on master — otherwise the tag lags master by commits and GitHub shows a phantom "1 commit" gap.
1. `git log v{PREV}..HEAD --oneline` — every commit must appear in the Commit Log; no silent omissions.
2. List every issue found since v{PREV} (bugs found while testing, GitHub issues, logs) — each must be in the changelog below, linked to its fix, or explicitly deferred with reason.
3. Work the changelog from the commit list + issue list; if a commit fixes a user-visible bug, it belongs under Bug Fixes, not only in the Commit Log.
4. Binary links must match `{VER}` exactly — build artifacts first, verify names exist in `artifacts/windows/`.

**Voice (IMPORTANT):** These notes are for readers — anyone who picks up the release page: future us, contributors, and curious people checking what changed. Write like you're explaining to a smart friend, not like a corporate changelog:
- No "User-Reported Issues" section, no "user" wording — it's "what we fixed", "what changed", "we found that..."
- Explain *what happened and why it mattered*, not just the technical fix. Say "pressing the hotkey used to stall until the index finished" instead of "blocking read on the service lock was replaced with try_read".
- Keep technical terms only when there's no plain way to say it (e.g. "search index" is fine, "tantivy" is not; "web engine" instead of "WebView2 environment").
- Short paragraphs under "What changed" bullets. One idea per bullet.

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