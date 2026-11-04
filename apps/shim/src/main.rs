//! One-release migration launcher for installs created before process
//! isolation (2.37.0 and earlier).
//!
//! Those installs ship an updater that resolves the runtime by name
//! (`bin\Nex.exe`) and then runs `--ensure-config`, `--sync-startup`, and
//! `--foreground` against it after installing a new version. This binary
//! forwards every call to the sibling `NexUIProcess.exe`, so the first
//! in-app upgrade completes instead of rolling back.
//!
//! It is installed only on updater-driven upgrades and removed by the next
//! release: from 2.38.0 on, the updater prefers `NexUIProcess.exe`.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::process::{Command, ExitCode};

const UI_EXE: &str = "NexUIProcess.exe";

/// Commands whose exit status the caller depends on are run to completion;
/// background/foreground launches start the UI detached and exit immediately.
fn wait_for_exit(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    if args.iter().any(|arg| arg == "--foreground" || arg == "--background") {
        return false;
    }
    true
}

fn sibling_ui_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(UI_EXE);
    candidate.is_file().then_some(candidate)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let Some(ui) = sibling_ui_path() else {
        eprintln!("Nex launcher: {UI_EXE} was not found next to Nex.exe");
        return ExitCode::FAILURE;
    };

    if wait_for_exit(&args) {
        return match Command::new(&ui).args(&args).status() {
            Ok(status) => ExitCode::from(
                status
                    .code()
                    .unwrap_or(1)
                    .clamp(0, u8::MAX as i32) as u8,
            ),
            Err(error) => {
                eprintln!("Nex launcher: could not start {}: {error}", ui.display());
                ExitCode::FAILURE
            }
        };
    }

    match Command::new(&ui).args(&args).spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Nex launcher: could not start {}: {error}", ui.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wait_for_exit;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn quick_commands_wait_for_the_ui() {
        assert!(wait_for_exit(&args(&["--ensure-config"])));
        assert!(wait_for_exit(&args(&["--sync-startup"])));
        assert!(wait_for_exit(&args(&["--quit"])));
        assert!(wait_for_exit(&args(&["--status"])));
    }

    #[test]
    fn runtime_launches_are_detached() {
        assert!(!wait_for_exit(&args(&["--foreground"])));
        assert!(!wait_for_exit(&args(&["--background"])));
        assert!(!wait_for_exit(&args(&[])));
    }
}
