//! Bounded agent loop on the native ChatGPT (Responses) path.
//!
//! One turn = POST /responses with `tools` schemas from [`crate::agent::tools`]
//! registry, stream text deltas as `chatDelta` (same as chat today), collect
//! `function_call` items, run policy → approval → dispatch, append
//! `function_call_output`, repeat. Stops at 12 turns / 60s wall / cancel /
//! model finish without calls. Never panics; all failures become `agentDone`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::approvals;
use super::store;
use super::tools;
use super::AgentEvent;

/// Max model turns per goal (plan §3).
pub(crate) const MAX_TURNS: u64 = 12;
/// Wall-clock budget per goal run.
pub(crate) const WALL_CLOCK_SECS: u64 = 60;
/// Approval wait per gated call; timeout defaults to deny.
const APPROVAL_TIMEOUT_SECS: u64 = 120;
/// Chars of tool output / summary surfaced in step events.
const STEP_DETAIL_CHARS: usize = 500;

/// Minimal config snapshot: provider picks the transport, the rest feeds it.
#[derive(Debug, Clone)]
pub(crate) struct AgentConfig {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
}

/// Parsed `function_call` output item.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolCall {
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) arguments: Value,
}

/// One model turn: streamed text plus any function calls.
#[derive(Debug, Default)]
pub(crate) struct TurnOutcome {
    pub(crate) text: String,
    pub(crate) calls: Vec<ToolCall>,
}

/// Approval decision for a single call.
pub(crate) enum Approval {
    Auto,
    Allow(String),
    Deny,
}

/// Pure parse: `response.output_item.done` item → [`ToolCall`].
/// Missing fields or non-call items → `None`. Never panics.
pub(crate) fn parse_function_call(item: &Value) -> Option<ToolCall> {
    let obj = item.as_object()?;
    if obj.get("type")?.as_str()? != "function_call" {
        return None;
    }
    let call_id = obj
        .get("call_id")
        .and_then(Value::as_str)
        .or_else(|| obj.get("id").and_then(Value::as_str))?;
    let name = obj.get("name").and_then(Value::as_str)?;
    if call_id.is_empty() || name.is_empty() {
        return None;
    }
    let arguments = match obj.get("arguments") {
        None | Some(Value::Null) => Value::Object(Default::default()),
        Some(Value::String(s)) => {
            serde_json::from_str(s).unwrap_or(Value::Object(Default::default()))
        }
        Some(other) => other.clone(),
    };
    Some(ToolCall {
        call_id: call_id.to_string(),
        name: name.to_string(),
        arguments,
    })
}

/// Char-boundary-safe truncation for step details / summaries.
pub(crate) fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push_str("…[truncated]");
    out
}

/// Responses `tools` array from the registry (function shape).
pub(crate) fn tools_body() -> Value {
    Value::Array(
        tools::registry()
            .iter()
            .map(|spec| {
                json!({
                    "type": "function",
                    "name": spec.name,
                    "description": spec.description,
                    "parameters": spec.parameters,
                })
            })
            .collect(),
    )
}

/// Chat-completions `tools` array from the registry (function shape:
/// `{"type":"function","function":{name,description,parameters}}`).
pub(crate) fn chat_tools_body() -> Value {
    Value::Array(
        tools::registry()
            .iter()
            .map(|spec| {
                json!({
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": spec.parameters,
                    },
                })
            })
            .collect(),
    )
}

/// Incremental `choices[].delta.tool_calls[]` assembler. One call's chunks
/// arrive split across SSE payloads; accumulate by index until the turn ends.
#[derive(Debug, Default)]
pub(crate) struct ToolCallParts {
    slots: std::collections::BTreeMap<u64, ToolCallSlot>,
}

#[derive(Debug, Default)]
struct ToolCallSlot {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallParts {
    /// Fold one SSE data payload's `delta.tool_calls[]` into the slots.
    /// Unknown shapes are ignored; never panics.
    pub(crate) fn feed(&mut self, data: &Value) {
        let Some(calls) = data
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
        else {
            return;
        };
        for chunk in calls {
            let index = chunk.get("index").and_then(Value::as_u64).unwrap_or(0);
            let slot = self.slots.entry(index).or_default();
            if slot.id.is_empty() {
                if let Some(id) = chunk.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        slot.id = id.to_string();
                    }
                }
            }
            if let Some(name) = chunk
                .pointer("/function/name")
                .and_then(Value::as_str)
                .or_else(|| chunk.get("name").and_then(Value::as_str))
            {
                slot.name.push_str(name);
            }
            if let Some(args) = chunk
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .or_else(|| chunk.get("arguments").and_then(Value::as_str))
            {
                slot.arguments.push_str(args);
            }
        }
    }

    /// Complete calls in index order; nameless slots are dropped.
    pub(crate) fn finish(self) -> Vec<ToolCall> {
        self.slots
            .into_iter()
            .filter_map(|(index, slot)| {
                if slot.name.is_empty() {
                    return None;
                }
                let arguments = if slot.arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    serde_json::from_str(&slot.arguments)
                        .unwrap_or(Value::Object(Default::default()))
                };
                Some(ToolCall {
                    call_id: if slot.id.is_empty() {
                        format!("call-{index}")
                    } else {
                        slot.id
                    },
                    name: slot.name,
                    arguments,
                })
            })
            .collect()
    }
}

