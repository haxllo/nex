# Creation Features Plan — branch feat/creation-actions

## Goals

Three smart action rows injected when a query has no matching local result: create folder, create file, open domain URL. Single shared injection engine.

## Shared design

- **Injection:** `runtime_loop.rs` `apply_search_results` empty-result branch (alongside existing web-search injection)
- **Precedence ladder** (first match wins):
  1. Real DB hit → normal results, no injection
  2. Query ends with `\` → create-folder row
  3. Whitelisted extension → create-file row
  4. Domain/URL → open-url row
  5. Web search row (existing)
- **Rows:** kind `"action"`, render under Actions group header; title describes action, subtitle = resolved target path
- **Dispatch:** `runtime_actions.rs` `execute_action_selection` new id-prefix arms
- **Existence pre-check at row render:** exists → row flips from "Create" to "Open existing"

## Phase 1 — Folder creation (NOW)

1. `config.rs`: `default_create_dir` key (default: empty → fallback `USERPROFILE\Desktop` at use site), template line, validation, migration (`raw_has_key` pattern)
2. `action_registry.rs`: `ACTION_CREATE_FOLDER_PREFIX` const + `dynamic_provider_create_folder_action(query, cfg)` — detect trailing `\`, resolve target path, stat existence → two id variants (create vs open)
3. `runtime_loop.rs`: empty-branch injection before web search
4. `runtime_actions.rs`: dispatch arm — `create_dir_all` → status text `"Created folder: <path>"`; open variant → open explorer
5. Verify: build + manual test

## Phase 2 — File creation

1. `config.rs`: `create_file_extensions` list + `create_actions_enabled`
2. `action_registry.rs`: `ACTION_CREATE_FILE_PREFIX` + provider fn (whitelist ext check, DB title-exists check via service optional, existence stat)
3. `runtime_loop.rs` injection (precedence slot 3)
4. `runtime_actions.rs`: dispatch — create empty file (never overwrite; exists → open) → status text
5. Conflict UX: existing target → `"exists — press Enter again to open"` flow

## Phase 3 — Domain navigation

1. `action_registry.rs`: `ACTION_OPEN_URL_PREFIX` + provider fn (TLD list + scheme detect)
2. `runtime_loop.rs` injection (precedence slot 4)
3. `runtime_actions.rs`: dispatch — `launch_open_target` (existing fn) with `https://` prefix
4. `config.rs`: `open_url_in_default_browser`, `url_tlds`
5. Edge rules: `site.com.html` → create-file wins; bare `localhost:port` ignored without scheme

## v2 candidates (not in scope)

- DB-folder path matching for target resolution (`docs\new` where `docs` is indexed)
- Context-menu "New folder here" / "New file here"
- Command verbs `>new file` / `>new folder`

## Verification

```bash
cargo build --release --bin Nex
```

Manual tests:

| Query | Expected row | Result |
|-------|-------------|--------|
| `docs\` (doesn't exist) | Create folder | Folder created on Desktop + toast |
| `docs\` (already exists) | Open existing folder | Explorer opens at `Desktop\docs` |
| `report.txt` (no DB match) | Create file | Empty file created + toast |
| `report.txt` (already exists) | Open existing file | File opens in default app |
| `youtube.com` | Open URL | Browser opens YouTube |
| `site.com.html` | Create file | `.html` in whitelist wins over domain |
| `nonexistent.xyz` | Web search fallback | No creation row (ext not whitelisted) |
