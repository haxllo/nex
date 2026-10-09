//! Native ChatGPT OAuth for Nex. Credentials are stored only in Nex-owned
//! DPAPI-protected storage and are never imported from or mirrored to the CLI.

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const ORIGINATOR: &str = "codex_cli_rs";
const CALLBACK_PORTS: [u16; 2] = [1455, 1457];
const AUTH_FILE_MARKER: &[u8] = b"NEXCHATGPT1";
static AUTH_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// ChatGPT-plan inference base (Responses wire API).
pub(crate) const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Pending browser sign-in: callback listener is already bound so a
/// bind failure surfaces immediately.
pub(crate) struct PendingLogin {
    listener: TcpListener,
    port: u16,
    verifier: String,
    state: String,
    deadline: Instant,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
}

/// Encrypted Nex-only token store.
fn nex_auth_file() -> PathBuf {
    nex_auth_file_at(&crate::config::stable_app_data_dir())
}

/// Prior Nex copy of shared credentials; it is discarded rather than reused.
fn legacy_nex_auth_file() -> PathBuf {
    legacy_nex_auth_file_at(&crate::config::stable_app_data_dir())
}

fn nex_auth_file_at(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("chatgpt-auth.dpapi")
}

fn legacy_nex_auth_file_at(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("codex-auth.json")
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

fn read_tokens() -> Result<Option<StoredTokens>, String> {
    read_tokens_at(&crate::config::stable_app_data_dir())
}

fn read_tokens_at(app_data_dir: &Path) -> Result<Option<StoredTokens>, String> {
    let tokens = std::fs::read(nex_auth_file_at(app_data_dir))
        .ok()
        .and_then(|protected| crate::secure_storage::unprotect(&protected, AUTH_FILE_MARKER))
        .and_then(|plaintext| serde_json::from_slice::<StoredTokens>(&plaintext).ok())
        .filter(|tokens| !tokens.access_token.is_empty());
    // Older versions mirrored the same refresh token to the Codex CLI. Reusing
    // it could rotate or invalidate that separate sign-in, so require Nex login.
    discard_legacy_auth_at(&legacy_nex_auth_file_at(app_data_dir))?;
    Ok(tokens)
}

/// Fresh ChatGPT-plan credentials: stored tokens, refreshing first when
/// the access token is expired. Returns `(access_token, account_id)`.
pub(crate) fn fresh_tokens() -> Result<Option<(String, Option<String>)>, String> {
    let Some(mut tokens) = read_tokens()? else {
        return Ok(None);
    };
    if access_expired(&tokens.access_token) {
        if tokens.refresh_token.is_empty() {
            return Ok(None);
        }
        let Some(refreshed) = refresh(&tokens.refresh_token) else {
            return Ok(None);
        };
        let Some(access_token) = refreshed.access_token.filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        tokens.access_token = access_token;
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
        persist(&tokens)?;
    }
    if tokens.account_id.is_none() {
        tokens.account_id = jwt_payload(&tokens.id_token)
            .as_ref()
            .and_then(|p| claim(p, &["https://api.openai.com/auth", "chatgpt_account_id"]))
            .map(str::to_string);
    }
    Ok(Some((tokens.access_token, tokens.account_id)))
}

pub(crate) fn is_signed_in() -> bool {
    matches!(read_tokens(), Ok(Some(_)))
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

fn persist(tokens: &StoredTokens) -> Result<(), String> {
    let plaintext = serde_json::to_vec(tokens)
        .map_err(|_| "Could not securely store the Nex ChatGPT sign-in.".to_string())?;
    let protected = crate::secure_storage::protect(&plaintext, AUTH_FILE_MARKER)
        .ok_or_else(|| "Could not securely store the Nex ChatGPT sign-in.".to_string())?;
    let app_data_dir = crate::config::stable_app_data_dir();
    persist_protected_at(
        &nex_auth_file_at(&app_data_dir),
        &legacy_nex_auth_file_at(&app_data_dir),
        &protected,
    )
}

fn persist_protected_at(path: &Path, legacy_path: &Path, protected: &[u8]) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|_| "Could not securely store the Nex ChatGPT sign-in.".to_string())?;
    }
    discard_legacy_auth_at(legacy_path)?;
    atomic_write(path, protected)
        .map_err(|_| "Could not securely store the Nex ChatGPT sign-in.".to_string())?;
    Ok(())
}

