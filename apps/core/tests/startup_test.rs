#[cfg(not(target_os = "windows"))]
#[test]
fn startup_service_reports_unsupported_platform_off_windows() {
    let check = nex_core::startup::is_enabled();
    assert!(matches!(
        check,
        Err(nex_core::startup::StartupError::UnsupportedPlatform)
    ));

    let set = nex_core::startup::set_enabled(false, std::path::Path::new("/tmp/test"));
    assert!(matches!(
        set,
        Err(nex_core::startup::StartupError::UnsupportedPlatform)
    ));
}

#[test]
fn startup_command_quotes_executable_and_adds_background_flag() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be valid")
        .as_nanos();
    let exe_path = std::env::temp_dir().join(format!("nex-startup-{unique}.exe"));
    std::fs::write(&exe_path, b"stub").expect("temp executable file should be created");

    let command =
        nex_core::startup::startup_command_for_executable(&exe_path).expect("should build");

    assert!(command.starts_with('"'));
    assert!(command.contains("--background"));
    assert!(command.contains(exe_path.to_string_lossy().as_ref()));

    std::fs::remove_file(exe_path).expect("temp executable file should be removed");
}

#[test]
fn startup_command_rejects_missing_executable() {
    let missing = std::env::temp_dir().join("nex-startup-missing.exe");
    let result = nex_core::startup::startup_command_for_executable(&missing);
    assert!(result.is_err());
}
