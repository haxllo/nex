use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use nex_core::action_executor::{launch_open_target, launch_path, LaunchError};

fn unique_temp_path(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "nex-{label}-{}-{unique}.tmp",
        std::process::id()
    ))
}

#[test]
fn rejects_empty_launch_path() {
    let result = launch_path("");
    assert_eq!(result, Err(LaunchError::EmptyPath));
}

#[test]
fn rejects_missing_launch_path() {
    let missing = unique_temp_path("missing-path");
    let missing_str = missing.to_string_lossy().to_string();
    let result = launch_path(&missing_str);

    assert_eq!(result, Err(LaunchError::MissingPath(missing)));
}

#[test]
fn accepts_existing_launch_path() {
    let file_path = unique_temp_path("existing-path");
    let file_path_str = file_path.to_string_lossy().to_string();

    fs::write(&file_path, b"ok").expect("should create temp file");
    let result = launch_path(&file_path_str);
    fs::remove_file(&file_path).expect("should clean temp file");

    assert!(result.is_ok());
}

#[test]
fn rejects_empty_open_target() {
    let result = launch_open_target("   ");
    assert_eq!(result, Err(LaunchError::EmptyPath));
}

#[test]
fn accepts_web_open_target() {
    let result = launch_open_target("https://duckduckgo.com/?q=swiftfind");
    assert!(result.is_ok());
}

#[test]
fn accepts_shell_open_target_via_launch_path() {
    let result = launch_path("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
    assert!(result.is_ok());
}
