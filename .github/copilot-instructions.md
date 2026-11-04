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
3. **Commit identity.** Author is the repo owner. Commit bodies must contain no
   AI/bot names, signatures, or co-author trailers. Do not commit unless asked.
4. **Process boundaries.** UI (Medium IL) never indexes; indexer (Medium IL)
   never renders; helper (High IL) is a broker only.
5. **Entry point.** `Nex.exe` was removed; use `NexUIProcess.exe`.
