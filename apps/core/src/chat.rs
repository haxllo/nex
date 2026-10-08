//! Chat provider boundary for the Nex overlay.
//!
//! Provider credentials and network/process access stay in Rust. The
//! WebView receives only public connection settings and streamed text.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const CONFIG_MAGIC: &[u8] = b"NEXCHAT1";
static REQUEST_ID: AtomicU64 = AtomicU64::new(0);
static ACTIVE_REQUEST: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigureRequest {
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendRequest {
    pub message: String,
    #[serde(default)]
    pub history: Vec<ChatTurn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConfig {
    provider: String,
    base_url: String,
    model: String,
    api_key: String,
}

impl Default for StoredConfig {
    fn default() -> Self {
        Self {
            provider: "openai-compatible".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            api_key: String::new(),
        }
    }
}

pub(crate) fn public_config() -> Value {
    let config = load_config().unwrap_or_default();
    let account_connected = config.provider == "codex" && codex_authenticated();
    json!({
        "provider": config.provider,
        "baseUrl": config.base_url,
        "model": config.model,
        "configured": if config.provider == "codex" { account_connected } else { !config.api_key.is_empty() },
        "accountConnected": account_connected,
    })
}

pub(crate) fn configure(raw: &str) -> Result<Value, String> {
    let mut request: ConfigureRequest = serde_json::from_str(raw)
        .map_err(|_| "Those chat settings could not be read.".to_string())?;
    request.provider = request.provider.trim().to_ascii_lowercase();
    if !matches!(request.provider.as_str(), "openai-compatible" | "codex") {
        return Err("Choose a supported chat provider.".into());
    }
    if request.provider == "codex" && request.model.trim().is_empty() {
        request.model = "gpt-6-luna".into();
    }
    if request.model.trim().is_empty() || request.model.chars().count() > 128 {
        return Err("Enter a model name under 128 characters.".into());
    }
    if request.provider == "openai-compatible" {
        let url = url::Url::parse(request.base_url.trim())
            .map_err(|_| "Enter a valid provider URL.".to_string())?;
        if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
            return Err("The provider URL must use HTTP or HTTPS.".into());
        }
    }
    if request.api_key.trim().is_empty() {
        request.api_key = load_config().unwrap_or_default().api_key;
    }
    if request.provider == "openai-compatible" && request.api_key.trim().is_empty() {
        return Err("Add an API key before saving this provider.".into());
    }
    if request.base_url.len() > 2048 || request.api_key.len() > 4096 {
        return Err("One of the provider settings is too long.".into());
    }

    let config = StoredConfig {
        provider: request.provider,
        base_url: request.base_url.trim().trim_end_matches('/').to_string(),
        model: request.model.trim().to_string(),
        api_key: request.api_key,
    };
    store_config(&config)?;
    Ok(public_config())
}

pub(crate) fn connect(provider: &str, mut push: impl FnMut(Value) + Send + 'static) {
    if provider != "codex" {
        push(json!({"chatConnecting":false,"chatError":"Choose ChatGPT to connect an account."}));
        return;
    }
    if codex_authenticated() {
        push(json!({"chatConnecting":false,"chatConfig":public_config(),"chatNotice":"ChatGPT is already connected. Choose a model and start chatting."}));
        return;
    }
    match crate::codex_auth::start_login_server() {
        Ok(server) => {
            push(json!({"chatConnecting":true,"chatNotice":"ChatGPT sign-in opened in your browser. Nex will confirm the connection automatically."}));
            let _ = std::thread::Builder::new()
                .name("nex-codex-login-watch".into())
                .spawn(move || {
                    let deadline = Duration::from_secs(300);
                    match crate::codex_auth::wait_for_login(server, deadline) {
                        Ok(()) if codex_authenticated() => {
                            push(json!({"chatConnecting":false,"chatConfig":public_config(),"chatNotice":"ChatGPT connected. Choose a model and start chatting."}));
                        }
                        _ => {
                            push(json!({"chatConnecting":false,"chatConfig":public_config(),"chatError":"ChatGPT sign-in was not confirmed. Try again in a moment."}));
                        }
                    }
                });
            return;
        }
        Err(error) => {
            // No CLI fallback: stores are independent, so a CLI sign-in
            // would land in the other app's account, not ours.
            push(json!({"chatConnecting":false,"chatError":format!("Could not start ChatGPT sign-in: {error}")}));
            return;
        }
    }
}

pub(crate) fn disconnect(mut push: impl FnMut(Value) + Send + 'static) {
    if !codex_authenticated() {
        push(json!({"chatDisconnecting":false,"chatConfig":public_config(),"chatModels":{"provider":"codex","models":[]},"chatNotice":"ChatGPT is already signed out."}));
        return;
    }
    // Native only: the CLI holds a different account now, so its logout
    // must never run from here.
    match crate::codex_auth::logout() {
        Ok(_) if !codex_authenticated() => {
            push(json!({"chatDisconnecting":false,"chatConfig":public_config(),"chatModels":{"provider":"codex","models":[]},"chatNotice":"ChatGPT account disconnected. Connect again any time."}));
        }
        _ => {
            push(json!({"chatDisconnecting":false,"chatError":"Could not sign out of ChatGPT. Try again in a moment."}));
        }
    }
}

pub(crate) fn fetch_models(raw: &str) -> Result<Value, String> {
    let mut request: ConfigureRequest = serde_json::from_str(raw)
        .map_err(|_| "Those provider settings could not be read.".to_string())?;
    request.provider = request.provider.trim().to_ascii_lowercase();
    let models = match request.provider.as_str() {
        "codex" => fetch_codex_models()?,
        "openai-compatible" => {
            if request.api_key.trim().is_empty() {
                request.api_key = load_config().unwrap_or_default().api_key;
            }
            if request.api_key.trim().is_empty() {
                return Err("Add and save your API key before loading models.".into());
            }
            let base_url = url::Url::parse(request.base_url.trim())
                .map_err(|_| "Enter a valid provider URL before loading models.".to_string())?;
            if !matches!(base_url.scheme(), "https" | "http") || base_url.host_str().is_none() {
                return Err("The provider URL must use HTTP or HTTPS.".into());
            }
            fetch_compatible_models(base_url.as_str().trim_end_matches('/'), &request.api_key)?
        }
        _ => return Err("Choose a supported chat provider first.".into()),
    };
    Ok(json!({"chatModels":{"provider":request.provider,"models":models}}))
}

fn fetch_compatible_models(base_url: &str, api_key: &str) -> Result<Vec<Value>, String> {
    let endpoint = format!("{base_url}/models");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(12))
        .timeout_read(Duration::from_secs(12))
        .build();
    let response = match agent.get(&endpoint)
        .set("Authorization", &format!("Bearer {api_key}"))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            return Err(http_error(code, response.into_string().unwrap_or_default(), api_key));
        }
        Err(ureq::Error::Transport(_)) => return Err("Could not fetch models. Check the API URL and your connection.".into()),
    };
    let payload: Value = response.into_json()
        .map_err(|_| "The provider returned an unreadable model list.".to_string())?;
    let mut models = payload.get("data").and_then(Value::as_array)
        .ok_or_else(|| "This provider did not return an OpenAI-compatible model list.".to_string())?
        .iter()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty() && id.len() <= 128)
        .map(|id| json!({"id":id,"name":id,"description":"Available from this provider."}))
        .collect::<Vec<_>>();
    sort_model_recommendations(&mut models);
    models.truncate(80);
    Ok(models)
}

