<!-- release: 3.1.1 -->
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

  The page is built from two parts, and only the second one comes from here:

  1. The heading row. The first "## " line, printed heavy and short. The app
     puts this build's version on the same baseline at the right end of that
     row, as a button that opens the release page. Never write a version, a
     date, or a "changelog" label into the text: the page already has all
     three, and repeating them is what made three competing titles the first
     time this page was built.
  2. The body, from here. Everything between the heading and the first "### "
     is the intro and stays visible. Every "### " becomes a collapsed drawer
     that opens when clicked.

  There is no banner. A release does not need a picture, ship no image, and a
  build that finds none says nothing about it.

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



## The rim shows only the pane

The glass edge changed again. The rim now reads what sits under the panel instead of what sits beside it, so a small window no longer repeats the desktop next to it inside its own border.

The light along the edge follows the band's width too, which makes the dock and a tall panel read as the same piece of glass.

### The glass edge

- **The rim reads from inside.** The border samples the backdrop under the pane and pulls it out toward the edge, so nothing from outside the window can appear in it.
- **Corners keep the depth of the sides.** The bend shortens through a corner instead of stopping halfway, so the turn matches the straight runs.
- **The glow follows the band.** It used to be the same 14 pixels whatever the band was, covering a quarter of a tall panel's band and two thirds of the dock's. It now grows with the band and fades out at the end instead of stopping on a step.
- **The fan.** Both halves of each edge pull toward their own corner, so the rim gathers at the corners instead of travelling one way around the border.
- **Less blur behind it.** The desktop passes through unblurred, leaving the frost and the thin rim line to do the work.

### Under the hood

- When Windows moves its change journal while Nex is reading it, the indexer retries instead of rescanning the whole drive.
