use std::fmt::{Display, Formatter};
use std::path::Path;

#[derive(Debug)]
pub enum StartupError {
    Io(std::io::Error),
    Command(String),
    UnsupportedPlatform,
}

impl Display for StartupError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Command(error) => write!(f, "command error: {error}"),
            Self::UnsupportedPlatform => write!(f, "unsupported platform"),
        }
    }
}

impl std::error::Error for StartupError {}

impl From<std::io::Error> for StartupError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[cfg(target_os = "windows")]
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const UNINSTALL_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}_is1";
#[cfg(target_os = "windows")]
const VALUE_NAME: &str = "Nex";

/// Windows keeps an enabled/disabled flag beside every Run entry, and the login
/// shell honours it over the presence of the Run value itself. Nex used to touch
/// only the Run value, so switching launch-at-startup off and on again restored the
/// value while the flag still said "disabled" and the login skipped it.
#[cfg(target_os = "windows")]
const STARTUP_APPROVED_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The 12-byte blob Task Manager writes for a Run entry: first byte 0x02 enabled,
/// 0x03 disabled, the rest a timestamp it tolerates as zero.
pub(crate) fn startup_approved_bytes(enabled: bool) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    bytes[0] = if enabled { 0x02 } else { 0x03 };
    bytes
}

/// Keep the flag in step with the Run value. Best effort: a machine that refuses
/// the write still gets the Run value, which is what older builds relied on.
#[cfg(target_os = "windows")]
fn set_startup_approved(enabled: bool) {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_BINARY,
    };

    let subkey = to_wide(STARTUP_APPROVED_SUBKEY);
    let value_name = to_wide(VALUE_NAME);
    let blob = startup_approved_bytes(enabled);
    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return;
    }
    let status = unsafe {
        RegSetValueExW(
            key,
            value_name.as_ptr(),
            0,
            REG_BINARY,
            blob.as_ptr(),
            blob.len() as u32,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if status != ERROR_SUCCESS {
        crate::logging::warn(&format!(
            "[nex] startup_approved write failed status={status}; Windows may still skip Nex at login"
        ));
    }
}
#[cfg(target_os = "windows")]
const LEGACY_VALUE_NAME: &str = "SwiftFind";
const STARTUP_ARG: &str = "--background";

pub fn startup_command_for_executable(executable_path: &Path) -> Result<String, StartupError> {
    if executable_path.as_os_str().is_empty() {
        return Err(StartupError::Command(
            "executable path is empty".to_string(),
        ));
    }
    if !executable_path.exists() {
        return Err(StartupError::Command(format!(
            "executable path does not exist: {}",
            executable_path.display()
        )));
    }
    if !executable_path.is_file() {
        return Err(StartupError::Command(format!(
            "executable path is not a file: {}",
            executable_path.display()
        )));
    }

    Ok(format!(
        "\"{}\" {}",
        executable_path.to_string_lossy(),
        STARTUP_ARG
    ))
}

#[cfg(target_os = "windows")]
pub fn is_enabled() -> Result<bool, StartupError> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    };

    let subkey = to_wide(RUN_SUBKEY);
    let value_name = to_wide(VALUE_NAME);
    let legacy_value_name = to_wide(LEGACY_VALUE_NAME);
    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };

    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status != ERROR_SUCCESS {
        return Err(registry_error("query run key", status));
    }

    let query_value_exists = |value_name: &[u16]| {
        let mut value_type = 0_u32;
        let mut size = 0_u32;
        unsafe {
            RegQueryValueExW(
                key,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut value_type,
                std::ptr::null_mut(),
                &mut size,
            )
        }
    };
    let status = query_value_exists(&value_name);
    let legacy_status = if status == ERROR_FILE_NOT_FOUND {
        query_value_exists(&legacy_value_name)
    } else {
        ERROR_FILE_NOT_FOUND
    };
    unsafe {
        RegCloseKey(key);
    }

    if status == ERROR_FILE_NOT_FOUND && legacy_status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status != ERROR_SUCCESS {
        if status != ERROR_FILE_NOT_FOUND {
            return Err(registry_error("query run value", status));
        }
    }
    if legacy_status != ERROR_SUCCESS && legacy_status != ERROR_FILE_NOT_FOUND {
        return Err(registry_error("query legacy run value", legacy_status));
    }

    Ok(true)
}