fn fetch_codex_models() -> Result<Vec<Value>, String> {
    match fetch_codex_models_native() {
        Ok(models) => Ok(models),
        Err(native_error) => {
            if cli_available("codex") {
                fetch_codex_models_cli()
            } else {
                Err(format!("{native_error} You can still enter a model ID manually."))
            }
        }
    }
}

fn codex_request(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
) -> Result<ureq::Request, String> {
    let Some((access, account)) = crate::codex_auth::fresh_tokens() else {
        return Err("Connect your ChatGPT account first.".into());
    };
    let mut request = match method {
        "GET" => agent.get(url),
        _ => agent.post(url),
    };
    for (name, value) in crate::codex_auth::backend_headers(&access, account.as_deref()) {
        request = request.set(&name, &value);
    }
    Ok(request)
}

fn fetch_codex_models_native() -> Result<Vec<Value>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(12))
        .timeout_read(Duration::from_secs(12))
        .build();
    let url = format!("{}/models", crate::codex_auth::CODEX_BASE_URL);
    let response = codex_request(&agent, "GET", &url)?
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => {
                crate::codex_auth::forget();
                "ChatGPT session expired. Reconnect your account.".to_string()
            }
            ureq::Error::Status(code, response) => {
                codex_http_error(code, response.into_string().unwrap_or_default())
            }
            ureq::Error::Transport(_) => {
                "Could not reach ChatGPT. Check your connection.".to_string()
            }
        })?;
    if response.status() != 200 {
        return Err("ChatGPT did not return its model list.".into());
    }
    let payload: Value = response
        .into_json()
        .map_err(|_| "ChatGPT returned an unreadable model list.".to_string())?;
    let mut models = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "ChatGPT returned an unreadable model list.".to_string())?
        .iter()
        .filter_map(|model| {
            let id = model
                .get("id")
                .or_else(|| model.get("model"))
                .or_else(|| model.get("name"))?
                .as_str()?;
            if id.is_empty() || id.len() > 128 {
                return None;
            }
            let name = model
                .get("displayName")
                .or_else(|| model.get("display_name"))
                .and_then(Value::as_str)
                .unwrap_or(id);
            let description = model
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("Available with your ChatGPT account.");
            let is_default = model
                .get("isDefault")
                .or_else(|| model.get("is_default"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(json!({"id":id,"name":name,"description":description,"default":is_default}))
        })
        .collect::<Vec<_>>();
    sort_model_recommendations(&mut models);
    models.truncate(80);
    Ok(models)
}

fn fetch_codex_models_cli() -> Result<Vec<Value>, String> {
    if !cli_available("codex") {
        return Err("Codex CLI was not found. Install it and connect your account first.".into());
    }
    if !codex_authenticated() {
        return Err("Connect your ChatGPT account before loading models.".into());
    }
    let args = vec!["app-server".into(), "--stdio".into()];
    let mut child = cli_command("codex", &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start ChatGPT to load its available models.".to_string())?;
    let mut stdin = child.stdin.take().ok_or("ChatGPT did not open its model channel.")?;
    let stdout = child.stdout.take().ok_or("ChatGPT did not open its model channel.")?;
    let lines = spawn_line_reader(stdout);
    let init = json!({"method":"initialize","id":1,"params":{"clientInfo":{"name":"nex","title":"Nex","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}});
    writeln!(stdin, "{init}").map_err(|_| "Could not initialize the ChatGPT model channel.".to_string())?;
    let init_deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if std::time::Instant::now() >= init_deadline {
            kill_provider_tree(&mut child);
            return Err("ChatGPT did not respond while loading models. Try again in a moment.".into());
        }
        let line = match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                kill_provider_tree(&mut child);
                return Err("ChatGPT closed its model channel unexpectedly.".into());
            }
        };
        if serde_json::from_str::<Value>(&line).ok().and_then(|v| v.get("id").and_then(Value::as_i64)) == Some(1) { break; }
    }
    writeln!(stdin, "{}", json!({"method":"initialized","params":{}})).ok();
    writeln!(stdin, "{}", json!({"method":"model/list","id":2,"params":{"includeHidden":false}}))
        .map_err(|_| "Could not request the ChatGPT model list.".to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    let payload = loop {
        if std::time::Instant::now() >= deadline {
            kill_provider_tree(&mut child);
            return Err("ChatGPT did not return its model list in time. Try again.".into());
        }
        let line = match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                kill_provider_tree(&mut child);
                return Err("ChatGPT closed its model channel unexpectedly.".into());
            }
        };
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if value.get("id").and_then(Value::as_i64) == Some(2) { break value; }
        }
    };
    kill_provider_tree(&mut child);
    if let Some(error) = payload.pointer("/error/message").and_then(Value::as_str) {
        return Err(error.chars().take(240).collect());
    }
    let mut models = payload.pointer("/result/data").and_then(Value::as_array)
        .ok_or_else(|| "ChatGPT returned an unreadable model list.".to_string())?
        .iter()
        .filter(|model| !model.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?;
            if id.is_empty() || id.len() > 128 { return None; }
            Some(json!({
                "id":id,
                "name":model.get("displayName").and_then(Value::as_str).unwrap_or(id),
                "description":model.get("description").and_then(Value::as_str).unwrap_or("Available with your ChatGPT account."),
                "default":model.get("isDefault").and_then(Value::as_bool).unwrap_or(false)
            }))
        })
        .collect::<Vec<_>>();
    sort_model_recommendations(&mut models);
    models.truncate(80);
    Ok(models)
}

