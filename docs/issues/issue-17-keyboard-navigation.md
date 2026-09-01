# Issue #17 — Keyboard Navigation (Investigation)

**Branch:** `investigation/issue-17-keyboard-nav`
**Reporter:** haxllo
**Opened:** 2026-08-31
**Status:** OPEN — no comments, no labels, no triage
**Body:** "cannot navigate with keyboard"

---

## TL;DR

Issue #17 is a **misclassification**. Keyboard navigation already works
for the primary flow (typing in the input moves selection through results;
Arrow keys navigate; Enter opens; Escape closes). The actual gap is
**visual selection feedback** — the user can't *see* which row is
selected because the `.selected` class has **no CSS rule**. So the issue
is real, but it's a visual/a11y gap, not a missing handler.

A secondary gap exists: the overlay is a popup that does not auto-focus
the query box when shown in some scenarios (tray menu hotkey path,
focus-loss recovery) — focus is sometimes lost to the parent window
before the user can type.

---

## What already works

`apps/core/assets/app.js:644-711` — primary keyboard handler (capture
phase on `window`):

| Key | Action | Source |
|---|---|---|
| `>` | Enter command mode | `app.js:649-653` |
| `Backspace` (in command mode, empty query) | Exit command mode | `app.js:655-661` |
| `ArrowDown` / `ArrowUp` | `moveSelection(±1)` | `app.js:663-664` |
| `Ctrl+J` / `Ctrl+K` | Same as ArrowDown/Up | `app.js:665` |
| `ArrowLeft` / `ArrowRight` | Grid nav (only in grid mode) | `app.js:666-683` |
| `Enter` | Submit | `app.js:685-687` |
| `Escape` | Close power menu → context menu → `post("escape")` | `app.js:689-708` |
| `Ctrl+Home` / `Ctrl+End` | Jump to first/last selectable | `app.js:664` (via `moveSelection`) |

A second handler at `app.js:931-937` closes the context menu on Escape.

Selection state is tracked:
- `let selected = 0` — `app.js:31` (or `-1` when empty — `app.js:163`)
- `setSelected(i, scroll)` — `app.js:441-451` toggles `.selected` class
- `moveSelection(delta)` — `app.js:473-480`
- `moveSelectionGrid(dx, dy)` — `app.js:519-546`
- `rowMap` — `app.js:35`, rebuilt every render — `app.js:378-381`
- `scrollToSelected()` — `app.js:463-471` (`scrollIntoView({ block: "nearest" })`)

Rust side maintains authoritative index via `ShimState.selected` and
`set_selected_index()` (`apps/core/src/overlay/shim.rs:401-410`,
`overlay/model.rs:119`). `runtime_loop.rs` has 25+ touchpoints keeping it
within `[0, max_results]`.

---

## What's actually broken

### Gap 1 — selected row has no visible style (HIGH)

`.selected` class is set by JS but **style.css has zero `selected`
selectors** (verified — full grep of `apps/core/assets/style.css` for
`selected` returns nothing). The only row highlight comes from
`.row:hover` (`style.css:232`). When the user moves the cursor away
the row looks identical to every other row.

User experience: keyboard navigation works, but the user has no idea
where they are in the list. **This is what #17 is reporting.**

### Gap 2 — focus lost when overlay shown via tray (MEDIUM)

The Rust host calls `focus_input()` (`apps/core/src/overlay/host.rs:1432-1436`)
via `evaluate_script("window.nex&&window.nex.focus()")` from these sites:
- `host.rs:468` (FocusInput command)
- `host.rs:670`, `host.rs:692` (initial show)
- `host.rs:716` (FocusReassert)
- `host.rs:871` (Focused(false) re-assert path)

But:
- Tray-menu path shows the overlay without re-asserting focus
  (`apps/core/src/overlay/tray.rs` — show path does not call
  `focus_input`).
- After `Escape` closes the power menu (`app.js:689-708`) focus
  returns to `<input id="query">` via `input.focus()` at `app.js:691`,
  but if the WebView itself lost OS-level focus (e.g. user clicked
  the title bar), the WebView must regain focus first.

### Gap 3 — no a11y semantics on selected row (LOW)

