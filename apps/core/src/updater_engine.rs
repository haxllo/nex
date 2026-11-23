//! The update engine: download, verify, swap in the new build, or put the old
//! one back.
//!
//! This is the native replacement for the apply half of
//! `scripts/windows/update-nex.ps1`. It is deliberately wired behind a switch
//! and is **not** the default: the script has years of real-world recovery in
//! it, and it can be exercised by a machine only. Phase 2 exists so the two can
//! be compared on one real update before anything is removed.
//!
//! The ordering that matters, and why:
//!
//! 1. Everything is downloaded and verified **before** the running app is
//!    touched. A bad download must never leave the user with nothing running.
//! 2. The install tree is moved aside as a whole, not overwritten in place, so
//!    a failed install can be undone by moving it back.
//! 3. Nothing is deleted until the new build has answered `--ensure-config` and
//!    `--sync-startup`. The backup is the only thing standing between a broken
//!    install and a reinstall by hand.
//!
//! The pure parts (manifest verification, checksums, installer arguments) are
//! separated from the process work so they can be tested without an install.

use std::path::{Path, PathBuf};

/// Where an update is in its sequence. Reported in errors so a failure names
/// the step instead of an errno from somewhere vague.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Check,
    Download,
    Verify,
    Stop,
    Backup,
    Install,
    Confirm,
    Restart,
    Rollback,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Download => "download",
            Self::Verify => "verify",
            Self::Stop => "stop",
            Self::Backup => "backup",
            Self::Install => "install",
            Self::Confirm => "confirm",
            Self::Restart => "restart",
            Self::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateError {
    pub stage: Stage,
    pub message: String,
}

impl UpdateError {
    pub(crate) fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.stage.as_str(), self.message)
    }
}

impl std::error::Error for UpdateError {}

/// What a completed update reports back.
///
/// "Nothing to do" is its own answer rather than a successful install of the
/// version that was already there: reinstalling over a working install is
/// destructive work with no upside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated {
        version: String,
        install_root: PathBuf,
        /// The previous install, when it was kept.
        backup: Option<PathBuf>,
    },
    AlreadyCurrent {
        version: String,
    },
}

/// Scope the installer is told to use, and whether it needs an elevation
/// prompt. Kept as one decision so they cannot drift apart: scope describes
/// *where the install lives*, elevation only says whether removing or writing
/// there needs administrator rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallScope {
    pub all_users: bool,
    pub elevate: bool,
}

impl InstallScope {
    pub fn argument(self) -> &'static str {
        if self.all_users {
            "/ALLUSERS"
        } else {
            "/CURRENTUSER"
        }
    }
}

/// The publisher's manifest for one release, as written by the release
/// workflow. Only the fields that decide whether an installer is safe to run
/// are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub artifact: String,
    pub version: String,
    pub channel: Option<String>,
    pub setup_sha256: String,
}

/// Verify the manifest against the release being installed.
///
/// Every check is a real failure seen in the wild: a stale manifest left next
/// to a newer installer, a channel mismatch after switching to beta, and a
/// manifest without a checksum — which cannot establish anything and must not
/// be treated as "nothing to verify".
pub fn verify_manifest(
    manifest: &ReleaseManifest,
    expected_version: &str,
    expected_artifact: &str,
    expected_channel: &str,
) -> Result<(), String> {
    if manifest.artifact != expected_artifact {
        return Err(format!(
            "manifest artifact mismatch: expected '{expected_artifact}', got '{}'",
            manifest.artifact
        ));
    }
    let manifest_version = crate::updater::normalize_version(&manifest.version);
    if manifest_version != expected_version {
        return Err(format!(
            "manifest version mismatch: expected '{expected_version}', got '{manifest_version}'"
        ));
    }
    if let Some(channel) = manifest.channel.as_deref() {
        if !channel.eq_ignore_ascii_case(expected_channel) {
            return Err(format!(
                "manifest channel mismatch: expected '{expected_channel}', got '{channel}'"
            ));
        }
    }
    if manifest.setup_sha256.trim().is_empty() {
        return Err("manifest carries no setup checksum, so the installer cannot be verified".into());
    }
    Ok(())
}