/// Responses `function_call_output` item → chat `role:tool` message parts.
/// Anything else → `None`. Never panics.
pub(crate) fn tool_output_parts(item: &Value) -> Option<(&str, &str)> {
    let obj = item.as_object()?;
    if obj.get("type")?.as_str()? != "function_call_output" {
        return None;
    }
    let id = obj.get("call_id").and_then(Value::as_str)?;
    let output = obj.get("output").and_then(Value::as_str).unwrap_or("");
    Some((id, output))
}

/// Assistant turn carrying tool calls, echoed back so the next
/// chat-completions request accepts the `role:tool` results.
fn assistant_calls_message(text: &str, calls: &[ToolCall]) -> Value {
    json!({
        "role": "assistant",
        "content": text,
        "tool_calls": calls.iter().map(|call| json!({
            "id": call.call_id,
            "type": "function",
            "function": {
                "name": call.name,
                "arguments": serde_json::to_string(&call.arguments).unwrap_or_default(),
            },
        })).collect::<Vec<_>>(),
    })
}

fn push_event(push: &mut impl FnMut(Value), event: AgentEvent) {
    let value = serde_json::to_value(&event).unwrap_or(Value::Null);
    if !value.is_null() {
        push(value);
    }
}

fn args_summary(call: &ToolCall) -> String {
    let raw = match &call.arguments {
        Value::Object(map) if map.is_empty() => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    truncate_chars(&raw, 120)
}

/// Transport-agnostic driver (no network): testable core of the loop.
/// `transport` maps current input items → streamed text + calls;
/// `approve` decides per call (may push the approval card via `push`);
/// `execute` runs the tool with an optional approval token.
pub(crate) fn drive_loop(
    input: &mut Vec<Value>,
    max_turns: u64,
    deadline: Instant,
    cancel: &AtomicBool,
    transport: &mut impl FnMut(&[Value], &mut dyn FnMut(Value)) -> Result<TurnOutcome, String>,
    approve: &mut impl FnMut(&ToolCall, &mut dyn FnMut(Value)) -> Approval,
    execute: &mut impl FnMut(&ToolCall, Option<&str>) -> Result<String, String>,
    push: &mut impl FnMut(Value),
) {
    let mut push_ref: &mut dyn FnMut(Value) = push;
    for step in 1..=max_turns {
        if cancel.load(Ordering::SeqCst) {
            push_event(&mut push_ref, AgentEvent::done("Cancelled."));
            return;
        }
        if Instant::now() >= deadline {
            push_event(
                &mut push_ref,
                AgentEvent::done("Stopped after 60s. Try a narrower goal."),
            );
            return;
        }
        let turn = match transport(input.as_slice(), &mut *push_ref) {
            Ok(turn) => turn,
            Err(error) => {
                push_event(
                    &mut push_ref,
                    AgentEvent::done(format!("Error: {}", truncate_chars(&error, 240))),
                );
                return;
            }
        };
        // Note: live transports push `chatDelta` chunks during streaming;
        // stub transports may use the sink the same way. Nothing re-emitted.
        if turn.calls.is_empty() {
            let summary = if turn.text.trim().is_empty() {
                "Done.".to_string()
            } else {
                truncate_chars(turn.text.trim(), 240)
            };
            push_event(&mut push_ref, AgentEvent::done(summary));
            return;
        }
        for call in &turn.calls {
            if cancel.load(Ordering::SeqCst) {
                push_event(&mut push_ref, AgentEvent::done("Cancelled."));
                return;
            }
            push_event(
                &mut push_ref,
                AgentEvent::step(step, &call.name, "running", args_summary(call)),
            );
            let token: Option<String> = match approve(call, &mut push_ref) {
                Approval::Auto => None,
                Approval::Allow(token) => Some(token),
                Approval::Deny => {
                    push_event(
                        &mut push_ref,
                        AgentEvent::step(step, &call.name, "denied", "not approved"),
                    );
                    push_event(
                        &mut push_ref,
                        AgentEvent::done(format!("Stopped: '{}' was not approved.", call.name)),
                    );
                    return;
                }
            };
            match execute(call, token.as_deref()) {
                Ok(output) => {
                    let detail = truncate_chars(output.trim(), STEP_DETAIL_CHARS);
                    push_event(
                        &mut push_ref,
                        AgentEvent::step(
                            step,
                            &call.name,
                            "done",
                            if detail.is_empty() { "(empty)".to_string() } else { detail },
                        ),
                    );
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call.call_id,
                        "output": output,
                    }));
                }
                Err(error) => {
                    let detail = truncate_chars(&error, STEP_DETAIL_CHARS);
                    push_event(&mut push_ref, AgentEvent::step(step, &call.name, "error", detail));
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call.call_id,
                        "output": format!("tool error: {error}"),
                    }));
                }
            }
        }
    }
    push_event(
        &mut push_ref,
        AgentEvent::done(format!(
            "Stopped after {max_turns} steps without finishing. Try a narrower goal."
        )),
    );
}

