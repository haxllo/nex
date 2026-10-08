use std::path::{Path, PathBuf};

/// Max characters returned by a single tool result (plan §3: truncation budget).
pub(crate) const OUTPUT_CAP_CHARS: usize = 4000;
/// Max directory entries returned by `fs_list`.
pub(crate) const LIST_CAP: usize = 100;
/// Max hits returned by `fs_search`.
pub(crate) const SEARCH_CAP: usize = 20;

/// Tool declaration, shaped for later Responses `tools` use.
#[derive(Debug, Clone)]
pub(crate) struct ToolSpec {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) parameters: serde_json::Value,
}

/// True for tools that mutate the world or run code; the loop must hold a
/// user approval token before dispatching them.
pub(crate) fn needs_approval(name: &str) -> bool {
    matches!(name, "shell_exec" | "app_open" | "url_open")
}

/// Issue a per-call approval nonce (32 hex chars) the loop hands to the UI
/// and back into `dispatch` via `approval_token`.
pub(crate) fn issue_approval_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Fallback: mix wall clock + pid when the OS RNG is unavailable.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = nanos ^ ((pid << 64) | (nanos >> 64) ^ 0x9e3779b97f4a7c15);
        bytes = mixed.to_le_bytes();
    }
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn path_param(desc: &str, required: bool) -> serde_json::Value {
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": desc }
        },
        "additionalProperties": false
    });
    if required {
        schema["required"] = serde_json::json!(["path"]);
    }
    schema
}

/// Full tool registry: 3 read-only + 3 approval-gated.
pub(crate) fn registry() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "fs_read",
            description: "Read a text file under the current directory or home. Output capped at 4000 chars.",
            parameters: path_param("File path to read (relative or absolute).", true),
        },
        ToolSpec {
            name: "fs_list",
            description: "List directory entry names (no metadata, no recursion). Capped at 100 entries.",
            parameters: path_param("Directory path to list (relative or absolute).", true),
        },
        ToolSpec {
            name: "fs_search",
            description: "Find files by filename substring under the current directory. Capped at 20 hits.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Filename substring to match (case-insensitive)." }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "shell_exec",
            description: "Run a shell command in the user's home directory (20s timeout, no window). Use cmd.exe syntax. Requires user approval.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run." }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "app_open",
            description: "Open an app, file, or shell target via the OS shell. Requires user approval.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "App name, file path, or shell target to open." }
                },
                "required": ["target"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "url_open",
            description: "Open an http(s) URL in the default browser. Requires user approval.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "http(s) URL to open." }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        },
    ]
}

/// `approval` is the token the loop was issued for this call; read tools
/// ignore it. Write tools compare it against `approval_token` in `args_json`.
pub(crate) fn dispatch(
    name: &str,
    args_json: &serde_json::Value,
    approval: Option<&str>,
) -> Result<String, String> {
    if needs_approval(name) {
        let provided = args_json
            .get("approval_token")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let ok = match approval {
            Some(expected) => !expected.is_empty() && provided == expected,
            None => false,
        };
        if !ok {
            return Err(format!(
                "approval-required: '{name}' needs user approval before it can run"
            ));
        }
    }
    match name {
        "fs_read" => {
            let path = args_json
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "fs_read: missing required string arg 'path'".to_string())?;
            fs_read(Path::new(path))
        }
        "fs_list" => {
            let path = args_json
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "fs_list: missing required string arg 'path'".to_string())?;
            fs_list(Path::new(path))
        }
        "fs_search" => {
            let query = args_json
                .get("query")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "fs_search: missing required string arg 'query'".to_string())?;
            fs_search(query)
        }
        "shell_exec" => {
            let command = args_json
                .get("command")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "shell_exec: missing required string arg 'command'".to_string())?;
            shell_exec(command)
        }
        "app_open" => {
            let target = args_json
                .get("target")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "app_open: missing required string arg 'target'".to_string())?;
            app_open(target)
        }
        "url_open" => {
            let url = args_json
                .get("url")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "url_open: missing required string arg 'url'".to_string())?;
            url_open(url)
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Allowed read roots: current dir + user's home.
fn allowed_roots() -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(2);
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(std::fs::canonicalize(&cwd).unwrap_or(cwd));
    }
    for key in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(key) {
            if home.is_empty() {
                continue;
            }
            let p = PathBuf::from(home);
            let canon = std::fs::canonicalize(&p).unwrap_or(p);
            if !roots.contains(&canon) {
                roots.push(canon);
            }
        }
    }
    roots
}