fn sort_model_recommendations(models: &mut [Value]) {
    models.sort_by_key(|model| {
        let id = model.get("id").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let description = model.get("description").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let light = ["mini", "nano", "flash", "haiku", "luna", "small", "fast", "affordable"]
            .iter().any(|term| id.contains(term) || description.contains(term));
        let is_default = model.get("default").and_then(Value::as_bool).unwrap_or(false);
        (if light { 0 } else if is_default { 1 } else { 2 }, if is_default { 0 } else { 1 }, id)
    });
    for (index, model) in models.iter_mut().enumerate() {
        if let Some(object) = model.as_object_mut() { object.insert("featured".into(), Value::Bool(index == 0)); }
    }
}

fn codex_authenticated() -> bool {
    // Nex store only. The CLI holds a different account now, so its
    // status must never stand in for ours.
    crate::codex_auth::is_signed_in()
}

pub(crate) fn cancel() {
    let id = ACTIVE_REQUEST.load(Ordering::SeqCst);
    ACTIVE_REQUEST.store(id.wrapping_add(1), Ordering::SeqCst);
}

/// Current provider snapshot for agent goals: (provider, model, base_url,
/// api_key). Falls back to defaults when nothing is configured; the caller
/// gates providers.
pub(crate) fn agent_backend() -> (String, String, String, String) {
    let config = load_config().unwrap_or_default();
    (
        config.provider,
        config.model,
        config.base_url,
        config.api_key,
    )
}

