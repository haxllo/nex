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
            // "sysdm" is a Control Panel dialog (no ms-settings: page);
            // everything else is an ms-settings:{uri} launch protocol.
            let (path, id) = if page.uri == "sysdm" {
                ("sysdm.cpl".to_string(), "settings:sysdm".to_string())
            } else {
                (
                    format!("ms-settings:{}", page.uri),
                    format!("settings:{}", page.uri),
                )
            };
            SearchItem::new(&id, "settings", page.title, &path)
        })
        .collect()
}

/// Glyph for a settings page (`ms-settings:{uri}` suffix), falling back
/// to the gear when the page has no dedicated glyph.
pub fn settings_glyph(uri: &str) -> u16 {
    if let Some((_, code)) = official_glyph(uri) {
        return code;
    }
    SETTINGS_PAGES
        .iter()
        .find(|page| page.uri == uri)
        .map(|page| page.glyph)
        .unwrap_or(GLYPH_GEAR)
}

/// True when the page's official glyph lives in the SettingsIcons.ttf
/// private font rather than the Segoe system fonts.
pub fn settings_glyph_is_file_font(uri: &str) -> bool {
    official_glyph(uri).map(|(file_font, _)| file_font).unwrap_or(false)
}

/// Official per-page glyphs scraped from the system's own settings
/// search index (`ImmersiveControlPanel/.../AllSystemSettings_*.xml`,
/// first Page node per entry joined to our URIs). Entries absent here
/// keep the curated Segoe codepoints above.
fn official_glyph(uri: &str) -> Option<(bool, u16)> {
    const TABLE: &[(&str, bool, u16)] = &[
        ("display", true, 0xE770),
        ("sound", false, 0xE767),
        ("notifications", true, 0xEA8F),
        ("multitasking", false, 0xEE40),
        ("bluetooth", false, 0xE772),
        ("network-airplanemode", false, 0xE709),
        ("personalization-background", false, 0xEB9F),
        ("personalization-colors", false, 0xE2B1),
        ("personalization-start", true, 0xE8FC),
        ("signinoptions", false, 0xE8D7),
        ("windowsupdate", false, 0xE895),
        ("storagesense", false, 0xEDA2),
        ("network-advancedsettings", false, 0xEDA3),
        ("otherusers", false, 0xE8FA),
        ("focus", true, 0xE5C4),
        ("clipboard", false, 0xE77F),
        ("about", false, 0xE946),
        ("devices", false, 0xE772),
        ("devices-touchpad", false, 0xEFA5),
        ("devices-printers", false, 0xE749),
        ("network-ethernet", false, 0xE839),
        ("network-proxy", false, 0xE968),
        ("taskbar", false, 0xE90E),
        ("privacy-microphone", false, 0xE720),
        ("troubleshoot", false, 0xE90F),
        ("speech", true, 0xEFA9),
        ("privacy-general", true, 0xF4DD),
        ("privacy-location", true, 0xE37A),
        ("privacy-notifications", true, 0xEA8F),
        ("privacy-documents", true, 0xE8A5),
        ("privacy-pictures", false, 0xEB9F),
        ("privacy-videos", false, 0xE8B2),
        ("easeofaccess-narrator", false, 0xed4d),
        ("backup", true, 0xee6e),
        ("workplace", false, 0xe821),
        ("savedpasskeys", true, 0xee7e),
        ("installed-apps", false, 0xe71d),
        ("advanced-apps", true, 0xefdd),
        ("defaultapps", true, 0xe42a),
        ("maps", false, 0xe826),
        ("pen", false, 0xedc6),
        ("devices-touch", false, 0xeda4),
        ("usb", false, 0xe88e),
        ("devices-keyboard", false, 0xe765),
        ("network-wifi", false, 0xe701),
        ("network-dialup", false, 0xe83c),
        ("themes", false, 0xe771),
        ("lockscreen", false, 0xee3f),
        ("fonts", false, 0xe8d2),
        ("privacy-speech", false, 0xefa9),
        ("cortana-windowssearch", true, 0xe773),
        ("privacy-webcam", false, 0xe722),
        ("privacy-contacts", false, 0xe716),
        ("privacy-calendar", false, 0xe787),
        ("privacy-email", false, 0xe715),
        ("privacy-messaging", false, 0xe8bd),
        ("privacy-radios", false, 0xec05),
        ("powersleep", false, 0xe7e8),
        ("project", false, 0xebc6),
        ("regionlanguage", false, 0xf2b7),
        ("typing", false, 0xe765),
    ];
    TABLE.iter().find(|(u, _, _)| *u == uri).map(|(_, f, c)| (*f, *c))
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
    SettingsPage { title: "Advanced display", uri: "display-advanced", glyph: 0xE7F4 },
    SettingsPage { title: "Graphics", uri: "display-advancedgraphics", glyph: 0xE7F4 },
    SettingsPage { title: "Recovery", uri: "recovery", glyph: 0xE72C },
    SettingsPage { title: "Activation", uri: "activation", glyph: 0xE713 },
    SettingsPage { title: "Storage Sense", uri: "storagesense", glyph: 0xE74E },
    SettingsPage { title: "Troubleshoot", uri: "troubleshoot", glyph: 0xE90F },
    SettingsPage { title: "Cameras", uri: "cameras", glyph: 0xE77D },
    SettingsPage { title: "AutoPlay", uri: "devices-autoplay", glyph: 0xE74E },
    SettingsPage { title: "Advanced network settings", uri: "network-advancedsettings", glyph: 0xE701 },
    SettingsPage { title: "Lock screen", uri: "personalization-lockscreen", glyph: 0xE72E },
    SettingsPage { title: "Themes", uri: "personalization-themes", glyph: 0xE790 },
    SettingsPage { title: "Fonts", uri: "personalization-fonts", glyph: 0xE8D2 },
    SettingsPage { title: "Device use", uri: "personalization-devices", glyph: 0xE713 },
    SettingsPage { title: "Taskbar items", uri: "personalization-taskbar-items", glyph: 0xE771 },
    SettingsPage { title: "System tray", uri: "personalization-taskbar-systemtray", glyph: 0xE771 },
    SettingsPage { title: "Installed apps", uri: "apps-installedapps", glyph: 0xE7C4 },
    SettingsPage { title: "Advanced app settings", uri: "apps-advancedsettings", glyph: 0xE713 },
    SettingsPage { title: "Websites", uri: "apps-websites", glyph: 0xE774 },
    SettingsPage { title: "Video playback", uri: "apps-videoplayback", glyph: 0xE714 },
    SettingsPage { title: "Email & accounts", uri: "emailandaccounts", glyph: 0xE715 },
    SettingsPage { title: "Windows backup", uri: "accounts-backup", glyph: 0xE713 },
    SettingsPage { title: "Other users", uri: "otherusers", glyph: 0xE716 },
    SettingsPage { title: "Pen & Windows Ink", uri: "handwriting", glyph: 0xE70F },
    SettingsPage { title: "Speech", uri: "speech", glyph: 0xE720 },
    SettingsPage { title: "Voice activation", uri: "voiceactivation", glyph: 0xE720 },
    SettingsPage { title: "Captures", uri: "gaming-gamedvr", glyph: 0xE714 },
    SettingsPage { title: "Game Mode", uri: "gaming-gamemode", glyph: 0xE7FC },
    SettingsPage { title: "Xbox networking", uri: "gaming-xboxnetwork", glyph: 0xE701 },
    SettingsPage { title: "Narrator", uri: "accessibility-narrator", glyph: 0xE713 },
    SettingsPage { title: "Eye control", uri: "accessibility-eyecontrol", glyph: 0xE713 },
    SettingsPage { title: "Visual effects", uri: "accessibility-visualeffects", glyph: 0xE790 },
    SettingsPage { title: "Text cursor", uri: "accessibility-textcursor", glyph: 0xE713 },
    SettingsPage { title: "Color filters", uri: "accessibility-colorfilters", glyph: 0xE790 },
    SettingsPage { title: "Contrast themes", uri: "accessibility-contrast", glyph: 0xE7B8 },
    SettingsPage { title: "Live captions", uri: "accessibility-livecaptions", glyph: 0xE8C9 },
    SettingsPage { title: "Voice access", uri: "accessibility-voiceaccess", glyph: 0xE720 },
    SettingsPage { title: "General privacy", uri: "privacy-general", glyph: 0xE713 },
    SettingsPage { title: "Location", uri: "privacy-location", glyph: 0xE81D },
    SettingsPage { title: "Notifications privacy", uri: "privacy-notifications", glyph: 0xE7C4 },
    SettingsPage { title: "Speech privacy", uri: "privacy-speechtyping", glyph: 0xE720 },
    SettingsPage { title: "Activity history", uri: "privacy-activitieshistory", glyph: 0xE81C },
    SettingsPage { title: "Documents", uri: "privacy-documents", glyph: 0xE8A5 },
    SettingsPage { title: "Pictures", uri: "privacy-pictures", glyph: 0xEB9F },
    SettingsPage { title: "Videos", uri: "privacy-videos", glyph: 0xE714 },
    SettingsPage { title: "Diagnostics & feedback", uri: "privacy-feedback", glyph: 0xE713 },
    SettingsPage { title: "Windows Security", uri: "windowsdefender", glyph: 0xE740 },
    SettingsPage { title: "Optional features", uri: "optionalfeatures", glyph: 0xE713 },
    SettingsPage { title: "For developers", uri: "developer", glyph: 0xE713 },
    SettingsPage { title: "Advanced options", uri: "windowsupdate-settings", glyph: 0xE895 },
    SettingsPage { title: "Optional updates", uri: "windowsupdate-optionalupdates", glyph: 0xE895 },
    SettingsPage { title: "Update history", uri: "windowsupdate-history", glyph: 0xE81C },
    SettingsPage { title: "Advanced system settings", uri: "sysdm", glyph: 0xE713 },
    SettingsPage { title: "Visual Effects", uri: "easeofaccess-visualeffects", glyph: 0xE713 },
    SettingsPage { title: "Mouse Pointer and Touch", uri: "easeofaccess-mousepointer", glyph: 0xE713 },
    SettingsPage { title: "Text Cursor", uri: "easeofaccess-cursor", glyph: 0xE713 },
    SettingsPage { title: "Color Filters", uri: "easeofaccess-colorfilter", glyph: 0xE713 },
    SettingsPage { title: "Contrast Themes", uri: "easeofaccess-highcontrast", glyph: 0xE713 },
    SettingsPage { title: "Narrator", uri: "easeofaccess-narrator", glyph: 0xE713 },
    SettingsPage { title: "Speech", uri: "easeofaccess-speechrecognition", glyph: 0xE713 },
    SettingsPage { title: "Eye Control", uri: "easeofaccess-eyecontrol", glyph: 0xE713 },
    SettingsPage { title: "Hearing Aids", uri: "easeofaccess-hearingaids", glyph: 0xE713 },
    SettingsPage { title: "Your Info", uri: "yourinfo", glyph: 0xE713 },
    SettingsPage { title: "Linked Devices", uri: "linkeddevices", glyph: 0xE713 },
    SettingsPage { title: "Family", uri: "family-group", glyph: 0xE713 },
    SettingsPage { title: "Windows Backup", uri: "backup", glyph: 0xE713 },
    SettingsPage { title: "Access Work or School", uri: "workplace", glyph: 0xE713 },
    SettingsPage { title: "Passkey Settings", uri: "savedpasskeys", glyph: 0xE713 },
    SettingsPage { title: "Dynamic Lock", uri: "signinoptions-dynamiclock", glyph: 0xE713 },
    SettingsPage { title: "Set Up Windows Hello Face Sign-in", uri: "signinoptions-launchfaceenrollment", glyph: 0xE713 },
    SettingsPage { title: "Set Up Fingerprint Sign-in", uri: "signinoptions-launchfingerprintenrollment", glyph: 0xE713 },
    SettingsPage { title: "Set Up Security Key Sign-in", uri: "signinoptions-launchsecuritykeyenrollment", glyph: 0xE713 },
    SettingsPage { title: "Set Up PIN Sign-in", uri: "signinoptions-launchpinenrollment", glyph: 0xE713 },
    SettingsPage { title: "Apps & Features", uri: "appsfeatures", glyph: 0xE7C4 },
    SettingsPage { title: "Installed Apps", uri: "installed-apps", glyph: 0xE713 },
    SettingsPage { title: "Advanced App Settings", uri: "advanced-apps", glyph: 0xE713 },
    SettingsPage { title: "Default Apps", uri: "defaultapps", glyph: 0xE713 },
    SettingsPage { title: "Default Browser", uri: "defaultbrowsersettings", glyph: 0xE7C4 },
    SettingsPage { title: "App Actions", uri: "appactions", glyph: 0xE7C4 },
    SettingsPage { title: "Offline Maps", uri: "maps", glyph: 0xE713 },
    SettingsPage { title: "Download Offline Maps", uri: "maps-downloadmaps", glyph: 0xE7C4 },
    SettingsPage { title: "Apps for Websites", uri: "appsforwebsites", glyph: 0xE7C4 },
    SettingsPage { title: "Video Playback", uri: "videoplayback", glyph: 0xE7C4 },
    SettingsPage { title: "Devices", uri: "connecteddevices", glyph: 0xE772 },
    SettingsPage { title: "Printers & Scanners", uri: "printers", glyph: 0xE772 },
    SettingsPage { title: "Mobile Devices", uri: "mobile-devices", glyph: 0xE772 },
    SettingsPage { title: "Link a Phone", uri: "mobile-devices-addphone", glyph: 0xE772 },
    SettingsPage { title: "Open Phone Link", uri: "mobile-devices-addphone-direct", glyph: 0xE772 },
    SettingsPage { title: "Cameras", uri: "camera", glyph: 0xE772 },
    SettingsPage { title: "Mouse", uri: "mousetouchpad", glyph: 0xE772 },
    SettingsPage { title: "Pen & Windows Ink", uri: "pen", glyph: 0xE713 },
    SettingsPage { title: "Pen Shortcut Button", uri: "pen-button", glyph: 0xE772 },
    SettingsPage { title: "Touch", uri: "devices-touch", glyph: 0xE713 },
    SettingsPage { title: "AutoPlay", uri: "autoplay", glyph: 0xE772 },
    SettingsPage { title: "USB", uri: "usb", glyph: 0xE713 },
    SettingsPage { title: "Keyboard", uri: "devices-keyboard", glyph: 0xE713 },
    SettingsPage { title: "Printer Auto Discovery", uri: "printers-autodiscovery", glyph: 0xE772 },
    SettingsPage { title: "Network & Internet", uri: "network-status", glyph: 0xE701 },
    SettingsPage { title: "Wi-Fi", uri: "network-wifi", glyph: 0xE713 },
    SettingsPage { title: "Manage Known Networks", uri: "network-wifisettings", glyph: 0xE701 },
    SettingsPage { title: "Mobile Hotspot", uri: "network-mobilehotspot", glyph: 0xE701 },
    SettingsPage { title: "Dial-up", uri: "network-dialup", glyph: 0xE713 },
    SettingsPage { title: "Cellular", uri: "network-cellular", glyph: 0xE701 },
    SettingsPage { title: "Cellular Device Identity", uri: "cellular-id", glyph: 0xE701 },
    SettingsPage { title: "Advanced Sharing Settings", uri: "network-advancedsharing", glyph: 0xE701 },
    SettingsPage { title: "Themes", uri: "themes", glyph: 0xE713 },
    SettingsPage { title: "Dynamic Lighting", uri: "personalization-lighting", glyph: 0xE771 },
    SettingsPage { title: "Lock Screen", uri: "lockscreen", glyph: 0xE713 },
    SettingsPage { title: "Text Input", uri: "personalization-textinput", glyph: 0xE771 },
    SettingsPage { title: "Touch Keyboard", uri: "personalization-touchkeyboard", glyph: 0xE771 },
    SettingsPage { title: "Customize Copilot Key", uri: "personalization-textinput-copilot-hardwarekey", glyph: 0xE771 },
    SettingsPage { title: "Start Folders", uri: "personalization-start-places", glyph: 0xE771 },
    SettingsPage { title: "Start Companions", uri: "personalization-start-companions", glyph: 0xE771 },
    SettingsPage { title: "Fonts", uri: "fonts", glyph: 0xE713 },
    SettingsPage { title: "Device Usage", uri: "deviceusage", glyph: 0xE771 },
    SettingsPage { title: "Find My Device", uri: "findmydevice", glyph: 0xE713 },
    SettingsPage { title: "Device Encryption", uri: "deviceencryption", glyph: 0xE713 },
    SettingsPage { title: "Speech", uri: "privacy-speech", glyph: 0xE713 },
    SettingsPage { title: "Activity History", uri: "privacy-activityhistory", glyph: 0xE713 },
    SettingsPage { title: "Searching Windows", uri: "cortana-windowssearch", glyph: 0xE713 },
    SettingsPage { title: "Passkeys", uri: "privacy-passkeys", glyph: 0xE713 },
    SettingsPage { title: "AI Models", uri: "privacy-systemaimodels", glyph: 0xE713 },
    SettingsPage { title: "Camera", uri: "privacy-webcam", glyph: 0xE713 },
    SettingsPage { title: "Voice Activation", uri: "privacy-voiceactivation", glyph: 0xE713 },
    SettingsPage { title: "Account Info", uri: "privacy-accountinfo", glyph: 0xE713 },
    SettingsPage { title: "Contacts", uri: "privacy-contacts", glyph: 0xE713 },
    SettingsPage { title: "Calendar", uri: "privacy-calendar", glyph: 0xE713 },
    SettingsPage { title: "Phone Calls", uri: "privacy-phonecalls", glyph: 0xE713 },
    SettingsPage { title: "Call History", uri: "privacy-callhistory", glyph: 0xE713 },
    SettingsPage { title: "Email", uri: "privacy-email", glyph: 0xE713 },
    SettingsPage { title: "Tasks", uri: "privacy-tasks", glyph: 0xE713 },
    SettingsPage { title: "Messaging", uri: "privacy-messaging", glyph: 0xE713 },
    SettingsPage { title: "Motion", uri: "privacy-motion", glyph: 0xE713 },
    SettingsPage { title: "Radios", uri: "privacy-radios", glyph: 0xE713 },
    SettingsPage { title: "Other Devices", uri: "privacy-customdevices", glyph: 0xE713 },
    SettingsPage { title: "App Diagnostics", uri: "privacy-appdiagnostics", glyph: 0xE713 },
    SettingsPage { title: "Automatic File Downloads", uri: "privacy-automaticfiledownloads", glyph: 0xE713 },
    SettingsPage { title: "Downloads Folder", uri: "privacy-downloadsfolder", glyph: 0xE713 },
    SettingsPage { title: "Music Library", uri: "privacy-musiclibrary", glyph: 0xE713 },
    SettingsPage { title: "File System", uri: "privacy-broadfilesystemaccess", glyph: 0xE713 },
    SettingsPage { title: "Screenshot Borders", uri: "privacy-graphicscapturewithoutborder", glyph: 0xE713 },
    SettingsPage { title: "Screenshots and Apps", uri: "privacy-graphicscaptureprogrammatic", glyph: 0xE713 },
    SettingsPage { title: "HDR Settings", uri: "display-hdr", glyph: 0xE713 },
    SettingsPage { title: "Default Graphics Settings", uri: "display-advancedgraphics-default", glyph: 0xE713 },
    SettingsPage { title: "Volume Mixer", uri: "apps-volume", glyph: 0xE713 },
    SettingsPage { title: "All Sound Devices", uri: "sound-devices", glyph: 0xE713 },
    SettingsPage { title: "Default Audio Output", uri: "sound-defaultoutputproperties", glyph: 0xE713 },
    SettingsPage { title: "Default Microphone", uri: "sound-defaultinputproperties", glyph: 0xE713 },
    SettingsPage { title: "Focus", uri: "quiethours", glyph: 0xE713 },
    SettingsPage { title: "Power & Battery", uri: "powersleep", glyph: 0xE713 },
    SettingsPage { title: "Battery Saver", uri: "batterysaver-settings", glyph: 0xE713 },
    SettingsPage { title: "Battery Usage", uri: "batterysaver-usagedetails", glyph: 0xE713 },
    SettingsPage { title: "Storage Sense", uri: "storagepolicies", glyph: 0xE713 },
    SettingsPage { title: "Cleanup Recommendations", uri: "storagerecommendations", glyph: 0xE713 },
    SettingsPage { title: "Default Save Locations", uri: "savelocations", glyph: 0xE713 },
    SettingsPage { title: "Disks & Volumes", uri: "disksandvolumes", glyph: 0xE713 },
    SettingsPage { title: "Energy Recommendations", uri: "energyrecommendations", glyph: 0xE713 },
    SettingsPage { title: "Nearby Sharing", uri: "crossdevice", glyph: 0xE713 },
    SettingsPage { title: "For Developers", uri: "developers", glyph: 0xE713 },
    SettingsPage { title: "Projecting to This PC", uri: "project", glyph: 0xE713 },
    SettingsPage { title: "Remote Desktop", uri: "remotedesktop", glyph: 0xE713 },
    SettingsPage { title: "Control Center", uri: "controlcenter", glyph: 0xE713 },
    SettingsPage { title: "System Components", uri: "systemcomponents", glyph: 0xE713 },
    SettingsPage { title: "AI Components", uri: "aicomponents", glyph: 0xE713 },
    SettingsPage { title: "Settings Home", uri: "home", glyph: 0xE713 },
    SettingsPage { title: "Date & Time", uri: "dateandtime", glyph: 0xE823 },
    SettingsPage { title: "Language & Region", uri: "regionlanguage", glyph: 0xE713 },
    SettingsPage { title: "Regional Format", uri: "regionformatting", glyph: 0xE823 },
    SettingsPage { title: "Typing", uri: "typing", glyph: 0xE713 },
    SettingsPage { title: "Advanced Keyboard Settings", uri: "keyboard-advanced", glyph: 0xE823 },
    SettingsPage { title: "Text Suggestions", uri: "devicestyping-hwkbtextsuggestions", glyph: 0xE823 },
    SettingsPage { title: "Add a Display Language", uri: "regionlanguage-adddisplaylanguage", glyph: 0xE823 },
    SettingsPage { title: "Set Display Language", uri: "regionlanguage-setdisplaylanguage", glyph: 0xE823 },
    SettingsPage { title: "Check for Windows Updates", uri: "windowsupdate-action", glyph: 0xE895 },
    SettingsPage { title: "Advanced Options", uri: "windowsupdate-options", glyph: 0xE895 },
    SettingsPage { title: "Active Hours", uri: "windowsupdate-activehours", glyph: 0xE895 },
    SettingsPage { title: "Uninstall Updates", uri: "windowsupdate-uninstallupdates", glyph: 0xE895 },
    SettingsPage { title: "Restart Options", uri: "windowsupdate-restartoptions", glyph: 0xE895 },
    SettingsPage { title: "Delivery Optimization", uri: "delivery-optimization", glyph: 0xE895 },
    SettingsPage { title: "Delivery Optimization Activity", uri: "delivery-optimization-activity", glyph: 0xE895 },
    SettingsPage { title: "Delivery Optimization Advanced Options", uri: "delivery-optimization-advanced", glyph: 0xE895 },
    SettingsPage { title: "Windows Insider Program", uri: "windowsinsider-optin", glyph: 0xE895 },
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
            let is_ms_settings = item.path.starts_with("ms-settings:");
            let is_sysdm = item.path == "sysdm.cpl";
            assert!(is_ms_settings || is_sysdm, "unexpected path {}", item.path);
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
        // Official override wins over the curated codepoint.
        assert_eq!(settings_glyph("display"), 0xE770);
        assert!(settings_glyph_is_file_font("display"));
        assert!(!settings_glyph_is_file_font("sound"));
        assert_eq!(settings_glyph("no-such-page"), GLYPH_GEAR);
    }
}