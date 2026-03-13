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
const VALUE_NAME: &str = "Nex";
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
        return Ok(());
    }
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
        return Err(registry_error("delete run value", status));
    }

    Err(registry_error("delete legacy run value", legacy_status))
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
