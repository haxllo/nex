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

  - The FIRST "## " line is the heading. Four words or so. Which words is the
    ladder under "Choosing the heading"; a second "## " becomes another
    drawer, so there is one heading only.
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

  ## Choosing the heading

  Three shapes, in order. The first one that is true names the release.

  1. It adds a capability: something the reader could not do before (attach a
     PDF, ask about an image). Name the capability the way they would describe
     using it, not as a spec line or a changelog entry: "Pictures and PDFs in
     the conversation", not "Attachment Support Added".
  2. Nothing new to do, and the work sits on one surface (the media panel, the
     glass edge). Name the surface: "The media panel", "The glass edge". This
     is the normal shape when a release is fixes and adjustments, and it is
     honest: it tells the reader where to look.
  3. Neither: no new capability and no single surface. Then "Fixes and
     adjustments" is the heading. It is the last resort, never a habit, and it
     must not hide the one item people will notice: that item gets a drawer of
     its own and the rest follow underneath.

  A cosmetic adjustment never earns a heading. More blur, a slightly deeper
  corner, a warmer edge light are bullets inside the surface's drawer. If a
  reader would not spot the difference, the line belongs in the release page
  and not here at all.

  Never a heading that names the defect it replaced or claims a win ("The rim
  shows only the pane", "Search is fixed"), a version-diff opener ("The glass
  edge changed again"), a category used as decoration ("Polish", "Improvements",
  "Stability") where a surface would do, a superlative, "finally", "only", an
  exclamation mark, or a comparison with a build the reader never saw.

  ## What earns a line

  The page answers one question: after updating, what will I see that is
  different? Sort each change into one of these before wording it.

  - A feature lets the reader do something they could not do. It earns the
    heading plus a drawer.
  - An adjustment changes the result of a task they already had: blur amount,
    corner depth, edge light, row order. It is a bullet, never a heading.
  - Noise is a change you can only describe with a number that exists on
    screen ("blur 8 to 10"). It stays in the release page's commit log.

  Performance and robustness count only when the reader would notice them:
  typing, opening, or a crash that stopped happening. "Under the hood" is
  optional, at most three lines, and only for a change that is invisible but
  noticeable (fewer rescans, less memory, faster startup). Mechanism stays in
  the release page.

  A bullet names the thing, then says what the reader sees. Two lines at most,
  four to six in a drawer. Numbers only when they describe something visible.
  A "used to" clause is allowed once per drawer, where the sentence is
  otherwise meaningless; the same past-against-present shape on every bullet
  means a defect story is being told instead of a behaviour.

-->






## The glass edge

The rim reads the backdrop inside the panel and stretches it toward the border, so the glass shows what is behind the panel rather than what sits beside the window.

- **Reading from the pane itself.** Colour comes from just inside the edge, across the whole band.
- **Edge light sized to the band.** The glow grows with the panel and fades out at the end, so the dock and a tall panel read the same.
- **Corners with the sides' depth.** The bend shortens through a corner, which keeps the turn as deep as the straight runs.
- **Each half of an edge pulls toward its own corner,** so the rim gathers where corners meet.
- **No blur between the desktop and the glass.** The frost and a thin rim line carry the effect.

### Under the hood

- When Windows moves its change journal while Nex is reading it, the indexer retries instead of rescanning the whole drive.
