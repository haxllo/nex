# Taste (Continuously Learned by [CommandCode][cmd])

[cmd]: https://commandcode.ai/


# codebase
- SOURCE_AUDIT_BUGS_FIXES_IMPROVEMENTS.md is the authoritative documentation source; other docs may be unreliable. Confidence: 0.70

# git
- Use multi-line commit messages with detailed bullet points and include "Co-authored-by: CommandCodeBot <noreply@commandcode.ai>" as the last line. Confidence: 0.75
- Commit each fix independently — do not batch multiple fixes into a single commit. Confidence: 0.70

# debugging
- Add diagnostic logging before making targeted fixes, then revert the diagnostics in a clean commit (no debug cruft in final code). Confidence: 0.70

# build
- Verify every change with `cargo build -p nex-launch --bin nex` before committing. Confidence: 0.75

# documentation
- Use plain text download links (no emoji icons) in the release template Binary section. Confidence: 0.70

# testing
- Do not run `cargo test` — tests are broken after major migrations and will not pass. Confidence: 0.85