/// Cancel flag of the latest goal run, so `chatCancel` stops the loop
/// between steps. Set per run by the runtime worker thread.
static CURRENT_RUN: std::sync::Mutex<Option<Arc<AtomicBool>>> =
    std::sync::Mutex::new(None);

/// Remember `flag` as the run [`cancel_current`] stops.
pub(crate) fn track_cancel(flag: Arc<AtomicBool>) {
    if let Ok(mut current) = CURRENT_RUN.lock() {
        *current = Some(flag);
    }
}

/// Flip the latest run's cancel flag (shared with `chatCancel`).
pub(crate) fn cancel_current() {
    if let Ok(current) = CURRENT_RUN.lock() {
        if let Some(flag) = current.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

/// Fresh run id: ms timestamp + 48 bits from the approval nonce RNG.
/// Reuses [`tools::issue_approval_token`] (no new deps).
pub(crate) fn new_run_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let nonce = tools::issue_approval_token();
    format!("{ms:x}-{}", &nonce[..12])
}

/// Compact read-only summary of a stored run's events for resume context.
/// Tolerates foreign payload shapes; never panics.
pub(crate) fn resume_context(events: &[(String, String)], cap_chars: usize) -> String {
    let mut lines = Vec::with_capacity(events.len());
    for (kind, payload) in events {
        let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let line = match kind.as_str() {
            "agentStep" | "step" => {
                let step = value
                    .get("step")
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let tool = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let state = value.get("state").and_then(Value::as_str).unwrap_or("");
                let detail = value.get("detail").and_then(Value::as_str).unwrap_or("");
                format!("step {step} · {tool} · {state} — {}", truncate_chars(detail, 200))
            }
            "agentDone" | "done" => {
                let summary = value.get("summary").and_then(Value::as_str).unwrap_or(payload);
                format!("done: {}", truncate_chars(summary, 200))
            }
            _ => truncate_chars(payload, 200),
        };
        lines.push(line);
    }
    truncate_chars(&lines.join("\n"), cap_chars)
}

/// Goal run with optional `!retry` resume: stored events re-emit as
/// read-only history, then the stored goal runs fresh under a new id.
/// Store failures never fail the run (log + continue).
pub(crate) fn run_goal_resuming(
    goal: String,
    resume_run_id: Option<String>,
    config: AgentConfig,
    cancel: Arc<AtomicBool>,
    mut push: impl FnMut(Value) + Send + 'static,
) {
    let store = match store::open() {
        Ok(store) => Some(store),
        Err(error) => {
            crate::runtime::log_info(&format!("[nex][agent] run store unavailable: {error}"));
            None
        }
    };
    run_goal_resuming_with(goal, resume_run_id, config, cancel, &mut push, store.as_ref());
}

fn run_goal_resuming_with(
    goal: String,
    resume_run_id: Option<String>,
    config: AgentConfig,
    cancel: Arc<AtomicBool>,
    push: &mut impl FnMut(Value),
    store: Option<&store::Store>,
) {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (goal, resume_run_id, config, cancel, store);
        push_event(push, AgentEvent::done("Agent goals need Windows."));
        return;
    }
    #[cfg(target_os = "windows")]
    {
        run_goal_resuming_live(goal, resume_run_id, config, cancel, push, store);
    }
}

