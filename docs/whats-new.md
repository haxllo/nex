<!-- release: 2.41.0 -->
<!--
  WHAT THIS FILE IS
  This is the page people see inside Nex, in the What's New view, after they
  update. It is NOT the GitHub release page — that one is
  docs/releases/v<version>-notes.md and carries the commit log.

  EDIT THIS FILE BEFORE EACH RELEASE, IN THE SAME COMMIT AS THE VERSION BUMP.
  apps/core/build.rs bakes it into the binary at compile time.

  THE FIRST LINE OF THIS FILE IS A MARKER THAT NAMES THE RELEASE THIS TEXT IS
  FOR. Update it to the version being released. If it does not match, the
  release workflow still succeeds — it logs a warning and never blocks — but
  the What's New view will show whatever was written last time, which is worse
  than showing nothing. Update the marker and the prose together.

  Do not quote comment syntax inside this block: a nested opener or a stray
  terminator closes the block early and the rest of these instructions appear
  as literal text in the app.

  HTML COMMENTS ARE REMOVED BEFORE THEY REACH THE APP (build.rs strips them),
  so use them freely for notes to yourself and to whatever agent edits this
  next. Everything outside a comment is shown to the user verbatim.

  THE BANNER
  The picture at the top is docs/releases/assets/v<version>-hero.webp, or
  whats-new-hero.webp when there is no versioned file. It is baked into the
  binary as a data URI by apps/core/build.rs, so it needs no network and no
  cache. Missing file = no picture, with a build warning; it never fails a
  build. Roughly 2.2:1, dark, no text in the image. This markdown cannot
  include a picture itself: there is exactly one banner per release, and it is
  placed between the heading and the first paragraph.

  FORMAT
  - The FIRST "## " line is the page heading: a phrase that says what this
    release means to the person reading it, not a version number.
  - Everything between that line and the first "### " is the intro: two or
    three short paragraphs, always visible, no bullets unless the list really
    is the whole story.
  - Every "### " becomes a COLLAPSED DRAWER with its bullets inside. Order
    them by what people will look for first, and keep each one to a handful of
    lines. A "#### " inside a drawer is a bold sub-line.
  - "- " is a bullet. Bold the part that names the thing, then say what
    changed in the reader's words: "closing a view no longer drops the panel",
    not a walkthrough of the fix.
  - Keep it to what someone will notice. A change with no visible effect
    belongs in the release notes' Commit Log, not here. Drawers are cheap, so
    four of them is fine; ten is not.

  THE HEADING IS ONE SHORT HEAVY LINE
  Four words or so, naming what the release does for the reader. It is the
  only loud line on the page: no eyebrow above it, no second title, no version
  number in it. The version lives in the header row as metadata.

  RUN THE PROSE THROUGH THE HUMANIZER SKILL BEFORE FINISHING
  ~/.agents/skills/humanizer/SKILL.md. The tells it removes are the ones this
  page had the first time it was written: "not X but Y" contrasts, a staged
  opener ("The rest is..."), forced triads, dashes where a full stop belongs,
  borrowed marketing words ("seamless", "powerful", "effortless"), bold labels
  doing the work of a sentence, and the same "used to X, now Y" shape in every
  bullet. Read the result out loud. If a line sounds written to fill a section,
  cut the section instead.

  VOICE
  Write to the person using Nex. Never mention tickets, commits, files,
  functions or tests — those are for docs/releases/v<version>-notes.md. Aim
  for one screen before anything is opened.
-->

## Chat knows your folder

Chat can hold a folder for as long as a conversation lasts, so "fix the build
in here" works without a pasted path. One approval now covers the rest of a
task. Closing a view no longer drops the panel to a sliver, and Tab opens Chat.

### Chat

- **A folder for the conversation**: a chip in the header names the folder you
  picked. Click it to see what is inside, open a file, attach one to the next
  message, or clear it and go back to full paths.
- **One approval covers the task**: approve once and the remaining steps run
  without asking again. Deleting, running commands and changing the folder
  still ask every time.
- **The card says what will happen**: the sentence at the top matches the line
  left in the transcript, and the target it will touch is a full path.
- **The message box has depth**: a slightly raised surface, so it reads as a
  field in both themes.

### Search and the panel

- **Closing a view keeps the panel still**: it used to fall to a thin sliver
  for a moment before the right height came back.
- **Your message stays in view**: sending from halfway up the transcript left
  what you just wrote below the fold. There is a jump-to-newest button once you
  have scrolled away.
- **Tab opens Chat**: the first Tab goes there, and the placeholder says so.

### Media

- **Players keep their place**: sessions are tracked separately now, so the
  switcher no longer shuffles and one player cannot stand in for another.

### Fixes

- Quitting no longer leaves the mouse and keyboard unresponsive before the
  window closes.
- Opening Nex right after launch uses the same glass surface as every other
  time. It used to fall back to a flat dark panel while the page loaded.
