//! Whether an app or shortcut target still exists on disk.
//!
//! Two places need the same answer. Start-menu discovery must stop emitting an
//! app row once the program behind it is gone: uninstalling a program leaves its
//! Start-menu shortcut behind with a dead target, and the shell keeps listing the
//! resulting `AppsFolder` entry, so without a filesystem check the row outlives
//! the program. Quick Launch needs it for pins, which are kept in config but must
//! not render when there is nothing left to launch.
//!
//! Two shapes have to be told apart. A parsing name such as
//! `{FOLDERID-…}\relative\path` or an absolute `C:\…\App.exe` names a file and can
//! be checked. A bare AppUserModelID such as `…_8wekyb3d8bbwe!App` names a package
//! and cannot be checked without asking the shell, so it is kept.

use std::path::PathBuf;

/// The `{FOLDERID-…}` prefix of a shell parsing name rendered the way the shell
/// writes it, so a parsed name can be compared without turning text back into a
/// GUID.
#[cfg(target_os = "windows")]
pub(crate) fn known_folder_key(guid: &windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

/// Filesystem root of every known folder an `AppsFolder` parsing name can start
/// with, resolved through the shell so the answer matches this machine's drive
/// layout instead of a guessed one.
#[cfg(target_os = "windows")]
pub(crate) fn known_folder_roots() -> Vec<(String, PathBuf)> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_CommonStartMenu, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_LocalAppData,
        FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesCommon,
        FOLDERID_ProgramFilesCommonX86, FOLDERID_ProgramFilesX86, FOLDERID_Public,
        FOLDERID_RoamingAppData, FOLDERID_StartMenu, FOLDERID_System, FOLDERID_SystemX86,
        FOLDERID_UserProfiles, FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };

    let ids = [
        &FOLDERID_System,
        &FOLDERID_SystemX86,
        &FOLDERID_ProgramFiles,
        &FOLDERID_ProgramFilesX86,
        &FOLDERID_ProgramFilesCommon,
        &FOLDERID_ProgramFilesCommonX86,
        &FOLDERID_Windows,
        &FOLDERID_ProgramData,
        &FOLDERID_LocalAppData,
        &FOLDERID_RoamingAppData,
        &FOLDERID_UserProfiles,
        &FOLDERID_Desktop,
        &FOLDERID_Documents,
        &FOLDERID_StartMenu,
        &FOLDERID_CommonStartMenu,
        &FOLDERID_Public,
    ];

    let mut roots = Vec::with_capacity(ids.len());
    for id in &ids {
        // SAFETY: `id` is a live FOLDERID constant and the string returned here
        // is CoTaskMem-allocated by the shell for this call.
        let Ok(raw) = (unsafe { SHGetKnownFolderPath(*id, KF_FLAG_DEFAULT, None) }) else {
            continue;
        };
        if raw.is_null() {
            continue;
        }
        // SAFETY: `raw` is NUL terminated and owned by this call.
        let text = unsafe { raw.to_string() }.ok();
        // SAFETY: `raw` came from `SHGetKnownFolderPath` and is not used after.
        unsafe { CoTaskMemFree(Some(raw.0 as *const core::ffi::c_void)) };
        if let Some(text) = text {
            roots.push((known_folder_key(*id), PathBuf::from(text)));
        }
    }
    roots
}

/// Whether an `AppsFolder` entry still points at something installed.
#[cfg(target_os = "windows")]
pub(crate) fn shell_app_target_exists(app_id: &str, roots: &[(String, PathBuf)]) -> bool {
    let trimmed = app_id.trim();
    let Some(close) = trimmed.starts_with('{').then(|| trimmed.find('}')).flatten() else {
        // The absolute-path form: `shell:AppsFolder\C:\…\App.exe`.
        let path = std::path::Path::new(trimmed);
        return !path.is_absolute() || path.exists();
    };

    let guid_text = &trimmed[..=close];
    let tail = trimmed[close + 1..].trim_start_matches('\\');
    match roots
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(guid_text))
    {
        Some((_, root)) if !tail.is_empty() => root.join(tail).exists(),
        // An unknown known-folder id cannot be resolved, so it is kept.
        _ => true,
    }
}

