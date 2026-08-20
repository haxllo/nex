use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

const INSTALLED_UPDATER_RELATIVE_PATH: &str = "scripts/update-nex.ps1";
const INSTALLED_UPDATER_FALLBACK_NAME: &str = "update-nex.ps1";
const DEV_UPDATER_RELATIVE_PATH: &str = "scripts/windows/update-nex.ps1";
const MAX_ANCESTOR_SCAN_DEPTH: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    Stable,
    Beta,
}

impl UpdateChannel {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateLaunchError {
    UnsupportedPlatform,
    EnvironmentUnavailable(String),
    ScriptNotFound { checked_paths: Vec<PathBuf> },
    LaunchFailed(String),
}

impl Display for UpdateLaunchError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "updater is only available on Windows"),
            Self::EnvironmentUnavailable(message) => write!(f, "{message}"),
            Self::ScriptNotFound { checked_paths } => {
                if checked_paths.is_empty() {
                    write!(f, "update script not found")
                } else {
                    let joined = checked_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(f, "update script not found (checked: {joined})")
                }
            }
            Self::LaunchFailed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for UpdateLaunchError {}

pub fn launch_updater(channel: UpdateChannel) -> Result<PathBuf, UpdateLaunchError> {
    let script_path = resolve_updater_script()?;
    launch_updater_script(script_path.as_path(), channel)?;
    Ok(script_path)
}

/// Run the updater script to completion, capturing its output.
/// Launches with a hidden window (no console flash).
pub fn run_updater_capture(
    channel: UpdateChannel,
) -> Result<std::process::Output, UpdateLaunchError> {
    let script_path = resolve_updater_script()?;
    run_updater_script(script_path.as_path(), channel)
}

/// Summarize an updater script run into a single user-facing status line.
/// Prefers the script's NEX_UPDATE_RESULT marker (JSON); falls back to
/// exit code / stderr.
pub fn summarize_update_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().rev() {
        if let Some((_, json)) = line.split_once("NEX_UPDATE_RESULT:") {
            let json = json.trim();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                let status = value
                    .get("status")
                    .and_then(|field| field.as_str())
                    .unwrap_or("");
                let version = value
                    .get("version")
                    .and_then(|field| field.as_str())
                    .unwrap_or("");
                return match status {
                    "up-to-date" => format!("Up to date (v{version})"),
                    "updated" => format!("Updated to v{version}"),
                    _ => {
                        let message = value
                            .get("message")
                            .and_then(|field| field.as_str())
                            .unwrap_or("unknown error");
                        format!("Update failed: {message}")
                    }
                };
            }
        }
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last_err = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if output.status.success() {
        "Update finished".to_string()
    } else if !last_err.is_empty() {
        format!("Update failed: {last_err}")
    } else {
        format!(
            "Update failed (exit {})",
            output.status.code().unwrap_or(-1)
        )
    }
}

