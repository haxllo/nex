# macOS Spotlight Feature Reference

> **UNWANTED — DEPRECATED**: This document was a competitive-analysis reference for the early roadmap. It is not referenced by any current code, plan, or decision. The actionable comparison is in [`../plans/audit-and-roadmap.md` Section 3](../plans/audit-and-roadmap.md#3-competitive-landscape), which is kept up to date. This file is preserved for historical context only — do not update.
>
> The original v1.x roadmap (no Spotlight parity) is set in [`../plans/audit-and-roadmap.md`](../plans/audit-and-roadmap.md). If competitive analysis against Spotlight is needed in the future, add it to that single source of truth.

This document captures a practical feature breakdown of Spotlight for parity planning.

## Major Features

- System-wide search from one entry point.
- App launching.
- File and folder search by name, metadata, and indexed content.
- In-app indexed content search (for apps that integrate with Spotlight).
- Natural language style queries (for many common intents).
- Quick calculations and conversions (math, units, currency).
- Contacts, Mail, Calendar, and Messages surface in results (when enabled and indexed).
- Web/Siri suggestions and knowledge-style answers (when enabled).
- Quick actions directly from results (for supported result types).

## Minor Features

- Typing suggestions and query completions.
- Recent activity and history influence ranking.
- Spell correction and fuzzy matching behavior.
- Category-aware ranking (apps, documents, web, etc.).
- Keyboard-first navigation.
- Per-source inclusion/exclusion controls in system settings.
- Privacy controls for suggestions and indexing scope.
- Consistent result metadata (subtitle/path/context snippets).
- Fast incremental index updates in background.

## UI-Visible Spotlight Features

- Centered floating launcher panel.
- Rounded container with soft shadow.
- Single focused search input at top.
- Placeholder text when input is empty.
- Dynamic results list that appears as you type.
- Icon + primary title + secondary context line per result row.
- Clear hover/focus highlight for active row.
- Sectioned/grouped result presentation in many query types.
- Top/best match prominence.
- Inline "no results" state when nothing matches.
- Keyboard hint affordances (arrow navigation, open actions).
- Smooth open/close and list update transitions.

