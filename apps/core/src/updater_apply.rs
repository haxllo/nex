//! The apply half of the native updater: fetch, verify, swap, or put back.
//!
//! Called only when an engine switch asks for it — the script is still the
//! default. Nothing here decides *which* install to touch (that is
//! `update_install`) or *what* safe means for a manifest (that is
//! `updater_engine`); this module performs the steps in an order that keeps the
//! user's install recoverable at every point.
//!
//! Concretely, the guarantees are:
//!
//! * Nothing is stopped or moved until the installer is downloaded **and** its
//!   checksum matched the manifest.
//! * The install tree is renamed aside before the installer runs, and only
//!   deleted after the new build has answered `--ensure-config` and
//!   `--sync-startup`. A failure at any later step moves the old tree back.
//! * Every step is appended to one log file, and a failure names its stage, so
//!   "the update did nothing" can be answered from the log instead of guessed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::updater_engine::{
    backup_directory, checksum_matches, installer_arguments, installer_log_path, verify_manifest,
    InstallScope, ReleaseManifest, Stage, UpdateError,
};
pub use super::updater_engine::UpdateOutcome;

/// How long a silent installer may run before the attempt is abandoned and
/// rolled back. The installer itself finishes in seconds; anything near this
/// is a hung child, not a slow one.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long the runtime gets to exit before it is killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Attempts for the backup move, spaced by `BACKUP_RETRY_DELAY`. Antivirus
/// scans and dying WebView2 children are the usual blockers.
const BACKUP_ATTEMPTS: u32 = 6;
const BACKUP_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Short-lived helper invocations (`--quit`, `--ensure-config`).
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const RUNTIME_IMAGES: [&str; 5] = [
    "NexUIProcess.exe",
    "NexIndexerProcess.exe",
    "NexHelper.exe",
    "nex-core.exe",
    "swiftfind-core.exe",
];

/// Run one update, end to end.
///
/// `keep_backup` leaves the previous install on disk after a successful update
/// so a bad build can be put back by hand.
/// How one update run should behave.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateOptions {
    /// Keep the previous install on disk after a successful update.
    pub keep_backup: bool,
    /// Install even when the installed version is already current.
    ///
    /// This exists so the engine can be exercised deliberately on a machine
    /// that has nothing newer to install: a forced run reinstalls the build that
    /// is already there, which is the safest way to test the destructive path,
    /// because the version that comes back is known-good.
    pub force: bool,
}

