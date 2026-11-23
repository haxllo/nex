use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

const INSTALLED_UPDATER_RELATIVE_PATH: &str = "scripts/update-nex.ps1";
const INSTALLED_UPDATER_FALLBACK_NAME: &str = "update-nex.ps1";
const DEV_UPDATER_RELATIVE_PATH: &str = "scripts/windows/update-nex.ps1";
const MAX_ANCESTOR_SCAN_DEPTH: usize = 10;
const RELEASES_API: &str = "https://api.github.com/repos/haxllo/nex/releases?per_page=40";
const RELEASES_USER_AGENT: &str = "Nex-Updater";

/// What an update check can answer.
///
/// A failed check is its own answer. The previous shape returned
/// `(false, None)` for "up to date", for "no update script", and for "GitHub
/// was unreachable" alike, so a permanently broken updater looked exactly like
/// a current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate { latest: String },
    Available { version: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct ReleaseTag {
    #[serde(default)]
    pub(crate) tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct ReleaseAsset {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) browser_download_url: String,
}

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

/// Which apply path an update uses.
///
/// `Script` stays the default while the native engine is unproven on a real
/// install: it is the only path that has ever recovered a half-moved install,
/// and losing that to save a child process would be a bad trade. `Native` is
/// opt-in per run, so one machine can try it without changing what every user
/// gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateEngine {
    Script,
    Native,
}

impl UpdateEngine {
    /// `NEX_UPDATE_ENGINE=native` selects the native engine; anything else
    /// (including unset) keeps the script.
    pub fn from_environment() -> Self {
        match std::env::var("NEX_UPDATE_ENGINE") {
            Ok(value) if value.trim().eq_ignore_ascii_case("native") => Self::Native,
            _ => Self::Script,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Native => "native",
        }
    }
}

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
                    "version-skew" => {
                        let message = value
                            .get("message")
                            .and_then(|field| field.as_str())
                            .unwrap_or("running copy differs from installed copy; restart Nex");
                        format!("Restart Nex: {message}")
                    }
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
        .find(|line| {
            let line = line.trim();
            !line.is_empty()
                && !line.starts_with("+ CategoryInfo")
                && !line.starts_with("+ FullyQualifiedErrorId")
        })
        .or_else(|| stderr.lines().rev().find(|line| !line.trim().is_empty()))
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

/// Check if an update is available by running the updater in capture mode
/// and parsing the output. Returns (available, detected version).
/// Everything an update would do, decided and written out but not performed.
///
/// This exists because "which install would be modified, and with elevation or
/// not?" has been the wrong answer before — a stale HKLM registration beside a
/// per-user one, or an orphan copy shadowing the real install. The plan is read
/// from the same functions a native apply path will use, so the diagnostic
/// cannot describe behaviour that differs from what the updater does.
pub fn update_plan_json(channel: UpdateChannel) -> String {
    let facts = crate::update_install::collect_install_facts();
    let target = crate::update_install::resolve_install_target(&facts);
    let installed = crate::update_install::installed_version(&target.root);
    let runtime = crate::update_install::runtime_executable(&target.root);

    let (status, latest, reason) = match fetch_releases() {
        Ok(releases) => {
            match select_release(
                &releases,
                channel,
                installed.as_deref().unwrap_or("0.0.0"),
            ) {
                UpdateCheck::Available { version } => ("update-available", Some(version), None),
                UpdateCheck::UpToDate { latest } => ("up-to-date", Some(latest), None),
                UpdateCheck::Failed { reason } => ("check-failed", None, Some(reason)),
            }
        }
        Err(reason) => ("check-failed", None, Some(reason)),
    };

    let assets = latest
        .as_deref()
        .map(crate::update_install::release_asset_names);

    serde_json::json!({
        "channel": channel.as_arg(),
        "status": status,
        "reason": reason,
        "installed_version": installed,
        "installed_root": target
            .root
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string(),
        "install_source": target.source.as_str(),
        "needs_elevation": target.needs_elevation,
        "runtime_executable": runtime.to_string_lossy(),
        "latest_version": latest,
        "setup_asset": assets.as_ref().map(|(setup, _)| setup.clone()),
        "manifest_asset": assets.as_ref().map(|(_, manifest)| manifest.clone()),
    })
    .to_string()
}

/// Ask GitHub whether a newer release exists, in process.
///
/// This used to start PowerShell and scrape `NEX_UPDATE_RESULT:` out of the
/// update script: 1.8 s of interpreter start for one HTTPS GET, and every
/// failure mode (no script, no interpreter, no network, unparseable output)
/// arrived as "up to date". Selecting a release is pure and tested; only the
/// fetch can fail, and now it says so.
pub fn check_update_available(channel: UpdateChannel) -> UpdateCheck {
    let releases = match fetch_releases() {
        Ok(releases) => releases,
        Err(reason) => return UpdateCheck::Failed { reason },
    };
    select_release(&releases, channel, running_version())
}