/// Append one event to the store; failures log and never fail the run.
#[cfg(target_os = "windows")]
fn persist_event(store: Option<&store::Store>, run_id: &str, value: &Value) {
    let Some(store) = store else { return };
    let kind = value.get("t").and_then(Value::as_str).unwrap_or("event");
    if kind != "agentStep" && kind != "agentDone" {
        return;
    }
    if let Err(error) = store.append_event(run_id, kind, &value.to_string()) {
        crate::runtime::log_info(&format!("[nex][agent] run store append failed: {error}"));
    }
}

#[cfg(target_os = "windows")]
fn run_goal_resuming_live(
    goal: String,
    resume_run_id: Option<String>,
    config: AgentConfig,
    cancel: Arc<AtomicBool>,
    push: &mut impl FnMut(Value),
    store: Option<&store::Store>,
) {
    let mut goal = goal;
    let mut history_note = String::new();
    if let Some(resume_id) = resume_run_id.as_deref().map(str::trim) {
        if !resume_id.is_empty() {
            let loaded = store
                .ok_or_else(|| "the run store is unavailable".to_string())
                .and_then(|store| store.load_run(resume_id));
            match loaded {
                Ok((old_goal, _status, events)) => {
                    for (_kind, payload) in &events {
                        if let Ok(value) = serde_json::from_str::<Value>(payload) {
                            push(value);
                        }
                    }
                    if goal.trim().is_empty() {
                        goal = old_goal;
                    }
                    history_note = resume_context(&events, 2000);
                }
                Err(_) => {
                    push_event(push, AgentEvent::done(format!("Unknown run: {resume_id}")));
                    return;
                }
            }
        }
    }
    let run_id = new_run_id();
    if let Some(store) = store {
        if let Err(error) = store.create_run(&run_id, &goal) {
            crate::runtime::log_info(&format!("[nex][agent] run store create failed: {error}"));
        }
    }
    // Seam: wrap `push` so every step/done event carries the new run id
    // (additive field the UI ignores except to display) and is persisted,
    // without touching `drive_loop` or the transports.
    let mut push_persist = |mut value: Value| {
        if let Some(obj) = value.as_object_mut() {
            let tag = obj.get("t").and_then(Value::as_str).unwrap_or("");
            if tag == "agentStep" || tag == "agentDone" {
                obj.insert("run_id".into(), Value::String(run_id.clone()));
            }
        }
        persist_event(store, &run_id, &value);
        push(value);
    };
    let started_detail = match resume_run_id.as_deref().map(str::trim) {
        Some(resume_id) if !resume_id.is_empty() => format!("resumed from {resume_id}"),
        _ => truncate_chars(goal.trim(), 120),
    };
    push_event(
        &mut push_persist,
        AgentEvent::step(0, "run", "started", started_detail).with_run_id(&run_id),
    );
    let model_goal = if history_note.trim().is_empty() {
        goal.clone()
    } else {
        format!(
            "{goal}\n\n[History from run {} — context only, do not re-execute it:]\n{history_note}",
            resume_run_id.as_deref().unwrap_or("").trim(),
        )
    };
    match config.provider.as_str() {
        "codex" => run_codex_goal(model_goal, config, cancel.clone(), &mut push_persist),
        "openai-compatible" => run_openai_goal(model_goal, config, cancel.clone(), &mut push_persist),
        _ => push_event(
            &mut push_persist,
            AgentEvent::done(
                "Agent goals need the ChatGPT or an OpenAI-compatible provider. Choose one in settings, then try again.",
            ),
        ),
    }
    if let Some(store) = store {
        let status = if cancel.load(Ordering::SeqCst) { "cancelled" } else { "done" };
        if let Err(error) = store.finish_run(&run_id, status) {
            crate::runtime::log_info(&format!("[nex][agent] run store finish failed: {error}"));
        }
    }
}

/// Shared approval gate: reads run free, writes/exec hold for a UI token.
#[cfg(target_os = "windows")]
fn approve_call(call: &ToolCall, push: &mut dyn FnMut(Value)) -> Approval {
    if !tools::needs_approval(&call.name) {
        return Approval::Auto;
    }
    let token = tools::issue_approval_token();
    push_event(
        &mut { push },
        AgentEvent::approval(&call.call_id, &call.name, args_summary(call)),
    );
    let rx = approvals::request_approval(call.call_id.clone());
    match rx.recv_timeout(Duration::from_secs(APPROVAL_TIMEOUT_SECS)) {
        Ok(true) => Approval::Allow(token),
        _ => Approval::Deny, // deny + timeout/disconnect default to deny
    }
}

