# ADR-0007: Push UI state fire-and-forget instead of evaluating script synchronously

## Status

Accepted.

## Date

2026-06-19

## Context

The panel is rebuilt on every keystroke. Rust produces the state, the page renders
it. The original implementation called `evaluate_script` with the state embedded in
a JavaScript expression and waited for the call to return.

`evaluate_script` blocks until the page has executed the script. On the keystroke
path that means the host waits for the web engine to schedule and run work, which
turned a search result into a round trip and could stall when the page was busy. A
slow paint became indistinguishable from a slow search.

## Decision

Rust posts state as JSON with `PostWebMessageAsJson` and does not wait. The page
applies the snapshot in one render pass. User intent travels back as short
messages.

There is no synchronous script evaluation on the hot path.

## Alternatives considered

### Keep `evaluate_script` but make it faster

Rejected because the blocking is the problem, not the script. Any synchronous call
puts the host's progress in the web engine's hands.

### Batch state into fewer `evaluate_script` calls

An optimisation of the wrong mechanism. It reduces the number of round trips
without removing the round trip.

### Have the page poll the host

Rejected because it adds latency between a state change and its display, and makes
the page responsible for knowing when something changed.

## Consequences

- The host cannot know whether the page rendered successfully. A malformed snapshot
  is a page-side problem that surfaces as a console error rather than a host
  failure, so the page needs to be defensive about what it receives.
- Ordering is guaranteed by the message channel, not by the call returning, so
  sequences of snapshots must be safe to apply in order and to drop if stale.
- The page holds no authority. It can request that something be launched, but the
  host decides. This is what keeps a page that receives untrusted content from
  being able to act on its own.
- Measuring a slow first keystroke has to distinguish a slow search from a slow
  paint. The page therefore reports the wall-clock time of the last printable
  keypress with each query, so the host can tell the two apart. Without that, the
  two failures look identical in timing.