/// Whether a `shell:` launch target still names something the system has.
///
/// Explorer answers an argument it cannot resolve by opening its default folder,
/// so a `shell:` target left behind by an uninstall replaced the program with the
/// Documents folder and reported success. Rows are pruned from the index and pins
/// are hidden, but a click that lands between an uninstall and the next app pass
/// still arrives here, so the launch itself has to answer for it.
///
/// `shell:AppsFolder\<app id>` is checked with [`shell_app_target_exists`]. Other
/// shell identities (`shell:RecycleBinFolder`, `shell:MyComputerFolder`, ...) name
/// a namespace rather than a file and are left to the shell.
#[cfg(target_os = "windows")]
pub(crate) fn shell_target_resolves(target: &str) -> bool {
    const APPS_FOLDER: &str = "shell:AppsFolder\\";

    let trimmed = target.trim();
    if !trimmed
        .to_ascii_lowercase()
        .starts_with(&APPS_FOLDER.to_ascii_lowercase())
    {
        return true;
    }
    let app_id = trimmed[APPS_FOLDER.len()..].trim();
    app_id.is_empty() || shell_app_target_exists(app_id, &known_folder_roots())
}

/// Whether a `quick_launch.pinned` value still points at something launchable.
///
/// Bookmarks are always kept: they have no local target to lose. Everything else
/// is a shell identity or a path, and is dropped when nothing is there any more.
pub(crate) fn launch_target_exists(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    if crate::config::normalize_bookmark_url(trimmed).is_ok() {
        return true;
    }

    #[cfg(target_os = "windows")]
    if let Some(app_id) = trimmed.strip_prefix("shell:AppsFolder\\") {
        return shell_app_target_exists(app_id, &known_folder_roots());
    }

    std::path::Path::new(trimmed).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uninstalling a program leaves its Start-menu shortcut behind with a dead
    /// target, and the shell keeps listing the resulting AppsFolder entry. Both
    /// the app row and a Quick Launch pin have to stop existing for the user once
    /// the program is gone.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_target_that_no_longer_exists_is_reported_as_gone() {
        use windows::Win32::UI::Shell::FOLDERID_ProgramFilesX86;

        let dir = std::env::temp_dir().join("nex-app-target-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("here.exe"), b"x").unwrap();

        let roots = vec![(known_folder_key(&FOLDERID_ProgramFilesX86), dir.clone())];
        let key = roots[0].0.clone();
        assert!(shell_app_target_exists(&format!("{key}\\here.exe"), &roots));
        assert!(!shell_app_target_exists(&format!("{key}\\gone.exe"), &roots));

        let here = dir.join("here.exe").to_string_lossy().to_string();
        let gone = dir.join("gone.exe").to_string_lossy().to_string();
        assert!(launch_target_exists(&here));
        assert!(!launch_target_exists(&gone));
        // The absolute-path form of the same shell entry, as a pin.
        assert!(launch_target_exists(&format!("shell:AppsFolder\\{here}")));
        assert!(!launch_target_exists(&format!("shell:AppsFolder\\{gone}")));

        // A package activation id names no file and is kept, as is a bookmark.
        assert!(shell_app_target_exists(
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            &roots
        ));
        assert!(launch_target_exists("shell:AppsFolder\\Chrome"));
        assert!(launch_target_exists("https://linear.app/"));

        // An unknown known-folder id cannot be checked, so it is kept.
        assert!(shell_app_target_exists(
            "{00000000-0000-0000-0000-000000000000}\\gone.exe",
            &roots
        ));

        // The launch-time question, for the same two shapes.
        assert!(shell_target_resolves(&format!("shell:AppsFolder\\{here}")));
        assert!(!shell_target_resolves(&format!("shell:AppsFolder\\{gone}")));
        assert!(shell_target_resolves("shell:AppsFolder\\Chrome"));
        assert!(shell_target_resolves("shell:RecycleBinFolder"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