fn resolve_updater_script() -> Result<PathBuf, UpdateLaunchError> {
    let exe_path = std::env::current_exe().map_err(|error| {
        UpdateLaunchError::EnvironmentUnavailable(format!(
            "could not resolve current executable path: {error}"
        ))
    })?;
    let cwd = std::env::current_dir().map_err(|error| {
        UpdateLaunchError::EnvironmentUnavailable(format!(
            "could not resolve current working directory: {error}"
        ))
    })?;
    let checked_paths = updater_script_candidates(&exe_path, &cwd);
    let script_path = checked_paths
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
        .ok_or_else(|| UpdateLaunchError::ScriptNotFound {
            checked_paths: checked_paths.clone(),
        })?;
    Ok(script_path)
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
fn build_updater_command(script_path: &Path, channel: UpdateChannel) -> std::process::Command {
    let mut command = std::process::Command::new("powershell.exe");
    command
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(script_path)
        .arg("-Channel")
        .arg(channel.as_arg());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(target_os = "windows")]
fn launch_updater_script(
    script_path: &Path,
    channel: UpdateChannel,
) -> Result<(), UpdateLaunchError> {
    build_updater_command(script_path, channel)
        .spawn()
        .map_err(|error| {
            UpdateLaunchError::LaunchFailed(format!(
                "failed to launch updater script '{}': {error}",
                script_path.display()
            ))
        })?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_updater_script(
    script_path: &Path,
    channel: UpdateChannel,
) -> Result<std::process::Output, UpdateLaunchError> {
    build_updater_command(script_path, channel)
        .output()
        .map_err(|error| {
            UpdateLaunchError::LaunchFailed(format!(
                "failed to run updater script '{}': {error}",
                script_path.display()
            ))
        })
}

#[cfg(not(target_os = "windows"))]
fn launch_updater_script(
    _script_path: &Path,
    _channel: UpdateChannel,
) -> Result<(), UpdateLaunchError> {
    Err(UpdateLaunchError::UnsupportedPlatform)
}

#[cfg(not(target_os = "windows"))]
fn run_updater_script(
    _script_path: &Path,
    _channel: UpdateChannel,
) -> Result<std::process::Output, UpdateLaunchError> {
    Err(UpdateLaunchError::UnsupportedPlatform)
}

fn updater_script_candidates(exe_path: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(exe_dir) = exe_path.parent() {
        if let Some(install_root) = exe_dir.parent() {
            push_unique(
                &mut candidates,
                install_root.join(INSTALLED_UPDATER_RELATIVE_PATH),
            );
            push_unique(
                &mut candidates,
                install_root.join(INSTALLED_UPDATER_FALLBACK_NAME),
            );
        }
        collect_ancestor_candidates(exe_dir, &mut candidates, DEV_UPDATER_RELATIVE_PATH);
    }

    collect_ancestor_candidates(cwd, &mut candidates, DEV_UPDATER_RELATIVE_PATH);

    candidates
}

fn collect_ancestor_candidates(base: &Path, out: &mut Vec<PathBuf>, relative: &str) {
    for ancestor in base.ancestors().take(MAX_ANCESTOR_SCAN_DEPTH) {
        push_unique(out, ancestor.join(relative));
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|existing| existing == &candidate) {
        paths.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        updater_script_candidates, DEV_UPDATER_RELATIVE_PATH, INSTALLED_UPDATER_RELATIVE_PATH,
    };

    #[test]
    fn updater_candidates_prefer_installed_layout_before_repo_fallback() {
        let root =
            std::env::temp_dir().join(format!("nex-updater-installed-{}", std::process::id()));
        let exe_path = root.join("bin/Nex.exe");
        let cwd = root.clone();

        let candidates = updater_script_candidates(&exe_path, &cwd);

        assert_eq!(candidates[0], root.join(INSTALLED_UPDATER_RELATIVE_PATH));
        assert_eq!(candidates[1], root.join("update-nex.ps1"));
    }

    #[test]
    fn updater_candidates_include_repo_style_script_lookup() {
        let root = std::env::temp_dir().join(format!("nex-updater-repo-{}", std::process::id()));
        let repo = root.join("repo");
        let exe_path = repo.join("target/debug/Nex.exe");
        let cwd = repo.join("apps/core");

        let candidates = updater_script_candidates(&exe_path, &cwd);

        assert!(candidates
            .iter()
            .any(|candidate| candidate == &repo.join(DEV_UPDATER_RELATIVE_PATH)));
    }

    #[test]
    fn updater_candidates_are_deduplicated() {
        let repo = std::env::temp_dir().join(format!("nex-updater-dedupe-{}", std::process::id()));
        let exe_path = repo.join("target/debug/Nex.exe");
        let cwd = repo.join("target/debug");

        let candidates = updater_script_candidates(&exe_path, &cwd);
        let unique = candidates.iter().collect::<std::collections::BTreeSet<_>>();

        assert_eq!(candidates.len(), unique.len());
    }
}
