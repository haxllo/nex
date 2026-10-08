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

/// Minimal config snapshot: the loop only needs the model id.
#[derive(Debug, Clone)]
pub(crate) struct AgentConfig {
    pub(crate) model: String,
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

/// Entry point (Task 8 wires it to a worker thread; runs sync here).
/// `cancel` mirrors the `ACTIVE_REQUEST` id pattern in `chat.rs` but as a
/// shared flag so `chatCancel` can stop the loop between steps.
pub(crate) fn run_goal(
    goal: String,
    config: AgentConfig,
    cancel: Arc<AtomicBool>,
    mut push: impl FnMut(Value) + Send + 'static,
) {
    #[cfg(target_os = "windows")]
    {
        run_goal_live(goal, config, cancel, &mut push);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (goal, config, cancel);
        push_event(&mut push, AgentEvent::done("Agent goals need Windows."));
    }
}

#[cfg(target_os = "windows")]
fn run_goal_live(
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
    let mut approve = |call: &ToolCall, push: &mut dyn FnMut(Value)| -> Approval {
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
    };
    let mut execute = |call: &ToolCall, approval: Option<&str>| -> Result<String, String> {
        let mut args = call.arguments.clone();
        if let (Some(token), Some(obj)) = (approval, args.as_object_mut()) {
            obj.insert("approval_token".into(), Value::String(token.to_string()));
        }
        tools::dispatch(&call.name, &args, approval)
    };
    drive_loop(
        &mut input,
        MAX_TURNS,
        deadline,
        &cancel,
        &mut transport,
        &mut approve,
        &mut execute,
        push,
    );
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