/// Shared dispatch: threads the approval token back into gated tools.
#[cfg(target_os = "windows")]
fn execute_call(call: &ToolCall, approval: Option<&str>) -> Result<String, String> {
    let mut args = call.arguments.clone();
    if let (Some(token), Some(obj)) = (approval, args.as_object_mut()) {
        obj.insert("approval_token".into(), Value::String(token.to_string()));
    }
    tools::dispatch(&call.name, &args, approval)
}

#[cfg(target_os = "windows")]
fn run_codex_goal(
    goal: String,
    config: AgentConfig,
    cancel: Arc<AtomicBool>,
    push: &mut impl FnMut(Value),
) {
    let mut input = vec![json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": goal }],
    })];
    let deadline = Instant::now() + Duration::from_secs(WALL_CLOCK_SECS);
    let model = config.model.clone();
    let mut transport = |items: &[Value], sink: &mut dyn FnMut(Value)| {
        stream_turn(&model, items, &cancel, sink)
    };
    drive_loop(
        &mut input,
        MAX_TURNS,
        deadline,
        &cancel,
        &mut transport,
        &mut approve_call,
        &mut execute_call,
        push,
    );
}

/// OpenAI-compatible goal run: same turn loop, chat-completions transport.
/// The loop's `input` only carries `function_call_output` items; the chat
/// `messages` (with `role:tool` results) live in the transport closure.
#[cfg(target_os = "windows")]
fn run_openai_goal(
    goal: String,
    config: AgentConfig,
    cancel: Arc<AtomicBool>,
    push: &mut impl FnMut(Value),
) {
    if config.api_key.trim().is_empty() {
        push_event(
            push,
            AgentEvent::done(
                "Add an API key for the OpenAI-compatible provider in settings, then try again.",
            ),
        );
        return;
    }
    let mut input: Vec<Value> = Vec::new();
    let mut messages = vec![json!({"role": "user", "content": goal})];
    let mut consumed = 0usize;
    let deadline = Instant::now() + Duration::from_secs(WALL_CLOCK_SECS);
    let mut transport = |items: &[Value], sink: &mut dyn FnMut(Value)| {
        for item in items.iter().skip(consumed) {
            if let Some((id, output)) = tool_output_parts(item) {
                messages.push(json!({"role": "tool", "tool_call_id": id, "content": output}));
            }
        }
        consumed = items.len();
        let turn = stream_openai_turn(
            &config.model,
            &config.base_url,
            &config.api_key,
            &messages,
            &cancel,
            sink,
        )?;
        if !turn.calls.is_empty() {
            messages.push(assistant_calls_message(&turn.text, &turn.calls));
        } else if !turn.text.trim().is_empty() {
            messages.push(json!({"role": "assistant", "content": turn.text}));
        }
        Ok(turn)
    };
    drive_loop(
        &mut input,
        MAX_TURNS,
        deadline,
        &cancel,
        &mut transport,
        &mut approve_call,
        &mut execute_call,
        push,
    );
}

/// One live chat-completions turn with tools: mirrors `stream_openai` in
/// `chat.rs` (same endpoint/auth/SSE shape) but sends the function `tools`
/// array and accumulates `delta.tool_calls[]` chunks into full calls.
/// Text deltas push `chatDelta` exactly like chat today.
#[cfg(target_os = "windows")]
fn stream_openai_turn(
    model: &str,
    base_url: &str,
    api_key: &str,
    messages: &[Value],
    cancel: &AtomicBool,
    push: &mut dyn FnMut(Value),
) -> Result<TurnOutcome, String> {
    use std::io::{BufRead, BufReader};

    let mut outcome = TurnOutcome::default();
    let mut calls = ToolCallParts::default();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "tools": chat_tools_body(),
    });
    let response = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream")
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(code, response) => crate::chat::http_error(
                code,
                response.into_string().unwrap_or_default(),
                api_key,
            ),
            ureq::Error::Transport(_) => {
                "Could not reach the provider. Check the URL and your connection.".to_string()
            }
        })?;
    let mut reader = BufReader::new(response.into_reader());
    let mut line = String::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(outcome);
        }
        line.clear();
        let count = match reader.read_line(&mut line) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(_) => return Err("The provider connection ended unexpectedly.".into()),
        };
        if count == 0 {
            break;
        }
        let Some(payload) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            break;
        }
        let Ok(data) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(text) = data
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            outcome.text.push_str(text);
            push(json!({"chatDelta": {"text": text}}));
        }
        calls.feed(&data);
        if let Some(message) = data.pointer("/error/message").and_then(Value::as_str) {
            return Err(message.to_string());
        }
    }
    outcome.calls = calls.finish();
    Ok(outcome)
}

