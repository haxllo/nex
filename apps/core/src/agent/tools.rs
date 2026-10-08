use std::path::{Path, PathBuf};

/// Max characters returned by a single tool result (plan §3: truncation budget).
pub(crate) const OUTPUT_CAP_CHARS: usize = 4000;
/// Max directory entries returned by `fs_list`.
pub(crate) const LIST_CAP: usize = 100;
/// Max hits returned by `fs_search`.
pub(crate) const SEARCH_CAP: usize = 20;
/// Max paths returned by `fs_glob`.
pub(crate) const GLOB_CAP: usize = 50;
/// Max matches returned by `fs_grep`.
pub(crate) const GREP_CAP: usize = 30;
/// Max walkdir entries visited per `fs_glob` call before stopping.
pub(crate) const GLOB_VISIT_BUDGET: usize = 50_000;
/// Max file entries visited per `fs_grep` call before stopping.
pub(crate) const GREP_FILE_BUDGET: usize = 50_000;
/// Files larger than this are skipped by `fs_grep`.
pub(crate) const GREP_MAX_BYTES: u64 = 1024 * 1024;
/// Leading bytes sniffed for NULs to skip binary files in `fs_grep`.
pub(crate) const GREP_SNIFF_BYTES: usize = 8192;
/// Max chars of matched line text kept per `fs_grep` hit.
pub(crate) const GREP_LINE_CHARS: usize = 200;

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