pub(crate) fn start(raw: String, mut push: impl FnMut(Value) + Send + 'static) {
    let id = REQUEST_ID.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    ACTIVE_REQUEST.store(id, Ordering::SeqCst);
    let request: SendRequest = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => {
            push(json!({"chatError":"That message could not be sent."}));
            return;
        }
    };
    if request.message.trim().is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("nex-chat-response".into())
        .spawn(move || {
            let config = match load_config() {
                Ok(config) => config,
                Err(error) => {
                    push(json!({"chatError":error}));
                    return;
                }
            };
            let mut emit = |text: &str, status: Option<&str>| {
                if ACTIVE_REQUEST.load(Ordering::SeqCst) == id {
                    push(json!({"chatDelta":{"text":text,"status":status}}));
                }
            };
            let result = match config.provider.as_str() {
                "openai-compatible" => stream_openai(&config, &request, id, &mut emit),
                "codex" => stream_codex(&config, &request, id, &mut emit),
                _ => Err("Choose a chat provider in settings.".into()),
            };
            if ACTIVE_REQUEST.load(Ordering::SeqCst) == id {
                match result {
                    Ok(()) => push(json!({"chatDone":true})),
                    Err(error) => push(json!({"chatError":error})),
                }
            }
        });
    if let Err(error) = spawned {
        crate::logging::warn(&format!("[nex][chat] could not start response worker: {error}"));
    }
}

fn stream_openai(
    config: &StoredConfig,
    request: &SendRequest,
    id: u64,
    emit: &mut impl FnMut(&str, Option<&str>),
) -> Result<(), String> {
    let mut messages = Vec::with_capacity(request.history.len().min(24) + 1);
    for turn in request.history.iter().rev().take(24).collect::<Vec<_>>().into_iter().rev() {
        if matches!(turn.role.as_str(), "user" | "assistant") {
            messages.push(json!({"role":turn.role,"content":turn.content}));
        }
    }
    messages.push(json!({"role":"user","content":request.message}));
    let body = json!({"model":config.model,"messages":messages,"stream":true});
    let endpoint = format!("{}/chat/completions", config.base_url);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(1))
        .build();
    let response = match agent
        .post(&endpoint)
        .set("Authorization", &format!("Bearer {}", config.api_key))
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream")
        .send_json(body)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            return Err(http_error(code, response.into_string().unwrap_or_default(), &config.api_key));
        }
        Err(ureq::Error::Transport(_)) => {
            return Err("Could not reach the provider. Check the URL and your connection.".into());
        }
    };

    let mut reader = BufReader::new(response.into_reader());
    let mut line = String::new();
    loop {
        if ACTIVE_REQUEST.load(Ordering::SeqCst) != id {
            return Ok(());
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
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(message) = event
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            emit(message, None);
        }
        if let Some(error) = event.pointer("/error/message").and_then(Value::as_str) {
            return Err(error.to_string());
        }
    }
    Ok(())
}