/// Whether a downloaded installer matches the manifest's checksum.
///
/// Compared case-insensitively: the release workflow writes lower-case, and a
/// hand-edited manifest with upper-case hex is still the same hash.
pub fn checksum_matches(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

/// The installer's command line, matching the script's:
/// silent, no wizard, no restart, a log file, an explicit target directory,
/// an explicit scope, and `/NEXUPDATER` so the installer does not start Nex
/// itself — this engine restarts it after confirming the new build works.
pub fn installer_arguments(log_path: &Path, install_root: &Path, scope: InstallScope) -> Vec<String> {
    vec![
        "/SILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/NOCANCEL".into(),
        "/SP-".into(),
        format!("/LOG=\"{}\"", log_path.display()),
        format!("/DIR=\"{}\"", install_root.display()),
        scope.argument().into(),
        "/NEXUPDATER".into(),
    ]
}

/// Working directory for one update attempt, under the user's update cache.
pub fn work_directory(cache_root: &Path, artifact: &str, stamp: &str) -> PathBuf {
    cache_root.join(format!("{artifact}-update-{stamp}"))
}

/// Backups live beside the update cache so a failed update can be undone
/// without knowing anything about the install tree's history.
pub fn backup_directory(cache_root: &Path, stamp: &str) -> PathBuf {
    cache_root.join("backups").join(format!("nex-backup-{stamp}"))
}

/// The installer's log, kept next to the update's own log so one folder
/// explains a failed update.
pub fn installer_log_path(work_dir: &Path) -> PathBuf {
    work_dir.join("setup.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ReleaseManifest {
        ReleaseManifest {
            artifact: "nex-3.3.0-windows-x64".into(),
            version: "3.3.0".into(),
            channel: Some("stable".into()),
            setup_sha256: "ABCDEF".into(),
        }
    }

    #[test]
    fn a_matching_manifest_verifies() {
        assert_eq!(
            verify_manifest(&manifest(), "3.3.0", "nex-3.3.0-windows-x64", "stable"),
            Ok(())
        );
        // A manifest written without a channel is not a mismatch.
        let mut no_channel = manifest();
        no_channel.channel = None;
        assert_eq!(
            verify_manifest(&no_channel, "3.3.0", "nex-3.3.0-windows-x64", "stable"),
            Ok(())
        );
    }

    #[test]
    fn a_stale_manifest_is_rejected_rather_than_installed() {
        let error = verify_manifest(&manifest(), "3.4.0", "nex-3.3.0-windows-x64", "stable")
            .expect_err("version mismatch must be rejected");
        assert!(error.contains("version mismatch"), "{error}");

        let error = verify_manifest(&manifest(), "3.3.0", "nex-3.3.1-windows-x64", "stable")
            .expect_err("artifact mismatch must be rejected");
        assert!(error.contains("artifact mismatch"), "{error}");
    }

    #[test]
    fn a_channel_mismatch_is_rejected() {
        let error = verify_manifest(&manifest(), "3.3.0", "nex-3.3.0-windows-x64", "beta")
            .expect_err("channel mismatch must be rejected");
        assert!(error.contains("channel mismatch"), "{error}");
    }

    #[test]
    fn a_manifest_without_a_checksum_cannot_verify_an_installer() {
        let mut empty = manifest();
        empty.setup_sha256 = "   ".into();
        let error = verify_manifest(&empty, "3.3.0", "nex-3.3.0-windows-x64", "stable")
            .expect_err("a missing checksum must be an error, not a pass");
        assert!(error.contains("checksum"), "{error}");
    }

    #[test]
    fn checksums_compare_case_insensitively_and_ignore_padding() {
        assert!(checksum_matches("abc123", "ABC123"));
        assert!(checksum_matches("abc123\n", " abc123 "));
        assert!(!checksum_matches("abc123", "abc124"));
    }

    #[test]
    fn installer_arguments_match_the_script() {
        let args = installer_arguments(
            Path::new(r"C:\work\setup.log"),
            Path::new(r"C:\Users\user\AppData\Local\Programs\Nex"),
            InstallScope {
                all_users: false,
                elevate: false,
            },
        );
        assert_eq!(
            args,
            vec![
                "/SILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/NOCANCEL",
                "/SP-",
                r#"/LOG="C:\work\setup.log""#,
                r#"/DIR="C:\Users\user\AppData\Local\Programs\Nex""#,
                "/CURRENTUSER",
                "/NEXUPDATER",
            ]
        );

        let all_users = installer_arguments(
            Path::new(r"C:\work\setup.log"),
            Path::new(r"C:\Program Files\Nex"),
            InstallScope {
                all_users: true,
                elevate: true,
            },
        );
        assert!(all_users.contains(&"/ALLUSERS".to_string()));
    }

    #[test]
    fn work_and_backup_directories_are_stamped_and_separate() {
        let cache = Path::new(r"C:\Users\user\AppData\Local\Nex\updates");
        let work = work_directory(cache, "nex-3.3.0-windows-x64", "20261123-120000");
        assert_eq!(work, cache.join("nex-3.3.0-windows-x64-update-20261123-120000"));
        let backup = backup_directory(cache, "20261123-120000");
        assert_eq!(backup, cache.join("backups").join("nex-backup-20261123-120000"));
        assert_ne!(work, backup);
    }
}
