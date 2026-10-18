# Chat: web access, skills, and memory (recorded idea)

**Status:** recorded only, not scheduled. No code written.

**Goal:** Chat can research the web, load skills, and remember durable facts about
the user — each of them opt-in, never on by default.

---

## Why this shape

Raycast's Windows client was the reference, and it settles two questions:

- Its AI chat has built-in `web_search` and `read_page` tools, and it treats web
  search as a per-model capability, not a user toggle. The user switch is
  permission (`Ask` / `Auto`, allow list), not access.
- Its skills are plain `SKILL.md` folders on disk (`skillDirectories`), the same
  format Claude Code and opencode already use. Its memory is a single always-loaded
  text block.

Hermes Agent (Nous Research, MIT) supplies the tool contracts and the guard rails.
It is Python with ~9,800 files and a gateway, scheduler, and remote-execution
substrate — none of which belongs in a launcher. Only three of its ideas are
portable, and they are the three below.

Sources: `C:\Program Files\WindowsApps\Raycast.Raycast_2.8.2.0_x64__qypenmj9wpt2a\Raycast`
(frontend tool map + `backend\index.mjs` tool plumbing);
`G:\Ciqada\tmp\opencode\hermes-agent` (`tools/web_tools.py`, `tools/memory_tool.py`,
`tools/skills_tool.py`, `skills/`).

## 1. Web access

Current state: `apps/core/src/chat.rs` is a provider boundary with no tool loop, and
`SYSTEM_PROMPT` promises the model does not browse. So this is a new capability, not
a config flag.

Sketch:

- One tool loop in `chat.rs`: read `tool_calls` off the existing OpenAI stream, run the
  tool, append the result, re-request. `ureq` is already the HTTP client.
- Two tools, schemas taken from Hermes:
  - `web_search { query: string, limit: int, default 5 }`
  - `web_extract { urls: [string], max 5, char_limit: int, default 15000 }`
- Keyless default is DuckDuckGo's HTML endpoint. A user-supplied Brave/Exa/Tavily key
  in `config.toml` is the upgrade path; do not build the provider matrix up front.

Security, not optional:

- Reject non-`http(s)` schemes, localhost, and private/link-local addresses, and
  re-check after every redirect.
- 15 s timeout, ~2 MB response cap, truncate to `char_limit` and spill the full text
  to a temp file the model can page through.
- Fetched pages are untrusted data, never instructions. `LOCAL_CONTEXT_RULE` in
  `chat.rs` already states this rule for selected files; reuse its wording.

## 2. Skills

Same `SKILL.md` + YAML frontmatter format the user already has skills in. Progressive
disclosure: `skills_list` (name + description) → `skill_view(name)` → `skill_view(name,
file_path)` for a skill's bundled `scripts/` and `references/`. Point at a configurable
skill directory, defaulting to the one Nex owns under `%APPDATA%\Nex\skills`.

## 3. Memory

One `memory` tool with batched operations (`[{action, content?, old_text?}]`), applied
atomically against a char limit, stored in a small file and injected into every turn.
Hermes' split rule is worth keeping: durable user facts go to memory; task-specific
know-how belongs in a skill, where it only loads when relevant.

## User choice

One setting: `web_access = "off" | "ask" | "on"`, default `off`. Skills and memory get
their own independent switches. Permission prompts and an allow list come later, if
ask-mode proves annoying.

## Deliberately not now

Result cache, PDF extraction, multi-provider matrix, tracing UI, subagents, cron, and
the messaging gateway. All optional once the simple version is measurably short.