/// One live Responses turn with tools: mirrors `stream_codex_native` in
/// `chat.rs` (same body + `tools` array, same SSE event loop) but also
/// collects `function_call` items. Text deltas push `chatDelta` like today.
#[cfg(target_os = "windows")]
fn stream_turn(
    model: &str,
    input: &[Value],
    cancel: &AtomicBool,
    push: &mut dyn FnMut(Value),
) -> Result<TurnOutcome, String> {
    use std::io::{BufRead, BufReader};

    let mut outcome = TurnOutcome::default();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build();
    let url = format!("{}/responses", crate::codex_auth::CODEX_BASE_URL);
    let body = json!({
        "model": model,
        "instructions": "You are Nex, a concise assistant inside the Nex launcher. Use the provided tools to complete the user's goal step by step. Keep text replies short.",
        "input": input,
        "tools": tools_body(),
        "stream": true,
        "store": false,
        "max_output_tokens": 1024,
    });
    let Some((access, account)) = crate::codex_auth::fresh_tokens() else {
        return Err("Connect your ChatGPT account first.".into());
    };
    let mut request = agent.post(&url);
    for (name, value) in crate::codex_auth::backend_headers(&access, account.as_deref()) {
        request = request.set(&name, &value);
    }
    let response = request
        .set("Accept", "text/event-stream")
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(code, response) => {
                response.into_string().unwrap_or_default();
                format!("ChatGPT returned HTTP {code}.")
            }
            ureq::Error::Transport(_) => {
                "Could not reach ChatGPT. Check your connection.".to_string()
            }
        })?;
    let mut reader = BufReader::new(response.into_reader());
    let mut line = String::new();
    let mut event = String::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(outcome);
        }
        line.clear();
        let count = match reader.read_line(&mut line) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(_) => return Err("The ChatGPT connection ended unexpectedly.".into()),
        };
        if count == 0 {
            break;
        }
        let line = line.trim();
        if let Some(kind) = line.strip_prefix("event:") {
            event = kind.trim().to_string();
            continue;
        }
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            break;
        }
        let Ok(data) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if event == "response.output_text.delta" {
            if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                outcome.text.push_str(delta);
                push(json!({"chatDelta": {"text": delta}}));
            }
            continue;
        }
        if event == "response.output_item.done" || event == "response.completed" {
            let before = outcome.calls.len();
            find_function_calls(&data, &mut outcome.calls);
            if outcome.calls.len() == before {
                crate::runtime::log_info(&format!(
                    "[nex][agent] ignored output item shape: {}",
                    truncate_chars(payload, 200)
                ));
            }
            continue;
        }
        if data
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some()
        {
            let message = data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!(
                "ChatGPT error: {}",
                truncate_chars(message, 280)
            ));
        }
        crate::runtime::log_info(&format!(
            "[nex][agent] ignored SSE shape '{event}': {}",
            truncate_chars(payload, 200)
        ));
    }
    Ok(outcome)
}

