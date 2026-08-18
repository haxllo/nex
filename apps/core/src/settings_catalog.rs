//! Curated catalog of Windows Settings pages (`ms-settings:` URIs).
//!
//! Windows exposes no queryable settings catalog, so launchers ship a
//! static list. Each page maps to the Segoe Fluent Icons / Segoe MDL2
//! Assets glyph that the Settings app itself uses for its category, so
//! search rows carry an icon "belonging" to the page.
//!
//! Pages launch via `ms-settings:{uri}` through ShellExecute (the
//! protocol handler is registered system-wide).

use crate::model::SearchItem;

pub struct SettingsPage {
    pub title: &'static str,
    /// `ms-settings:` URI suffix, e.g. `"display"`.
    pub uri: &'static str,
    /// Segoe Fluent Icons / Segoe MDL2 Assets codepoint.
    pub glyph: u16,
}

/// Rows appended to the Start-menu apps provider result.
pub fn settings_page_items() -> Vec<SearchItem> {
    SETTINGS_PAGES
        .iter()
        .map(|page| {
            let path = format!("ms-settings:{}", page.uri);
            SearchItem::new(
                &format!("settings:{}", page.uri),
                "settings",
                page.title,
                &path,
            )
        })
        .collect()
}

/// Glyph for a settings page (`ms-settings:{uri}` suffix), falling back
/// to the gear when the page has no dedicated glyph.
pub fn settings_glyph(uri: &str) -> u16 {
    SETTINGS_PAGES
        .iter()
        .find(|page| page.uri == uri)
        .map(|page| page.glyph)
        .unwrap_or(GLYPH_GEAR)
}

pub const GLYPH_GEAR: u16 = 0xE713;

