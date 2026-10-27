#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    EmptyPath,
    MissingPath(PathBuf),
    LaunchFailed { message: String, code: Option<i32> },
    /// Structured launch failure with context for diagnostics
    Structured {
        path: String,
        kind: String,
        reason: String,
        code: Option<i32>,
    },
}

impl Display for LaunchError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPath => write!(f, "empty path"),
            Self::MissingPath(path) => write!(f, "path does not exist: {}", path.display()),
            Self::LaunchFailed { message, code } => {
                if let Some(code) = code {
                    write!(f, "launch failed: {message} (code {code})")
                } else {
                    write!(f, "launch failed: {message}")
                }
            }
            Self::Structured { path, kind, reason, code } => {
                if let Some(code) = code {
                    write!(f, "launch failed [{kind}]: {path} - {reason} (code {code})")
                } else {
                    write!(f, "launch failed [{kind}]: {path} - {reason}")
                }
            }
        }
    }
}

impl std::error::Error for LaunchError {}

pub fn launch_path(path: &str) -> Result<(), LaunchError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(LaunchError::EmptyPath);
    }

    if is_non_filesystem_open_target(trimmed) {
        return launch_open(trimmed);
    }

    let candidate = Path::new(trimmed);
    if !candidate.exists() {
        return Err(LaunchError::MissingPath(candidate.to_path_buf()));
    }

    launch_existing_path(candidate)?;

    Ok(())
}

pub fn launch_open_target(target: &str) -> Result<(), LaunchError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(LaunchError::EmptyPath);
    }
    launch_open(trimmed)
}

pub fn launch_as_admin(target: &str) -> Result<(), LaunchError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(LaunchError::EmptyPath);
    }
    launch_runas(trimmed)
}

#[cfg(target_os = "windows")]
fn launch_existing_path(candidate: &Path) -> Result<(), LaunchError> {
    let target = candidate.to_string_lossy().into_owned();
    launch_open(&target)
}

#[cfg(target_os = "windows")]
fn launch_open(target: &str) -> Result<(), LaunchError> {
    if target.trim().to_ascii_lowercase().starts_with("shell:") {
        return launch_shell_target(target);
    }

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // Safety: AllowSetForegroundWindow(ASFW_ANY) grants the spawned process
    // permission to SetForegroundWindow its own window. Without this, the
    // launched program opens behind nex's overlay because the overlay still
    // owns foreground when ShellExecuteW returns.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }

    let wide_target = to_wide(&target);

    // For directories, ask the shell to *explore* rather than *open*:
    // the default "open" verb goes through the file-association lookup,
    // and folders without a registered association (e.g. dot-prefixed
    // directories like `.codex`, `.config`) trigger an "Open with"
    // dialog. `explore` is the verb Explorer uses to open a folder
    // window and bypasses association lookup.
    let wide_verb_explore: Vec<u16> = "explore".encode_utf16().chain(std::iter::once(0)).collect();
    let verb_ptr = if Path::new(target).is_dir() {
        wide_verb_explore.as_ptr()
    } else {
        std::ptr::null()
    };

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_ptr,
            wide_target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        let kind = if Path::new(target).is_dir() { "directory" } else { "file" };
        return Err(LaunchError::Structured {
            path: target.to_string(),
            kind: kind.to_string(),
            reason: format!("ShellExecuteW returned {}", result),
            code: Some(result as i32),
        });
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn launch_runas(target: &str) -> Result<(), LaunchError> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY, SW_SHOWNORMAL};

    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }

    let wide_target = to_wide(target);
    let wide_verb: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            wide_verb.as_ptr(),
            wide_target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        return Err(LaunchError::Structured {
            path: target.to_string(),
            kind: "elevated".to_string(),
            reason: format!("ShellExecuteW runas returned {}", result),
            code: Some(result as i32),
        });
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn launch_runas(_target: &str) -> Result<(), LaunchError> {
    Err(LaunchError::LaunchFailed {
        message: "runas not available on this platform".into(),
        code: None,
    })
}

