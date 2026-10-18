# Hotkey double-toggle at startup (one press toggles twice) — RESOLVED

Symptom: for roughly the first 8–10 s after launch, one hotkey press toggled the
overlay twice (show then hide) and felt laggy; afterwards it behaved. Bare `Win`
was the reported hotkey, but any combo with more than one detection path is
exposed.

## Cause

Four paths report the same physical press:

| Path | Where | Covers |
|---|---|---|
| Raw input (`WM_INPUT`) | `overlay/host.rs` | nex's own path, normal focus |
| In-process LL hook | `overlay/hotkey.rs` | window before the helper connects; combos `RegisterHotKeyW` refuses |
| `WM_HOTKEY` fallback | `overlay/hotkey.rs` | combos `RegisterHotKeyW` delivers |
| Helper pipe `HOTKEY` | `apps/helper` → `hotkey.rs` pipe reader | **only** path that sees elevated focus (UIPI blocks raw input and the Medium-IL hook) |

They fire within milliseconds of each other, and dedup used to run in the runtime
worker **on receipt** (`HOTKEY_DEBOUNCE`, 50 ms). That anchor is wrong whenever
the worker is busy: during startup (first show, quick launch, icons, indexing)
the first event is dequeued immediately, the worker then spends hundreds of ms
elsewhere, and the duplicate is dequeued *seconds* later — after the window has
expired. Both copies toggled. Once startup work drained, the copies dequeued back
to back and the window caught the second, which is why the bug "fixed itself"
after ~10 s.

Evidence in `%APPDATA%\Nex\logs\nex.log`: at startup the run shows
`WM_INPUT bare Win key-up, toggling` and `pipe: HOTKEY received from helper`
with no `Hotkey: debounced (within 50ms window)` between them, and that debounce
line only starts appearing later in the same run. Also `show_latency worker_ms=809`
on the first show.

## Fix

`87ac9f8`. Dedup moved to the sender: `hotkey::send_hotkey_event()` (50 ms
same-press gate over a process-local clock, `same_press()` is the pure predicate).
All six real senders route through it; the runtime's receipt-time debounce and its
`HOTKEY_DEBOUNCE` static are gone. A replayed helper backlog collapses the same
way — those lines arrive back to back.

## History (why it came back)

- **v2.10.0** (`5b10d71`, `20190ae`): removed the `WM_INPUT` Win toggle so only
  the hook/helper consumed the Win key-up — no double-toggle.
- **6854445** (`refactor(hotkey): nex owns all detection`): re-added the
  `WM_INPUT` Win key-up toggle, helper out of detection.
- **055a47a** (`nex owns detection, helper assists Win focus over elevated
  windows`): re-enabled helper Win detection (`detect: target_is_win` in
  `helper-config.json`). Two consumers of one key-up again.
- **v2.26.0**: "Stale duplicate toggles: single-consumer Win key-up, parity
  collapse" — that compensating fix was **reverted during bisect**, so the pair
  was guarded by nothing but the receipt window.

## Rules for future hotkey work

- Every sender of `OverlayEvent::Hotkey` calls `hotkey::send_hotkey_event`. No
  direct `event_tx.send(OverlayEvent::Hotkey(..))`.
- Never add receipt-time dedup in the runtime; the queue delay is not the press.
- Never delete a detection path for being redundant: raw input does not reach
  elevated foreground windows, and the Medium-IL hook is blocked there too. The
  helper's `HOTKEY` line is the only reason the hotkey works over Task Manager.
- Diagnosing a recurrence: grep the log for `duplicate detection path ignored`
  (a drop happened), `duplicate` count per press, and `show_latency worker_ms` to
  confirm worker congestion. With the gate in place a double-toggle means a
  sender bypassed it, not that the window is too small.