fn stream_codex(
    config: &StoredConfig,
    request: &SendRequest,
    id: u64,
    emit: &mut impl FnMut(&str, Option<&str>),
) -> Result<(), String> {
    let mut emitted = 0usize;
    let mut counting_emit = |text: &str, status: Option<&str>| {
        emitted += text.len();
        emit(text, status);
    };
    match stream_codex_native(config, request, id, &mut counting_emit) {
        Ok(()) => Ok(()),
        Err(native_error) => {
            // Auth failures mean the shared store is unusable — the CLI
            // would fail the same way. Anything else falls back while the
            // CLI still exists.
            if emitted == 0
                && !native_error.contains("Reconnect")
                && !native_error.contains("usage limit")
                && !native_error.contains("message limit")
                && !native_error.contains("Connect your ChatGPT")
                && cli_available("codex")
            {
                crate::runtime::log_info(&format!(
                    "[nex][chat] codex native failed ({native_error}), falling back to CLI"
                ));
                emit("", Some("Trying alternate path…"));
                match stream_codex_cli(config, request, id, emit) {
                    Ok(()) => Ok(()),
                    Err(fallback_error) => {
                        let lower = fallback_error.to_lowercase();
                        if lower.contains("401") || lower.contains("unauthorized") {
                            Err("ChatGPT sign-in expired. Reconnect your account and try again.".to_string())
                        } else {
                            Err(fallback_error)
                        }
                    }
                }
            } else {
                Err(native_error)
            }
        }
    }
}

/// Friendly ChatGPT backend failures: quota/rate limits with reset time,
/// expired sessions, over-long conversations, else the backend message.
fn codex_http_error(code: u16, body: String) -> String {
    if code == 401 || code == 403 {
        return "ChatGPT session expired. Reconnect your account.".to_string();
    }
    let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let message = payload
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let lowered = format!("{body} {message}").to_lowercase();
    if code == 429
        || lowered.contains("rate_limit")
        || lowered.contains("rate limit")
        || lowered.contains("quota")
        || lowered.contains("usage limit")
        || lowered.contains("limit reached")
    {
        let mut out =
            "ChatGPT usage limit reached. Wait a bit or check your plan usage.".to_string();
        if let Some(when) = ["/error/resets_at", "/resets_at", "/error/reset_at", "/reset_at"]
            .iter()
            .filter_map(|path| payload.pointer(path).and_then(Value::as_i64))
            .next()
        {
            out.push_str(&format!(" ({})", describe_reset(when)));
        }
        return out;
    }
    if lowered.contains("maximum context")
        || lowered.contains("context length")
        || lowered.contains("too many requests in this conversation")
        || lowered.contains("conversation too long")
        || lowered.contains("open a new chat")
        || lowered.contains("new conversation")
    {
        return "This chat hit the message limit. Start a new conversation to continue.".to_string();
    }
    if !message.is_empty() && message.len() < 280 {
        return format!("ChatGPT error: {message}");
    }
    format!("ChatGPT returned HTTP {code}.")
}

fn describe_reset(resets_at: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = (resets_at - now).max(0);
    if delta < 90 {
        return "resets in under a minute".to_string();
    }
    let minutes = delta / 60;
    if minutes < 90 {
        return format!("resets in about {minutes}m");
    }
    format!("resets in about {}h", minutes / 60)
}