fn discard_legacy_auth_at(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove the previous Nex ChatGPT sign-in.".to_string()),
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    atomic_write_with_replace(path, contents, |from, to| std::fs::rename(from, to))
}

fn atomic_write_with_replace(
    path: &Path,
    contents: &[u8],
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let (temporary_path, mut file) = loop {
        let suffix = AUTH_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = path.as_os_str().to_os_string();
        temporary_name.push(format!(".{}.{}.tmp", std::process::id(), suffix));
        let temporary_path = PathBuf::from(temporary_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => break (temporary_path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };

    let write_result = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    if let Err(error) = replace(&temporary_path, path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    Ok(())
}

fn clear_nex_auth() -> Result<(), String> {
    clear_nex_auth_at(&crate::config::stable_app_data_dir())
}

fn clear_nex_auth_at(app_data_dir: &Path) -> Result<(), String> {
    for path in [
        nex_auth_file_at(app_data_dir),
        legacy_nex_auth_file_at(app_data_dir),
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Could not clear the Nex ChatGPT sign-in.".to_string()),
        }
    }
    Ok(())
}

/// Clears only Nex-owned credentials; separate Codex CLI sign-in is untouched.
pub(crate) fn forget() -> Result<(), String> {
    clear_nex_auth()
}

/// Signs Nex out locally without revoking tokens that may also belong to the CLI.
pub(crate) fn logout() -> Result<bool, String> {
    let was_signed_in = matches!(read_tokens(), Ok(Some(_)));
    clear_nex_auth()?;
    Ok(was_signed_in)
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
        .ok_or_else(|| {
            "Could not start ChatGPT sign-in (local callback unavailable).".to_string()
        })?;
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
    let mut url =
        url::Url::parse(&format!("{ISSUER}/oauth/authorize")).expect("codex authorize endpoint");
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
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
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
    let body = concat!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n",
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">",
        "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">",
        "<title>Signed in — Nex</title>",
        "<style>",
        "*{box-sizing:border-box;margin:0;padding:0}",
        "body{min-height:100vh;display:flex;align-items:center;justify-content:center;",
        "font-family:Inter,'Segoe UI',system-ui,sans-serif;color:#f4f4f6;",
        "background:radial-gradient(120% 90% at 50% 0%,#2b3342 0%,#171a21 55%,#101216 100%)}",
        ".card{width:min(400px,calc(100vw - 48px));padding:36px 32px 30px;border-radius:16px;text-align:center;",
        "background:rgba(30,30,30,.55);border:1px solid rgba(255,255,255,.12);",
        "box-shadow:0 24px 60px rgba(0,0,0,.45),inset 0 1px 0 rgba(255,255,255,.12);",
        "backdrop-filter:blur(18px) saturate(1.15)}",
        ".mark{display:flex;justify-content:center;align-items:flex-end;gap:5px;height:34px;margin-bottom:16px}",
        ".mark span{width:6px;border-radius:4px;background:#6ea8fe}",
        ".mark span:nth-child(1){height:14px;opacity:.45}",
        ".mark span:nth-child(2){height:28px}",
        ".mark span:nth-child(3){height:19px;opacity:.6}",
        ".check{width:44px;height:44px;margin:0 auto 14px;border-radius:50%;display:flex;align-items:center;justify-content:center;",
        "background:rgba(74,222,128,.14);border:1px solid rgba(74,222,128,.4)}",
        ".check svg{width:22px;height:22px;fill:none;stroke:#4ade80;stroke-width:2.4;stroke-linecap:round;stroke-linejoin:round}",
        "h1{font-size:19px;font-weight:600;letter-spacing:-.02em;margin-bottom:8px}",
        "p{font-size:13px;line-height:1.55;color:#aeb6c2;margin-bottom:22px}",
        "button{min-height:38px;padding:0 22px;border:0;border-radius:9px;background:#6ea8fe;color:#fff;",
        "font:600 13px Inter,'Segoe UI',system-ui,sans-serif;cursor:pointer}",
        "button:hover{filter:brightness(1.08)}",
        "</style></head><body><div class=\"card\">",
        "<div class=\"mark\" aria-hidden=\"true\"><span></span><span></span><span></span></div>",
        "<div class=\"check\"><svg viewBox=\"0 0 24 24\"><path d=\"M20 6 9 17l-5-5\"/></svg></div>",
        "<h1>ChatGPT connected to Nex</h1>",
        "<p>This Nex sign-in is separate from any Codex CLI sign-in. Close this window and return to Nex.</p>",
        "<button type=\"button\" onclick=\"window.close()\">Close this window</button>",
        "</div></body></html>",
    );
    let mut stream = reader.into_inner();
    let _ = stream.write_all(body.as_bytes());
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
    let (id_token, access_token, refresh_token) =
        match (tokens.id_token, tokens.access_token, tokens.refresh_token) {
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
    })?;
    Ok(())
}