/// Resolve `path` (relative to cwd) and refuse anything escaping the allowed roots.
fn resolve_within_roots(path: &Path) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot determine cwd: {e}"))?;
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let canon = std::fs::canonicalize(&joined)
        .map_err(|e| format!("cannot resolve '{}': {e}", joined.display()))?;
    if allowed_roots().iter().any(|r| canon.starts_with(r)) {
        Ok(canon)
    } else {
        Err(format!(
            "path '{}' escapes allowed roots (current dir + home)",
            joined.display()
        ))
    }
}

fn truncate_chars(s: &str) -> String {
    if s.chars().count() <= OUTPUT_CAP_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(OUTPUT_CAP_CHARS).collect();
    out.push_str("\n...[truncated at 4000 chars]");
    out
}

fn fs_read(path: &Path) -> Result<String, String> {
    let canon = resolve_within_roots(path)?;
    if canon.is_dir() {
        return Err(format!("fs_read: '{}' is a directory", canon.display()));
    }
    let text =
        std::fs::read_to_string(&canon).map_err(|e| format!("fs_read: '{}': {e}", canon.display()))?;
    Ok(truncate_chars(&text))
}

fn fs_list(path: &Path) -> Result<String, String> {
    let canon = resolve_within_roots(path)?;
    if !canon.is_dir() {
        return Err(format!("fs_list: '{}' is not a directory", canon.display()));
    }
    let mut names = Vec::new();
    let entries =
        std::fs::read_dir(&canon).map_err(|e| format!("fs_list: '{}': {e}", canon.display()))?;
    for entry in entries.flatten() {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names.truncate(LIST_CAP);
    Ok(names.join("\n"))
}

// Real index first, stub fallback when the index is cold/unavailable.
// Same tantivy query path the launcher uses (`TantivyIndex::search`,
// the indexed half of `CoreService::search_with_filter_internal`);
// opened on demand at the live index dir, never via workers or UI state.
fn fs_search(query: &str) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("fs_search: missing required string arg 'query'".to_string());
    }
    if let Some(hits) = search_index(query) {
        return Ok(hits.join("\n"));
    }
    stub_fs_search(query)
}

/// Index lookup returning `None` on ANY failure so the caller can fall
/// back to the stub scan instead of hard-erroring on a cold index.
fn search_index(query: &str) -> Option<Vec<String>> {
    let dir = tantivy_dir()?;
    search_index_at(query, &dir)
}

fn tantivy_dir() -> Option<PathBuf> {
    let cfg = crate::config::load(None).unwrap_or_default();
    let parent = cfg.index_db_path.parent()?;
    Some(parent.join("index.tantivy"))
}

fn search_index_at(query: &str, dir: &Path) -> Option<Vec<String>> {
    let idx = crate::tantivy_search::TantivyIndex::open(dir).ok()?;
    let items = idx.search(query, SEARCH_CAP).ok()?;
    Some(
        items
            .into_iter()
            .map(|item| item.path.clone())
            .take(SEARCH_CAP)
            .collect(),
    )
}

// STUB fallback (index cold/unavailable): filename substring scan under
// the current directory. Recursive, skips symlinks, bounded traversal budget.
fn stub_fs_search(query: &str) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("fs_search: missing required string arg 'query'".to_string());
    }
    let needle = query.to_lowercase();
    let cwd = std::env::current_dir().map_err(|e| format!("cannot determine cwd: {e}"))?;
    let mut hits = Vec::new();
    let mut stack = vec![cwd.clone()];
    let mut visited: usize = 0;
    while let Some(dir) = stack.pop() {
        if hits.len() >= SEARCH_CAP || visited > 20_000 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if hits.len() >= SEARCH_CAP || visited > 20_000 {
                break;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.to_lowercase().contains(&needle) {
                let rel = entry
                    .path()
                    .strip_prefix(&cwd)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| entry.path().to_string_lossy().into_owned());
                hits.push(rel);
            }
        }
    }
    hits.sort();
    hits.truncate(SEARCH_CAP);
    Ok(hits.join("\n"))
}

