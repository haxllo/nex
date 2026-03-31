#[test]
fn validates_and_canonicalizes_hotkey() {
    let canonical = nex_core::settings::validate_hotkey(" shift + ctrl + p ").unwrap();
    assert_eq!(canonical, "Ctrl+Shift+P");
}

#[test]
fn rejects_reserved_hotkey() {
    let result = nex_core::settings::validate_hotkey("Alt+Space");
    assert!(result.is_err());
}

#[test]
fn rejects_win_modifier_hotkey() {
    let result = nex_core::settings::validate_hotkey("Win+P");
    assert!(result.is_err());
}

#[test]
fn validates_max_results_range() {
    assert!(nex_core::settings::validate_max_results(5).is_ok());
    assert!(nex_core::settings::validate_max_results(100).is_ok());
    assert!(nex_core::settings::validate_max_results(4).is_err());
    assert!(nex_core::settings::validate_max_results(101).is_err());
}

#[test]
fn suggests_safe_hotkey_presets_excluding_current_hotkey() {
    let suggestions = nex_core::settings::suggested_hotkey_presets("Ctrl+Shift+Space", 3);
    assert_eq!(
        suggestions,
        vec![
            "Ctrl+Alt+Space".to_string(),
            "Alt+Shift+Space".to_string(),
            "Ctrl+Shift+P".to_string()
        ]
    );
}

#[test]
fn suggests_no_hotkey_presets_when_limit_is_zero() {
    let suggestions = nex_core::settings::suggested_hotkey_presets("Ctrl+Space", 0);
    assert!(suggestions.is_empty());
}
