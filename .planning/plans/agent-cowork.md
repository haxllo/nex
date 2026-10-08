# Nex Agentic Chat ("Cowork-style") Plan

**Branch:** `feat/agent-cowork`
**Date:** 2026-10-08
**Owner:** haxllo
**Status:** Draft → Awaiting approval

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn chat from single-answer into delegated goals: user states an outcome, Nex plans steps, calls typed tools, shows progress, asks approval for writes/exec, and reports back.

**Architecture:** A bounded Rust agent loop (`apps/core/src/agent/`) drives the existing native Responses transports (ChatGPT backend + OpenAI-compatible), feeds tool results back into context, and streams step events to the chat view. Tools are small validated functions over existing subsystems (index search, fs, `action_executor`, clipboard). No new dependencies, no new binaries, no changes to vendored code.

**Tech Stack:** Rust (ureq SSE, rusqlite, serde), WebView2 overlay (app.js/style.css/index.html), existing IPC envelope (`OverlayMessage`/`OverlayEvent`).

---

## 1. Why now

- Chat already streams, keeps history, and speaks Responses natively — the only missing piece for agency is the loop + tools.
- All tool backends exist: tantivy index search, fs, `launch_open_target`, clipboard, power-confirm UX pattern.
- Reference harness anatomy studied (loop, tool registry with retries, skills, session state, budgets); we implement the Nex-sized subset.

### Non-goals

- No multi-agent/subagents, no cron/scheduled tasks, no long-term memory (Phase 3 covers persistence of runs only, not cross-run learning).
- No changes to search/launcher behavior, updater, overlay chrome, or vendored `third_party/codex`.
- No new crates, no new binaries, no new top-level IPC transports (reuse `OverlayMessage`/`OverlayEvent` + `push_chat` JSON).
- No third-party agent names anywhere in code, notes, or commits.

---

## 2. Inventory (confirmed via read)

| Path | Role in plan |
|------|--------------|
| `apps/core/src/chat.rs` | `start(raw, push)`, `ACTIVE_REQUEST` cancel, `stream_openai`, `stream_codex_native`, `conversation_prompt`, `codex_http_error`, unit-test precedent at file bottom |
| `apps/core/src/codex_auth.rs` | `fresh_tokens()`, `backend_headers()`, `CODEX_BASE_URL` — reuse for authed tool-capable requests |
| `apps/core/src/overlay/ipc.rs` | `OverlayMessage` enum + `KNOWN` list + `check_len` + parse tests (`chat_disconnect_request_parses` pattern) |
| `apps/core/src/overlay/host.rs` | `handle_ipc` maps messages → `OverlayEvent` |
| `apps/core/src/overlay/model.rs` | `OverlayEvent` enum |
| `apps/core/src/runtime_loop.rs` | `ChatSend` arm spawns `chat::start`; `open_url_in_browser` helper precedent; `OverlayEvent::ChatCancel` arm |
| `apps/core/src/action_executor.rs` | `launch_path`, `launch_open_target` (reuse for open tools) |
| `apps/core/src/discovery.rs` / tantivy index | file search backend for `fs_search` tool |
| `apps/core/assets/app.js` | `renderChatMessages`, `post()`, `FLAT_IPC_PAYLOADS`, approval-less message actions precedent |
| `apps/core/assets/index.html`, `style.css` | chat view markup + dropdown/card style tokens to copy |
| `index_store.rs` (rusqlite precedent) | SQLite access pattern for the runs table |

---

## 3. Architecture

```
chat input (goal) → runtime_loop::AgentGoal → agent::run_loop
  → model turn (Responses + tools schemas, stream text deltas as today)
  → function_call event → policy check (read: auto / write+exec: approval)
  → approved? → execute tool → append result → next turn (max 12)
  → step events pushed as chat JSON (agentStep / agentApproval / agentDone)
  → chat view renders step cards inline in the thread
Cancel: existing chatCancel flips the run token; loop checks between steps.
Budgets: 12 steps, 1024 output tokens/turn (existing), 60s wall clock per run.
```

Tool results re-enter as `function_call_output` items (Responses) or plain text (chat-completions path). Approval pauses the loop on a crossbeam channel with a 120s timeout defaulting to deny. Every tool validates args (path traversal rejection, command allowlist: read-only commands auto-run, everything else needs approval) and truncates output to 4000 chars.

