//! Which installation an update would actually modify, decided in process.
//!
//! The PowerShell updater works this out with four fallbacks (running copy,
//! then HKCU, then a HKLM entry that is genuinely under Program Files, then a
//! stale HKLM entry pointing at a per-user directory). That order is what keeps
//! a dual-scope machine, or a machine left with an orphan copy, from updating
//! the wrong tree — and it is also the part that has to be right *before*
//! anything is stopped, moved or installed.
//!
//! So it lives here as a pure function over collected facts: the precedence can
//! be tested without a registry, and the Windows collector is a thin read of
//! those same facts. Nothing in this module writes, stops or installs.

use std::path::{Path, PathBuf};

/// The executable the installer registers, and the first thing an install root
/// is validated against.
pub const RUNTIME_EXECUTABLE: &str = "bin/NexUIProcess.exe";

/// Lookup order for the installed runtime, matching the updater script: the UI
/// is what an install is identified by, but older layouts used other names.
const RUNTIME_CANDIDATES: [&str; 4] = [
    "NexUIProcess.exe",
    "NexHelper.exe",
    "nex-core.exe",
    "swiftfind-core.exe",
];

/// Where the install location came from. Carried out to `--update-plan` because
/// "which rule matched" is the question that answers a wrong-tree report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    /// The copy that is running right now; disambiguates dual-scope machines.
    RunningCopy,
    /// Registered under HKCU: a per-user install.
    HkcuRegistry,
    /// Registered under HKLM and genuinely under Program Files: all users.
    HklmProgramFiles,
    /// Registered under HKLM but pointing at a per-user directory. This is the
    /// stale hybrid an older updater bug left behind; it is updated as a
    /// per-user install and never treated as machine-wide.
    HklmStale,
    /// Nothing registered: the default per-user location.
    Default,
}

impl InstallSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunningCopy => "running-copy",
            Self::HkcuRegistry => "hkcu-registry",
            Self::HklmProgramFiles => "hklm-program-files",
            Self::HklmStale => "hklm-stale",
            Self::Default => "default",
        }
    }
}

/// Everything the decision depends on. Locations are expected to have been
/// validated by the collector (they must contain the runtime executable);
/// `running_root` is the install tree this process was started from, or `None`
/// for a repo build or a copy running outside an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFacts {
    pub running_root: Option<PathBuf>,
    pub hkcu_location: Option<PathBuf>,
    pub hklm_location: Option<PathBuf>,
    /// Program Files roots, used only to decide scope.
    pub program_files: Vec<PathBuf>,
    pub default_root: PathBuf,
}

/// The installation an update would modify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallTarget {
    pub root: PathBuf,
    /// Elevated only for permission: an all-users install, or a legacy copy
    /// outside the registered install that only an elevated installer can
    /// remove. Scope follows `root`, never this flag.
    pub needs_elevation: bool,
    pub source: InstallSource,
}

/// Precedence, in the script's order:
///
/// 1. the running copy, because that is the tree the user is looking at;
/// 2. HKCU, because a per-user registration is unambiguous;
/// 3. HKLM **only** when it is under Program Files;
/// 4. a stale HKLM entry, as the per-user tree it actually points at;
/// 5. the default per-user location.
pub fn resolve_install_target(facts: &InstallFacts) -> InstallTarget {
    if let Some(root) = facts.running_root.clone() {
        let needs_elevation = is_under_program_files(&root, &facts.program_files);
        return InstallTarget {
            root,
            needs_elevation,
            source: InstallSource::RunningCopy,
        };
    }

    if let Some(root) = facts.hkcu_location.clone() {
        return InstallTarget {
            root,
            needs_elevation: false,
            source: InstallSource::HkcuRegistry,
        };
    }

    if let Some(root) = facts.hklm_location.clone() {
        return if is_under_program_files(&root, &facts.program_files) {
            InstallTarget {
                root,
                needs_elevation: true,
                source: InstallSource::HklmProgramFiles,
            }
        } else {
            InstallTarget {
                root,
                needs_elevation: false,
                source: InstallSource::HklmStale,
            }
        };
    }

    InstallTarget {
        root: facts.default_root.clone(),
        needs_elevation: false,
        source: InstallSource::Default,
    }
}