/// Run `command` via the system shell: hidden window, 20s timeout,
/// stdout+stderr combined capped at [`OUTPUT_CAP_CHARS`].
fn home_dir() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Expand a leading `~/`, `~\`, or bare `~` (quoted or not) to the home
/// directory — cmd.exe does not expand tilde itself.
fn expand_home(command: &str) -> String {
    let home = home_dir().to_string_lossy().into_owned();
    let sep = std::path::MAIN_SEPARATOR;
    for prefix in ["~/", "~\\", "\"~/", "\"~\\", "'~/", "'~\\"] {
        if let Some(rest) = command.strip_prefix(prefix) {
            let quote = if prefix.starts_with(['"', '\'']) { &prefix[..1] } else { "" };
            return format!("{quote}{home}{sep}{rest}");
        }
    }
    if command == "~" || command == "\"~\"" || command == "'~'" {
        return home;
    }
    command.to_string()
}

fn shell_exec(command: &str) -> Result<String, String> {
    if command.trim().is_empty() {
        return Err("shell_exec: missing required string arg 'command'".to_string());
    }
    let command = expand_home(command);
    #[cfg(target_os = "windows")]
    fn make_cmd(command: &str) -> std::process::Command {
        use std::os::windows::process::CommandExt as _;
        let mut cmd = std::process::Command::new("cmd.exe");
        // /S keeps inner quotes intact; raw_arg passes the command
        // through verbatim because Rust's automatic quoting would escape
        // inner quotes in a way cmd.exe cannot parse.
        cmd.arg("/S").arg("/C").raw_arg(command);
        // CREATE_NO_WINDOW: no console flash over the overlay.
        cmd.creation_flags(0x08000000);
        // Predictable start point: the user's home directory.
        cmd.current_dir(home_dir());
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    fn make_cmd(command: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
    let mut child = make_cmd(&command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("shell_exec: spawn failed: {e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Drain pipes on a thread so a chatty child can't block on a full pipe
    // while the main thread enforces the timeout.
    let out_handle = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buf = Vec::new();
        if let Some(mut p) = stdout {
            let _ = p.read_to_end(&mut buf);
        }
        if let Some(mut p) = stderr {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let timeout = std::time::Duration::from_secs(20);
    let start = std::time::Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|e| format!("shell_exec: wait failed: {e}"))?
        {
            Some(status) => {
                let raw = out_handle.join().unwrap_or_default();
                let mut text = String::from_utf8_lossy(&raw).into_owned();
                if !status.success() {
                    text = format!("[exit {}]\n{text}", status.code().unwrap_or(-1));
                }
                return Ok(truncate_chars(&text));
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("shell_exec: timed out after 20s".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// Open an app/file/shell target. Reuses the launcher's own opener.
fn app_open(target: &str) -> Result<String, String> {
    if target.trim().is_empty() {
        return Err("app_open: missing required string arg 'target'".to_string());
    }
    crate::action_executor::launch_open_target(target)
        .map(|_| format!("opened {target}"))
        .map_err(|e| format!("app_open: '{target}': {e}"))
}

/// Open an http(s) URL in the default browser. Mirrors the ShellExecuteW
/// pattern of `open_url_in_browser` in `runtime_loop.rs`.
fn url_open(url: &str) -> Result<String, String> {
    let lower = url.trim().to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(format!("url_open: rejected non-http(s) URL: '{url}'"));
    }
    open_url_shell(url.trim());
    Ok(format!("opened {url}"))
}

#[cfg(target_os = "windows")]
fn open_url_shell(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn open_url_shell(_url: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_tool_errors() {
        let err = dispatch("nope", &json!({}), None).unwrap_err();
        assert!(err.contains("unknown tool"), "unexpected: {err}");
    }

    #[test]
    fn registry_lists_all_six_tools() {
        let names: Vec<_> = registry().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "fs_read",
                "fs_list",
                "fs_search",
                "shell_exec",
                "app_open",
                "url_open"
            ]
        );
        for spec in registry() {
            assert_eq!(spec.parameters["type"], "object");
        }
    }

    #[test]
    fn traversal_rejected() {
        // Relative escape: rejected (missing or outside roots — either way Err).
        assert!(dispatch("fs_read", &json!({ "path": "../../secret" }), None).is_err());
        // Absolute path outside all roots: must hit the traversal guard itself.
        let root = std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.ancestors().last().map(|r| r.to_path_buf()));
        if let Some(root) = root {
            let err = dispatch(
                "fs_read",
                &json!({ "path": root.to_string_lossy() }),
                None,
            )
            .unwrap_err();
            assert!(
                err.contains("allowed roots"),
                "expected traversal guard, got: {err}"
            );
        }
    }

    #[test]
    fn read_truncates_at_4000_chars() {
        // Temp file inside cwd so it stays within the allowed roots.
        let mut tmp = tempfile::NamedTempFile::new_in(".").unwrap();
        use std::io::Write as _;
        write!(tmp, "{}", "x".repeat(6000)).unwrap();
        let out = dispatch(
            "fs_read",
            &json!({ "path": tmp.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert!(out.starts_with(&"x".repeat(OUTPUT_CAP_CHARS)));
        assert!(out.contains("truncated"));
        assert!(out.chars().count() < 6000);
    }

    #[test]
    fn list_caps_at_100_entries() {
        // Temp dir inside cwd so it stays within the allowed roots.
        let dir = tempfile::tempdir_in(".").unwrap();
        for i in 0..120 {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let out = dispatch(
            "fs_list",
            &json!({ "path": dir.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert_eq!(out.lines().count(), LIST_CAP);
    }

    #[test]
    fn needs_approval_flags_only_write_tools() {
        for name in ["fs_read", "fs_list", "fs_search"] {
            assert!(!needs_approval(name), "{name} should be approval-free");
        }
        for name in ["shell_exec", "app_open", "url_open"] {
            assert!(needs_approval(name), "{name} should need approval");
        }
        assert!(!needs_approval("nope"));
    }

    #[test]
    fn approval_tokens_are_32_hex_and_unique() {
        let a = issue_approval_token();
        let b = issue_approval_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn shell_exec_without_token_errors_with_prefix() {
        let err = dispatch("shell_exec", &json!({ "command": "echo hi" }), None).unwrap_err();
        assert!(err.starts_with("approval-required:"), "unexpected: {err}");
        // Wrong token is the same as no token.
        let err = dispatch(
            "shell_exec",
            &json!({ "command": "echo hi", "approval_token": "wrong" }),
            Some("right"),
        )
        .unwrap_err();
        assert!(err.starts_with("approval-required:"), "unexpected: {err}");
    }

    #[test]
    fn shell_exec_with_token_runs_echo() {
        let tok = issue_approval_token();
        let out = dispatch(
            "shell_exec",
            &json!({ "command": "echo hi", "approval_token": &tok }),
            Some(&tok),
        )
        .unwrap();
        assert!(out.contains("hi"), "unexpected: {out}");
    }

    #[test]
    fn home_expansion_covers_quoted_forms() {
        let home = home_dir().to_string_lossy().into_owned();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(expand_home("~/a"), format!("{home}{sep}a"));
        assert_eq!(expand_home("~\\a"), format!("{home}{sep}a"));
        assert_eq!(expand_home("\"~/a\""), format!("\"{home}{sep}a\""));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("echo ~ thereafter"), "echo ~ thereafter");
    }

    #[test]
    fn shell_exec_starts_in_home_dir() {
        let tok = issue_approval_token();
        let out = dispatch(
            "shell_exec",
            &json!({ "command": "cd", "approval_token": &tok }),
            Some(&tok),
        )
        .unwrap();
        let expected = home_dir().to_string_lossy().into_owned();
        assert!(
            out.trim_end().eq_ignore_ascii_case(&expected),
            "cwd {out} != home {expected}"
        );
    }

    #[test]
    fn quoted_paths_survive_cmd_quoting() {
        // Regression: cmd.exe mangled inner quotes, so quoted absolute
        // paths failed with a syntax error. Needs Windows cmd.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a b");
        let command = format!("mkdir \"{}\"", target.display());
        let tok = issue_approval_token();
        dispatch(
            "shell_exec",
            &serde_json::json!({ "command": command, "approval_token": &tok }),
            Some(&tok),
        )
        .unwrap();
        assert!(target.is_dir(), "quoted mkdir failed");
    }

    #[test]
    fn open_tools_without_token_error() {
        let err = dispatch("app_open", &json!({ "target": "notepad" }), None).unwrap_err();
        assert!(err.starts_with("approval-required:"), "unexpected: {err}");
        let err = dispatch("url_open", &json!({ "url": "https://example.com" }), None).unwrap_err();
        assert!(err.starts_with("approval-required:"), "unexpected: {err}");
    }

    #[test]
    fn url_open_rejects_non_http_even_with_token() {
        let tok = issue_approval_token();
        let err = dispatch(
            "url_open",
            &json!({ "url": "file:///etc/passwd", "approval_token": &tok }),
            Some(&tok),
        )
        .unwrap_err();
        assert!(err.contains("non-http"), "unexpected: {err}");
    }

    #[test]
    fn index_query_returns_without_error() {
        // Real machine index: may be empty/cold — assert only no-panic/no-error.
        let _ = search_index("notepad");
        let _ = search_index("zzz-argle-bargle-qqq");
        if let Some(hits) = search_index("notepad") {
            assert!(hits.len() <= SEARCH_CAP);
        }
    }

    #[test]
    fn broken_index_dir_returns_none_so_caller_falls_back() {
        // A regular file as the index dir: create_dir_all fails → None.
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(search_index_at("notepad", file.path()).is_none());
    }

    #[test]
    fn stub_finds_unique_temp_file() {
        // Temp dir inside cwd (the stub's scan root).
        let dir = tempfile::tempdir_in(".").unwrap();
        let unique = format!("nex-agent-stub-{}.txt", std::process::id());
        std::fs::write(dir.path().join(&unique), "x").unwrap();
        let out = stub_fs_search(&unique).unwrap();
        assert!(out.contains(&unique), "unexpected: {out}");
    }

    #[test]
    fn fs_search_never_hard_errors_on_garbage_query() {
        let out = dispatch("fs_search", &json!({ "query": "zzz-argle-bargle-qqq" }), None).unwrap();
        assert!(out.lines().count() <= SEARCH_CAP);
    }
}
