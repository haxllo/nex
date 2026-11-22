# ADR-0001: Rust with tao and wry hosting a WebView2 panel

## Status

Accepted. Supersedes `docs/decisions/ADR-001-core-stack.md`.

## Date

2026-06-18

## Context

Nex is a launcher whose entire visible surface is one floating panel: a search
field, a list of result rows, a grid, and a handful of secondary views. The panel
is short-lived and re-opened dozens of times an hour, so show latency and visual
quality carry more weight than the size of the dependency graph.

Three constraints decided this:

- Windows input handling, file discovery, and window management need direct Win32
  access, which rules out a managed runtime for the core.
- The panel's visual design changes constantly during development. A design that
  lives in CSS can be iterated in a browser; a design expressed as Rust widget
  trees cannot.
- Summoning the panel must not pay a cold-start cost.

The project had already migrated through two UI approaches before this decision, so
the cost of migrating again was known and was accepted a third time only against a
concrete measurement.

## Decision

Build the core in Rust, and host the panel's content in a WebView2 control through
`tao` (windowing) and `wry` (web view). HTML, CSS, and JavaScript define the panel.
The same Rust library is linked by every binary.

## Alternatives considered

### Native Win32 with GDI and DWM

The original approach. It gives complete control of painting and no web engine at
all.

Rejected because every visual change became a code change, and the panel's design
is iterative. The measured cost was also the complaint that drove the migration:
reaching a polished look in immediate-mode painting was slower than the design was
changing.

### Iced, a Rust native UI toolkit

Work had begun on an Iced 0.14 migration, including an overlay split into boot,
view, model, theme, and geometry modules.

Rejected because it moved the design into Rust widget code, which is the same
problem as GDI with a different API. The migration was abandoned rather than
finished, and its planning artifacts are recorded in
[../history/timeline.md](../history/timeline.md).

### Slint

Evaluated and rejected during the same period. It imposes its own markup language
and runtime model, which adds a second mental model without removing the need for
Win32 interop.

## Consequences

- The panel is ordinary HTML and CSS, so layout and contrast can be measured in a
  browser outside the app. This is the main practical benefit and it is used by the
  testing approach.
- Assets are embedded with `include_str!` and served over a custom protocol. There
  is no asset directory to drift, but **any asset change needs a rebuild**, which
  is the single most common development surprise.
- A WebView2 runtime must exist on the machine. It ships with current Windows.
- The panel costs a browser engine's memory. The WebView is deliberately kept
  resident between shows, which is what makes a show instant.
- GPU work under a WebView is not composable with native drawing in the way one
  might expect, which is why the glass material is a separate window. See
  [0006](0006-native-glass-renderer.md).
- Accessibility, text selection, and rich content come from the engine rather than
  being reimplemented.