/// Windows "Open File Location": open Explorer on the item's containing
/// directory with the item itself selected. Works for files and folders;
/// non-filesystem targets (shell:/ms-settings: URIs, .cpl dialogs) are
/// rejected by the caller.
#[cfg(target_os = "windows")]
pub fn open_file_location(target: &str) -> Result<(), LaunchError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(LaunchError::EmptyPath);
    }
    // Explorer treats a trailing backslash as "select the last fragment"
    // and then fails to select anything; strip it so every path variant
    // (file, folder, drive root) ends without one.
    let clean = trimmed.trim_end_matches('\\');
    if clean.is_empty() {
        return Err(LaunchError::EmptyPath);
    }

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }

    let select_arg = format!("/select,\"{clean}\"");
    let wide_explorer = to_wide("explorer.exe");
    let wide_params = to_wide(&select_arg);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide_explorer.as_ptr(),
            wide_params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        return Err(LaunchError::Structured {
            path: clean.to_string(),
            kind: "location".to_string(),
            reason: format!("ShellExecuteW returned {}", result),
            code: Some(result as i32),
        });
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn open_file_location(target: &str) -> Result<(), LaunchError> {
    let _ = target;
    Err(LaunchError::LaunchFailed {
        message: "open file location not available on this platform".into(),
        code: None,
    })
}

/// Resolve the `shell:AppsFolder\…` start-menu path forms to a real
/// filesystem path by expanding the GUID with SHGetKnownFolderPath. Three
/// alias shapes flow through here:
/// 1. `{KNOWNFOLDER-GUID}\relative\path` — a known-folder alias with a real
///    file tail (Program Files / System32 shortcuts).
/// 2. `C:\…\path` — the drive path embedded right after the prefix
///    (legacy indexing wrote these verbatim behind the alias).
/// 3. A bare app alias (AUMID): UWP packages ("…!App") have no file and
///    return None; desktop-program AUMIDs resolve through the uninstall
///    registry map ("Brave" → brave.exe, Squirrel "Discord…" → install dir).
#[cfg(target_os = "windows")]
pub fn resolve_shell_appsfolder_path(value: &str) -> Option<String> {
    const TOKEN: &str = "shell:appsfolder\\";
    let trimmed = value.trim();
    if !trimmed.to_ascii_lowercase().starts_with(TOKEN) {
        return None;
    }
    let rest = trimmed.get(TOKEN.len()..)?.trim();
    if rest.is_empty() {
        return None;
    }

    // Shape 1: known-folder GUID alias with a relative tail.
    if let Some((first, tail)) = rest.split_once('\\') {
        let segment = first.trim_start_matches('{').trim_end_matches('}');
        if segment.len() == 36 {
            if let (Some(guid), Some(folder)) =
                (parse_guid(segment), parse_guid(segment).and_then(|g| known_folder_path(&g)))
            {
                let base = folder.trim_end_matches('\\');
                if tail.is_empty() {
                    return Some(base.to_string());
                }
                let candidate = format!("{base}\\{tail}");
                // Only hand back paths that actually exist; a missing tail
                // means the alias did not mirror a real file.
                std::fs::metadata(&candidate).ok()?;
                Some(candidate)
            } else {
                // Real GUID-shaped alias that failed to resolve — not an
                // AUMID, don't cross-lookup against the uninstall map.
                None
            }
        } else if looks_like_filesystem_path(rest) {
            // Shape 2: drive path embedded directly after the prefix.
            if std::fs::metadata(rest).is_ok() {
                Some(rest.to_string())
            } else {
                None
            }
        } else {
            // Shape 3: desktop-program AUMID without a backslash. UWP
            // packages ("…!App") are rejected inside the lookup.
            crate::uninstall_registry::app_path_by_aumid(rest)
        }
    } else {
        // No backslash at all: the whole remainder is one alias.
        if looks_like_filesystem_path(rest) && std::fs::metadata(rest).is_ok() {
            return Some(rest.to_string());
        }
        crate::uninstall_registry::app_path_by_aumid(rest)
    }
}

