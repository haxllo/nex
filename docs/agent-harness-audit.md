# Nex Chat Harness Audit

Scored against the 58-item **Harness Audit Checklist** from
`cauchyturing/agent-harness-engineering` (91 patterns / 9 domains), cross-read
with `Turi-Labs/awesome-harness`, OpenAI `openai-agents-python`, Microsoft
`agent-framework` and LangChain `langgraph`.

Every score below is from **reading Nex's source**, not from the checklist's
assumptions. Where a claim is unverified it says so. Repos cloned to
`D:\agent-harness-research` (read-only reference, not a dependency).

- Scope: `apps/core/src/chat.rs` (3,683 lines), `chat_tools.rs` (5,282),
  `chat_eval.rs` (274).
- Baseline: `master` after CIQ-145 merge (`34f245e4`).

## Verdict

**Capability is not the gap.** 12 tools ship and work: open, reveal, open_url,
create_folder, create_file, read_file, search_paths, edit_file, move_path,
copy_path, delete_path, run_command. CIQ-146…185 already cover the feature
surface.

**The gap is the layers *around* the tools.** Nex is a good tool surface with
a thin harness. It is roughly a well-built MVP harness by this checklist's own
scale, and the missing items cluster in four places that all fail the same
way: **the session has no memory of itself.**

---

## Scorecard

| Category | Pass / Total | % | Notes |
|---|---|---|---|
| Architecture | 3 / 8 | 38% | two files, no layer boundary |
| Agentic Loop | 4 / 8 | 50% | bounded history, no accounting |
| Tool System | 5 / 8 | 63% | strong single-tool, no batching |
| Permission & Safety | 4 / 8 | 50% | good path gate, no modes |
| Extensibility | 0 / 6 | 0% | no hooks, no plugins (by design) |
| LLM Integration | 3 / 8 | 38% | 2 providers, no retry, no cost |
| UI/UX | 3 / 6 | 50% | streams, but no explicit state machine |
| Infrastructure | 3 / 6 | 50% | config validation real, no cleanup registry |
| **Overall** | **25 / 58** | **43%** | minimum viable is 70% |

This is the honest number. Several items are *deliberately* absent — Nex is a
launcher, not a coding CLI, and should not grow a plugin SDK. Those are marked
**N/A** below and excluded from the "should fix" list. The score above counts
them as misses anyway, so read the per-item notes, not the summary.

---

## The four structural gaps

These are the findings worth acting on. Everything else is either already done
or should not be built.

### 1. No token or cost accounting — at all

The checklist wants per-turn token usage tracked in real time
(`Agentic Loop`, `LLM Integration`). Nex has **none**:

- `usage` appears in `chat.rs` only inside error-message string matching
  (`"usage limit"` → a ChatGPT plan message), never as accounting.
- No `prompt_tokens` / `total_tokens` field is parsed from any response.
- No per-session or per-turn budget exists, so nothing can enforce one.

Consequence: a user cannot see what a long tool loop cost, and the app cannot
warn before a spend limit is hit. Cheap to add — providers already return
`usage` in the response body — and it is the cheapest way to make the loop
observable.

### 2. A session cannot be resumed, because tool results are not persisted

`ChatTurn` stores `role` + `content` only (`chat.rs:67`). Tool results are not
part of that type, so after a reload the model loses every action it took and
re-derives it — or worse, repeats a destructive one. This is CIQ-158, already
filed, and it is the single highest-value harness fix available.

The checklist calls for *"user messages and tool results persisted to disk
before the next LLM call"* (`Agentic Loop`). Nex persists neither mid-turn.

### 3. No compaction — bounded history silently drops the middle of a task

`bounded_history(&history, 24, HISTORY_CONTEXT_BUDGET_CHARS)` keeps the **last**
24 turns within 24,000 chars, newest-first (`chat.rs:2455`).