`<ul id="list" role="listbox">` (`apps/core/assets/index.html:55`) and
rows have `role="option"` (`app.js:213`) — but:
- No `tabindex` on rows (would be wrong for a listbox; only the listbox
  itself should be tabbable, which it isn't either).
- No `aria-selected="true|false"` is ever set on rows.
- No `aria-activedescendant` on the listbox pointing at the selected row.
- `<input id="query">` has no `aria-controls` linking to `#list`.

Screen-reader users can't tell where selection is, and the listbox
pattern is implemented halfway.

### Gap 4 — no way to keep results, only navigate (LOW)

Pressing Arrow keys while in the input doesn't have a `Home`/`End` (only
Ctrl+Home/End), no `PageUp`/`PageDown`. Grid view has no per-row `End` to
jump to end of current row. Not a blocker — typical launcher power users
expect this.

### Gap 5 — type-to-jump missing (NICE-TO-HAVE)

Many launchers (Spotlight, Raycast, Flow) let users press a letter to
jump to the next row starting with that letter when not in the input.
Nex's textbox swallows everything while focused, which is the
correct Spotlight behavior — flagging only because the user said
"cannot navigate with keyboard" and might mean something adjacent.

---

## Reproduction (for Gap 1, primary)

1. Build: `cargo build --release --bin Nex`
2. Launch: `./nex.exe`
3. Press global hotkey to show overlay.
4. Type a partial query (e.g. `git`).
5. Use ArrowDown once. Selection *moves* internally (Rust confirms via
   `selected_index`); the row has `class="selected"` in DOM (DevTools).
6. Move mouse off the overlay. The previously-selected row is
   **visually identical** to its neighbors.

Expected: the selected row has a background, border, or accent.
Actual: no `.selected` rule exists in `style.css`.

---

## Files to touch (proposed fix scope)

- `apps/core/assets/style.css` — add `.row.selected { ... }` block
  (also `.row.selected .title`, etc. for contrast).
- `apps/core/assets/app.js` — optionally add `aria-selected` toggling
  in `setSelected` (`app.js:441-451`), and `aria-activedescendant` on
  the listbox (`apps/core/assets/index.html:55`).
- `apps/core/src/overlay/tray.rs` — call `focus_input()` when showing
  via tray.
- Optional: `apps/core/src/config.rs` — add `keyboard.*` section for
  future keybinding config (out of scope for this issue).

No Rust core changes needed. No schema migration needed. No release
blocker.

---

## Estimate

- Style fix: 1 CSS block (~15 lines), one-line tweak to existing JS.
- a11y attrs: ~10 lines in `app.js`.
- Tray focus: ~5 lines in `tray.rs`.
- Total: **S — under 100 lines, no architectural change**.

---

## Recommendation

1. Treat #17 as a **visual feedback** bug, not "keyboard doesn't work".
2. Reply on the issue asking for clarification (does typing in the
   input box work? does ArrowDown move selection in DOM?). If the
   reporter truly cannot move selection with keys, that's a deeper bug
   and we'd want their build/version.
3. Land the CSS fix as a small follow-up; it does not need a milestone.

## Evidence trail

- `apps/core/assets/app.js:31, 152-167, 209-298, 378-381, 441-471, 473-546, 644-711, 931-937`
- `apps/core/assets/index.html:55`
- `apps/core/assets/style.css` — full file, no `selected` rule
- `apps/core/src/overlay/host.rs:468, 660-664, 670, 692, 716, 871, 1013-1035, 1221-1358, 1432-1436, 1973-1998`
- `apps/core/src/overlay/hotkey.rs:84-113, 285-447, 587, 694-722`
- `apps/core/src/overlay/shim.rs:355-374, 401-418`
- `apps/core/src/overlay/model.rs:119`
- `apps/core/src/config.rs:264-307`
- `apps/core/src/runtime_loop.rs` — 25+ `selected_index` references

## Cross-references

- `docs/bottlenecks-and-fixes.md`
- `docs/SOURCE_AUDIT_BUGS_FIXES_IMPROVEMENTS.md`
- AGENTS.md → "Overlay Architecture" / "Config" / "Helper binary"