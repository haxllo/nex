<!--
  WHAT THIS FILE IS
  This is the text people see inside Nex, in the What's New view, after they
  update. It is NOT the GitHub release page — that one is
  docs/releases/v<version>-notes.md and carries the commit log.

  EDIT THIS FILE BEFORE EACH RELEASE, IN THE SAME COMMIT AS THE VERSION BUMP.
  apps/core/build.rs bakes it into the binary at compile time, so a build
  without it produces an app whose What's New view is the tips fallback only.
  A unit test fails when the baked text is empty.

  HTML COMMENTS ARE REMOVED BEFORE THEY REACH THE APP (build.rs strips them),
  so use them freely for notes to yourself and to whatever agent edits this
  next. Everything outside a comment is shown to the user verbatim.

  FORMAT
  - The first "## " line becomes the page heading. Make it say what this
    release means for the person reading it, not the version number.
  - Then a short paragraph — two or three sentences, plain words.
  - "### " makes a section heading. Use sections when a release has enough
    changes that a flat list becomes a wall; skip them when it does not.
  - "- " makes a bullet. Bold the part that names the thing, then say what
    changed in words the reader uses: "opening no longer shows a held
    picture", not a walkthrough of the fix.
  - Keep it to what someone will notice. A change with no visible effect
    belongs in the release notes' Commit Log, not here.

  VOICE
  Write to the person using Nex. Never mention tickets, commits, files,
  functions or tests — those are for docs/releases/v<version>-notes.md.
  Aim for well under a screen of scrolling. If it needs more, the release
  probably needs splitting into sections rather than more bullets.
-->

## What's new

<!--
  Replace the heading and the lines below for each release. Steps:
  1. Change the "## " heading to this release's one-line summary.
  2. Rewrite the paragraph under it.
  3. Replace the bullets. Delete sections you do not need.
  4. Leave this file's comments in place; they are stripped at build time.
-->

This update focuses on how the app starts and how it closes, along with the
notes you see right here.

### Changes

- **The first thing you see**: opening Nex right after launch now comes up
  with the same glass surface as every other time, instead of briefly
  falling back to a flat dark panel.
- **Closing the app**: quitting no longer leaves the mouse and keyboard
  unresponsive for a couple of seconds before the window goes away.
- **This page**: what you are reading is now written for each release by
  hand, so it says what changed rather than listing every commit.