fn stream_codex_native(
    config: &StoredConfig,
    request: &SendRequest,
    id: u64,
    emit: &mut impl FnMut(&str, Option<&str>),
) -> Result<(), String> {
    let prompt = conversation_prompt(request);
    let started = Instant::now();
    crate::runtime::log_info(&format!(
        "[nex][chat] codex native start model={} prompt_chars={}",
        config.model,
        prompt.chars().count()
    ));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build();
    let url = format!("{}/responses", crate::codex_auth::CODEX_BASE_URL);
    let mut input_items: Vec<Value> = request
        .history
        .iter()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter(|turn| matches!(turn.role.as_str(), "user" | "assistant"))
        .map(|turn| {
            // The backend validates content parts per role: user turns
            // take input_* parts, assistant turns take output_* parts.
            let part = if turn.role == "assistant" { "output_text" } else { "input_text" };
            json!({
                "type": "message",
                "role": turn.role,
                "content": [{"type": part, "text": turn.content.chars().take(6000).collect::<String>()}],
            })
        })
        .collect();
    input_items.push(json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": request.message}],
    }));
    let body = json!({
        "model": config.model,
        "instructions": SYSTEM_PROMPT,
        "input": input_items,
        "stream": true,
        "store": false,
        "max_output_tokens": 1024,
    });
    let response = codex_request(&agent, "POST", &url)?
        .set("Accept", "text/event-stream")
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => {
                // Revoked server-side: drop local auth everywhere.
                crate::codex_auth::forget();
                "ChatGPT session expired. Reconnect your account.".to_string()
            }
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                crate::runtime::log_info(&format!(
                    "[nex][chat] codex native HTTP {code}: {}",
                    body.chars().take(240).collect::<String>()
                ));
                codex_http_error(code, body)
            }
            ureq::Error::Transport(inner) => {
                crate::runtime::log_info(&format!(
                    "[nex][chat] codex native transport error: {inner:?}"
                ));
                "Could not reach ChatGPT. Check your connection.".to_string()
            }
        })?;
    let mut reader = BufReader::new(response.into_reader());
    crate::runtime::log_info(&format!(
        "[nex][chat] codex native connected in {}ms",
        started.elapsed().as_millis()
    ));
    let mut line = String::new();
    let mut event = String::new();
    let mut first_token_logged = false;
    loop {
        if ACTIVE_REQUEST.load(Ordering::SeqCst) != id {
            return Ok(());
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
                if !first_token_logged && !delta.is_empty() {
                    first_token_logged = true;
                    crate::runtime::log_info(&format!(
                        "[nex][chat] codex native first token in {}ms",
                        started.elapsed().as_millis()
                    ));
                }
                emit(delta, None);
            }
        } else if let Some(message) = data
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            if !first_token_logged && !message.is_empty() {
                first_token_logged = true;
                crate::runtime::log_info(&format!(
                    "[nex][chat] codex native first token in {}ms",
                    started.elapsed().as_millis()
                ));
            }
            emit(message, None);
        }
        if let Some(error) = data
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| {
                (event == "response.failed" || event == "error")
                    .then(|| {
                        data.get("message")
                            .and_then(Value::as_str)
                            .or_else(|| data.pointer("/response/error/message").and_then(Value::as_str))
                    })
                    .flatten()
            })
        {
            return Err(format!("ChatGPT error: {}", error.chars().take(280).collect::<String>()));
        }
    }
    crate::runtime::log_info(&format!(
        "[nex][chat] codex native done in {}ms",
        started.elapsed().as_millis()
    ));
    Ok(())
}