/// Curated page list. Glyphs follow the Windows Settings iconography
/// (Segoe MDL2 Assets codepoints, kept by Segoe Fluent Icons).
pub const SETTINGS_PAGES: &[SettingsPage] = &[
    SettingsPage { title: "System", uri: "system", glyph: 0xE713 },
    SettingsPage { title: "Display", uri: "display", glyph: 0xE7F4 },
    SettingsPage { title: "Sound", uri: "sound", glyph: 0xE768 },
    SettingsPage { title: "Notifications", uri: "notifications", glyph: 0xE7C4 },
    SettingsPage { title: "Focus assist", uri: "focus", glyph: 0xE915 },
    SettingsPage { title: "Power & battery", uri: "power", glyph: 0xE850 },
    SettingsPage { title: "Battery saver", uri: "battery-saver", glyph: 0xE850 },
    SettingsPage { title: "Storage", uri: "storage", glyph: 0xE74E },
    SettingsPage { title: "Multitasking", uri: "multitasking", glyph: 0xE713 },
    SettingsPage { title: "Clipboard", uri: "clipboard", glyph: 0xE7BA },
    SettingsPage { title: "About", uri: "about", glyph: 0xE713 },
    SettingsPage { title: "Bluetooth & devices", uri: "bluetooth", glyph: 0xE702 },
    SettingsPage { title: "Devices", uri: "devices", glyph: 0xE790 },
    SettingsPage { title: "Mouse", uri: "devices-mouse", glyph: 0xE77F },
    SettingsPage { title: "Touchpad", uri: "devices-touchpad", glyph: 0xE77F },
    SettingsPage { title: "Printers & scanners", uri: "devices-printers", glyph: 0xE74E },
    SettingsPage { title: "USB", uri: "devices-usb", glyph: 0xE713 },
    SettingsPage { title: "Phone Link", uri: "phone", glyph: 0xE725 },
    SettingsPage { title: "Network & Internet", uri: "network", glyph: 0xE701 },
    SettingsPage { title: "Wi-Fi", uri: "wifi", glyph: 0xE701 },
    SettingsPage { title: "Ethernet", uri: "network-ethernet", glyph: 0xE701 },
    SettingsPage { title: "Mobile hotspot", uri: "mobile-hotspot", glyph: 0xE701 },
    SettingsPage { title: "Airplane mode", uri: "network-airplanemode", glyph: 0xE709 },
    SettingsPage { title: "VPN", uri: "network-vpn", glyph: 0xE701 },
    SettingsPage { title: "Proxy", uri: "network-proxy", glyph: 0xE701 },
    SettingsPage { title: "Data usage", uri: "datausage", glyph: 0xE701 },
    SettingsPage { title: "Personalization", uri: "personalization", glyph: 0xE771 },
    SettingsPage { title: "Background", uri: "personalization-background", glyph: 0xE771 },
    SettingsPage { title: "Colors", uri: "personalization-colors", glyph: 0xE771 },
    SettingsPage { title: "Night light", uri: "nightlight", glyph: 0xE915 },
    SettingsPage { title: "Start", uri: "personalization-start", glyph: 0xE771 },
    SettingsPage { title: "Taskbar", uri: "taskbar", glyph: 0xE771 },
    SettingsPage { title: "Apps", uri: "apps", glyph: 0xE7C4 },
    SettingsPage { title: "Default apps", uri: "apps-defaultapps", glyph: 0xE713 },
    SettingsPage { title: "Startup apps", uri: "startupapps", glyph: 0xE713 },
    SettingsPage { title: "Accounts", uri: "accounts", glyph: 0xE713 },
    SettingsPage { title: "Sign-in options", uri: "signinoptions", glyph: 0xE81C },
    SettingsPage { title: "Family options", uri: "family", glyph: 0xE7E8 },
    SettingsPage { title: "Time & language", uri: "time-language", glyph: 0xE823 },
    SettingsPage { title: "Date & time", uri: "time-language-dateandtime", glyph: 0xE823 },
    SettingsPage { title: "Language", uri: "time-language-language", glyph: 0xE700 },
    SettingsPage { title: "Region", uri: "time-language-region", glyph: 0xE700 },
    SettingsPage { title: "Typing", uri: "time-language-keyboard", glyph: 0xE77B },
    SettingsPage { title: "Gaming", uri: "gaming", glyph: 0xE713 },
    SettingsPage { title: "Game bar", uri: "gaming-gamebar", glyph: 0xE713 },
    SettingsPage { title: "Accessibility", uri: "easeofaccess", glyph: 0xE713 },
    SettingsPage { title: "Magnifier", uri: "easeofaccess-magnifier", glyph: 0xE713 },
    SettingsPage { title: "Keyboard", uri: "easeofaccess-keyboard", glyph: 0xE77B },
    SettingsPage { title: "Mouse pointer", uri: "easeofaccess-mouse", glyph: 0xE77F },
    SettingsPage { title: "Audio", uri: "easeofaccess-audio", glyph: 0xE768 },
    SettingsPage { title: "Display", uri: "easeofaccess-display", glyph: 0xE7F4 },
    SettingsPage { title: "Captions", uri: "easeofaccess-closedcaptioning", glyph: 0xE713 },
    SettingsPage { title: "Privacy & security", uri: "privacy", glyph: 0xE713 },
    SettingsPage { title: "Camera privacy", uri: "privacy-camera", glyph: 0xE77D },
    SettingsPage { title: "Microphone privacy", uri: "privacy-microphone", glyph: 0xE768 },
    SettingsPage { title: "Windows Update", uri: "windowsupdate", glyph: 0xE895 },
    SettingsPage { title: "Search", uri: "search", glyph: 0xE721 },
    SettingsPage { title: "Search permissions", uri: "search-permissions", glyph: 0xE721 },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_items_are_well_formed() {
        let items = settings_page_items();
        assert!(!items.is_empty());
        for item in &items {
            assert!(item.kind == "settings");
            assert!(item.path.starts_with("ms-settings:"));
            assert!(item.id.starts_with("settings:"));
            assert!(!item.title.is_empty());
        }
    }

    #[test]
    fn catalog_uris_are_unique() {
        let mut uris: Vec<&str> = SETTINGS_PAGES.iter().map(|p| p.uri).collect();
        let len = uris.len();
        uris.sort_unstable();
        uris.dedup();
        assert_eq!(uris.len(), len, "duplicate ms-settings URI in catalog");
    }

    #[test]
    fn known_page_glyph_resolves() {
        assert_eq!(settings_glyph("display"), 0xE7F4);
        assert_eq!(settings_glyph("no-such-page"), GLYPH_GEAR);
    }
}