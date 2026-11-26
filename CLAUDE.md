# Nex — Agent Entry Point

`AGENTS.md` is the source of truth. Read it before changing anything; this file
only repeats the rules that must never be skipped.

1. **Linear first.** Search the Nex project (P-CIQ-2) for a matching issue
   before touching code. Reuse it or create one, and set its state from what
   actually happened. Questions and planning do not need an issue.
2. **Verify, then report.** Use `CARGO_TARGET_DIR=G:\nex-target` and a release
   build; never claim a check passed unless it was run.
3. **Commit identity.** Author is the human responsible for the change — the repo
   owner here, an outside contributor on their own PR — under their own real
   address; never a bot, an AI, or a placeholder like `<your-email@example.com>`,
   which GitHub credits to whoever registered it. Commit bodies contain no
   AI/bot names or co-author trailers. Do not commit unless asked.
4. **Process boundaries.** `NexUIProcess.exe` (Medium IL) never indexes,
   `NexIndexerProcess.exe` (Medium IL) never renders, and `NexHelper.exe`
   (High IL) is a broker, not a supervisor.
5. **Entry point.** `Nex.exe` no longer exists; the UI binary is
   `NexUIProcess.exe`.

6. **Branch first.** Issue implementation work goes on its own branch, never
   straight to `master` unless the user explicitly asks for master. The
   `commit-msg` hook refuses an issue commit on `master`/`main`.
7. **Releasing is CI's job.** Bump the three crate versions, rewrite
   `docs/whats-new.md` (prose plus its `release:` marker), commit, push a `v*`
   tag. `.github/workflows/release.yml` drafts the release notes, builds,
   packages and publishes. Never write `docs/releases/v<ver>-notes.md` — CI
   generates it. Do not run the packaging scripts, and do not release unless
   the user asked for one.
