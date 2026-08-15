# Laptop Handling Report

Investigation date: 2026-08-15. Read-only audit — no code changes.

Scope: how Nex behaves on laptops across the hardware-specific Windows
states: sleep/resume, dock/undock + display scaling, session lock (lid
close), battery/power state, and the elevated-helper hotkey path.

## Summary

Nex has **no explicit laptop/power-aware logic**. It does not monitor
power, session, or display-topology changes. Several behaviors are
handled *implicitly* by per-show recomputation (positioning + scale),
which covers the most visible laptop failure modes (overlay on the wrong
monitor after docking, wrong DPI after scaling change). The gaps are
listed in the Findings section.

## Findings

### 1. Sleep / resume — NO handling

- No `WM_POWERBROADCAST` / `PBT_APMRESUMEAUTOMATIC` handling anywhere in
  `apps/core` or `apps/helper`.
- No hide-on-sleep, no re-registration of hotkeys after resume, no
  clock/index refresh trigger after resume.
- Implicit mitigation: the overlay hides on focus loss (`Focused(false)`
  + grace → `Escape`, host.rs:652-669), so a laptop sleeping with the
  overlay open will typically dismiss it on wake when focus goes
  elsewhere.
- WH_KEYBOARD_LL hooks: Windows can silently unload low-level hooks
  after sleep or when the hook thread stops pumping. Nex's hook thread
  (overlay/hotkey.rs) runs a continuous `GetMessageW` pump, so the hook
  survives in practice; there is no watchdog/re-hook path if Windows
  drops it.
- Risk: hotkey dead after resume if the hook is unloaded and the
  RegisterHotKeyW fallback (hotkey.rs:690, helper main.rs:672) is not
  active (Win-key hotkeys have no fallback).

### 2. Dock / undock, monitor topology, DPI — implicit only

- `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` once at startup
  (runtime_loop.rs:206).
- No `WM_DISPLAYCHANGE`, `WM_DPICHANGED`, or `ScaleFactorChanged`
  handlers in the tao/wry event loop (host.rs handles only `Focused`).
- Mitigation: the overlay is repositioned on **every show** via
  `cursor_monitor_work_area()` + `window.scale_factor()`
  (host.rs:1320-1355, `MonitorFromPoint` on the cursor), so after a
  dock/undock or scaling change, the next show lands on the correct
  monitor at the correct scale — even if the window geometry was stale.
- Residual risk: while the window is *visible* during a topology change
  (rare: hotkey → dock mid-show), geometry is not corrected until the
  next show.
- The window is created `WS_POPUP` and sized in logical px via tao; wry
  applies the current scale factor at creation.

### 3. Session lock (lid close, Win+L) — NO handling

- No `WM_WTSSESSION_CHANGE` / `WTSRegisterSessionNotification`, no
  `WM_ENDSESSION` handling, no hide-on-lock.
- Mitigation is the same focus-loss Escape path (host.rs:652-669): the
  secure desktop steals focus → nex hides after the grace window.
- No re-detect of theme/accent after unlock (accent is re-read on Show,
  so the first show after unlock refreshes it — see 2.14.2 accent
  fix).

### 4. Battery / power state — NO awareness

- No `GetSystemPowerStatus` / AC-vs-battery checks anywhere.
- Indexing (startup indexer thread + background refresh,
  everything_bridge.rs:28) runs regardless of power source.
- No battery threshold for indexing, file watching, or update checks.
- Updater (updater.rs) is only invoked on command — no background check
  to throttle.

### 5. Power actions (the laptop-facing feature)

- power_actions.rs: Lock via `LockWorkStation` (rundll32), Sleep via
  `SetSuspendState` (powrprof rundll32, args `0,1,0`), Shutdown,
  Restart, Sign Out — all through confirmation rows
  (runtime_actions.rs:59-76) and the tray menu (host.rs:905-909,
  `TrayLock`/`TraySleep`).
- These are the only laptop-specific user features; they work over the
  normal Windows APIs and are unaffected by the gaps above.

### 6. Elevated helper (NexHelper.exe)

- Runs at High IL via scheduled task (NexHelperV2), created on first
  run. Separate process — survives laptop sleep; reconnects to nex.exe
  over a named pipe.
- WH_KEYBOARD_LL hook installed once at startup (helper main.rs:655);
  no re-hook after resume (same risk as #1, with RegisterHotKeyW
  fallback for non-Win hotkeys at helper main.rs:672).
- UAC for task creation comes from a background context (nex startup) —
  Windows may park the consent prompt in the taskbar instead of the
  secure desktop (reported on this machine during 2.14.2 install).
  Windows-side behavior; candidate mitigation: defer task creation
  until nex is foreground.

## Gaps (no action taken — report only)

| # | Gap | Impact | Suggested fix direction |
|---|---|---|---|
| 1 | No sleep/resume handling | Stale overlay geometry/state on wake; possible hotkey loss | `WM_POWERBROADCAST` listener → hide overlay on suspend; re-arm hotkey watchdogs on resume |
| 2 | No display-change handling while visible | Wrong position/scale mid-show after dock | Handle `WM_DISPLAYCHANGE`/`ScaleFactorChanged` → recompute position |
| 3 | No session-lock handling | Overlay may linger on lock screen | `WM_WTSSESSION_CHANGE` → hide + reset state on lock |
| 4 | No battery awareness | Indexing/refresh burn battery on AC-less laptops | Check `GetSystemPowerStatus`; pause background index on battery |
| 5 | No hotkey re-hook watchdog | Hotkey dead after sleep if Windows unloads the LL hook | Periodic verification + re-`SetWindowsHookExW` |
| 6 | UAC-from-background task creation | Prompt minimized to taskbar (Windows-side) | Defer NexHelperV2 task creation until first foreground show |

## Verified OK

- Per-show repositioning covers dock/undock and scale changes between
  shows (host.rs:1320-1355).
- Focus-loss → auto-hide covers sleep and lock in the common case
  (host.rs:652-669).
- Power actions use standard Windows APIs (power_actions.rs).
- 2.14.2 accent re-detect on Show refreshes theme/accent after
  unlock/resume.