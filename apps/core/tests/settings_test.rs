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
fn validates_win_modifier_hotkey() {
    let canonical = nex_core::settings::validate_hotkey("win + p").unwrap();
    assert_eq!(canonical, "Win+P");
}

#[test]
fn letter_f_is_a_key_not_a_function_key() {
    assert_eq!(
        nex_core::settings::validate_hotkey("Ctrl+Shift+F").unwrap(),
        "Ctrl+Shift+F"
    );
    assert_eq!(nex_core::settings::validate_hotkey("Ctrl+F13").unwrap(), "Ctrl+F13");
    assert!(nex_core::settings::validate_hotkey("Ctrl+F25").is_err());
}

#[test]
fn new_hotkey_rejects_bare_typing_keys() {
    assert!(nex_core::settings::validate_new_hotkey("Space").is_err());
    assert!(nex_core::settings::validate_new_hotkey("A").is_err());
    assert!(nex_core::settings::validate_new_hotkey("Ctrl+Win").is_err());
    assert_eq!(nex_core::settings::validate_new_hotkey("Win").unwrap(), "Win");
    assert_eq!(nex_core::settings::validate_new_hotkey("F9").unwrap(), "F9");
    assert_eq!(
        nex_core::settings::validate_new_hotkey("ctrl+space").unwrap(),
        "Ctrl+Space"
    );
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
            "Win".to_string(),
            "Ctrl+Alt+Space".to_string(),
            "Alt+Shift+Space".to_string(),
        ]
    );
}

#[test]
fn suggests_no_hotkey_presets_when_limit_is_zero() {
    let suggestions = nex_core::settings::suggested_hotkey_presets("Ctrl+Space", 0);
    assert!(suggestions.is_empty());
}