/// Whether `path` sits inside one of the Program Files roots.
///
/// Compared component by component rather than as a string prefix: the script's
/// `StartsWith` treats `C:\Program FilesFoo` as being under `C:\Program Files`,
/// which would ask for an elevated update of a directory that does not need
/// one. Windows paths are compared case-insensitively, and both separator
/// styles normalize because the roots come from the environment.
pub fn is_under_program_files(path: &Path, program_files: &[PathBuf]) -> bool {
    let subject = components(path);
    if subject.is_empty() {
        return false;
    }
    program_files.iter().any(|base| {
        let base = components(base);
        !base.is_empty() && subject.len() >= base.len() && base[..] == subject[..base.len()]
    })
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// The installed runtime inside `root`: the first candidate that exists, or
/// the canonical path when none does (so callers still name what should be
/// there).
/// The UI runtime, which is the one that is started.
///
/// [`runtime_executable`] answers "any runtime image", which is the right question
/// for a version stamp and the wrong one for starting: with `NexUIProcess.exe`
/// missing it found `NexHelper.exe`, and starting that asks for elevation and
/// fails. Starting Nex means starting this file or nothing.
pub fn ui_executable(root: &Path) -> PathBuf {
    root.join("bin").join("NexUIProcess.exe")
}

pub fn runtime_executable(root: &Path) -> PathBuf {
    for candidate in RUNTIME_CANDIDATES {
        let path = root.join("bin").join(candidate);
        if path.is_file() {
            return path;
        }
    }
    root.join("bin").join(RUNTIME_CANDIDATES[0])
}

/// The installed version, read from the runtime's version resource.
///
/// The script used the same source (`(Get-Item $exe).VersionInfo.FileVersion`),
/// which is what the installer stamps — not what the running binary was
/// compiled with. Keeping that distinction is what lets the updater notice a
/// stale orphan copy that is older than the installed one.
pub fn installed_version(root: &Path) -> Option<String> {
    let exe = runtime_executable(root);
    crate::win_version_resource::file_version(&exe.to_string_lossy())
}

/// Release asset names for a tag: `nex-<version>-windows-x64-setup.exe` and the
/// sibling manifest. Built from the normalized version so a `v3.3.0` tag and a
/// `3.3` tag name the same assets.
pub fn release_asset_names(tag_or_version: &str) -> (String, String) {
    let version = crate::updater::normalize_version(tag_or_version);
    let base = format!("nex-{version}-windows-x64");
    (format!("{base}-setup.exe"), format!("{base}-manifest.json"))
}

/// Read the facts a real update would be decided from.
///
/// Registered locations are validated here, not in `resolve_install_target`:
/// a registry entry left behind by an uninstall, pointing at a directory that
/// no longer holds the runtime, is not an install and must not win.
#[cfg(target_os = "windows")]
pub fn collect_install_facts() -> InstallFacts {
    let (hkcu, hklm) = crate::startup::registered_install_locations();
    let holds_runtime = |location: Option<PathBuf>| {
        location.filter(|root| root.join(RUNTIME_EXECUTABLE).is_file())
    };

    InstallFacts {
        running_root: running_install_root(),
        hkcu_location: holds_runtime(hkcu),
        hklm_location: holds_runtime(hklm),
        program_files: program_files_roots(),
        default_root: default_install_root(),
    }
}

/// The install tree this process was started from, or `None` for a repo build
/// (and for any copy running outside a `bin` directory, which is not an
/// install). Mirrors the script's `Get-RunningInstallRoot`.
#[cfg(target_os = "windows")]
fn running_install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin = exe.parent()?;
    if !bin
        .file_name()
        .map(|name| name.to_string_lossy().eq_ignore_ascii_case("bin"))
        .unwrap_or(false)
    {
        return None;
    }
    let root = bin.parent()?.to_path_buf();
    root.join(RUNTIME_EXECUTABLE).is_file().then_some(root)
}