fn stream_codex_cli(
    config: &StoredConfig,
    request: &SendRequest,
    id: u64,
    emit: &mut impl FnMut(&str, Option<&str>),
) -> Result<(), String> {
    let prompt = conversation_prompt(request);
    let work_dir = isolated_work_dir()?;
    if !cli_available("codex") {
        return Err("Codex CLI was not found. Install it and connect your account first.".into());
    }
    let args = vec![
        "exec".into(), "--json".into(), "--sandbox".into(), "read-only".into(),
        "--skip-git-repo-check".into(), "--cd".into(),
        work_dir.to_string_lossy().into_owned(), "--model".into(), config.model.clone(), "-".into(),
    ];
    let mut child = cli_command("codex", &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&work_dir)
        .spawn()
        .map_err(|_| "Codex CLI was not found. Install it and connect your account first.".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(prompt.as_bytes()).is_err() {
            kill_provider_tree(&mut child);
            return Err("Could not send the message to ChatGPT.".into());
        }
    }
    let stdout = child.stdout.take().ok_or("ChatGPT did not open its response stream.")?;
    let stderr = child.stderr.take().ok_or("ChatGPT did not open its error stream.")?;
    let lines = spawn_line_reader(stdout);
    let errors = spawn_line_reader(stderr);
    let mut full_text = String::new();
    let mut provider_error = String::new();
    let mut stderr_detail = String::new();
    loop {
        if ACTIVE_REQUEST.load(Ordering::SeqCst) != id {
            kill_provider_tree(&mut child);
            return Ok(());
        }
        while let Ok(line) = errors.try_recv() {
            append_error_detail(&mut stderr_detail, &line);
        }
        let line = match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(message) = event.get("message").and_then(Value::as_str)
            .or_else(|| event.pointer("/error/message").and_then(Value::as_str))
        {
            if matches!(event.get("type").and_then(Value::as_str), Some("error" | "turn.failed")) {
                append_error_detail(&mut provider_error, message);
            }
        }
        if let Some(item) = event.get("item") {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "agent_message" {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if text.starts_with(&full_text) {
                        let delta = &text[full_text.len()..];
                        if !delta.is_empty() { emit(delta, None); }
                        full_text = text.to_string();
                    } else if text != full_text {
                        emit("\n", None);
                        emit(text, None);
                        full_text = text.to_string();
                    }
                }
            } else if matches!(kind, "command_execution" | "mcp_tool_call") {
                emit("", Some("Working with a tool…"));
            }
        }
    }
    let status = child.wait().map_err(|_| "ChatGPT response process stopped unexpectedly.".to_string())?;
    while let Ok(line) = errors.try_recv() {
        append_error_detail(&mut stderr_detail, &line);
    }
    if status.success() && provider_error.is_empty() {
        Ok(())
    } else if !provider_error.is_empty() {
        Err(format!("ChatGPT error: {}", provider_error.chars().take(280).collect::<String>()))
    } else if !stderr_detail.is_empty() {
        Err(format!("ChatGPT error: {}", stderr_detail.chars().take(280).collect::<String>()))
    } else {
        Err("ChatGPT could not complete this response. Check its connection and selected model.".into())
    }
}

fn append_error_detail(target: &mut String, detail: &str) {
    let detail = detail.trim();
    if detail.is_empty() || target.len() >= 512 { return; }
    if !target.is_empty() { target.push_str("; "); }
    target.push_str(&detail.chars().take(256).collect::<String>());
    while target.len() > 512 { target.pop(); }
}

fn spawn_line_reader(reader: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("nex-chat-provider-stream".into())
        .spawn(move || {
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() { break; }
                    }
                    _ => break,
                }
            }
        });
    rx
}

const SYSTEM_PROMPT: &str = "You are Nex, a concise assistant inside the Nex launcher. Answer directly in plain text, no tools, no file access. Default to short answers under 120 words; expand only when asked. Match the user's language.";

fn conversation_prompt(request: &SendRequest) -> String {
    let mut prompt = String::from(SYSTEM_PROMPT);
    prompt.push_str("\n\n");
    for turn in request.history.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev() {
        let role = if turn.role == "assistant" { "Assistant" } else { "User" };
        prompt.push_str(role);
        prompt.push_str(":\n");
        prompt.push_str(&turn.content.chars().take(6000).collect::<String>());
        prompt.push_str("\n\n");
    }
    prompt.push_str("User:\n");
    prompt.push_str(&request.message);
    prompt
}

fn isolated_work_dir() -> Result<std::path::PathBuf, String> {
    let path = crate::config::stable_app_data_dir().join("chat-workspace");
    std::fs::create_dir_all(&path).map_err(|_| "Could not prepare a private chat workspace.".to_string())?;
    Ok(path)
}

