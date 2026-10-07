//! Native ChatGPT account auth speaking the official ChatGPT OAuth protocol
//! (spec: vendored `third_party/codex`, openai/codex `rust-v0.160.1`).
//! No CLI install required. Tokens live in the standard ChatGPT home
//! (`~/.codex/auth.json`), so the CLI and Nex share one sign-in.

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const ORIGINATOR: &str = "codex_cli_rs";
const CALLBACK_PORTS: [u16; 2] = [1455, 1457];

/// ChatGPT-plan inference base (Responses wire API).
pub(crate) const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Pending browser sign-in: callback listener is already bound so a
/// bind failure surfaces immediately (caller falls back to the CLI).
pub(crate) struct PendingLogin {
    listener: TcpListener,
    port: u16,
    verifier: String,
    state: String,
    deadline: Instant,
}

#[derive(Debug, Deserialize)]
struct AuthFile {
    #[serde(default)]
    tokens: Option<StoredTokens>,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
}

fn codex_home() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".codex")
}

fn auth_file() -> PathBuf {
    codex_home().join("auth.json")
}

/// Nex-owned token store. Primary source of truth; the CLI home above
/// is only mirrored for interop (login once, both apps work).
fn nex_auth_file() -> PathBuf {
    crate::config::stable_app_data_dir().join("codex-auth.json")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(12))
        .timeout_read(Duration::from_secs(15))
        .build()
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_b64url(nbytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; nbytes];
    getrandom::fill(&mut buf).map_err(|_| "Could not generate sign-in secret.".to_string())?;
    Ok(b64url(&buf))
}

fn jwt_payload(jwt: &str) -> Option<serde_json::Value> {
    let mut parts = jwt.split('.');
    let payload = match (parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(p), Some(_)) => p,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn claim<'a>(payload: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut value = payload;
    for key in path {
        value = value.get(*key)?;
    }
    value.as_str()
}

fn access_expired(access_token: &str) -> bool {
    let expired = jwt_payload(access_token)
        .and_then(|p| p.get("exp")?.as_i64())
        .map(|exp| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            exp <= now + 60
        })
        .unwrap_or(true);
    expired
}

fn read_tokens() -> Option<StoredTokens> {
    if let Some(tokens) = read_tokens_file(&nex_auth_file()) {
        return Some(tokens);
    }
    // One-time import: a CLI sign-in is adopted into our store.
    let tokens = read_tokens_file(&auth_file())?;
    persist(&tokens);
    Some(tokens)
}

fn read_tokens_file(path: &PathBuf) -> Option<StoredTokens> {
    let bytes = std::fs::read(path).ok()?;
    let file: AuthFile = serde_json::from_slice(&bytes).ok()?;
    let tokens = file.tokens?;
    if tokens.access_token.is_empty() {
        return None;
    }
    Some(tokens)
}

/// Fresh ChatGPT-plan credentials: stored tokens, refreshing first when
/// the access token is expired. Returns `(access_token, account_id)`.
pub(crate) fn fresh_tokens() -> Option<(String, Option<String>)> {
    let mut tokens = read_tokens()?;
    if access_expired(&tokens.access_token) {
        if tokens.refresh_token.is_empty() {
            return None;
        }
        let refreshed = refresh(&tokens.refresh_token)?;
        tokens.access_token = refreshed.access_token.filter(|s| !s.is_empty())?;
        if let Some(rt) = refreshed.refresh_token.filter(|s| !s.is_empty()) {
            tokens.refresh_token = rt;
        }
        if let Some(id) = refreshed.id_token.filter(|s| !s.is_empty()) {
            tokens.account_id = jwt_payload(&id)
                .as_ref()
                .and_then(|p| claim(p, &["https://api.openai.com/auth", "chatgpt_account_id"]))
                .map(str::to_string);
            tokens.id_token = id;
        }
        persist(&tokens);
    }
    if tokens.account_id.is_none() {
        tokens.account_id = jwt_payload(&tokens.id_token)
            .as_ref()
            .and_then(|p| claim(p, &["https://api.openai.com/auth", "chatgpt_account_id"]))
            .map(str::to_string);
    }
    Some((tokens.access_token, tokens.account_id))
}