#[cfg(target_os = "windows")]
fn program_files_roots() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "windows")]
fn default_install_root() -> PathBuf {
    match std::env::var("LOCALAPPDATA") {
        Ok(local_app_data) if !local_app_data.trim().is_empty() => {
            PathBuf::from(local_app_data).join("Programs").join("Nex")
        }
        _ => PathBuf::from(r"C:\Program Files\Nex"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        running: Option<&str>,
        hkcu: Option<&str>,
        hklm: Option<&str>,
    ) -> InstallFacts {
        InstallFacts {
            running_root: running.map(PathBuf::from),
            hkcu_location: hkcu.map(PathBuf::from),
            hklm_location: hklm.map(PathBuf::from),
            program_files: vec![
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Program Files (x86)"),
            ],
            default_root: PathBuf::from(r"C:\Users\user\AppData\Local\Programs\Nex"),
        }
    }

    #[test]
    fn the_running_copy_wins_because_it_is_the_tree_the_user_sees() {
        let target = resolve_install_target(&facts(
            Some(r"C:\Users\user\AppData\Local\Programs\Nex"),
            Some(r"D:\Nex"),
            Some(r"C:\Program Files\Nex"),
        ));
        assert_eq!(target.root, PathBuf::from(r"C:\Users\user\AppData\Local\Programs\Nex"));
        assert_eq!(target.source, InstallSource::RunningCopy);
        assert!(!target.needs_elevation);
    }

    #[test]
    fn a_running_copy_under_program_files_is_updated_elevated() {
        let target = resolve_install_target(&facts(Some(r"C:\Program Files\Nex"), None, None));
        assert_eq!(target.source, InstallSource::RunningCopy);
        assert!(target.needs_elevation);
    }

    #[test]
    fn hkcu_is_taken_before_a_machine_wide_entry() {
        let target = resolve_install_target(&facts(
            None,
            Some(r"C:\Users\user\AppData\Local\Programs\Nex"),
            Some(r"C:\Program Files\Nex"),
        ));
        assert_eq!(target.source, InstallSource::HkcuRegistry);
        assert!(!target.needs_elevation);
    }

    #[test]
    fn a_hklm_entry_under_program_files_is_all_users() {
        let target = resolve_install_target(&facts(None, None, Some(r"C:\Program Files\Nex")));
        assert_eq!(target.source, InstallSource::HklmProgramFiles);
        assert!(target.needs_elevation);
    }

    #[test]
    fn a_hklm_entry_outside_program_files_is_the_stale_hybrid() {
        // The older updater bug: an HKLM registration pointing at a per-user
        // directory. It must not be treated as a machine-wide install.
        let target = resolve_install_target(&facts(
            None,
            None,
            Some(r"C:\Users\user\AppData\Local\Programs\Nex"),
        ));
        assert_eq!(target.source, InstallSource::HklmStale);
        assert!(!target.needs_elevation);
    }

    #[test]
    fn nothing_registered_falls_back_to_the_default_per_user_root() {
        let target = resolve_install_target(&facts(None, None, None));
        assert_eq!(
            target.root,
            PathBuf::from(r"C:\Users\user\AppData\Local\Programs\Nex")
        );
        assert_eq!(target.source, InstallSource::Default);
        assert!(!target.needs_elevation);
    }

    #[test]
    fn program_files_is_matched_by_path_component_not_by_prefix() {
        let roots = vec![PathBuf::from(r"C:\Program Files")];
        assert!(is_under_program_files(Path::new(r"C:\Program Files\Nex"), &roots));
        // Its own root, not a child of `C:\Program Files` — the resolver is
        // handed both roots, and a component walk keeps them apart.
        assert!(!is_under_program_files(
            Path::new(r"C:\Program Files (x86)\Nex"),
            &roots
        ));
        assert!(is_under_program_files(
            Path::new(r"C:\Program Files (x86)\Nex"),
            &[PathBuf::from(r"C:\Program Files (x86)")]
        ));
        assert!(is_under_program_files(Path::new(r"c:\program files\nex"), &roots));
        assert!(is_under_program_files(Path::new(r"C:/Program Files/Nex"), &roots));
        // The script's string prefix would call this elevated.
        assert!(!is_under_program_files(Path::new(r"C:\Program FilesFoo\Nex"), &roots));
        assert!(!is_under_program_files(Path::new(r"D:\Nex"), &roots));
        assert!(!is_under_program_files(Path::new(r""), &roots));
    }

    #[test]
    fn release_assets_follow_the_normalized_version() {
        assert_eq!(
            release_asset_names("v3.3.0"),
            (
                "nex-3.3.0-windows-x64-setup.exe".to_string(),
                "nex-3.3.0-windows-x64-manifest.json".to_string()
            )
        );
        assert_eq!(
            release_asset_names("3.3"),
            (
                "nex-3.3.0-windows-x64-setup.exe".to_string(),
                "nex-3.3.0-windows-x64-manifest.json".to_string()
            )
        );
    }

    #[test]
    fn a_root_without_a_runtime_still_names_the_canonical_executable() {
        let root = std::env::temp_dir().join(format!("nex-install-none-{}", std::process::id()));
        let exe = runtime_executable(&root);
        assert_eq!(exe, root.join("bin").join(RUNTIME_CANDIDATES[0]));
        assert_eq!(installed_version(&root), None);
    }
}
