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

/// Read-only tool registry.
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
    ]
}

pub(crate) fn dispatch(name: &str, args_json: &serde_json::Value) -> Result<String, String> {
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

// TEMPORARY STUB (Task 4 wires the real index): filename substring scan under
// the current directory. Recursive, skips symlinks, bounded traversal budget.
fn fs_search(query: &str) -> Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_tool_errors() {
        let err = dispatch("nope", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"), "unexpected: {err}");
    }

    #[test]
    fn registry_lists_three_read_tools() {
        let names: Vec<_> = registry().iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["fs_read", "fs_list", "fs_search"]);
    }

    #[test]
    fn traversal_rejected() {
        // Relative escape: rejected (missing or outside roots — either way Err).
        assert!(dispatch("fs_read", &json!({ "path": "../../secret" })).is_err());
        // Absolute path outside all roots: must hit the traversal guard itself.
        let root = std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.ancestors().last().map(|r| r.to_path_buf()));
        if let Some(root) = root {
            let err = dispatch(
                "fs_read",
                &json!({ "path": root.to_string_lossy() }),
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
        )
        .unwrap();
        assert_eq!(out.lines().count(), LIST_CAP);
    }
}