/// Headers for ChatGPT backend requests: bearer + account identity.
pub(crate) fn backend_headers(
    access_token: &str,
    account_id: Option<&str>,
) -> Vec<(String, String)> {
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
            claim(
                &payload,
                &["https://api.openai.com/auth", "chatgpt_account_id"]
            ),
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
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer tok")
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "chatgpt-account-id" && v == "acc-1")
        );
        let bare = backend_headers("tok", None);
        assert_eq!(bare.len(), 2);
    }

    #[test]
    fn upgrade_discards_legacy_nex_auth_without_importing_cli_auth() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path().join("Nex");
        let cli_auth = temp.path().join(".codex").join("auth.json");
        std::fs::create_dir_all(&app_data).unwrap();
        std::fs::create_dir_all(cli_auth.parent().unwrap()).unwrap();
        std::fs::write(
            legacy_nex_auth_file_at(&app_data),
            br#"{"refresh_token":"legacy-token"}"#,
        )
        .unwrap();
        std::fs::write(&cli_auth, b"separate CLI credentials").unwrap();

        assert!(read_tokens_at(&app_data).unwrap().is_none());
        assert!(!legacy_nex_auth_file_at(&app_data).exists());
        assert!(!nex_auth_file_at(&app_data).exists());
        assert_eq!(
            std::fs::read(cli_auth).unwrap(),
            b"separate CLI credentials"
        );
    }

    #[test]
    fn signing_out_removes_only_nex_owned_auth_files() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path().join("Nex");
        let cli_auth = temp.path().join(".codex").join("auth.json");
        std::fs::create_dir_all(&app_data).unwrap();
        std::fs::create_dir_all(cli_auth.parent().unwrap()).unwrap();
        let current_auth = nex_auth_file_at(&app_data);
        let legacy_auth = legacy_nex_auth_file_at(&app_data);
        std::fs::write(&current_auth, b"DPAPI-protected credentials").unwrap();
        std::fs::write(&legacy_auth, b"legacy Nex credentials").unwrap();
        std::fs::write(&cli_auth, b"separate CLI credentials").unwrap();

        clear_nex_auth_at(&app_data).unwrap();

        assert!(!current_auth.exists());
        assert!(!legacy_auth.exists());
        assert_eq!(
            std::fs::read(cli_auth).unwrap(),
            b"separate CLI credentials"
        );
    }

    #[test]
    fn failed_token_replacement_preserves_existing_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let auth_file = temp.path().join("chatgpt-auth.dpapi");
        std::fs::write(&auth_file, b"previous protected credentials").unwrap();

        let result = atomic_write_with_replace(&auth_file, b"rotated credentials", |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "forced replacement failure",
            ))
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&auth_file).unwrap(),
            b"previous protected credentials"
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);

        atomic_write(&auth_file, b"rotated credentials").unwrap();
        assert_eq!(std::fs::read(&auth_file).unwrap(), b"rotated credentials");
    }

    #[test]
    fn legacy_cleanup_failure_is_reported_before_credentials_are_used_or_saved() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path().join("Nex");
        let legacy_auth = legacy_nex_auth_file_at(&app_data);
        std::fs::create_dir_all(&legacy_auth).unwrap();
        let current_auth = nex_auth_file_at(&app_data);

        assert!(read_tokens_at(&app_data).is_err());
        assert!(persist_protected_at(&current_auth, &legacy_auth, b"protected").is_err());
        assert!(clear_nex_auth_at(&app_data).is_err());
        assert!(!current_auth.exists());
        assert!(legacy_auth.is_dir());
    }
}