fn running_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub(crate) fn fetch_releases() -> Result<Vec<ReleaseTag>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(8))
        .build();
    let response = match agent
        .get(RELEASES_API)
        .set("User-Agent", RELEASES_USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("GitHub returned HTTP {code} for the release list"));
        }
        Err(ureq::Error::Transport(error)) => {
            return Err(format!("could not reach GitHub: {error}"));
        }
    };
    response
        .into_json::<Vec<ReleaseTag>>()
        .map_err(|error| format!("the release list was not readable: {error}"))
}

/// The selection rules, ported from `scripts/windows/update-nex.ps1` so the
/// in-process check cannot disagree with the updater that applies the update.
pub(crate) fn select_release(
    releases: &[ReleaseTag],
    channel: UpdateChannel,
    running: &str,
) -> UpdateCheck {
    let Some(newest) = newest_release(releases, channel) else {
        let channel_name = if channel == UpdateChannel::Beta {
            "beta"
        } else {
            "stable"
        };
        return UpdateCheck::Failed {
            reason: format!("no {channel_name} release found"),
        };
    };

    let latest = normalize_version(&newest.tag_name);
    if compare_versions(running, &latest) >= std::cmp::Ordering::Equal {
        return UpdateCheck::UpToDate { latest };
    }
    UpdateCheck::Available { version: latest }
}

/// The newest release on a channel: the one place that rule lives, shared by
/// the update check and the engine that installs.
pub(crate) fn newest_release(
    releases: &[ReleaseTag],
    channel: UpdateChannel,
) -> Option<&ReleaseTag> {
    let wants_beta = channel == UpdateChannel::Beta;
    releases
        .iter()
        .filter(|release| is_beta_release(release) == wants_beta)
        .filter(|release| !normalize_version(&release.tag_name).is_empty())
        .max_by(|left, right| {
            compare_versions(
                &normalize_version(&left.tag_name),
                &normalize_version(&right.tag_name),
            )
        })
}

/// A release is beta when GitHub flags it as a prerelease, or its tag says so.
fn is_beta_release(release: &ReleaseTag) -> bool {
    if release.prerelease {
        return true;
    }
    let tag = release.tag_name.as_str();
    match tag.find("-beta") {
        Some(index) => {
            let rest = &tag[index + "-beta".len()..];
            rest.is_empty() || rest.starts_with('.') || rest.starts_with('-')
        }
        None => false,
    }
}

/// Strip a leading `v`, and give a two-part version the patch it omits.
pub(crate) fn normalize_version(tag_or_version: &str) -> String {
    let trimmed = tag_or_version.trim();
    let without_prefix = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let (core, suffix) = match without_prefix.find(['-', '+']) {
        Some(index) => (&without_prefix[..index], &without_prefix[index..]),
        None => (without_prefix, ""),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() == 2 && parts.iter().all(|part| is_numeric(part)) {
        return format!("{core}.0{suffix}");
    }
    without_prefix.to_string()
}

fn is_numeric(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit())
}

