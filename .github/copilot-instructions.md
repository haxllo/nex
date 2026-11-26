# Nex repository instructions

Read `AGENTS.md` first; it is authoritative. The rules that must not be
skipped:

1. **Linear first.** For any bug report, feature request, or implementation
   task, search the Nex Linear project (P-CIQ-2) for a matching issue before
   editing code. Reuse it or create one, then keep it current. Questions,
   planning, and status requests do not need an issue.
2. **Verify, then report.** Build release with
   `CARGO_TARGET_DIR=G:\nex-target` and run the relevant gate. Never claim a
   check passed unless it was run.
3. **Commit identity.** Author is the human responsible for the change — the repo
   owner here, an outside contributor on their own PR — under their own real
   address; never a bot, an AI, or a placeholder like `<your-email@example.com>`,
   which GitHub credits to whoever registered it. Commit bodies must contain no
   AI/bot names, signatures, or co-author trailers. Do not commit unless asked.
4. **Process boundaries.** UI (Medium IL) never indexes; indexer (Medium IL)
   never renders; helper (High IL) is a broker only.
5. **Entry point.** `Nex.exe` was removed; use `NexUIProcess.exe`.

6. **Branch first.** Issue implementation work goes on its own branch, never
   straight to `master` unless the user explicitly asks for master. The
   `commit-msg` hook refuses an issue commit on `master`/`main`.
7. **Releasing is CI's job.** Bump the three crate versions, rewrite
   `docs/whats-new.md` (prose plus its `release:` marker), commit, push a `v*`
   tag. `.github/workflows/release.yml` drafts the release notes, builds,
   packages and publishes. Never write `docs/releases/v<ver>-notes.md` — CI
   generates it. Do not run the packaging scripts, and do not release unless
   the user asked for one.