This is correct and deliberate for a launcher: it guarantees a bounded prompt.
But it is truncation, not compaction. There is no summary of what was evicted,
so a long task loses its early constraints without any signal to the model or
user. The checklist's requirement — *"compaction triggers are budget-based;
summary quality validated before discarding the original"* — is unmet because
there is no summary at all.

Fix is not "add a summariser" (that costs a call per eviction). The cheaper fix
is to carry a short **durable task note** across evictions.

### 4. Tools execute one at a time, with no per-tool budget

Independent calls in a single model turn run sequentially. The checklist wants
concurrent batching and a per-tool result budget; Nex has a global
`COMMAND_OUTPUT_LIMIT = 16 * 1024` and `MAX_WRITE_BYTES = 1 MiB`, applied by
tool kind rather than by measured cost.

This one is a **judgement call, not a bug**. Concurrent tool execution on a
single-threaded UI with a confirmation gate is genuinely risky — two
simultaneous `delete_path` confirmations is a worse UX than a slower queue.
Recommendation: leave sequential, and fix only the *feedback* problem
(CIQ-178: a 43-second command looks like a freeze).

---

## What is genuinely good (and should not be changed)

Worth recording, because these were built correctly and a rewrite would lose
them:

- **Cache-stable tool ordering.** `all_tool_names()` is a `&'static [&'static str]`
  array (`chat_tools.rs:1560`). Declaration order cannot drift between
  requests, so the prompt prefix is stable. This is the checklist's
  "cache-stable ordering" item, passed by construction.
- **Modular system prompt.** `system_prompt()` assembles identity,
  capabilities (`capability_sentence()`), rules and constraints in a fixed
  order, with one test per rule asserting the rule is present
  (`chat.rs:3125+`). Rules are additive and individually testable — the
  checklist's "system prompt pipeline", passed properly.
- **Real path gate.** Non-absolute `parent` is rejected with
  `IllegalName { reason: "parent must be an absolute path" }` before anything
  is joined or written (`chat_tools.rs:693`). Traversal is blocked at the
  argument boundary, not after.
- **Honest loop exhaustion.** `MAX_TOOL_ROUNDS = 25` produces a report listing
  exactly which actions ran and that anything after them did not
  (`chat_tools.rs:1229`). This is better than most production harnesses: it
  does not pretend the task completed.
- **Two providers behind one call path.** `openai-compatible` and `codex`
  switch without call-site changes.
- **Streaming that handles provider reality.** Two providers split tool-call
  arguments differently — one by `index`, the other by `item_id` under the
  Responses API — and `ToolCallAccumulator` (`chat.rs:862`) joins both, with
  `truncated` marking a call whose arguments never finished arriving. This is
  not the checklist's explicit `receiving → tool_use → text → done` state
  machine (there is no such enum), but the hard part — not corrupting a
  fragmented tool call — is solved.
- **Delimited untrusted data.** Tool results arrive between `<<<TOOL_DATA`
  / `>>>`, and the prompt spends a full section forbidding instruction-following
  from inside them. CIQ-155 tracks the residual injection risk.

---

## Should not be built (deliberate non-goals)

Marked **N/A** rather than "missing":

- **Hooks / plugins / skills / self-authoring** (all of Extensibility, 0/6).
  Nex is a launcher for end users, not a developer platform. A plugin SDK is
  attack surface with no user. The extension-parity item does not apply because
  there is nothing to be parity with.
- **MCP integration.** Same reasoning; the tool set is small, curated and
  audited. Not a gap.
- **Three permission modes / enterprise policy layer.** Nex has a single
  confirmation model, which is correct for the audience. A "deny-all" mode is
  the Settings toggle, not a permission system.
- **Subagents / swarm orchestration.** One user, one task, one loop.

---

## Next action (single, highest value)

**CIQ-158 — persist tool results so a session can resume.** It is already
filed, it is the preconditions for gaps 1 and 3 (you cannot account for or
compact a turn you did not record), and it fixes a real user-visible failure:
an agent that forgets its own actions mid-task.

Do not open new issues from this document until 158 lands. This audit is
context for that work, not a work queue.
