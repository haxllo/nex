# Product Requirements

## Functional Requirements

- `FR-001` Global Hotkey
- Register a user-configurable global shortcut (default `Ctrl+Space`) to show or hide launcher.

- `FR-002` Floating Search Bar
- Show centered, always-on-top, borderless UI with input focused on open.

- `FR-003` App Discovery
- Discover launchable applications from:
- Start menu shortcuts
- Installed apps metadata
- User-defined entries

- `FR-004` File and Folder Discovery
- Index files and folders from user-selected locations with include and exclude filters.

- `FR-005` Fuzzy Search
- Support typo-tolerant matching and rank by relevance plus recency and frequency.

- `FR-006` Launch Actions
- Launch selected item on `Enter`.
- Support open containing folder and run as admin as explicit actions.

- `FR-007` Keyboard Navigation
- Up and down navigation, tab category switch, escape to close, quick action hotkeys.

- `FR-008` Settings
- In-app settings for hotkey, indexed paths, ignore rules, result limits, and theme.

- `FR-009` Ranking Learning
- Track accepted results to improve ranking over time (local only).

- `FR-010` Error Feedback
- Show clear failures for missing files, access denied, and launch failures.

## Non-Functional Requirements

- `NFR-001` Performance
- Open UI in <= 60ms (P50), query response <= 15ms (P95) on warm index.

- `NFR-002` Resource Usage
- Idle CPU near zero; idle RAM target <= 120MB combined footprint.

- `NFR-003` Reliability
- No crashes on malformed shortcuts or inaccessible indexed paths.

- `NFR-004` Security
- No silent privilege escalation; elevated actions require explicit user command.

- `NFR-005` Privacy
- Index and usage history remain local by default; telemetry opt-in only.

- `NFR-006` Accessibility
- Full keyboard operation and high-contrast compatible UI.

## Compatibility Requirements

- Windows 10 22H2+ and Windows 11
- Per-user installation in MVP, machine-wide install as post-MVP option
