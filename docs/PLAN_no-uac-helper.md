## Plan: Replace `runas` with Scheduled Task (no UAC)

### Problem
Helper crashes → watchdog respawns → `ShellExecuteExW("runas")` → UAC prompt.

### Solution
Use a Windows scheduled task (created once, runs elevated) + `schtasks /run`.

### Implementation

**1. Create task on first launch (one-time UAC)**
- On first launch, `ShellExecuteExW("runas")` for `schtasks /create` with:
  - Task name: `NexHelper`
  - Run: `nex-helper.exe --pipe \\.\pipe\nex-hotkey --config "%APPDATA%\Nex\helper-config.json"`
  - Run level: `HIGHEST`
  - Trigger: `ONEVENT` (manual-only via `/run`)
- This is ONE UAC prompt, ever.

**2. nex.exe writes config, then runs task**
- Before spawn: write `%APPDATA%\Nex\helper-config.json` with target key, mods, target PID
- `std::process::Command::new("schtasks").args(["/run", "/tn", "NexHelper"])` — no UAC
- `connect_pipe()` retries until helper creates the pipe (same logic as now)

**3. Helper reads config from file**
- `--pipe` and `--config` args tell the helper where to create the pipe and where to read config
- Reads `helper-config.json` for target key, mods, PID

**4. Helper restarts without UAC**
- If helper crashes → watchdog detects → `schtasks /run` again → no UAC

**5. Drop skips TerminateProcess**
- `Drop` handler already checks `helper_process_handle.is_some()` before terminating
- With scheduled task, handle is `None` → skip termination → helper detects pipe break and exits naturally

### Files to change

| File | Changes |
|---|---|
| `apps/core/src/overlay/hotkey.rs` | New `spawn_helper_task()` replacing `spawn_helper()` + `ShellExecuteExW("runas")`. Create task if missing. Write config file. |
| `apps/helper/src/main.rs` | Read config from `--config` file instead of CLI-only. After pipe break, DON'T exit — wait for reconnection (makes helper persistent). |

### Files to create

| File | Content |
|---|---|
| `apps/helper/src/config.rs` | Config struct, read from JSON file |

### Alternative: persistent helper (reconnect)
Helper doesn't exit on pipe break → loops back to start. nex.exe reconnects on crash. This eliminates the watchdog entirely — no respawn needed.

### Risky bits
- `schtasks` CLI output parsing for error detection
- Config file race (helper reads before nex finishes writing)
- Multiple nex instances (unlikely in practice)
- Task creation needs absolute path to helper EXE (which we don't know at install time — resolved at first launch)
