<!-- release: 3.0.0 -->
<!--
  WHAT THIS FILE IS
  The page people see inside Nex, in the What's New view, after they update.
  It is NOT the GitHub release page: that one is
  docs/releases/v<version>-notes.md and carries the commit log. Do not copy one
  into the other.

  apps/core/build.rs bakes this file into the binary at compile time. Nothing
  is fetched at runtime.

  ## Where this page sits

  It does not open by itself. At launch the app compares the version actually
  running with whats_new_seen in state.json and, when they differ, shows a
  "See what's new" pill. The page opens when that pill is clicked, once per
  update. A fresh install records the version silently and never sees it.

  A page over an empty body is a working state while a release is being
  written: local builds render the fallback line, and the release workflow
  refuses to ship a file with no text outside these comments. That failure is
  the point of the rule, so do not "fix" it with placeholder prose.

  ## The marker

  The first line names the release this text is for. Update it with the prose.
  If it does not match the version being released the workflow warns and still
  ships, and the app shows whatever was written last time, which is worse than
  showing nothing.

  Do not quote comment syntax inside this block: a nested opener or a stray
  terminator closes the block early and these instructions appear as literal
  text in the app.

  HTML comments are removed before the text reaches the page (build.rs strips
  them), so use them freely for notes to yourself and to whoever edits this
  next. Everything outside a comment is shown to the user verbatim.

  ## What the app draws around this text

  The page is built from three parts, and only the middle one comes from here:

  1. The banner. docs/releases/assets/v<version>-hero.webp, or
     whats-new-hero.webp when no versioned file exists. It is baked into the
     binary as a data URI and placed between the heading and the first
     paragraph. Roughly 2.2:1, dark, no text in the image. A missing file warns
     at build time and draws no picture; it never fails a build. This markdown
     cannot include a picture of its own: there is one banner per release.
  2. The heading row. The first "## " line, printed heavy and short. The app
     puts this build's version on the same baseline at the right end of that
     row, as a button that opens the release page. Never write a version, a
     date, or a "changelog" label into the text: the page already has all
     three, and repeating them is what made three competing titles the first
     time this page was built.
  3. The body, from here. Everything between the heading and the first "### "
     is the intro and stays visible. Every "### " becomes a collapsed drawer
     that opens when clicked.

  ## Format

  - The FIRST "## " line is the heading. Four words or so, naming what the
    release does for the reader. A second "## " becomes another drawer, so one
    heading only.
  - The intro is two or three short paragraphs, always visible. Bullets are
    allowed there only when the list really is the whole story.
  - "### " is a drawer: a section title and its bullets. Order them by what
    people will look for first, keep each to a handful of lines, and stop at
    four or five. Drawers are cheap, but the page still has to be read.
  - "#### " inside a drawer is a bold sub-line, not a new drawer.
  - "- " is a bullet. Bold the part that names the thing, then say what
    changed in the reader's words: "closing a view no longer drops the panel",
    not a walkthrough of the fix. An indented line continues the bullet above
    it, so a long item can be written across lines.
  - Keep to what someone will notice. A change with no visible effect belongs
    in the release notes' commit log, not here.

  ## Voice

  Write to the person using Nex. Never mention tickets, commits, files,
  functions or tests. Aim for one screen before anything is opened.

  Run the prose through the humanizer skill before finishing:
  ~/.agents/skills/humanizer/SKILL.md. The tells it removes are the ones this
  page had the first time it was written: "not X but Y" contrasts, a staged
  opener ("The rest is..."), forced triads, dashes where a full stop belongs,
  borrowed marketing words ("seamless", "powerful", "effortless"), bold labels
  doing the work of a sentence, and the same "used to X, now Y" shape in every
  bullet. Read the result out loud. If a line sounds written to fill a section,
  cut the section instead.
-->

## Chat works in your files

Nex can read a file, write one, make a folder, move something, or run a command from the chat. Every step that touches your disk arrives as a card you can refuse.

A conversation can also carry a folder, so "open last week's invoices" finds them without a path.

### Chat

- Ask for a folder or file by name and Nex finds it in the index, without a disk walk.
- A large file is read in order, so the middle gets read too.
- Writing, editing and deleting all work, and a delete goes to the Recycle Bin. Something executable is flagged before it is saved.
- Commands run through PowerShell and print as they go. Stop ends the command, not just the answer.
- A file the model just made is previewed in the transcript.
- A reply that runs into the provider's output limit keeps going past it.
- When the model runs out of steps there is a Continue button.
- Questions about the machine get checked rather than answered from a stale guess.

### The transcript

- Markdown takes shape as it streams, so a long answer holds its formatting when it ends.
- Code is coloured and can be copied.
- A slow step says what it is doing and, once it is done, what it cost.
- A failure gets its own block, so a dropped connection leaves the reply above it alone.
- Stop leaves a mark where the answer ended.
- A long answer has a control that jumps back to the newest message.

### The folder a conversation is working in

- The folder sits in the bar above your message. Clicking it lists what is there and opens anything in it.
- **+** attaches a file from that folder to your next message, up to three.
- **×** clears it, and the model is told, so it stops treating bare names as paths.
- A step the model asks for mid-task can set the folder for the rest of that task.

### Approvals

- The card names what it is about to touch first: the file, the folder, the command.
- **Allow the rest** covers the remaining steps of that task. Deleting, running commands and opening unfamiliar links still ask every time.
- The card opens where you can see it.
- A step that never ran says why.

### The panel and media

- Tab opens Chat from the search box, and the box says so.
- Closing a view keeps the panel at its full height.
- Compact view shows no hover hints.
- Chat settings, conversation history and the composer follow the glass theme.
- Each playing app keeps its own session, so switching players leaves the right track on screen and the right dot lit.
- Album art follows the track, with a spinning record when a track has none.

### Fixes

- Log lines start with a local date and time.
- Quitting does not freeze when the indexer is slow to answer.
- The hotkey works over a fullscreen game, the tray says when it is being held back, and saving a new one does not flash console windows.
- Uninstalling an app from the menu returns to the panel.
- Removing a saved API key is acknowledged, and a provider saved without a URL is refused.
