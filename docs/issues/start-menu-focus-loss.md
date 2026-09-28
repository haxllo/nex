# Start-menu focus loss — OPEN

## Reproduction

1. Open the Windows Start menu.
2. While Start is still open, press the Nex hotkey.
3. Close the Start menu.
4. Press the Nex hotkey again.

## Observed behavior

- The Nex panel is visible and populated.
- The search input does not receive focus automatically.
- Typing does not work until the input is clicked manually.
- Confirmed with the bare-Win hotkey.
- Restarting Nex clears the condition.

## Current hypothesis

The outer Nex window can retain OS foreground ownership while the inner WebView2 search input remains unfocused after Start/Explorer takes and returns focus. Thus retrying only the outer-window foreground operation is not sufficient; recovery must also reassert DOM input focus.

## Investigation status

- Focus recovery through `UiCommand::FocusInput` and `UiCommand::FocusReassert` has been revised but has not resolved the issue.
- The revised recovery remains uncommitted on `fix/pr28-fast-icons`.
- Further diagnosis should capture the exact `Hotkey`, `FocusInput`, `FocusReassert`, and `WindowEvent::Focused` sequence during the reproduction.