#[cfg(target_os = "windows")]
pub fn run_update(
    channel: crate::updater::UpdateChannel,
    options: UpdateOptions,
) -> Result<UpdateOutcome, UpdateError> {
    let facts = crate::update_install::collect_install_facts();
    let target = crate::update_install::resolve_install_target(&facts);
    let scope = InstallScope {
        // Scope describes where the install lives. Elevation only says whether
        // writing there needs administrator rights; the two never decide each
        // other, which is how a per-user install was once registered machine
        // wide.
        all_users: target.needs_elevation,
        elevate: target.needs_elevation,
    };

    let release = resolve_release(channel)?;
    let stamp = timestamp_stamp();
    let cache_root = update_cache_root();
    let work_dir = super::updater_engine::work_directory(&cache_root, &release.artifact, &stamp);
    std::fs::create_dir_all(&work_dir).map_err(|error| {
        UpdateError::new(
            Stage::Download,
            format!("could not create '{}': {error}", work_dir.display()),
        )
    })?;
    let log_path = work_dir.join("updater.log");
    let mut log = RunLog::new(log_path.clone());

    // The script's two pre-flight guards, kept because both prevent destructive
    // work with no upside.
    //
    // A copy older than the installed one (an orphan left beside a real
    // install) must not "update" that install: the user is looking at a
    // different build, and the fix is to restart from the installed copy.
    if let Some(installed) = crate::update_install::installed_version(&target.root) {
        let running = env!("CARGO_PKG_VERSION");
        if crate::updater::compare_versions(running, &installed) == std::cmp::Ordering::Less {
            return Err(UpdateError::new(
                Stage::Check,
                format!(
                    "this copy is v{running} but v{installed} is installed at {}; restart Nex from the installed location instead of updating",
                    target.root.display()
                ),
            ));
        }
        if !options.force
            && crate::updater::compare_versions(&installed, &release.version)
                >= std::cmp::Ordering::Equal
        {
            log.step(&format!("already up to date (installed {installed})"));
            return Ok(UpdateOutcome::AlreadyCurrent { version: installed });
        }
        if options.force {
            log.step(&format!(
                "forced: reinstalling {} over installed {installed}",
                release.version
            ));
        }
    }
    log.step(&format!(
        "channel={} target={} version={} source={} elevate={}",
        channel.as_arg(),
        target.root.display(),
        release.version,
        target.source.as_str(),
        scope.elevate
    ));

    // --- download + verify, before anything is stopped -------------------
    let manifest_path = work_dir.join("manifest.json");
    let setup_path = work_dir.join("setup.exe");
    log.step(&format!("[1/5] downloading {}", release.manifest_url));
    download(&release.manifest_url, &manifest_path)
        .map_err(|message| UpdateError::new(Stage::Download, message))?;
    log.step(&format!("[1/5] downloading {}", release.setup_url));
    download(&release.setup_url, &setup_path)
        .map_err(|message| UpdateError::new(Stage::Download, message))?;

    log.step("[2/5] verifying manifest and installer");
    let manifest = read_manifest(&manifest_path)?;
    verify_manifest(
        &manifest,
        &release.version,
        &release.artifact,
        channel.as_arg(),
    )
    .map_err(|message| UpdateError::new(Stage::Verify, message))?;
    let actual = sha256_file(&setup_path)
        .map_err(|message| UpdateError::new(Stage::Verify, message))?;
    if !checksum_matches(&manifest.setup_sha256, &actual) {
        return Err(UpdateError::new(
            Stage::Verify,
            format!(
                "installer checksum mismatch: manifest says {}, file is {actual}",
                manifest.setup_sha256
            ),
        ));
    }
    log.step(&format!("[2/5] installer verified ({actual})"));

    let backup_dir = backup_directory(&cache_root, &stamp);

    // From here on, a failure must restore what was there before.
    let mut backed_up = false;
    let result = (|| -> Result<(), UpdateError> {
        log.step("[3/5] stopping the runtime");
        stop_runtime()
            .map_err(|message| UpdateError::new(Stage::Stop, message))?;

        log.step("[3/5] moving the current install aside");
        move_tree(&target.root, &backup_dir).map_err(|message| {
            UpdateError::new(Stage::Backup, message)
        })?;
        backed_up = true;
        log.step(&format!("[3/5] backup at {}", backup_dir.display()));

        log.step("[4/5] running the installer");
        let args = installer_arguments(
            &installer_log_path(&work_dir),
            &target.root,
            scope,
        );
        let code = run_installer(&setup_path, &args, scope.elevate)
            .map_err(|message| UpdateError::new(Stage::Install, message))?;
        if code != 0 {
            return Err(UpdateError::new(
                Stage::Install,
                format!("installer exited with code {code}"),
            ));
        }

        log.step("[5/5] confirming the new build");
        let new_exe = crate::update_install::runtime_executable(&target.root);
        if !new_exe.is_file() {
            return Err(UpdateError::new(
                Stage::Confirm,
                format!("no runtime at '{}' after the install", new_exe.display()),
            ));
        }
        for flag in ["--ensure-config", "--sync-startup"] {
            if let Err(message) = run_helper(&new_exe, flag, HELPER_TIMEOUT) {
                // The script ignored these exit codes; a failure here means the
                // new build cannot start, which is worth rolling back for.
                return Err(UpdateError::new(Stage::Confirm, message));
            }
        }

        log.step("[5/5] restarting");
        start_detached(&new_exe).map_err(|message| UpdateError::new(Stage::Restart, message))?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            if backed_up && !options.keep_backup {
                if let Err(error) = remove_tree(&backup_dir) {
                    log.step(&format!(
                        "warning: could not remove backup '{}': {error}",
                        backup_dir.display()
                    ));
                } else {
                    backed_up = false;
                }
            }
            log.step(&format!("update complete: {}", release.version));
            Ok(UpdateOutcome::Updated {
                version: release.version,
                install_root: target.root,
                backup: backed_up.then_some(backup_dir),
            })
        }
        Err(error) => {
            log.step(&format!("update failed at {}: {}", error.stage.as_str(), error.message));
            if backed_up {
                log.step("rolling back");
                match rollback(&target.root, &backup_dir) {
                    Ok(()) => log.step("rollback complete: previous install restored"),
                    Err(message) => log.step(&format!(
                        "ROLLBACK FAILED: {message} (backup kept at {})",
                        backup_dir.display()
                    )),
                }
            } else {
                log.step("nothing had moved; the previous install was never touched");
            }
            Err(error)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn run_update(
    _channel: crate::updater::UpdateChannel,
    _options: UpdateOptions,
) -> Result<UpdateOutcome, UpdateError> {
    Err(UpdateError::new(
        Stage::Check,
        "updating is only available on Windows",
    ))
}

/// The release being installed, with the two asset URLs the install needs.
struct ResolvedRelease {
    version: String,
    artifact: String,
    setup_url: String,
    manifest_url: String,
}

fn resolve_release(
    channel: crate::updater::UpdateChannel,
) -> Result<ResolvedRelease, UpdateError> {
    let releases = crate::updater::fetch_releases()
        .map_err(|message| UpdateError::new(Stage::Check, message))?;
    let release = crate::updater::newest_release(&releases, channel).ok_or_else(|| {
        UpdateError::new(
            Stage::Check,
            format!("no {} release found", channel.as_arg()),
        )
    })?;

    let version = crate::updater::normalize_version(&release.tag_name);
    let artifact = format!("nex-{version}-windows-x64");
    let (setup_name, manifest_name) = crate::update_install::release_asset_names(&release.tag_name);

    let url_for = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
            .ok_or_else(|| {
                UpdateError::new(
                    Stage::Check,
                    format!("release {} has no asset '{name}'", release.tag_name),
                )
            })
    };

    Ok(ResolvedRelease {
        version,
        artifact,
        setup_url: url_for(&setup_name)?,
        manifest_url: url_for(&manifest_name)?,
    })
}

/// Append-only log for one update attempt. Every step goes to the file first,
/// because the app it is updating may already be gone by the time a reader
/// looks — and to `logging` so it also lands in the normal log.
struct RunLog {
    path: PathBuf,
}

impl RunLog {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn step(&mut self, message: &str) {
        let line = format!("{} {}", timestamp_line(), message);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
        crate::logging::info(&format!("[nex] updater: {message}"));
    }
}

// ---------------------------------------------------------------------------
// Download, hashing and manifest parsing
// ---------------------------------------------------------------------------

fn download(url: &str, destination: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build();
    let response = agent
        .get(url)
        .set("User-Agent", "Nex-Updater")
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|error| format!("could not download '{url}': {error}"))?;

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(destination)
        .map_err(|error| format!("could not create '{}': {error}", destination.display()))?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|error| format!("could not write '{}': {error}", destination.display()))?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash '{}': {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_manifest(path: &Path) -> Result<ReleaseManifest, UpdateError> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        UpdateError::new(
            Stage::Verify,
            format!("could not read '{}': {error}", path.display()),
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        UpdateError::new(Stage::Verify, format!("manifest is not JSON: {error}"))
    })?;

    let text = |field: &str| {
        value
            .get(field)
            .and_then(|field| field.as_str())
            .map(str::to_string)
    };
    Ok(ReleaseManifest {
        artifact: text("artifact")
            .ok_or_else(|| UpdateError::new(Stage::Verify, "manifest is missing 'artifact'"))?,
        version: text("version")
            .ok_or_else(|| UpdateError::new(Stage::Verify, "manifest is missing 'version'"))?,
        channel: text("channel"),
        setup_sha256: value
            .get("artifacts")
            .and_then(|artifacts| artifacts.get("setup"))
            .and_then(|setup| setup.get("sha256"))
            .and_then(|sha| sha.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

pub(crate) fn update_cache_root() -> PathBuf {
    match std::env::var("LOCALAPPDATA") {
        Ok(local_app_data) if !local_app_data.trim().is_empty() => {
            PathBuf::from(local_app_data).join("Nex").join("updates")
        }
        _ => std::env::temp_dir().join("Nex").join("updates"),
    }
}

// ---------------------------------------------------------------------------
// File tree moves
// ---------------------------------------------------------------------------

/// Move a directory, retrying while something still holds a file open.
///
/// `rename` is the fast path and atomic on one volume. When the install and the
/// backup cache sit on different volumes (an install on `C:` with `LOCALAPPDATA`
/// on `D:`, or the reverse) a rename cannot work at all, so the fallback copies
/// and then deletes — the two steps the script's `Move-Item` performs.
fn move_tree(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Err(format!("'{}' does not exist", from.display()));
    }
    if to.exists() {
        return Err(format!("'{}' already exists", to.display()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create '{}': {error}", parent.display()))?;
    }

    let mut last_error = String::new();
    for attempt in 1..=BACKUP_ATTEMPTS {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error.to_string();
                // Cross-volume rename cannot succeed no matter how often it is
                // retried, so copy instead.
                if is_cross_device(&error) {
                    copy_tree(from, to)?;
                    remove_tree(from)?;
                    return Ok(());
                }
                if attempt < BACKUP_ATTEMPTS {
                    crate::logging::warn(&format!(
                        "[nex] updater: move blocked ({attempt}/{BACKUP_ATTEMPTS}): {error}"
                    ));
                    std::thread::sleep(BACKUP_RETRY_DELAY);
                }
            }
        }
    }
    Err(format!(
        "could not move '{}' to '{}': {last_error}",
        from.display(),
        to.display()
    ))
}

/// `ERROR_NOT_SAME_DEVICE` is 17. `ErrorKind::CrossesDevices` only exists on
/// recent toolchains, so the raw code is checked as well.
fn is_cross_device(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(17)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to)
        .map_err(|error| format!("could not create '{}': {error}", to.display()))?;
    let entries = std::fs::read_dir(from)
        .map_err(|error| format!("could not read '{}': {error}", from.display()))?;
    for entry in entries.flatten() {
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_tree(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination).map_err(|error| {
                format!(
                    "could not copy '{}' to '{}': {error}",
                    source.display(),
                    destination.display()
                )
            })?;
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove '{}': {error}", path.display())),
    }
}

fn rollback(install_root: &Path, backup: &Path) -> Result<(), String> {
    let _ = stop_runtime();
    remove_tree(install_root)?;
    move_tree(backup, install_root)?;
    let restored = crate::update_install::runtime_executable(install_root);
    if restored.is_file() {
        let _ = start_detached(&restored);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    let mut time: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut time) };
    (
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
    )
}

#[cfg(not(target_os = "windows"))]
fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    (1970, 1, 1, 0, 0, 0)
}

