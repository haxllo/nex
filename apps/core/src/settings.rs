use std::collections::BTreeSet;

pub const SAFE_HOTKEY_PRESETS: [&str; 6] = [
    "Ctrl+Shift+Space",
    "Ctrl+Alt+Space",
    "Alt+Shift+Space",
    "Ctrl+Shift+P",
    "Ctrl+Alt+P",
    "Ctrl+Shift+O",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsDraft {
    pub hotkey: String,
    pub max_results: u16,
    pub launch_at_startup: bool,
}

pub fn validate_hotkey(input: &str) -> Result<String, String> {
    let raw_parts: Vec<&str> = input
        .split('+')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();

    if raw_parts.len() < 2 {
        return Err("Hotkey must include at least one modifier and one key.".to_string());
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
        "win" | "windows" | "meta" => Err("Win/Meta combinations are not supported.".to_string()),
        _ => Err(format!(
            "Unsupported modifier '{input}'. Use Ctrl, Alt, or Shift."
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

    if let Some(number) = upper.strip_prefix('F') {
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

    Err("Key must be A-Z, 0-9, Space, or F1-F24.".to_string())
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
    ordered.push(key);
    ordered.join("+")
}

fn is_reserved_hotkey(canonical: &str) -> bool {
    matches!(
        canonical,
        "Alt+Tab" | "Alt+F4" | "Ctrl+Esc" | "Alt+Esc" | "Ctrl+Shift+Esc" | "Alt+Space"
    )
}
