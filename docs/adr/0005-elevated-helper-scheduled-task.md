# ADR-0005: Start the elevated helper through a scheduled task, not `runas`

## Status

Accepted.

## Date

2026-07-29

## Context

The helper must run elevated. The launcher also has to recover the helper when it
crashes or its pipe closes, and it does this automatically without the user asking.

The first implementation started the helper with `ShellExecuteExW` and the `runas`
verb. Every start therefore raised a User Account Control prompt. A helper that
crashed while the user was not watching produced a prompt on top of whatever they
were doing, and an automatic recovery path that interrupts the user is not a
recovery path.

## Decision

Register a Windows scheduled task named `NexHelperV2` that runs the helper at high
integrity, created once on first run behind a single UAC prompt. Subsequent starts
use `schtasks /run`, which raises no prompt.

The task is created with a manual trigger, so Windows never starts the helper on
its own; only Nex does.

## Alternatives considered

### Keep `runas` and accept the prompt

Rejected. The prompt appears at the worst possible moment, during an automatic
recovery the user did not initiate.

### Permanently elevated launcher

Run the whole application elevated.

Rejected because it puts a webview rendering HTML at high integrity, which is the
opposite of the reason the helper exists. See [0002](0002-three-process-split.md).

### No helper, accept a degraded hotkey

Ship without elevated input handling and document that the hotkey does not work
over elevated windows.

Rejected because the failure is invisible and looks like a bug. A launcher that
quietly stops responding whenever Task Manager is open is worse than one that asks
for permission once.

## Consequences

- One UAC prompt on first run is unavoidable. Declining it leaves Nex working for
  everything except elevated windows, and the tray tooltip reports
  `Nex (hotkey issue)` so the state is visible rather than silent.
- The helper's lifetime is managed by the Task Scheduler, so the UI does not hold a
  direct process handle. Stopping it is a task operation, not a handle operation,
  and the code must account for a helper that is already leaving.
- `schtasks` can report success while failing, so the task creation checks the exit
  code rather than trusting the launch.
- A user who uninstalls Nex by deleting the folder leaves the scheduled task behind.
  This is documented in the install guide rather than solved, because removing it
  automatically would require elevation at uninstall time.
- A corrupt or missing task is repairable by deleting it and restarting Nex, which
  is documented in the troubleshooting guide.