pub(crate) fn is_signed_in() -> bool {
    read_tokens().is_some()
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

fn refresh(refresh_token: &str) -> Option<RefreshResponse> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": refresh_token,
    });
    agent()
        .post(&format!("{ISSUER}/oauth/token"))
        .set("Content-Type", "application/json")
        .send_json(body)
        .ok()?
        .into_json::<RefreshResponse>()
        .ok()
}

fn persist(tokens: &StoredTokens) {
    let file = serde_json::json!({
        "auth_mode": "chatgpt",
        "tokens": tokens,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&file) {
        let path = nex_auth_file();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, &bytes);
        // Mirror for CLI interop; failures never block our store.
        let mirror = auth_file();
        if let Some(parent) = mirror.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&mirror, &bytes);
    }
}

/// Deletes both our store and the CLI mirror (revoked or signed out).
pub(crate) fn forget() {
    let _ = std::fs::remove_file(nex_auth_file());
    let _ = std::fs::remove_file(auth_file());
}

/// Native sign-out: revoke refresh (else access), then delete `auth.json`.
pub(crate) fn logout() -> Result<bool, String> {
    let tokens = read_tokens();
    if let Some(tokens) = &tokens {
        let (token, kind, with_client) = if !tokens.refresh_token.is_empty() {
            (tokens.refresh_token.as_str(), "refresh_token", true)
        } else {
            (tokens.access_token.as_str(), "access_token", false)
        };
        let mut body = serde_json::json!({
            "token": token,
            "token_type_hint": kind,
        });
        if with_client {
            body["client_id"] = serde_json::Value::String(CLIENT_ID.to_string());
        }
        // Best effort: local auth is removed even if revoke fails.
        let _ = agent()
            .post(&format!("{ISSUER}/oauth/revoke"))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(10))
            .send_json(body);
    }
    match std::fs::remove_file(nex_auth_file()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(tokens.is_some()),
        Err(e) => Err(format!("Could not clear ChatGPT sign-in: {e}")),
    }?;
    let _ = std::fs::remove_file(auth_file());
    Ok(true)
}

/// Starts the ChatGPT OAuth flow: binds the localhost callback, builds
/// the authorize URL, and opens it in the browser.
pub(crate) fn start_login_server() -> Result<PendingLogin, String> {
    let (listener, port) = CALLBACK_PORTS
        .iter()
        .find_map(|port| {
            TcpListener::bind(format!("127.0.0.1:{port}"))
                .ok()
                .map(|listener| (listener, *port))
        })
        .ok_or_else(|| "Could not start ChatGPT sign-in (local callback unavailable).".to_string())?;
    let verifier = random_b64url(64)?;
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = random_b64url(16)?;
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    let url = authorize_url(&redirect_uri, &challenge, &state);
    open_browser(url.as_str());
    Ok(PendingLogin {
        listener,
        port,
        verifier,
        state,
        deadline: Instant::now() + Duration::from_secs(300),
    })
}

fn authorize_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    let mut url = url::Url::parse(&format!("{ISSUER}/oauth/authorize"))
        .expect("codex authorize endpoint");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", ORIGINATOR);
    url.to_string()
}

fn open_browser(url: &str) {
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn read_callback_request(listener: &TcpListener) -> Option<(String, String)> {
    let (stream, _) = listener.accept().ok()?;
    // Accepted sockets inherit the listener's non-blocking mode on
    // Windows — back to blocking so the request line actually reads.
    let _ = stream.set_nonblocking(false);
    let _ = stream
        .set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let path = request_line.split_whitespace().nth(1)?.to_string();
    // Drain headers so the browser isn't left hanging.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
    }
    let query = path.split_once('?')?.1;
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        match k {
            "code" => code = Some(v.to_string()),
            "state" => state = Some(v.to_string()),
            _ => {}
        }
    }
    let body = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<html><body style=\"background:#1e1e1e;color:#f4f4f6;font-family:sans-serif;display:flex;height:90vh;align-items:center;justify-content:center\"><h2>Signed in. You can close this window and return to Nex.</h2></body></html>";
    let mut stream = reader.into_inner();
    let _ = stream.write_all(body);
    let _ = stream.flush();
    Some((code?, state?))
}

