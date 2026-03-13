# ADR-001: Core Stack and Process Model

- Status: superseded
- Date: 2026-02-11

## Context

The product requires:
- Very fast query handling
- Low idle resource usage
- Modern UI
- Stable integration with Windows hotkeys and file discovery APIs

## Decision

Original decision:
- Rust for the core service (`nex`)
- Legacy UI choice at that time: a Tauri UI with React and TypeScript (now removed)
- SQLite for local cache and metadata persistence
- Two-process model (always-on core + on-demand UI process)

Current state (superseding update):
- Rust core service (`nex`) remains.
- UI is now a native Win32 owner-draw overlay hosted directly in `nex`.
- Single-process runtime model is used in production.

## Rationale

- Rust gives high performance and predictable memory behavior
- At the time, Tauri offered fast UI iteration and clear process boundaries.
- The project later moved to native in-process overlay for tighter latency and lifecycle control.

## Consequences

Positive:
- Better performance control
- Lower runtime/process overhead with a single process
- Simpler install/upgrade/uninstall behavior (no secondary UI binary orchestration)

Negative:
- Native overlay rendering complexity moved into Rust/Win32 code
- Cross-platform UI reuse from the prior web stack no longer applies

## Follow-up ADRs

No additional ADR files are maintained in this repository today.
