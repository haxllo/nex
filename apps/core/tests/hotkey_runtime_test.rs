#[cfg(not(target_os = "windows"))]
use nex_core::hotkey_runtime::{
    default_hotkey_registrar, run_message_loop, HotkeyRuntimeError,
};
use nex_core::hotkey_runtime::{HotkeyRegistrar, HotkeyRegistration, MockHotkeyRegistrar};

#[test]
fn mock_registrar_tracks_registration_lifecycle() {
    let mut registrar = MockHotkeyRegistrar::default();

    let first = registrar.register_hotkey("Alt+Space").unwrap();
    assert_eq!(first, HotkeyRegistration::Noop("Alt+Space".to_string()));
    assert_eq!(registrar.registrations().len(), 1);

    registrar.unregister_all().unwrap();
    assert!(registrar.registrations().is_empty());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn default_registrar_is_noop_on_non_windows() {
    let mut registrar = default_hotkey_registrar();

    let registration = registrar.register_hotkey("Alt+Space").unwrap();
    assert_eq!(
        registration,
        HotkeyRegistration::Noop("Alt+Space".to_string())
    );
    registrar.unregister_all().unwrap();
}

#[cfg(not(target_os = "windows"))]
#[test]
fn message_loop_reports_unsupported_platform_off_windows() {
    let result = run_message_loop(|_| {});
    assert_eq!(result, Err(HotkeyRuntimeError::UnsupportedPlatform));
}