/// `20261123-120000` — unique per attempt without a date library.
pub(crate) fn timestamp_stamp() -> String {
    let (year, month, day, hour, minute, second) = local_time();
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn timestamp_line() -> String {
    let (year, month, day, hour, minute, second) = local_time();
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

// ---------------------------------------------------------------------------
// Processes: stop, install, restart
// ---------------------------------------------------------------------------

/// Stop everything that holds the install tree.
///
/// Order matters and is the script's, for reasons learned the hard way:
///
/// 1. The elevated helper cannot be killed by a medium-integrity `taskkill`,
///    so it is ended through its own scheduled task first. Skipping this leaves
///    `bin\NexHelper.exe` locked and every later file operation fails *after*
///    the app is already dead.
/// 2. A graceful `--quit` lets the UI flush and stop its own children.
/// 3. `taskkill` without `/T`: the updater may be a child of the process being
///    stopped, and killing the tree would kill the updater itself.
/// 4. Verify they are gone. Proceeding while a process still holds files is
///    exactly how an install ends up half-moved with nothing to roll back to.
#[cfg(target_os = "windows")]
fn stop_runtime() -> Result<(), String> {
    use std::time::Instant;

    let _ = run_captured("schtasks", &["/end", "/tn", "NexHelperV2"]);

    // Never ask an installed `NexUIProcess.exe --quit` to do this.
    //
    // `--quit` stops *every* Nex instance it can find, and it cannot tell that
    // this process is the updater rather than a stray copy: measured
    // 2026-11-24, the forced run died at "[3/5] stopping the runtime" because
    // the installed binary force-terminated its caller. A separate
    // `powershell.exe` never had that problem, so the script never needed to
    // know.
    //
    // What remains is the same mechanics the quit path uses, done directly:
    // a graceful close per PID (which is WM_CLOSE, so the UI saves what it
    // saves), a short wait, then a forced kill of whatever is left. All by PID,
    // and never this process.
    let own_pid = std::process::id();

    for (pid, _name) in others_alive(&running_runtime_processes(), own_pid) {
        let pid = pid.to_string();
        let _ = run_captured("taskkill", &["/PID", &pid]);
    }

    let graceful_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < graceful_deadline {
        if others_alive(&running_runtime_processes(), own_pid).is_empty() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    for (pid, _name) in others_alive(&running_runtime_processes(), own_pid) {
        let pid = pid.to_string();
        let _ = run_captured("taskkill", &["/PID", &pid, "/F"]);
    }

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        let alive: Vec<String> = others_alive(&running_runtime_processes(), own_pid)
            .into_iter()
            .map(|(pid, name)| format!("{name} [{pid}]"))
            .collect();
        if alive.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "could not stop {}; aborting before anything is moved",
                alive.join(", ")
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(not(target_os = "windows"))]
fn stop_runtime() -> Result<(), String> {
    Err("stopping the runtime is only available on Windows".into())
}

/// Run a command hidden and ignore its exit code, for the shell tools whose own
/// failure modes are already covered by the verification that follows.
#[cfg(target_os = "windows")]
fn run_captured(program: &str, args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let status = std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|error| format!("could not run '{program}': {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("'{program}' exited with {status}"))
    }
}

/// The Nex processes still running that are *not* this one.
///
/// Split out because it is the difference between a working self-update and a
/// dead app: the engine runs inside `NexUIProcess.exe`, so counting itself as
/// something to stop would either kill the updater after the install tree had
/// already moved, or make the wait loop time out on a process that is not
/// going away.
fn others_alive(processes: &[(u32, String)], own_pid: u32) -> Vec<(u32, String)> {
    processes
        .iter()
        .filter(|(pid, _)| *pid != own_pid)
        .cloned()
        .collect()
}

/// Which of the Nex images are running right now, with their process ids.
#[cfg(target_os = "windows")]
fn running_runtime_processes() -> Vec<(u32, String)> {
    running_processes()
        .into_iter()
        .filter(|(_, name)| {
            RUNTIME_IMAGES
                .iter()
                .any(|image| image.eq_ignore_ascii_case(name))
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn running_processes() -> Vec<(u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked and closed on every path; the
    // entry structure is passed exactly as the API expects.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
        while ok {
            let name = String::from_utf16_lossy(
                &entry
                    .szExeFile
                    .iter()
                    .copied()
                    .take_while(|unit| *unit != 0)
                    .collect::<Vec<u16>>(),
            );
            found.push((entry.th32ProcessID, name));
            ok = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    found
}

/// Run the installer and return its exit code.
///
/// Elevated installs go through `ShellExecuteExW` with the `runas` verb, which
/// is what raises the UAC prompt; a per-user install is started directly. Both
/// paths wait for the child and report the code, because an installer that
/// fails silently is worse than a visible error.
#[cfg(target_os = "windows")]
fn run_installer(setup: &Path, args: &[String], elevate: bool) -> Result<u32, String> {
    if elevate {
        return run_elevated(setup, &args.join(" "));
    }
    let mut command_line = quote_argument(&setup.display().to_string());
    for arg in args {
        command_line.push(' ');
        command_line.push_str(arg);
    }
    run_and_wait(&command_line, INSTALL_TIMEOUT)
}

#[cfg(target_os = "windows")]
fn run_elevated(program: &Path, arguments: &str) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let verb = wide("runas");
    let file = wide(&program.display().to_string());
    let parameters = wide(arguments);

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_SHOWNORMAL;

    // SAFETY: the structure is fully initialised above and outlives the call.
    let launched = unsafe { ShellExecuteExW(&mut info) };
    if launched == 0 {
        let error = std::io::Error::last_os_error();
        // Cancelling the UAC prompt is a user decision, not a bug: it is
        // reported plainly so the rollback can put everything back.
        return Err(if error.raw_os_error() == Some(1223) {
            "the elevation prompt was cancelled".to_string()
        } else {
            format!("could not start the installer elevated: {error}")
        });
    }
    if info.hProcess.is_null() {
        return Err("the installer process could not be waited on".into());
    }

    let code = wait_for_process(info.hProcess, INSTALL_TIMEOUT);
    unsafe { CloseHandle(info.hProcess) };
    code
}

/// Wait for a process handle with a deadline and return its exit code.
#[cfg(target_os = "windows")]
fn wait_for_process(handle: windows_sys::Win32::Foundation::HANDLE, timeout: Duration) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

    let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
    // SAFETY: `handle` is a live process handle owned by the caller.
    let waited = unsafe { WaitForSingleObject(handle, millis) };
    if waited == WAIT_TIMEOUT {
        return Err(format!(
            "the installer did not finish within {} minutes",
            timeout.as_secs() / 60
        ));
    }
    if waited != WAIT_OBJECT_0 {
        return Err(format!("waiting for the installer failed ({waited})"));
    }
    let mut code: u32 = 0;
    if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
        return Err(format!(
            "could not read the installer's exit code: {}",
            std::io::Error::last_os_error()
        ));
    }
    let _ = CloseHandle; // imported for symmetry with `run_elevated`
    Ok(code)
}

/// Start a command line, wait for it, and return its exit code.
#[cfg(target_os = "windows")]
fn run_and_wait(command_line: &str, timeout: Duration) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let mut buffer: Vec<u16> = command_line.encode_utf16().chain(std::iter::once(0)).collect();
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // SAFETY: the command line is NUL-terminated and mutable for the duration
    // of the call, and both returned handles are closed below.
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(),
            buffer.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(format!(
            "could not start '{}': {}",
            command_line,
            std::io::Error::last_os_error()
        ));
    }

    let code = wait_for_process(process.hProcess, timeout);
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    code
}

/// Run one of Nex's own short-lived switches (`--quit`, `--ensure-config`,
/// `--sync-startup`) and treat a non-zero exit as a failure.
#[cfg(target_os = "windows")]
fn run_helper(exe: &Path, flag: &str, timeout: Duration) -> Result<(), String> {
    let command_line = format!("{} {flag}", quote_argument(&exe.display().to_string()));
    match run_and_wait(&command_line, timeout) {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("'{flag}' exited with code {code}")),
        Err(message) => Err(message),
    }
}

/// Start Nex detached, with no console window.
#[cfg(target_os = "windows")]
fn start_detached(exe: &Path) -> Result<(), String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let command_line = format!("{} --foreground", quote_argument(&exe.display().to_string()));
    let mut buffer: Vec<u16> = command_line.encode_utf16().chain(std::iter::once(0)).collect();
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // SAFETY: as in `run_and_wait`; the child is intentionally not waited on.
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(),
            buffer.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(format!(
            "could not start '{}': {}",
            exe.display(),
            std::io::Error::last_os_error()
        ));
    }
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn start_detached(_exe: &Path) -> Result<(), String> {
    Err("starting the runtime is only available on Windows".into())
}