---

## Phase 1 — Tool protocol + approval UX

### Task 1: Agent event JSON contract (test-first)

**Files:**
- Modify: `apps/core/src/runtime_loop.rs` (nothing yet — contract lives in agent module)
- Create: `apps/core/src/agent/mod.rs` (empty module + event serializers)

**Step 1: Write the failing test**

In `apps/core/src/agent/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn step_event_shapes_match_ui_contract() {
        let e = AgentEvent::step(1, "fs_list", "running", "listed 4 entries");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["t"], "agentStep");
        assert_eq!(v["step"], 1);
    }
}
```

**Step 2: Run it**

Run: `$env:CARGO_TARGET_DIR='G:\nex-target'; cargo test -p nex --lib agent::`
Expected: FAIL with "cannot find" (module empty).

**Step 3: Implement minimal `AgentEvent` enum with `step/approval/done` variants serializing to `agentStep/agentApproval/agentDone`.**

**Step 4: Re-run — Expected: PASS.**

**Step 5: Commit**

```bash
git add apps/core/src/agent/mod.rs
git commit --author="Habeeb <mshabeeburrahman786@gmail.com>" -m "feat(agent): step event contract"
```

### Task 2: Tool registry with 3 read-only tools

**Files:**
- Create: `apps/core/src/agent/tools.rs`
- Test: inline `#[cfg(test)] mod tests` in same file

Tools: `fs_read` (file→truncated text, rejects `..` escapes outside allowed roots), `fs_list` (dir→names, cap 100), `fs_search` (query→top paths via existing discovery; stub to filename scan first, wire tantivy in Task 4).

**Step 1:** Test: unknown tool name errors; `fs_read` on `../../secret` rejected; output truncated at 4000 chars.

**Step 2:** Run, expect FAIL (no registry).

**Step 3:** Implement `ToolSpec { name, description, parameters: serde_json::Value }`, `TOOLS: &[ToolSpec]`, `dispatch(name, args_json) -> Result<String, String>`.

**Step 4:** Run, expect PASS. **Step 5:** Commit `feat(agent): read-only tool registry`.

### Task 3: Approval-required tools (no execution without consent)

**Files:**
- Modify: `apps/core/src/agent/tools.rs`

Add `shell_exec` (runs via `std::process::Command`, `CREATE_NO_WINDOW`, 20s timeout, output cap), `app_open` (wraps `action_executor::launch_open_target`), `url_open` (reuse `open_url_in_browser` logic — move it to a shared spot if needed, or duplicate the 15-line ShellExecuteW helper into tools.rs).

Each returns `Err("approval-required:…")` unless the call carries an approval token issued by the loop (see Task 5). Policy helper: `needs_approval(name) -> bool` — true for `shell_exec/app_open/url_open`, false for reads.

**Steps:** test-first (shell_exec without token errors; with token runs `cmd /c echo hi`), run, implement, run, commit `feat(agent): approval-gated write tools`.

### Task 4: Wire `fs_search` to the real index

**Files:**
- Modify: `apps/core/src/agent/tools.rs`

Replace the filename-scan stub with the tantivy/discovery query path used by the launcher (mirror the call the search worker makes; cap 20 hits, return paths only).

**Steps:** test with a temp dir containing a uniquely-named file (use `tempfile` crate — already a dev-dependency); run; implement; run; commit `feat(agent): index-backed file search`.

### Task 5: Approval plumbing runtime ↔ UI