/// Numeric comparison of the version core, ignoring any `-`/`+` suffix, with
/// missing components treated as zero — the same rule the script's
/// `Compare-Versions` applies.
pub(crate) fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    fn core(value: &str) -> Vec<u64> {
        let trimmed = value.trim();
        let without_prefix = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let head = match without_prefix.find(['-', '+']) {
            Some(index) => &without_prefix[..index],
            None => without_prefix,
        };
        head.split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    }

    let left = core(left);
    let right = core(right);
    for index in 0..left.len().max(right.len()) {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        match l.cmp(&r) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

fn resolve_updater_script() -> Result<PathBuf, UpdateLaunchError> {
    let exe_path = std::env::current_exe().map_err(|error| {
        UpdateLaunchError::EnvironmentUnavailable(format!(
            "could not resolve current executable path: {error}"
        ))
    })?;
    let checked_paths = updater_script_candidates(&exe_path);
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
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    // Let PowerShell and its Inno Setup child activate their own windows.
    // Windows otherwise treats this worker-thread launch as background work.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }

    let mut command = std::process::Command::new("powershell.exe");
    // Never inherit our working directory: if Nex runs from inside its own
    // install tree, the updater child would hold that tree open and its
    // later Move-Item of the install would fail with "in use" — by itself.
    command.current_dir(std::env::temp_dir());
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
    spawn_updater_detached(script_path, channel)
}

/// Spawn the real updater with handle inheritance DISABLED.
///
/// Rust's `Command::spawn()` always passes `bInheritHandles=TRUE`, so a
/// child of Nex inherits every inheritable handle Nex holds. If any of
/// those point into the install tree, the updater's later backup move
/// fails with "in use" *by itself* — surviving Nex's own death, which
/// is exactly the persistent (non-transient) failure this guards
/// against. Raw `CreateProcessW` with `bInheritHandles=FALSE` closes
/// that hole; the short-lived check/capture paths keep using
/// `build_updater_command` (no move happens there).
#[cfg(target_os = "windows")]
fn spawn_updater_detached(
    script_path: &Path,
    channel: UpdateChannel,
) -> Result<(), UpdateLaunchError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, DETACHED_PROCESS, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    // Let PowerShell and its Inno Setup child activate their own windows.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }

    let command_line = format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{}\" -Channel {}",
        script_path.display(),
        channel.as_arg()
    );
    let mut command_line: Vec<u16> =
        std::ffi::OsStr::new(&command_line).encode_wide().chain([0]).collect();
    let working_dir: Vec<u16> = std::env::temp_dir()
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let launched = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW | DETACHED_PROCESS,
            std::ptr::null(),
            working_dir.as_ptr(),
            &raw mut startup,
            &raw mut process_info,
        )
    };
    if launched == 0 {
        let code = unsafe { GetLastError() };
        return Err(UpdateLaunchError::LaunchFailed(format!(
            "failed to launch updater script '{}': CreateProcessW failed with code {code}",
            script_path.display()
        )));
    }
    unsafe {
        CloseHandle(process_info.hProcess);
        CloseHandle(process_info.hThread);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_updater_script(
    script_path: &Path,
    channel: UpdateChannel,
) -> Result<std::process::Output, UpdateLaunchError> {
    let mut command = build_updater_command(script_path, channel);
    // Lets the script detect version skew between this running copy and
    // the registered install before doing anything destructive.
    command.arg("-RunningVersion").arg(env!("CARGO_PKG_VERSION"));
    command.output().map_err(|error| {
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

fn updater_script_candidates(exe_path: &Path) -> Vec<PathBuf> {
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
        compare_versions, newest_release, normalize_version, select_release,
        updater_script_candidates, ReleaseAsset, ReleaseTag, UpdateChannel, UpdateCheck,
        DEV_UPDATER_RELATIVE_PATH, INSTALLED_UPDATER_RELATIVE_PATH,
    };

    fn release(tag: &str, prerelease: bool) -> ReleaseTag {
        ReleaseTag {
            tag_name: tag.to_string(),
            prerelease,
            assets: Vec::new(),
        }
    }

    #[test]
    fn the_newest_release_keeps_its_assets_for_the_engine() {
        // The check only needs a version, but the engine needs the URLs, so the
        // same selection has to carry them through.
        let mut with_assets = release("v3.4.0", false);
        with_assets.assets = vec![ReleaseAsset {
            name: "nex-3.4.0-windows-x64-setup.exe".into(),
            browser_download_url: "https://example.invalid/setup.exe".into(),
        }];
        let releases = vec![release("v3.3.0", false), with_assets];
        let newest = newest_release(&releases, UpdateChannel::Stable).expect("a stable release");
        assert_eq!(newest.tag_name, "v3.4.0");
        assert_eq!(newest.assets.len(), 1);
        assert_eq!(newest.assets[0].name, "nex-3.4.0-windows-x64-setup.exe");
    }

    #[test]
    fn versions_normalize_like_the_update_script() {
        assert_eq!(normalize_version("v3.3.0"), "3.3.0");
        assert_eq!(normalize_version("3.3"), "3.3.0");
        assert_eq!(normalize_version("3.3.0-beta.1"), "3.3.0-beta.1");
        assert_eq!(normalize_version("  v3.10.2  "), "3.10.2");
        assert_eq!(normalize_version(""), "");
    }

    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert_eq!(compare_versions("3.10.0", "3.9.0"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("3.3", "3.3.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("v3.3.0", "3.3.1"), std::cmp::Ordering::Less);
        // A pre-release suffix never outranks its own release.
        assert_eq!(
            compare_versions("3.4.0-beta.2", "3.4.0"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn the_newest_stable_release_wins_and_betas_are_skipped() {
        let releases = vec![
            release("v3.3.0", false),
            release("v3.10.0", false),
            release("v3.11.0-beta.1", true),
            release("v9.9.9", false),
        ];
        // v9.9.9 sorts highest, so it is the one that must not be a beta.
        assert_eq!(
            select_release(&releases, UpdateChannel::Stable, "3.3.0"),
            UpdateCheck::Available {
                version: "9.9.9".into()
            }
        );

        let stable_only = vec![release("v3.3.0", false), release("v3.10.0", false)];
        assert_eq!(
            select_release(&stable_only, UpdateChannel::Stable, "3.3.0"),
            UpdateCheck::Available {
                version: "3.10.0".into()
            }
        );
    }

    #[test]
    fn the_beta_channel_only_offers_betas() {
        let releases = vec![
            release("v3.3.0", false),
            release("v3.11.0-beta.1", true),
            release("v3.10.0-beta", true),
        ];
        assert_eq!(
            select_release(&releases, UpdateChannel::Beta, "3.3.0"),
            UpdateCheck::Available {
                version: "3.11.0-beta.1".into()
            }
        );
    }

    #[test]
    fn a_beta_tag_is_recognized_without_the_prerelease_flag() {
        let releases = vec![release("v3.12.0-beta.2", false)];
        assert_eq!(
            select_release(&releases, UpdateChannel::Stable, "3.3.0"),
            UpdateCheck::Failed {
                reason: "no stable release found".into()
            }
        );
        assert_eq!(
            select_release(&releases, UpdateChannel::Beta, "3.3.0"),
            UpdateCheck::Available {
                version: "3.12.0-beta.2".into()
            }
        );
    }

    #[test]
    fn running_the_newest_release_is_up_to_date_not_a_failed_check() {
        let releases = vec![release("v3.3.0", false)];
        assert_eq!(
            select_release(&releases, UpdateChannel::Stable, "3.3.0"),
            UpdateCheck::UpToDate {
                latest: "3.3.0".into()
            }
        );
        // A running build ahead of the published release is current too.
        assert_eq!(
            select_release(&releases, UpdateChannel::Stable, "3.4.0"),
            UpdateCheck::UpToDate {
                latest: "3.3.0".into()
            }
        );
    }

    // Measurement probe, not a gate: hits the network on purpose.
    #[test]
    #[ignore = "hits the GitHub release API"]
    fn what_the_live_check_answers_and_costs() {
        let started = std::time::Instant::now();
        let stable = super::check_update_available(UpdateChannel::Stable);
        let stable_ms = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let beta = super::check_update_available(UpdateChannel::Beta);
        let beta_ms = started.elapsed().as_millis();
        println!("LIVE stable: {stable:?} in {stable_ms} ms");
        println!("LIVE beta:   {beta:?} in {beta_ms} ms");
    }

    #[test]
    fn an_unreadable_release_list_is_a_failed_check_not_up_to_date() {
        // The defect this replaces: an empty or unparseable answer used to be
        // indistinguishable from "you are current".
        assert_eq!(
            select_release(&[], UpdateChannel::Stable, "3.3.0"),
            UpdateCheck::Failed {
                reason: "no stable release found".into()
            }
        );
        let untagged = vec![release("", false)];
        assert!(matches!(
            select_release(&untagged, UpdateChannel::Stable, "3.3.0"),
            UpdateCheck::Failed { .. }
        ));
    }

    #[test]
    fn updater_candidates_prefer_installed_layout_before_repo_fallback() {
        let root =
            std::env::temp_dir().join(format!("nex-updater-installed-{}", std::process::id()));
        let exe_path = root.join("bin/Nex.exe");
        let candidates = updater_script_candidates(&exe_path);

        assert_eq!(candidates[0], root.join(INSTALLED_UPDATER_RELATIVE_PATH));
        assert_eq!(candidates[1], root.join("update-nex.ps1"));
    }

    #[test]
    fn updater_candidates_include_repo_style_script_lookup() {
        let root = std::env::temp_dir().join(format!("nex-updater-repo-{}", std::process::id()));
        let repo = root.join("repo");
        let exe_path = repo.join("target/debug/Nex.exe");
        let candidates = updater_script_candidates(&exe_path);

        assert!(candidates
            .iter()
            .any(|candidate| candidate == &repo.join(DEV_UPDATER_RELATIVE_PATH)));
    }

    #[test]
    fn updater_candidates_are_deduplicated() {
        let repo = std::env::temp_dir().join(format!("nex-updater-dedupe-{}", std::process::id()));
        let exe_path = repo.join("target/debug/Nex.exe");
        let candidates = updater_script_candidates(&exe_path);
        let unique = candidates.iter().collect::<std::collections::BTreeSet<_>>();

        assert_eq!(candidates.len(), unique.len());
    }
}