#[cfg(not(target_os = "windows"))]
fn run_helper(_exe: &Path, _flag: &str, _timeout: Duration) -> Result<(), String> {
    Err("running the runtime is only available on Windows".into())
}

#[cfg(not(target_os = "windows"))]
fn run_installer(_setup: &Path, _args: &[String], _elevate: bool) -> Result<u32, String> {
    Err("installing is only available on Windows".into())
}

/// Quote one argument for a Windows command line, following the `CommandLineToArgvW`
/// rules: wrap in quotes, and double any backslash that precedes a quote or the
/// closing quote.
fn quote_argument(value: &str) -> String {
    if !value.is_empty() && !value.contains([' ', '\t', '"']) {
        return value.to_string();
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for ch in value.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                quoted.push('\\');
            }
            '"' => {
                for _ in 0..=backslashes {
                    quoted.push('\\');
                }
                quoted.push('"');
                backslashes = 0;
            }
            other => {
                backslashes = 0;
                quoted.push(other);
            }
        }
    }
    for _ in 0..backslashes {
        quoted.push('\\');
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{others_alive, quote_argument, UpdateOptions};

    #[test]
    fn stopping_never_counts_the_updater_itself() {
        // A self-update runs inside NexUIProcess.exe. Its own pid must not be
        // in the stop set, or the run kills itself after the install tree has
        // already moved.
        let processes = vec![
            (100u32, "NexUIProcess.exe".to_string()),
            (200u32, "NexIndexerProcess.exe".to_string()),
            (300u32, "NexUIProcess.exe".to_string()),
        ];
        assert_eq!(
            others_alive(&processes, 300),
            vec![
                (100u32, "NexUIProcess.exe".to_string()),
                (200u32, "NexIndexerProcess.exe".to_string()),
            ]
        );
        // With only itself running there is nothing to stop, so the wait loop
        // returns immediately instead of timing out.
        assert!(others_alive(&[(300u32, "NexUIProcess.exe".to_string())], 300).is_empty());
    }

    #[test]
    fn force_is_off_unless_asked_for() {
        // A forced run reinstalls the published version over itself, so it must
        // never be the default for a path that forgets to say otherwise.
        assert!(!UpdateOptions::default().force);
        assert!(!UpdateOptions::default().keep_backup);
    }

    #[test]
    fn arguments_are_quoted_like_the_windows_runtime_expects() {
        assert_eq!(quote_argument("C:\\Nex\\setup.exe"), "C:\\Nex\\setup.exe");
        assert_eq!(
            quote_argument("C:\\Program Files\\Nex\\setup.exe"),
            "\"C:\\Program Files\\Nex\\setup.exe\""
        );
        // Unquoted, a trailing backslash needs nothing: there is no closing
        // quote for it to escape.
        assert_eq!(quote_argument("C:\\Nex\\"), "C:\\Nex\\");
        // Quoted, it does: the backslash before the closing quote must double,
        // or CommandLineToArgvW reads the quote as an escape.
        assert_eq!(
            quote_argument("C:\\Program Files\\Nex\\"),
            "\"C:\\Program Files\\Nex\\\\\""
        );
        assert_eq!(quote_argument(""), "\"\"");
    }
}