**Files:**
- Modify: `apps/core/src/overlay/ipc.rs` (add `agentApprove`/`agentDeny` with call id, `KNOWN` entries, len checks, parse tests mirroring `chat_disconnect_request_parses`)
- Modify: `apps/core/src/overlay/host.rs` (`handle_ipc` arms → `OverlayEvent::AgentApprove(String)` / `AgentDeny`)
- Modify: `apps/core/src/overlay/model.rs` (event variants)
- Modify: `apps/core/src/runtime_loop.rs` (arms forward the decision into the agent run's pending channel; unknown/expired id is ignored, never errors)

**Steps:** test-first on parse (approve with 64-char id ok;  unhappy paths rejected); run `cargo test -p nex --lib overlay::ipc`; implement all four files; `cargo check -p nex`; commit `feat(agent): approval IPC plumbing`.

### Task 6: Approval card UI in chat thread

**Files:**
- Modify: `apps/core/assets/app.js` (render `agentApproval` as an inline card: tool name, arg summary, Approve/Deny buttons posting `agentApprove`/`agentDeny` with the id; step cards for `agentStep` lines under the running message)
- Modify: `apps/core/assets/style.css` (card style copied from `.chat-code-block` tokens: radius 7px, faint border, 11px type)
- Verify: `node --check apps/core/assets/app.js`

**Steps:** implement, node check, manual overlay check later in Phase 2 E2E; commit `feat(agent): approval card UI`.

---

## Phase 2 — Agent loop with budgets

### Task 7: Loop skeleton on the native ChatGPT path

**Files:**
- Create: `apps/core/src/agent/loop.rs`
- Modify: `apps/core/src/runtime_loop.rs` (new `AgentGoal(String)` event + arm spawning `agent::run`)

Loop: build Responses body = existing chat body + `tools` schemas from registry; reuse the SSE reader shape from `stream_codex_native`; on `response.output_item.done` with `function_call`, run policy → approval → dispatch → append `function_call_output` → next turn. Plain text deltas stream exactly like today. Stop at 12 steps / 60s wall / cancel token / model finish without calls (then `agentDone`).

**Steps:** test-first with a stub transport? No — test the pure parts: turn-budget cutoff (loop exits after N tool turns against a canned transcript), output truncation, cancel-flag check. Implement, run, commit `feat(agent): bounded agent loop`.

### Task 8: Wire goal entry + cancel + done UI

**Files:**
- Modify: `apps/core/src/runtime_loop.rs`, `apps/core/assets/app.js`

Goal entry: prefix `!` in chat input routes to `AgentGoal` instead of `chatSend` (documented in placeholder hint). Cancel button stops the loop mid-run (existing `chatCancel` flips a shared atomic the loop checks). `agentDone` renders the final summary line + clears streaming state.

**Steps:** implement, node check, commit `feat(agent): goal entry and cancel`.

### Task 9: OpenAI-compatible path parity

**Files:**
- Modify: `apps/core/src/agent/loop.rs`

Same loop against chat-completions transport (`/chat/completions` with `tools`, `tool_calls` deltas, `role:tool` results). CLI fallback stays tool-less (plain answer, no loop).

**Steps:** test transcript parsing for `tool_calls` chunks; implement; commit `feat(agent): openai-compatible tool loop`.

### Task 10: E2E manual verification + fix-ups

- Build debug, run `--foreground`, goal `!list files in Documents`, approve/deny flows, cancel mid-run, 12-step cutoff.
- Fix what breaks, one commit per fix, then commit `test(agent): e2e hardening`.

---

## Phase 3 — Persistence (runs resume)

### Task 11: SQLite runs table

**Files:**
- Create: `apps/core/src/agent/store.rs` (rusqlite, mirroring `index_store.rs` open pattern)
- Schema: `agent_runs(id TEXT PK, goal TEXT, status TEXT, steps INTEGER, created_ms INTEGER)`, `agent_events(run_id, seq, kind, payload)`
- Test-first: insert run + events, reload in order. Commit `feat(agent): run store`.

### Task 12: Resume + history UI

- Reopening chat lists the last run with its steps; `!retry <id>` resumes from stored events.
- Commit `feat(agent): resume runs`.

---

## 4. Risks

- Responses `function_call` streaming shape differs per model → parse tolerantly (accept `output_item.done` and delta-assembled partials); log unknown shapes, never crash the loop.
- Approval waits while overlay hidden → 120s default-deny plus hide cancels the wait.
- Token cost per turn grows with tool outputs → 4000-char truncation + 12-step cap + existing 1024 output cap.
- Long chain stalls the overlay thread → loop always runs on a spawned worker thread, results via `push_chat` like today.

## 5. Acceptance

- `!list files in <dir>` completes with visible steps and no approval prompts.
- `!open notepad` (or equivalent) waits for inline approval; deny stops cleanly.
- Cancel mid-run stops within 1s; 12-step ceiling hit shows a capped message, not a hang.
- `cargo check`, targeted `cargo test -p nex --lib agent::`, `node --check` all green per task.
- No third-party agent names in code, notes, or commits.