fn cli_available(program: &str) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new("where.exe")
            .arg(program)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new("which")
            .arg(program)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn kill_provider_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let pid = child.id().to_string();
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn cli_command(program: &str, args: &[String]) -> Command {
    let script = "$ErrorActionPreference='Stop'; $cli=Get-Command $env:NEX_CHAT_CLI -ErrorAction Stop; $nexArgs=ConvertFrom-Json -InputObject $env:NEX_CHAT_ARGS; & $cli.Source @nexArgs; exit $LASTEXITCODE";
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoLogo", "-NoProfile", "-Command", script])
        .env("NEX_CHAT_CLI", program)
        .env("NEX_CHAT_ARGS", serde_json::to_string(args).unwrap_or_else(|_| "[]".into()));
    // Hidden by default: a visible console steals foreground and the
    // overlay hides itself. Callers needing a visible window (interactive
    // `codex login`) override flags after this returns.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

pub(crate) fn http_error(code: u16, body: String, secret: &str) -> String {
    match code {
        401 | 403 => "Provider authentication failed. Check the saved API key or account connection.".into(),
        429 => "The provider is rate limiting requests. Wait a moment and try again.".into(),
        _ => {
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value.pointer("/error/message").and_then(Value::as_str).map(str::to_string));
            detail
                .filter(|text| text.len() < 400)
                .map(|text| if secret.is_empty() { text } else { text.replace(secret, "[redacted]") })
                .unwrap_or_else(|| format!("The provider returned HTTP {code}."))
        }
    }
}

fn config_path() -> std::path::PathBuf {
    crate::config::stable_app_data_dir().join("chat-provider.dpapi")
}

fn load_config() -> Result<StoredConfig, String> {
    let bytes = std::fs::read(config_path()).map_err(|_| "Configure a provider to start chatting.".to_string())?;
    let plain = dpapi_decrypt(&bytes).ok_or_else(|| "Saved chat credentials could not be unlocked. Reconnect your provider.".to_string())?;
    serde_json::from_slice(&plain).map_err(|_| "Saved chat settings are invalid. Reconfigure the provider.".into())
}

fn store_config(config: &StoredConfig) -> Result<(), String> {
    let bytes = serde_json::to_vec(config).map_err(|_| "Could not encode chat settings.".to_string())?;
    let encrypted = dpapi_encrypt(&bytes).ok_or_else(|| "Windows could not securely save the provider credentials.".to_string())?;
    let path = config_path();
    let parent = path.parent().ok_or("Could not locate Nex app data.")?;
    std::fs::create_dir_all(parent).map_err(|_| "Could not create Nex app data.".to_string())?;
    let temp = path.with_extension("dpapi.tmp");
    std::fs::write(&temp, encrypted).map_err(|_| "Could not write provider settings.".to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
            let _ = std::fs::remove_file(temp);
            return Err("Could not securely replace provider settings.".into());
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(temp, path).map_err(|_| "Could not replace provider settings.".to_string())?;
    Ok(())
}

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

fn dpapi_encrypt(input: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::CryptProtectData;
    let blob = DataBlob { cb_data: input.len() as u32, pb_data: input.as_ptr() as *mut u8 };
    let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
    let ok = unsafe {
        CryptProtectData(&blob as *const _ as *const _, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0x1, &mut output as *mut _ as *mut _)
    };
    if ok == 0 { return None; }
    let result = unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pb_data as _) };
    let mut protected = CONFIG_MAGIC.to_vec();
    protected.extend(result);
    Some(protected)
}

fn dpapi_decrypt(input: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::CryptUnprotectData;
    let payload = input.strip_prefix(CONFIG_MAGIC)?;
    let blob = DataBlob { cb_data: payload.len() as u32, pb_data: payload.as_ptr() as *mut u8 };
    let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
    let ok = unsafe {
        CryptUnprotectData(&blob as *const _ as *const _, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0x1, &mut output as *mut _ as *mut _)
    };
    if ok == 0 { return None; }
    let result = unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pb_data as _) };
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_errors_name_reset_time() {
        let message = codex_http_error(
            429,
            r#"{"error":{"message":"rate_limit_exceeded","resets_at":1999999999}}"#.into(),
        );
        assert!(message.contains("usage limit"), "{message}");
        assert!(message.contains("resets"), "{message}");
    }

    #[test]
    fn long_chats_suggest_a_fresh_conversation() {
        let message = codex_http_error(
            400,
            r#"{"error":{"message":"This conversation has reached the maximum context length"}}"#.into(),
        );
        assert!(message.contains("new conversation"), "{message}");
    }

    #[test]
    fn expired_sessions_ask_to_reconnect() {
        assert!(codex_http_error(401, String::new()).contains("Reconnect"));
        assert!(codex_http_error(500, String::new()).contains("HTTP 500"));
    }
}