/// Full tool registry: 6 read-only + 3 approval-gated.
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
            description: "Search the launcher app index (installed apps and launched items) by filename substring. Does NOT search the filesystem on disk: use fs_glob to find files/dirs by name, fs_grep for file contents. Capped at 20 hits.",
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
            name: "fs_glob",
            description: "Find files/dirs by name on disk: glob pattern (*, ?, **) matched recursively under root (default home, supports ~/env vars). One absolute path per line. Capped at 50 hits.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern: '*' any run within a segment, '?' one char, '**' crosses directories. No separator matches against the file name only." },
                    "root": { "type": "string", "description": "Directory to search under (default: home directory)." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "fs_grep",
            description: "Find file contents on disk: case-insensitive substring search under root (default home), optional file_pattern glob filter (e.g. *.toml). Substring only, no regex. Skips files over 1MB and binary files. Hits as path:line: text. Capped at 30 matches.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Case-insensitive substring to find in file lines (no regex)." },
                    "root": { "type": "string", "description": "Directory to search under (default: home directory)." },
                    "file_pattern": { "type": "string", "description": "Optional glob filter for file names (e.g. *.toml)." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "app_info",
            description: "Resolved environment facts: home directory, Nex app-data directory, working directory, OS, shell. Call this before guessing locations.",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
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
        "fs_glob" => {
            let pattern = args_json
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "fs_glob: missing required string arg 'pattern'".to_string())?;
            let root = args_json.get("root").and_then(serde_json::Value::as_str);
            fs_glob(pattern, root)
        }
        "fs_grep" => {
            let pattern = args_json
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "fs_grep: missing required string arg 'pattern'".to_string())?;
            let root = args_json.get("root").and_then(serde_json::Value::as_str);
            let file_pattern = args_json
                .get("file_pattern")
                .and_then(serde_json::Value::as_str);
            fs_grep(pattern, root, file_pattern)
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
        "app_info" => Ok(app_info()),
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Resolved environment facts so the model never guesses locations.
fn app_info() -> String {
    let info = serde_json::json!({
        "home": home_dir().to_string_lossy(),
        "app_data": crate::config::stable_app_data_dir().to_string_lossy(),
        "working_dir": std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        "os": std::env::consts::OS,
        "shell": "cmd.exe",
    });
    serde_json::to_string(&info).unwrap_or_default()
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

/// Expand `%NAME%`, `$NAME`/`${NAME}`, and a leading `~` in fs paths.
/// Unknown vars stay literal; never fails.
pub(crate) fn expand_env_vars(input: &str) -> String {
    let mut s: String = input.to_string();
    if s == "~" {
        return home_dir().to_string_lossy().into_owned();
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        let home = home_dir().to_string_lossy().into_owned();
        return format!("{home}{}{rest}", std::path::MAIN_SEPARATOR);
    }
    s = expand_percent_vars(&s);
    s = expand_dollar_vars(&s);
    s
}

fn expand_percent_vars(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(end) = bytes[i + 1..].iter().position(|&b| b == b'%') {
                let name = &s[i + 1..i + 1 + end];
                if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    if let Ok(val) = std::env::var(name) {
                        out.push_str(&val);
                    } else {
                        out.push_str(&s[i..i + 1 + end + 1]);
                    }
                    i += 1 + end + 1;
                    continue;
                }
            }
            out.push('%');
            i += 1;
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn expand_dollar_vars(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            if bytes.get(i + 1) == Some(&b'{') {
                if let Some(rel) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                    let name = &s[i + 2..i + 2 + rel];
                    if !name.is_empty()
                        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    {
                        if let Ok(val) = std::env::var(name) {
                            out.push_str(&val);
                        } else {
                            out.push_str(&s[i..i + 2 + rel + 1]);
                        }
                        i += 2 + rel + 1;
                        continue;
                    }
                }
                out.push('$');
                i += 1;
            } else {
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j == i + 1 {
                    out.push('$');
                    i += 1;
                } else {
                    let name = &s[i + 1..j];
                    if let Ok(val) = std::env::var(name) {
                        out.push_str(&val);
                    } else {
                        out.push_str(&s[i..j]);
                    }
                    i = j;
                }
            }
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Resolve `path` against the home directory (env vars + `~` expanded
/// first) and refuse anything escaping the allowed roots.
fn resolve_within_roots(path: &Path) -> Result<PathBuf, String> {
    let expanded = expand_env_vars(&path.to_string_lossy());
    let expanded_path = PathBuf::from(&expanded);
    let joined = if expanded_path.is_absolute() {
        expanded_path
    } else {
        home_dir().join(&expanded_path)
    };
    let canon = std::fs::canonicalize(&joined)
        .map_err(|e| format!("cannot resolve '{}': {e}", joined.display()))?;
    if allowed_roots().iter().any(|r| canon.starts_with(r)) {
        Ok(canon)
    } else {
        Err(format!(
            "path '{}' escapes allowed roots (home and working directory)",
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

/// Resolve an optional search `root` the same way `fs_read` resolves
/// paths: `~`/env expanded, relative joins home, must exist inside the
/// allowed roots. `None`/empty defaults to home. Invalid roots Err.
fn resolve_root(root: Option<&str>) -> Result<PathBuf, String> {
    let raw = match root {
        Some(r) if !r.trim().is_empty() => r.to_string(),
        _ => home_dir().to_string_lossy().into_owned(),
    };
    resolve_within_roots(Path::new(&raw))
}

/// `*` any run within a segment, `?` one char (both already lowercased).
fn segment_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Segment-wise match where a `**` segment crosses directories.
/// All matching is case-insensitive (both sides lowered by the caller).
fn path_match(pat: &[String], path: &[String]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        return (0..=path.len()).any(|i| path_match(&pat[1..], &path[i..]));
    }
    if path.is_empty() || !segment_match(&pat[0], &path[0]) {
        return false;
    }
    path_match(&pat[1..], &path[1..])
}

/// Match `pattern` against a walk entry: no separator matches the file
/// name only, else the `/`-separated path relative to the search root.
fn glob_match(pattern: &str, rel: &str, file_name: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    if !pat.contains('/') {
        return segment_match(&pat.to_lowercase(), &file_name.to_lowercase());
    }
    let psegs: Vec<String> = pat
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    let rsegs: Vec<String> = rel
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    path_match(&psegs, &rsegs)
}

/// Recursive filename match under `root` (default home). Skips nothing
/// (hidden dirs included), never follows symlinks. One path per line.
fn fs_glob(pattern: &str, root: Option<&str>) -> Result<String, String> {
    if pattern.trim().is_empty() {
        return Err("fs_glob: missing required string arg 'pattern'".to_string());
    }
    let base = resolve_root(root)?;
    let mut hits = Vec::new();
    let mut visited: usize = 0;
    for entry in walkdir::WalkDir::new(&base).follow_links(false) {
        visited += 1;
        if visited > GLOB_VISIT_BUDGET || hits.len() >= GLOB_CAP {
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_symlink() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&base)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if rel.is_empty() {
            continue; // the root itself
        }
        if glob_match(pattern, &rel, &entry.file_name().to_string_lossy()) {
            hits.push(entry.path().display().to_string());
        }
    }
    hits.sort();
    hits.truncate(GLOB_CAP);
    Ok(hits.join("\n"))
}

/// Line-oriented case-insensitive substring search under `root` (default
/// home). Substring only, no regex. Skips files >1MB and binary-looking
/// content (NUL in the first 8KB). Hits as `path:line: text`.
fn fs_grep(
    pattern: &str,
    root: Option<&str>,
    file_pattern: Option<&str>,
) -> Result<String, String> {
    if pattern.trim().is_empty() {
        return Err("fs_grep: missing required string arg 'pattern'".to_string());
    }
    let base = resolve_root(root)?;
    let needle = pattern.to_lowercase();
    let filter = file_pattern.filter(|f| !f.trim().is_empty());
    let mut out = Vec::new();
    let mut files_seen: usize = 0;
    'walk: for entry in walkdir::WalkDir::new(&base).follow_links(false) {
        let Ok(entry) = entry else { continue };
        let ft = entry.file_type();
        if ft.is_symlink() || !ft.is_file() {
            continue;
        }
        files_seen += 1;
        if files_seen > GREP_FILE_BUDGET || out.len() >= GREP_CAP {
            break;
        }
        let name = entry.file_name().to_string_lossy();
        if let Some(fp) = filter {
            let rel = entry
                .path()
                .strip_prefix(&base)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !glob_match(fp, &rel, &name) {
                continue;
            }
        }
        if entry.metadata().map(|m| m.len() > GREP_MAX_BYTES).unwrap_or(false) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else { continue };
        if bytes.len() as u64 > GREP_MAX_BYTES {
            continue;
        }
        if bytes[..bytes.len().min(GREP_SNIFF_BYTES)].contains(&0) {
            continue;
        }
        // ponytail: whole file read + lowercased per line; streaming reader if >1MB cap ever raised
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                let short: String = line.chars().take(GREP_LINE_CHARS).collect();
                out.push(format!("{}:{}: {}", entry.path().display(), i + 1, short));
                if out.len() >= GREP_CAP {
                    break 'walk;
                }
            }
        }
    }
    out.sort();
    out.truncate(GREP_CAP);
    Ok(out.join("\n"))
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
    fn registry_lists_all_tools() {
        let names: Vec<_> = registry().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "fs_read",
                "fs_list",
                "fs_search",
                "fs_glob",
                "fs_grep",
                "app_info",
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
        // Temp file inside the home dir so it stays within the allowed roots
        // (relative fs paths resolve against home).
        let mut tmp = tempfile::NamedTempFile::new_in(home_dir()).unwrap();
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
        // Temp dir inside home so it stays within the allowed roots.
        let dir = tempfile::tempdir_in(home_dir()).unwrap();
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
        for name in ["fs_read", "fs_list", "fs_search", "fs_glob", "fs_grep"] {
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
    fn env_vars_expand_percent_dollar_and_tilde() {
        let key = "NEX_TEST_EXPAND_PATH_VAR";
        unsafe { std::env::set_var(key, "C:\\nex-test-target") };
        assert_eq!(
            expand_env_vars(&format!("%{key}%\\sub")),
            "C:\\nex-test-target\\sub"
        );
        assert_eq!(
            expand_env_vars(&format!("${key}/sub")),
            "C:\\nex-test-target/sub"
        );
        assert_eq!(
            expand_env_vars(&format!("${{{key}}}/sub")),
            "C:\\nex-test-target/sub"
        );
        unsafe { std::env::remove_var(key) };
        // Unknown vars stay literal.
        assert_eq!(expand_env_vars("%NEX_NO_SUCH_VAR_XYZ%"), "%NEX_NO_SUCH_VAR_XYZ%");
        // Leading ~ resolves inside home.
        let home = home_dir().to_string_lossy().into_owned();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(expand_env_vars("~"), home);
        assert_eq!(expand_env_vars("~/docs"), format!("{home}{sep}docs"));
    }

    #[test]
    fn relative_paths_resolve_against_home() {
        // Unique file directly under home; a bare filename (relative) must find it.
        let unique = format!("nex-agent-reltest-{}-{}.txt", std::process::id(), "homejoin");
        let abs = home_dir().join(&unique);
        std::fs::write(&abs, "home-relative-ok").unwrap();
        let out = dispatch("fs_read", &json!({ "path": unique }), None).unwrap();
        assert!(out.contains("home-relative-ok"), "unexpected: {out}");
        let _ = std::fs::remove_file(&abs);
    }

    #[test]
    fn percent_var_path_resolves_end_to_end() {
        let key = "NEX_TEST_FS_EXPAND_VAR";
        let dir = tempfile::tempdir_in(home_dir()).unwrap();
        unsafe { std::env::set_var(key, dir.path().to_string_lossy().into_owned()) };
        std::fs::write(dir.path().join("via-var.txt"), "var-ok").unwrap();
        let out = dispatch(
            "fs_read",
            &json!({ "path": format!("%{key}%/via-var.txt") }),
            None,
        )
        .unwrap();
        assert!(out.contains("var-ok"), "unexpected: {out}");
        unsafe { std::env::remove_var(key) };
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
    fn quoted_paths_survive_cmd_quoting() {        // Regression: cmd.exe mangled inner quotes, so quoted absolute
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
    fn app_info_reports_resolved_paths() {
        let out = dispatch("app_info", &serde_json::json!({}), None).unwrap();
        let info: serde_json::Value = serde_json::from_str(&out).unwrap();
        for key in ["home", "app_data", "working_dir", "os", "shell"] {
            let value = info.get(key).and_then(|v| v.as_str()).unwrap_or_default();
            assert!(!value.is_empty(), "app_info missing {key}");
        }
        assert!(!info["home"].as_str().unwrap_or("").contains('%'));
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

    #[test]
    fn glob_finds_uniquely_named_nested_file() {
        let dir = tempfile::tempdir_in(home_dir()).unwrap();
        let nested = dir.path().join("sub").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        let unique = format!("nex-agent-glob-{}-{}.txt", std::process::id(), "nested");
        std::fs::write(nested.join(&unique), "x").unwrap();
        // Bare pattern matches the file name at any depth.
        let out = dispatch(
            "fs_glob",
            &json!({ "pattern": unique, "root": dir.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert!(out.contains(&unique), "unexpected: {out}");
        // Separator pattern matches the relative path across directories.
        let out = dispatch(
            "fs_glob",
            &json!({ "pattern": format!("**/{unique}"), "root": dir.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert!(out.contains(&unique), "unexpected: {out}");
    }

    #[test]
    fn glob_respects_result_cap() {
        let dir = tempfile::tempdir_in(home_dir()).unwrap();
        for i in 0..(GLOB_CAP + 20) {
            std::fs::write(dir.path().join(format!("cap-{i:03}.txt")), "x").unwrap();
        }
        let out = dispatch(
            "fs_glob",
            &json!({ "pattern": "cap-*.txt", "root": dir.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert_eq!(out.lines().count(), GLOB_CAP);
    }

    #[test]
    fn grep_finds_unique_string_with_line_number() {
        let dir = tempfile::tempdir_in(home_dir()).unwrap();
        let unique = format!("nex-agent-grep-{}-needle", std::process::id());
        std::fs::write(
            dir.path().join("hay.txt"),
            format!("first line\nsecond {unique} here\nthird line\n"),
        )
        .unwrap();
        let out = dispatch(
            "fs_grep",
            &json!({ "pattern": unique, "root": dir.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert!(out.contains(":2:"), "unexpected: {out}");
        assert!(out.contains("hay.txt"), "unexpected: {out}");
        // file_pattern filter keeps matching files, drops the rest.
        let out = dispatch(
            "fs_grep",
            &json!({ "pattern": unique, "root": dir.path().to_string_lossy(), "file_pattern": "*.txt" }),
            None,
        )
        .unwrap();
        assert!(out.contains(":2:"), "unexpected: {out}");
        let out = dispatch(
            "fs_grep",
            &json!({ "pattern": unique, "root": dir.path().to_string_lossy(), "file_pattern": "*.toml" }),
            None,
        )
        .unwrap();
        assert!(out.is_empty(), "unexpected: {out}");
    }

    #[test]
    fn grep_skips_files_over_1mb() {
        let dir = tempfile::tempdir_in(home_dir()).unwrap();
        let unique = format!("nex-agent-grepbig-{}-needle", std::process::id());
        let mut big = vec![b'x'; (GREP_MAX_BYTES + 1024) as usize];
        let needle = unique.as_bytes();
        big[..needle.len()].copy_from_slice(needle);
        std::fs::write(dir.path().join("big.txt"), big).unwrap();
        std::fs::write(dir.path().join("small.txt"), format!("has {unique}\n")).unwrap();
        let out = dispatch(
            "fs_grep",
            &json!({ "pattern": unique, "root": dir.path().to_string_lossy() }),
            None,
        )
        .unwrap();
        assert!(out.contains("small.txt"), "unexpected: {out}");
        assert!(!out.contains("big.txt"), "unexpected: {out}");
    }

    #[test]
    fn glob_and_grep_reject_invalid_root() {
        assert!(dispatch(
            "fs_glob",
            &json!({ "pattern": "*.txt", "root": "nex-no-such-dir-xyz" }),
            None,
        )
        .is_err());
        assert!(dispatch(
            "fs_grep",
            &json!({ "pattern": "x", "root": "nex-no-such-dir-xyz" }),
            None,
        )
        .is_err());
        assert!(dispatch("fs_glob", &json!({ "pattern": "" }), None).is_err());
        assert!(dispatch("fs_grep", &json!({ "pattern": "  " }), None).is_err());
    }
}
