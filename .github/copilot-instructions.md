# Nex repository instructions

Read `AGENTS.md` first; it is authoritative. The rules that must not be
skipped:

1. **Find the existing issue first.** For any bug report, feature request, or
   implementation task, follow "Finding the issue you are about to fix" in
   AGENTS.md before editing code: the backlink comment on a GitHub issue names its
   Linear id, and a search needs several wordings before you may conclude nothing
   matches. Reuse or widen what you find; never open a near-duplicate. Outside
   contributors record work in GitHub issues and are not asked for a Linear id.
2. **Verify, then report.** Build release with
   `CARGO_TARGET_DIR=G:\nex-target` and run the relevant gate. Never claim a
   check passed unless it was run.
3. **Commit identity.** Author is the human responsible for the change — the repo
   owner here, an outside contributor on their own PR — under their own real
   address, never a placeholder like `<your-email@example.com>` (GitHub credits it
   to whoever registered it). For the owner's own commits, no bot/AI name as author
   and no AI/bot trailers or signatures in the body; a contributor's commit message
   is their own business. Do not commit unless asked.
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