#[cfg(target_os = "windows")]
fn known_folder_path(folder: &windows_sys::core::GUID) -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut out: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &mut out) };
    if hr != 0 || out.is_null() {
        return None;
    }
    unsafe {
        let mut len = 0usize;
        while *out.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(out, len));
        CoTaskMemFree(out.cast());
        Some(text)
    }
}

#[cfg(target_os = "windows")]
fn parse_guid(text: &str) -> Option<windows_sys::core::GUID> {
    if text.len() != 36 {
        return None;
    }
    let compact: String = text.chars().filter(|c| *c != '-').collect();
    if compact.len() != 32 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hex = |from: usize, to: usize| u32::from_str_radix(&compact[from..to], 16).ok();
    let data1 = hex(0, 8)?;
    let data2 = hex(8, 12)? as u16;
    let data3 = hex(12, 16)? as u16;
    let bytes_a = hex(16, 20)?; // data4[0..2]
    let bytes_b = hex(20, 24)?; // data4[2..4]
    let bytes_c = hex(24, 28)?; // data4[4..6]
    let bytes_d = hex(28, 32)?; // data4[6..8]
    Some(windows_sys::core::GUID {
        data1,
        data2,
        data3,
        data4: [
            (bytes_a >> 8) as u8,
            bytes_a as u8,
            (bytes_b >> 8) as u8,
            bytes_b as u8,
            (bytes_c >> 8) as u8,
            bytes_c as u8,
            (bytes_d >> 8) as u8,
            bytes_d as u8,
        ],
    })
}

#[cfg(not(target_os = "windows"))]
pub fn resolve_shell_appsfolder_path(_value: &str) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn launch_shell_target(target: &str) -> Result<(), LaunchError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY;
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
    std::process::Command::new("explorer.exe")
        .arg(target)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|error| LaunchError::Structured {
            path: target.to_string(),
            kind: "shell".to_string(),
            reason: format!("explorer.exe spawn failed: {error}"),
            code: None,
        })?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn launch_existing_path(_candidate: &Path) -> Result<(), LaunchError> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn launch_open(_target: &str) -> Result<(), LaunchError> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn is_non_filesystem_open_target(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lowered = trimmed.to_ascii_lowercase();
    if lowered.starts_with("shell:") || lowered.starts_with("ms-") {
        return true;
    }

    if trimmed.contains("://") {
        return true;
    }

    !looks_like_filesystem_path(trimmed)
}

fn looks_like_filesystem_path(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }

    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

#[cfg(all(test, target_os = "windows"))]
mod shell_resolver_probe {
    use super::resolve_shell_appsfolder_path;

    #[test]
    fn resolves_pinned_app_targets() {
        // Shape 2: drive path embedded after the alias prefix. Use a path
        // that exists on every Windows install so the test is CI-safe.
        assert_eq!(
            resolve_shell_appsfolder_path("shell:AppsFolder\\C:\\Windows\\System32\\notepad.exe"),
            Some("C:\\Windows\\System32\\notepad.exe".to_string())
        );
        // Shape 1: known-folder GUID alias with file tail.
        let guid = resolve_shell_appsfolder_path("shell:AppsFolder\\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\charmap.exe");
        assert_eq!(guid.as_deref().map(|p| p.to_ascii_lowercase()).as_deref(), Some("c:\\windows\\system32\\charmap.exe"));
        // Shape 3: desktop AUMID resolved through the uninstall registry.
        let brave = resolve_shell_appsfolder_path("shell:AppsFolder\\Brave");
        if brave.is_some() {
            assert!(brave.as_deref().unwrap_or_default().to_ascii_lowercase().contains("brave.exe"));
        }
        // Shape 3 fallback: Squirrel AUMID, no DisplayIcon → install exe.
        let discord = resolve_shell_appsfolder_path("shell:AppsFolder\\com.squirrel.Discord.Discord");
        if discord.is_some() {
            assert!(discord.as_deref().unwrap_or_default().to_ascii_lowercase().ends_with("discord.exe"));
        }
        // UWP AUMID stays unresolved.
        assert_eq!(
            resolve_shell_appsfolder_path("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            None
        );
    }
}
