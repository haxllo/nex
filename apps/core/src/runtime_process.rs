use crate::runtime::{log_info, RuntimeError};
#[cfg(target_os = "windows")]
use crate::windows_overlay::{is_instance_window_present, signal_existing_instance_quit};

// ---------------------------------------------------------------------------
// Runtime executable name constants
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub(crate) const CURRENT_RUNTIME_EXE_NAME: &str = "nex.exe";
#[cfg(target_os = "windows")]
pub(crate) const LEGACY_RUNTIME_EXE_NAMES: &[&str] = &["nex-core.exe", "swiftfind-core.exe"];

#[cfg(target_os = "windows")]
pub(crate) fn runtime_executable_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(CURRENT_RUNTIME_EXE_NAME).chain(LEGACY_RUNTIME_EXE_NAMES.iter().copied())
}

// ---------------------------------------------------------------------------
// Hotkey registration helpers (cross-platform text helpers)
// ---------------------------------------------------------------------------

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn hotkey_registration_recovery_message(
    hotkey: &str,
    config_path: &std::path::Path,
) -> String {
    let suggestions = crate::settings::suggested_hotkey_presets(hotkey, 3);
    if suggestions.is_empty() {
        return format!(
            "Hotkey '{hotkey}' is unavailable. Open {} and choose a different modifier+key combination.",
            config_path.display()
        );
    }

    format!(
        "Hotkey '{hotkey}' is unavailable. Try {}. Edit {} to change it.",
        suggestions.join(", "),
        config_path.display()
    )
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn hotkey_registration_status_text(hotkey: &str) -> String {
    let suggestions = crate::settings::suggested_hotkey_presets(hotkey, 2);
    if suggestions.is_empty() {
        return format!("Hotkey unavailable: {hotkey}. Open config from the tray.");
    }

    format!(
        "Hotkey unavailable: {hotkey}. Try {}.",
        suggestions.join(" or ")
    )
}

// ---------------------------------------------------------------------------
// Updater launcher (cross-platform; fails on non-Windows)
// ---------------------------------------------------------------------------

pub(crate) fn launch_stable_updater() -> Result<std::path::PathBuf, String> {
    let script_path = crate::updater::launch_updater(crate::updater::UpdateChannel::Stable)
        .map_err(|error| error.to_string())?;
    log_info(&format!(
        "[nex] updater_launch channel=stable script={}",
        script_path.display()
    ));
    Ok(script_path)
}

// ---------------------------------------------------------------------------
// Runtime process introspection (Windows only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeProcessState {
    pub(crate) has_overlay_window: bool,
    pub(crate) other_runtime_pids: Vec<u32>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopRuntimeOutcome {
    AlreadyStopped,
    Graceful,
    Forced,
    Failed,
}

#[cfg(target_os = "windows")]
pub(crate) fn inspect_runtime_process_state() -> RuntimeProcessState {
    RuntimeProcessState {
        has_overlay_window: is_instance_window_present(),
        other_runtime_pids: runtime_process_pids_excluding_current().unwrap_or_default(),
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn stop_runtime_instance(
    timeout: std::time::Duration,
) -> Result<StopRuntimeOutcome, RuntimeError> {
    let mut state = inspect_runtime_process_state();
    if !state.has_overlay_window && state.other_runtime_pids.is_empty() {
        return Ok(StopRuntimeOutcome::AlreadyStopped);
    }

    if state.has_overlay_window {
        let _ = signal_existing_instance_quit().map_err(RuntimeError::Overlay)?;
        if wait_until_overlay_window_closed(timeout) {
            state = inspect_runtime_process_state();
            if state.other_runtime_pids.is_empty() {
                return Ok(StopRuntimeOutcome::Graceful);
            }
        }
    }

    let forced = force_terminate_other_runtime_processes()?;
    std::thread::sleep(std::time::Duration::from_millis(250));
    let post = inspect_runtime_process_state();
    if !post.has_overlay_window && post.other_runtime_pids.is_empty() {
        if forced {
            Ok(StopRuntimeOutcome::Forced)
        } else {
            Ok(StopRuntimeOutcome::Graceful)
        }
    } else {
        Ok(StopRuntimeOutcome::Failed)
    }
}

#[cfg(target_os = "windows")]
fn wait_until_overlay_window_closed(timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !is_instance_window_present() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    !is_instance_window_present()
}

#[cfg(target_os = "windows")]
fn force_terminate_other_runtime_processes() -> Result<bool, RuntimeError> {
    let current_pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let mut terminated_any = false;
    for exe_name in runtime_executable_names() {
        let command = format!(
            "taskkill /F /T /FI \"IMAGENAME eq {exe_name}\" /FI \"PID ne {}\" >NUL 2>&1",
            current_pid
        );
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg(command)
            .status()
            .map_err(RuntimeError::Io)?;
        terminated_any |= status.success();
    }
    Ok(terminated_any)
}

#[cfg(target_os = "windows")]
fn runtime_process_pids_excluding_current() -> Result<Vec<u32>, RuntimeError> {
    let current_pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let mut pids = Vec::new();
    for exe_name in runtime_executable_names() {
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg(format!(
                "tasklist /FI \"IMAGENAME eq {exe_name}\" /FO LIST /NH"
            ))
            .output()
            .map_err(RuntimeError::Io)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        pids.extend(parse_tasklist_pid_lines(&stdout));
    }
    pids.retain(|pid| *pid != current_pid);
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_tasklist_pid_lines(content: &str) -> Vec<u32> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.to_ascii_lowercase().starts_with("pid:") {
                return None;
            }
            let value = trimmed.split(':').nth(1)?.trim();
            value.parse::<u32>().ok()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Background process spawning (Windows only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub(crate) fn spawn_background_process() -> Result<(), RuntimeError> {
    use std::os::windows::process::CommandExt;

    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command.arg("--foreground");
    command.env("NEX_SUPPRESS_STDIO", "1");
    command.creation_flags(0x00000008 | 0x00000200 | 0x08000000);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    command.spawn()?;
    log_info("[nex] background process started");
    Ok(())
}

// ---------------------------------------------------------------------------
// Runtime mode label (cross-platform)
// ---------------------------------------------------------------------------

pub(crate) fn runtime_mode() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "windows-hotkey-runtime"
    }

    #[cfg(not(target_os = "windows"))]
    {
        "non-windows-noop"
    }
}

// ---------------------------------------------------------------------------
// Hotkey registration logging (Windows only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub(crate) fn log_registration(registration: &crate::hotkey_runtime::HotkeyRegistration) {
    match registration {
        crate::hotkey_runtime::HotkeyRegistration::Native(id) => {
            log_info(&format!("[nex] hotkey registered native_id={id}"));
        }
        crate::hotkey_runtime::HotkeyRegistration::Noop(label) => {
            log_info(&format!("[nex] hotkey registered noop={label}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Single-instance guard (Windows only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub(crate) struct SingleInstanceGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn acquire_single_instance_guard() -> Result<Option<SingleInstanceGuard>, String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mutex_name = to_wide("Local\\NexRuntimeSingleton");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        return Err(format!("CreateMutexW failed with error {error}"));
    }

    // ERROR_ALREADY_EXISTS
    let error = unsafe { GetLastError() };
    if error == 183 {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Ok(None);
    }

    Ok(Some(SingleInstanceGuard { handle }))
}

#[cfg(target_os = "windows")]
pub(crate) fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