#[cfg(target_os = "windows")]
pub fn set_enabled(enabled: bool, executable_path: &Path) -> Result<(), StartupError> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
    };

    let subkey = to_wide(RUN_SUBKEY);
    let value_name = to_wide(VALUE_NAME);
    let legacy_value_name = to_wide(LEGACY_VALUE_NAME);

    if enabled {
        let value = startup_command_for_executable(executable_path)?;
        let mut key = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error("create/open run key", status));
        }

        let value_wide = to_wide(&value);
        let status = unsafe {
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                value_wide.as_ptr() as *const u8,
                (value_wide.len() * std::mem::size_of::<u16>()) as u32,
            )
        };
        unsafe {
            let _ = RegDeleteValueW(key, legacy_value_name.as_ptr());
            RegCloseKey(key);
        }

        if status != ERROR_SUCCESS {
            return Err(registry_error("set run value", status));
        }
        set_startup_approved(true);
        return Ok(());
    }

    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status != ERROR_SUCCESS {
        return Err(registry_error("open run key for delete", status));
    }

    let status = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
    let legacy_status = unsafe { RegDeleteValueW(key, legacy_value_name.as_ptr()) };
    unsafe {
        RegCloseKey(key);
    }
    if (status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND)
        && (legacy_status == ERROR_SUCCESS || legacy_status == ERROR_FILE_NOT_FOUND)
    {
        set_startup_approved(false);
        return Ok(());
    }
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
        return Err(registry_error("delete run value", status));
    }

    Err(registry_error("delete legacy run value", legacy_status))
}

/// Install location registered by the Nex installer (HKCU first, then
/// HKLM). `None` when no registered install exists (portable/dev runs).
#[cfg(target_os = "windows")]
pub fn registered_install_location() -> Option<std::path::PathBuf> {
    let (hkcu, hklm) = registered_install_locations();
    hkcu.or(hklm)
}

/// Both registrations, kept apart.
///
/// The updater needs the distinction the merged getter throws away: an HKLM
/// entry that is not under Program Files is the stale hybrid an older updater
/// bug left behind, and it must not make the update machine-wide. Reading them
/// together also halves the registry work.
#[cfg(target_os = "windows")]
pub fn registered_install_locations() -> (
    Option<std::path::PathBuf>,
    Option<std::path::PathBuf>,
) {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let subkey = to_wide(UNINSTALL_SUBKEY);
    let value_name = to_wide("InstallLocation");
    let mut found: (Option<std::path::PathBuf>, Option<std::path::PathBuf>) = (None, None);
    for (index, hive) in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().enumerate() {
        let mut buffer = [0u16; 1024];
        let mut buffer_size = (buffer.len() * 2) as u32;
        let mut value_type = 0u32;
        let status = unsafe {
            RegGetValueW(
                hive,
                subkey.as_ptr(),
                value_name.as_ptr(),
                RRF_RT_REG_SZ,
                &mut value_type,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                &mut buffer_size,
            )
        };
        if status != ERROR_SUCCESS || buffer_size < 2 {
            continue;
        }
        let char_count = (buffer_size as usize).saturating_sub(2) / 2;
        let location = String::from_utf16_lossy(&buffer[..char_count]);
        let trimmed = location.trim();
        if !trimmed.is_empty() {
            if index == 0 {
                found.0 = Some(std::path::PathBuf::from(trimmed));
            } else {
                found.1 = Some(std::path::PathBuf::from(trimmed));
            }
        }
    }
    found
}

/// Warn loudly when this process runs from outside the registered install
/// (e.g. a stale copy shadowing the real install). Returns the registered
/// location when one exists and differs, so callers can surface it.
#[cfg(target_os = "windows")]
pub fn install_skew() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let registered = registered_install_location()?;
    let current_exe = std::env::current_exe().ok()?;
    // Installed layout is `<root>\bin\NexUIProcess.exe`.
    let running_root = current_exe.parent()?.parent()?.to_path_buf();
    let normalize = |path: &std::path::Path| {
        path.to_string_lossy().trim().replace('/', "\\").to_ascii_lowercase()
    };
    if normalize(&running_root) != normalize(&registered) {
        return Some((current_exe, registered));
    }
    None
}

#[cfg(not(target_os = "windows"))]
pub fn is_enabled() -> Result<bool, StartupError> {
    Err(StartupError::UnsupportedPlatform)
}

#[cfg(not(target_os = "windows"))]
pub fn set_enabled(_enabled: bool, _executable_path: &Path) -> Result<(), StartupError> {
    Err(StartupError::UnsupportedPlatform)
}

#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn registry_error(action: &str, status: u32) -> StartupError {
    StartupError::Command(format!("{action} failed with code {status}"))
}

#[cfg(test)]
mod startup_approved_tests {
    use super::*;

    /// Windows reads the first byte and ignores the rest; getting it backwards is
    /// what kept a re-enabled Nex from starting at login.
    #[test]
    fn startup_approved_bytes_marks_enabled() {
        assert_eq!(startup_approved_bytes(true)[0], 0x02);
        assert_eq!(startup_approved_bytes(false)[0], 0x03);
        assert_eq!(startup_approved_bytes(true).len(), 12);
        // Only the first byte carries the state.
        assert!(startup_approved_bytes(true)[1..].iter().all(|byte| *byte == 0));
    }
}
