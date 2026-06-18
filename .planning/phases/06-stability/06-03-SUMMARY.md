# 06-03: Hotkey Conflict Detection + Atomic Config Save

## Changes

### Task 1: Hotkey Conflict Detection
**File: `apps/core/src/runtime_process.rs`**
- Added `detect_hotkey_conflict_process(hotkey: &str) -> Option<String>` — matches the last key in a hotkey string against a known-conflict table (PowerToys, AHK, Discord, NVIDIA, AMD, Xbox Game Bar, Snipping Tool, OneNote) and returns a user-friendly conflict message.
- Updated `hotkey_registration_recovery_message` to call `detect_hotkey_conflict_process` and inject the conflict info before the suggestions.
- Updated `hotkey_registration_status_text` to call `detect_hotkey_conflict_process` and inject the conflict info before the suggestions.

### Task 2: Atomic Config Save with Backups
**File: `apps/core/src/config.rs`**
- `save_to_path`: Creates a timestamped `config.backup-{unix_ts}.toml` copy before each write (if the config file already exists).
- After a successful write, old backups are cleaned up — only the 5 most recent backups are kept.
- `write_atomic` already used temp-file + rename + backup recovery — left unchanged.

## Status: COMPLETE
## Verdict: PASS (`cargo check -p nex-cli` — zero errors, only pre-existing warnings)
