use std::collections::BTreeSet;

pub const SAFE_HOTKEY_PRESETS: [&str; 8] = [
    "Win",
    "Ctrl+Shift+Space",
    "Ctrl+Alt+Space",
    "Alt+Shift+Space",
    "Win+Shift+F1",
    "Win+Shift+F2",
    "Ctrl+Shift+P",
    "Ctrl+Shift+O",
];

pub fn validate_hotkey(input: &str) -> Result<String, String> {
    let raw_parts: Vec<&str> = input
        .split('+')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();

    // Allow single-key hotkeys (e.g. "Win" alone).
    // For standard chords (modifier+key), require at least 2 parts.
    // For a single key, validate it's a recognized key.
    if raw_parts.len() == 1 {
        let key = normalize_key(raw_parts[0])?;
        return Ok(key);
    }

    let key_raw = raw_parts[raw_parts.len() - 1];
    let key = normalize_key(key_raw)?;

    let mut modifiers: BTreeSet<&'static str> = BTreeSet::new();
    for part in &raw_parts[..raw_parts.len() - 1] {
        let modifier = normalize_modifier(part)?;
        modifiers.insert(modifier);
    }

    if modifiers.is_empty() {
        return Err("Hotkey must include at least one modifier.".to_string());
    }

    let canonical = canonical_hotkey(&modifiers, &key);
    if is_reserved_hotkey(&canonical) {
        return Err(
            "This hotkey is commonly reserved by Windows. Choose a different one.".to_string(),
        );
    }

    Ok(canonical)
}

/// Stricter check for a hotkey the user is saving now. Kept separate from
/// `validate_hotkey` so existing configs keep loading.
pub fn validate_new_hotkey(input: &str) -> Result<String, String> {
    let canonical = validate_hotkey(input)?;
    let parts: Vec<&str> = canonical.split('+').collect();
    let key = parts.last().copied().unwrap_or_default();
    if parts.len() == 1 && key != "Win" && !is_function_key(key) {
        // The hook eats the target key system-wide, so a bare letter, digit
        // or Space would stop it from typing anywhere.
        return Err("Add Ctrl, Alt, Shift or Win to this key.".to_string());
    }
    if parts.len() > 1 && key == "Win" {
        return Err("Win can be used alone or as a modifier, not as the last key.".to_string());
    }
    Ok(canonical)
}

fn is_function_key(key: &str) -> bool {
    key.len() > 1
        && key.starts_with('F')
        && key[1..].bytes().all(|b| b.is_ascii_digit())
}

pub fn validate_max_results(value: u16) -> Result<(), String> {
    if (5..=100).contains(&value) {
        Ok(())
    } else {
        Err("Max results must be between 5 and 100.".to_string())
    }
}

pub fn suggested_hotkey_presets(current: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }

    let current_canonical = validate_hotkey(current).ok();
    SAFE_HOTKEY_PRESETS
        .iter()
        .filter_map(|preset| validate_hotkey(preset).ok())
        .filter(|preset| current_canonical.as_ref() != Some(preset))
        .take(limit)
        .collect()
}

fn normalize_modifier(input: &str) -> Result<&'static str, String> {
    match input.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Ok("Ctrl"),
        "alt" => Ok("Alt"),
        "shift" => Ok("Shift"),
        "win" | "windows" | "meta" => Ok("Win"),
        _ => Err(format!(
            "Unsupported modifier '{input}'. Use Ctrl, Alt, Shift, or Win."
        )),
    }
}

fn normalize_key(input: &str) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("Hotkey key is required.".to_string());
    }

    let upper = raw.to_ascii_uppercase();
    if upper == "SPACE" {
        return Ok("Space".to_string());
    }
    if upper == "WIN" || upper == "LWIN" || upper == "RWIN" || upper == "META" {
        return Ok("Win".to_string());
    }

    // Only "F<digits>" is a function key; a bare "F" is the letter.
    if let Some(number) = upper
        .strip_prefix('F')
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    {
        if let Ok(parsed) = number.parse::<u8>() {
            if (1..=24).contains(&parsed) {
                return Ok(format!("F{parsed}"));
            }
        }
        return Err("Function key must be between F1 and F24.".to_string());
    }

    if upper.len() == 1 {
        let c = upper.chars().next().unwrap_or_default();
        if c.is_ascii_alphanumeric() {
            return Ok(upper);
        }
    }

    Err("Key must be A-Z, 0-9, Space, Win, or F1-F24.".to_string())
}

fn canonical_hotkey(modifiers: &BTreeSet<&'static str>, key: &str) -> String {
    let mut ordered = Vec::new();
    if modifiers.contains("Ctrl") {
        ordered.push("Ctrl");
    }
    if modifiers.contains("Alt") {
        ordered.push("Alt");
    }
    if modifiers.contains("Shift") {
        ordered.push("Shift");
    }
    if modifiers.contains("Win") {
        ordered.push("Win");
    }
    ordered.push(key);
    ordered.join("+")
}

fn is_reserved_hotkey(canonical: &str) -> bool {
    matches!(
        canonical,
        "Alt+Tab" | "Alt+F4" | "Ctrl+Esc" | "Alt+Esc" | "Ctrl+Shift+Esc" | "Alt+Space"
    )
}