/// Tolerantly collect every `{"type":"function_call",…}` object in an SSE
/// payload (covers `output_item.done` and `response.completed` shapes).
fn find_function_calls(value: &Value, out: &mut Vec<ToolCall>) {
    match value {
        Value::Object(map) => {
            if let Some(call) = parse_function_call(value) {
                out.push(call);
            } else {
                for child in map.values() {
                    find_function_calls(child, out);
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                find_function_calls(child, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_function_call_item() {
        let item = json!({
            "type": "function_call",
            "call_id": "call-1",
            "name": "fs_list",
            "arguments": "{\"path\":\".\"}"
        });
        let call = parse_function_call(&item).unwrap();
        assert_eq!(
            call,
            ToolCall {
                call_id: "call-1".into(),
                name: "fs_list".into(),
                arguments: json!({"path": "."}),
            }
        );
    }

    #[test]
    fn parse_missing_fields_yields_none() {
        assert!(parse_function_call(&json!({"type": "function_call", "name": "x"})).is_none());
        assert!(parse_function_call(&json!({"type": "function_call", "call_id": "c"})).is_none());
        assert!(
            parse_function_call(&json!({"type": "function_call", "call_id": "", "name": "x"}))
                .is_none()
        );
        assert!(parse_function_call(&json!({"nope": 1})).is_none());
    }

    #[test]
    fn parse_non_call_item_yields_none() {
        assert!(parse_function_call(&json!({"type": "message", "call_id": "c", "name": "x"})).is_none());
        assert!(parse_function_call(&json!([1, 2])).is_none());
        assert!(parse_function_call(&Value::Null).is_none());
    }

    #[test]
    fn truncation_caps_chars() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        let out = truncate_chars(&"x".repeat(10), 4);
        assert!(out.starts_with("xxxx") && out.contains("truncated"));
    }

    #[test]
    fn chat_tools_body_uses_function_shape() {
        let body = chat_tools_body();
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), tools::registry().len());
        for item in arr {
            assert_eq!(item["type"], "function");
            assert!(item["function"]["name"].is_string());
            assert!(item["function"]["description"].is_string());
            assert_eq!(item["function"]["parameters"]["type"], "object");
        }
    }

    #[test]
    fn chat_tool_calls_accumulate_across_chunks() {
        let mut parts = ToolCallParts::default();
        for raw in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"fs_li","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"st","arguments":"{\"path\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"content":"checking ","tool_calls":[{"index":0,"function":{"arguments":" \".\"}"}},{"index":1,"id":"call-2","type":"function","function":{"name":"fs_list","arguments":"{\"path\":\".\"}"}}]}}]}"#,
        ] {
            let data: Value = serde_json::from_str(raw).unwrap();
            parts.feed(&data);
        }
        let calls = parts.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call_id, "call-1");
        assert_eq!(calls[0].name, "fs_list");
        assert_eq!(calls[0].arguments, json!({"path": "."}));
        assert_eq!(calls[1].call_id, "call-2");
        assert_eq!(calls[1].name, "fs_list");
    }

    #[test]
    fn chat_stream_without_calls_finishes_clean() {
        let mut parts = ToolCallParts::default();
        for raw in [
            r#"{"choices":[{"delta":{"content":"All done."},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"unrelated":true}"#,
        ] {
            let data: Value = serde_json::from_str(raw).unwrap();
            parts.feed(&data);
        }
        assert!(parts.finish().is_empty());
    }

    #[test]
    fn function_output_converts_to_tool_message() {
        let item = json!({"type": "function_call_output", "call_id": "c1", "output": "a\nb"});
        assert_eq!(tool_output_parts(&item), Some(("c1", "a\nb")));
        assert!(tool_output_parts(&json!({"type": "message"})).is_none());
        assert!(tool_output_parts(&json!({"type": "function_call_output"})).is_none());
    }

    #[test]
    fn openai_401_points_at_api_key() {
        let message = crate::chat::http_error(401, String::new(), "secret");
        assert!(message.contains("API key"), "{message}");
    }

    #[test]
    fn tools_body_matches_registry() {
        let body = tools_body();
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), tools::registry().len());
        for item in arr {
            assert_eq!(item["type"], "function");
            assert!(item["name"].is_string());
            assert_eq!(item["parameters"]["type"], "object");
        }
    }

    #[test]
    fn loop_stops_at_turn_budget_against_canned_transcript() {
        let mut input = vec![json!({"type": "message", "role": "user"})];
        let cancel = AtomicBool::new(false);
        let mut turns = 0u32;
        let mut transport = |_items: &[Value], _sink: &mut dyn FnMut(Value)| {
            turns += 1;
            Ok(TurnOutcome {
                text: String::new(),
                calls: vec![ToolCall {
                    call_id: format!("call-{turns}"),
                    name: "fs_list".into(),
                    arguments: json!({"path": "."}),
                }],
            })
        };
        let mut approve =
            |_call: &ToolCall, _push: &mut dyn FnMut(Value)| -> Approval { Approval::Auto };
        let mut execute =
            |_call: &ToolCall, _approval: Option<&str>| -> Result<String, String> { Ok("a\nb".into()) };
        let mut pushed = Vec::new();
        let mut push = |v: Value| pushed.push(v);
        drive_loop(
            &mut input,
            3,
            Instant::now() + Duration::from_secs(30),
            &cancel,
            &mut transport,
            &mut approve,
            &mut execute,
            &mut push,
        );
        assert_eq!(turns, 3);
        let done = pushed.iter().rev().find(|v| v.get("t") == Some(&json!("agentDone")));
        let summary = done.unwrap()["summary"].as_str().unwrap();
        assert!(summary.contains("3 steps"), "unexpected: {summary}");
        // 1 seed + 3 outputs fed back into context.
        assert_eq!(input.len(), 4);
    }

    #[test]
    fn loop_finishes_when_model_returns_no_calls() {
        let mut input = Vec::new();
        let cancel = AtomicBool::new(false);
        let mut transport = |_items: &[Value], _sink: &mut dyn FnMut(Value)| {
            Ok(TurnOutcome {
                text: "All done.".into(),
                calls: Vec::new(),
            })
        };
        let mut approve =
            |_call: &ToolCall, _push: &mut dyn FnMut(Value)| -> Approval { Approval::Auto };
        let mut execute =
            |_call: &ToolCall, _approval: Option<&str>| -> Result<String, String> { unreachable!() };
        let mut pushed = Vec::new();
        let mut push = |v: Value| pushed.push(v);
        drive_loop(
            &mut input,
            12,
            Instant::now() + Duration::from_secs(30),
            &cancel,
            &mut transport,
            &mut approve,
            &mut execute,
            &mut push,
        );
        let last = pushed.last().unwrap();
        assert_eq!(last["t"], "agentDone");
        assert_eq!(last["summary"], "All done.");
    }

    #[test]
    fn loop_denial_stops_with_done_summary() {
        let mut input = Vec::new();
        let cancel = AtomicBool::new(false);
        let mut transport = |_items: &[Value], _sink: &mut dyn FnMut(Value)| {
            Ok(TurnOutcome {
                text: String::new(),
                calls: vec![ToolCall {
                    call_id: "c1".into(),
                    name: "shell_exec".into(),
                    arguments: json!({"command": "rm -rf /"}),
                }],
            })
        };
        let mut approve =
            |_call: &ToolCall, _push: &mut dyn FnMut(Value)| -> Approval { Approval::Deny };
        let mut execute =
            |_call: &ToolCall, _approval: Option<&str>| -> Result<String, String> { unreachable!() };
        let mut pushed = Vec::new();
        let mut push = |v: Value| pushed.push(v);
        drive_loop(
            &mut input,
            12,
            Instant::now() + Duration::from_secs(30),
            &cancel,
            &mut transport,
            &mut approve,
            &mut execute,
            &mut push,
        );
        assert_eq!(pushed.last().unwrap()["t"], "agentDone");
        assert!(pushed.iter().any(|v| v.get("state") == Some(&json!("denied"))));
    }

    #[test]
    fn new_run_ids_are_unique_and_nonempty() {
        let a = new_run_id();
        let b = new_run_id();
        assert!(!a.is_empty() && a.contains('-'));
        assert_ne!(a, b);
    }

    #[test]
    fn resume_context_summarizes_stored_events() {
        let events = vec![
            ("agentStep".to_string(), r#"{"t":"agentStep","step":1,"tool":"fs_list","state":"done","detail":"a\nb"}"#.to_string()),
            ("agentDone".to_string(), r#"{"t":"agentDone","summary":"All done."}"#.to_string()),
            ("bogus".to_string(), "not json at all".to_string()),
        ];
        let context = resume_context(&events, 2000);
        assert!(context.contains("fs_list"), "{context}");
        assert!(context.contains("All done."), "{context}");
        assert!(context.contains("not json at all"), "{context}");
        let capped = resume_context(&events, 10);
        assert!(capped.chars().count() <= 10 + "…[truncated]".len());
    }

    #[test]
    fn unknown_resume_id_pushes_error_done_without_running() {
        let dir = tempfile::tempdir().unwrap();
        let store = store::open_at(&dir.path().join("agent-runs.sqlite3")).unwrap();
        let config = AgentConfig {
            provider: "codex".into(),
            model: "m".into(),
            base_url: String::new(),
            api_key: String::new(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut pushed = Vec::new();
        {
            let mut push = |v: Value| pushed.push(v);
            run_goal_resuming_with(
                String::new(),
                Some("nope".into()),
                config,
                cancel,
                &mut push,
                Some(&store),
            );
        }
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0]["t"], "agentDone");
        assert!(pushed[0]["summary"].as_str().unwrap().contains("Unknown run"));
        // No fresh run was created for the unknown id.
        assert!(store.recent_runs(10).unwrap().is_empty());
    }

    #[test]
    fn loop_cancel_stops_between_steps() {
        let mut input = Vec::new();
        let cancel = AtomicBool::new(true);
        let mut calls = 0u32;
        let mut transport = |_items: &[Value], _sink: &mut dyn FnMut(Value)| {
            calls += 1;
            Ok(TurnOutcome::default())
        };
        let mut approve =
            |_call: &ToolCall, _push: &mut dyn FnMut(Value)| -> Approval { Approval::Auto };
        let mut execute =
            |_call: &ToolCall, _approval: Option<&str>| -> Result<String, String> { Ok(String::new()) };
        let mut pushed = Vec::new();
        let mut push = |v: Value| pushed.push(v);
        drive_loop(
            &mut input,
            12,
            Instant::now() + Duration::from_secs(30),
            &cancel,
            &mut transport,
            &mut approve,
            &mut execute,
            &mut push,
        );
        assert_eq!(calls, 0);
        assert_eq!(pushed.last().unwrap()["summary"], "Cancelled.");
    }
}
