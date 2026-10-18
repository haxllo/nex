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
  - No binary/download section: the packaged files are attached to the
    release itself, so linking them in the body only duplicates them.
  - Voice: the top of the notes is written for the person using nex, in
    normal words. Say what they will notice and what we did about it —
    "opening no longer shows a held picture", not a walkthrough of the
    fix. Naming what we changed (a function, a component, the swell) is
    fine where it helps — just not on every line, and never instead of
    saying what the person will see. A change with no visible effect
    belongs in the Commit Log only.
  - The Commit Log stays technical. It is the record for us; the notes
    above it are the explanation for everyone else.
-->

## What changed

- **{thing}**: {what it looked like before → what it looks like now, in
  words the person using nex would use}

### Features

- **{feature}**: {what they can now do that they could not before}

### Bug Fixes

- **{symptom they would have reported}**: {what was wrong, what it does now}

### Notes

- {limits worth knowing, and anything that could still look wrong}

## Commit Log

{list all commits since last tag, each linked to GitHub}
- [`{abbrev}`](https://github.com/haxllo/nex/commit/{abbrev}) {message}