/// Waits for the browser callback, exchanges the code, and persists auth.
pub(crate) fn wait_for_login(pending: PendingLogin, timeout: Duration) -> Result<(), String> {
    let deadline = (Instant::now() + timeout).min(pending.deadline);
    let _ = pending.listener.set_nonblocking(true);
    loop {
        if Instant::now() >= deadline {
            return Err("ChatGPT sign-in was not confirmed. Try again in a moment.".to_string());
        }
        match read_callback_request(&pending.listener) {
            Some((code, state)) => {
                if state != pending.state {
                    continue;
                }
                return complete_login(&code, &pending.verifier, pending.port);
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn complete_login(code: &str, verifier: &str, port: u16) -> Result<(), String> {
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    let response = agent()
        .post(&format!("{ISSUER}/oauth/token"))
        .send_form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", verifier),
        ])
        .map_err(|_| "ChatGPT sign-in exchange failed. Try again.".to_string())?;
    if response.status() != 200 {
        return Err("ChatGPT sign-in was not confirmed. Try again in a moment.".to_string());
    }
    let tokens: RefreshResponse = response
        .into_json()
        .map_err(|_| "ChatGPT sign-in returned an unreadable response.".to_string())?;
    let (id_token, access_token, refresh_token) = match (tokens.id_token, tokens.access_token, tokens.refresh_token) {
        (Some(id), Some(access), refresh) => (id, access, refresh.unwrap_or_default()),
        _ => return Err("ChatGPT sign-in returned incomplete credentials.".to_string()),
    };
    let account_id = jwt_payload(&id_token)
        .as_ref()
        .and_then(|p| claim(p, &["https://api.openai.com/auth", "chatgpt_account_id"]))
        .map(str::to_string);
    persist(&StoredTokens {
        id_token,
        access_token,
        refresh_token,
        account_id,
    });
    Ok(())
}

/// Headers for ChatGPT backend requests: bearer + account identity.
pub(crate) fn backend_headers(access_token: &str, account_id: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {access_token}"),
        ),
        ("originator".to_string(), ORIGINATOR.to_string()),
    ];
    if let Some(id) = account_id.filter(|id| !id.trim().is_empty()) {
        headers.push(("chatgpt-account-id".to_string(), id.to_string()));
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_jwt(payload: &serde_json::Value) -> String {
        let header = b64url(br#"{"alg":"none"}"#);
        let body = b64url(&serde_json::to_vec(payload).unwrap());
        format!("{header}.{body}.sig")
    }

    #[test]
    fn parses_account_claims_from_id_token() {
        let jwt = test_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-123",
                "chatgpt_user_id": "user-456",
            },
            "exp": 9_999_999_999i64,
        }));
        let payload = jwt_payload(&jwt).expect("payload parses");
        assert_eq!(
            claim(&payload, &["https://api.openai.com/auth", "chatgpt_account_id"]),
            Some("acc-123")
        );
        assert!(!access_expired(&jwt));
    }

    #[test]
    fn rejects_malformed_or_expired_tokens() {
        assert!(jwt_payload("not-a-jwt").is_none());
        let expired = test_jwt(&serde_json::json!({"exp": 1i64}));
        assert!(access_expired(&expired));
        assert!(access_expired(""));
    }

    #[test]
    fn authorize_url_carries_pkce_and_client() {
        let url = authorize_url("http://127.0.0.1:1455/auth/callback", "CHALLENGE", "STATE");
        for part in [
            "response_type=code",
            "client_id=app_EMoamEEZ73f0CkXaXp7hrann",
            "code_challenge=CHALLENGE",
            "code_challenge_method=S256",
            "state=STATE",
            "scope=openid",
        ] {
            assert!(url.contains(part), "missing {part}");
        }
    }

    #[test]
    fn backend_headers_carry_bearer_and_account() {
        let headers = backend_headers("tok", Some("acc-1"));
        assert!(headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer tok"));
        assert!(headers.iter().any(|(k, v)| k == "chatgpt-account-id" && v == "acc-1"));
        let bare = backend_headers("tok", None);
        assert_eq!(bare.len(), 2);
    }
}
