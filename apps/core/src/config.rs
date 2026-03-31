use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const APP_DISPLAY_NAME: &str = "Nex";
pub const LEGACY_APP_DISPLAY_NAME: &str = "SwiftFind";
#[cfg(target_os = "windows")]
const APP_DIR_NAME_WINDOWS: &str = "Nex";
#[cfg(target_os = "windows")]
const LEGACY_APP_DIR_NAME_WINDOWS: &str = "SwiftFind";
const APP_DIR_NAME_UNIX: &str = "nex";
const LEGACY_APP_DIR_NAME_UNIX: &str = "swiftfind";
const CONFIG_FILE_NAME: &str = "config.toml";
const LEGACY_CONFIG_FILE_NAME: &str = "config.json";

pub const CURRENT_CONFIG_VERSION: u32 = 11;
const LEGACY_IDLE_CACHE_TRIM_MS_V1: u32 = 1200;
const LEGACY_ACTIVE_MEMORY_TARGET_MB_V1: u16 = 80;
const TEMPLATE_REQUIRED_KEYS: &[&str] = &[
    "hotkey",
    "launch_at_startup",
    "max_results",
    "discovery_roots",
    "discovery_exclude_roots",
    "windows_search_enabled",
    "windows_search_fallback_filesystem",
    "show_files",
    "show_folders",
    "search_mode_default",
    "search_dsl_enabled",
    "search_query_results_with_delay",
    "search_delay_time_ms",
    "uninstall_actions_enabled",
    "web_search_provider",
    "web_search_custom_template",
    "clipboard_enabled",
    "clipboard_retention_minutes",
    "clipboard_exclude_sensitive_patterns",
    "plugins_enabled",
    "plugins_safe_mode",
    "game_mode_enabled",
    "plugin_paths",
    "idle_cache_trim_ms",
    "active_memory_target_mb",
    "index_max_items_total",
    "index_max_items_per_root",
    "index_max_items_per_query_seed",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    All,
    Apps,
    Files,
    Actions,
    Clipboard,
}

impl SearchMode {
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "all" => Some(Self::All),
            "apps" | "app" => Some(Self::Apps),
            "files" | "file" => Some(Self::Files),
            "actions" | "action" => Some(Self::Actions),
            "clipboard" | "clip" => Some(Self::Clipboard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    Duckduckgo,
    #[default]
    Google,
    Bing,
    Brave,
    Startpage,
    Ecosia,
    Yahoo,
    Custom,
}

impl WebSearchProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Duckduckgo => "DuckDuckGo",
            Self::Google => "Google",
            Self::Bing => "Bing",
            Self::Brave => "Brave",
            Self::Startpage => "Startpage",
            Self::Ecosia => "Ecosia",
            Self::Yahoo => "Yahoo",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub max_results: u16,
    pub index_db_path: PathBuf,
    pub config_path: PathBuf,
    pub discovery_roots: Vec<PathBuf>,
    pub discovery_exclude_roots: Vec<PathBuf>,
    pub windows_search_enabled: bool,
    pub windows_search_fallback_filesystem: bool,
    pub show_files: bool,
    pub show_folders: bool,
    pub hotkey: String,
    pub launch_at_startup: bool,
    pub hotkey_help: String,
    pub hotkey_recommended: Vec<String>,
    pub search_mode_default: SearchMode,
    pub search_dsl_enabled: bool,
    pub search_query_results_with_delay: bool,
    pub search_delay_time_ms: u16,
    pub uninstall_actions_enabled: bool,
    pub web_search_provider: WebSearchProvider,
    pub web_search_custom_template: String,
    pub clipboard_enabled: bool,
    pub clipboard_retention_minutes: u32,
    pub clipboard_exclude_sensitive_patterns: Vec<String>,
    pub plugins_enabled: bool,
    pub plugin_paths: Vec<PathBuf>,
    pub plugins_safe_mode: bool,
    pub game_mode_enabled: bool,
    pub idle_cache_trim_ms: u32,
    pub active_memory_target_mb: u16,
    pub index_max_items_total: u32,
    pub index_max_items_per_root: u32,
    pub index_max_items_per_query_seed: u32,
}

impl Default for Config {
    fn default() -> Self {
        let app_dir = stable_app_data_dir();
        let config_path = app_dir.join(CONFIG_FILE_NAME);
        Self {
            version: CURRENT_CONFIG_VERSION,
            max_results: 20,
            index_db_path: app_dir.join("index.sqlite3"),
            config_path,
            discovery_roots: default_discovery_roots(),
            discovery_exclude_roots: default_discovery_exclude_roots(),
            windows_search_enabled: true,
            windows_search_fallback_filesystem: true,
            show_files: true,
            show_folders: true,
            hotkey: "Ctrl+Space".to_string(),
            launch_at_startup: false,
            hotkey_help: format!(
                "Set `hotkey` as Modifier+Key (example: Ctrl+Space), then restart {APP_DISPLAY_NAME}."
            ),
            hotkey_recommended: vec![
                "Ctrl+Space".to_string(),
                "Ctrl+Shift+Space".to_string(),
                "Ctrl+Alt+Space".to_string(),
                "Alt+Shift+Space".to_string(),
                "Ctrl+Shift+P".to_string(),
                "Ctrl+Alt+P".to_string(),
            ],
            search_mode_default: SearchMode::All,
            search_dsl_enabled: true,
            search_query_results_with_delay: true,
            search_delay_time_ms: 90,
            uninstall_actions_enabled: true,
            web_search_provider: WebSearchProvider::Google,
            web_search_custom_template: String::new(),
            clipboard_enabled: true,
            clipboard_retention_minutes: 8 * 60,
            clipboard_exclude_sensitive_patterns: vec![
                "password".to_string(),
                "passcode".to_string(),
                "otp".to_string(),
                "token".to_string(),
                "secret".to_string(),
                "apikey".to_string(),
                "api_key".to_string(),
            ],
            plugins_enabled: true,
            plugin_paths: vec![app_dir.join("plugins")],
            plugins_safe_mode: true,
            game_mode_enabled: false,
            idle_cache_trim_ms: 900,
            active_memory_target_mb: 72,
            index_max_items_total: 120_000,
            index_max_items_per_root: 40_000,
            index_max_items_per_query_seed: 5_000,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(String),
    Validation(String),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Parse(error) => write!(f, "parse error: {error}"),
            Self::Validation(error) => write!(f, "validation error: {error}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(value: serde_json::Error) -> Self {
        Self::Parse(value.to_string())
    }
}

pub fn stable_app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(preferred) = windows_app_data_dir(APP_DIR_NAME_WINDOWS) {
            return migrate_legacy_app_data_dir(
                preferred,
                windows_app_data_dir(LEGACY_APP_DIR_NAME_WINDOWS),
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(preferred) = unix_app_data_dir(APP_DIR_NAME_UNIX) {
            return migrate_legacy_app_data_dir(
                preferred,
                unix_app_data_dir(LEGACY_APP_DIR_NAME_UNIX),
            );
        }
    }

    std::env::temp_dir().join(APP_DIR_NAME_UNIX)
}

pub fn stable_config_path() -> PathBuf {
    stable_app_data_dir().join(CONFIG_FILE_NAME)
}

fn stable_legacy_config_paths() -> Vec<PathBuf> {
    let current_dir = stable_app_data_dir();
    let mut paths = vec![current_dir.join(LEGACY_CONFIG_FILE_NAME)];

    #[cfg(target_os = "windows")]
    {
        if let Some(legacy_dir) = windows_app_data_dir(LEGACY_APP_DIR_NAME_WINDOWS) {
            if legacy_dir != current_dir {
                paths.push(legacy_dir.join(LEGACY_CONFIG_FILE_NAME));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(legacy_dir) = unix_app_data_dir(LEGACY_APP_DIR_NAME_UNIX) {
            if legacy_dir != current_dir {
                paths.push(legacy_dir.join(LEGACY_CONFIG_FILE_NAME));
            }
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

fn is_toml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("toml"))
        .unwrap_or(false)
}

pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
    let resolved_path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(stable_config_path);

    if !resolved_path.exists() {
        if path.is_none() {
            if let Some(legacy_path) = stable_legacy_config_paths()
                .into_iter()
                .find(|candidate| candidate.exists())
            {
                let raw = std::fs::read_to_string(&legacy_path)?;
                let mut cfg: Config = parse_text(&raw)?;
                let source_version = cfg.version;
                cfg.config_path = resolved_path.clone();

                if cfg.index_db_path.as_os_str().is_empty() {
                    cfg.index_db_path = resolved_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join("index.sqlite3");
                }

                let mut should_persist_migration = apply_migrations(&mut cfg, &raw);
                should_persist_migration |= rewrite_managed_paths_to_current_app_dir(&mut cfg);
                validate(&cfg).map_err(ConfigError::Validation)?;
                if should_persist_migration {
                    persist_migrated_config(
                        &cfg,
                        &resolved_path,
                        &raw,
                        source_version,
                        &legacy_path,
                    )?;
                }
                return Ok(cfg);
            }
        }

        let cfg = default_for_path(&resolved_path);
        validate(&cfg).map_err(ConfigError::Validation)?;
        return Ok(cfg);
    }

    let raw = std::fs::read_to_string(&resolved_path)?;
    let mut cfg: Config = parse_text(&raw)?;
    let source_version = cfg.version;
    cfg.config_path = resolved_path.clone();

    if cfg.index_db_path.as_os_str().is_empty() {
        cfg.index_db_path = resolved_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("index.sqlite3");
    }

    let mut should_persist_migration = apply_migrations(&mut cfg, &raw);
    should_persist_migration |= rewrite_managed_paths_to_current_app_dir(&mut cfg);
    validate(&cfg).map_err(ConfigError::Validation)?;
    if should_persist_migration {
        persist_migrated_config(&cfg, &resolved_path, &raw, source_version, &resolved_path)?;
    }
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<(), ConfigError> {
    save_to_path(cfg, &cfg.config_path)
}

pub fn save_to_path(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    validate(cfg).map_err(ConfigError::Validation)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let encoded = if is_toml_path(path) {
        toml::to_string_pretty(cfg)
            .map_err(|error| ConfigError::Parse(format!("toml encode error: {error}")))?
    } else {
        serde_json::to_string_pretty(cfg)?
    };
    write_atomic(path, &encoded)
}

pub fn write_user_template(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    validate(cfg).map_err(ConfigError::Validation)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if is_toml_path(path) {
        return write_user_template_toml(cfg, path);
    }

    let roots_section = json5_path_array_section(&cfg.discovery_roots);
    let excluded_roots_section = json5_path_array_section(&cfg.discovery_exclude_roots);

    let mut text = String::new();
    text.push_str("{\n");
    text.push_str("  // Nex config (comments are allowed).\n");
    text.push_str("  //\n");
    text.push_str("  // How to use this file:\n");
    text.push_str("  // - Edit values and save.\n");
    text.push_str("  // - Most settings apply automatically within about 1 second.\n");
    text.push_str("  // - Restart required after changing hotkey or index_db_path.\n");
    text.push_str("  // - Keep strings in double quotes.\n");
    text.push_str(
        "  // - Use double backslashes for Windows paths (C:\\\\Users\\\\Admin\\\\...).\n",
    );
    text.push_str("  // - true/false and numbers must not be quoted.\n");
    text.push_str("  // - This file is the active config while using .json format.\n");
    text.push_str("  //\n");
    text.push_str("  // Quick setup:\n");
    text.push_str("  // 1) Keep exactly ONE `hotkey` line uncommented.\n");
    text.push_str("  // 2) Save file.\n");
    text.push_str("  // 3) Restart only if you changed hotkey/index_db_path.\n");
    text.push_str("  //\n");
    text.push_str("  // Safer Windows-friendly hotkeys (uncomment one if you prefer):\n");

    for option in &cfg.hotkey_recommended {
        if option != &cfg.hotkey {
            text.push_str("  // \"hotkey\": ");
            text.push_str(&json_string(option));
            text.push_str(",\n");
        }
    }

    text.push_str(
        "  // Avoid common OS-reserved/conflicting shortcuts like Win+..., Alt+Tab, Ctrl+Esc.\n",
    );
    text.push_str("  \"hotkey\": ");
    text.push_str(&json_string(&cfg.hotkey));
    text.push_str(",\n");
    text.push_str("  // Start Nex automatically when you sign in (true/false)\n");
    text.push_str("  \"launch_at_startup\": ");
    text.push_str(if cfg.launch_at_startup {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n\n");

    text.push_str("  // Optional tuning:\n");
    text.push_str("  // Number of results shown per query (valid range: 5..100)\n");
    text.push_str("  \"max_results\": ");
    text.push_str(&cfg.max_results.to_string());
    text.push_str(",\n\n");

    text.push_str("  // Optional: folders scanned for local files.\n");
    text.push_str("  // Add/remove paths as needed.\n");
    text.push_str("  \"discovery_roots\": ");
    text.push_str(&roots_section);
    text.push_str(",\n\n");
    text.push_str("  // Optional: folders to exclude from local-file discovery.\n");
    text.push_str("  // Any file/folder under these roots is ignored.\n");
    text.push_str("  // Nex also skips common system/temp/dev-noise paths automatically\n");
    text.push_str("  // (for example: Windows, Program Files, AppData, node_modules, .git, __pycache__).\n");
    text.push_str("  // These built-in exclusions affect file/folder indexing only, not app discovery.\n");
    text.push_str("  \"discovery_exclude_roots\": ");
    text.push_str(&excluded_roots_section);
    text.push_str(",\n\n");
    text.push_str("  // Use Windows Search index for file/folder discovery when available.\n");
    text.push_str("  \"windows_search_enabled\": ");
    text.push_str(if cfg.windows_search_enabled {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n");
    text.push_str("  // Fall back to direct filesystem scan when Windows Search is unavailable.\n");
    text.push_str("  \"windows_search_fallback_filesystem\": ");
    text.push_str(if cfg.windows_search_fallback_filesystem {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n\n");
    text.push_str("  // Toggle file and folder visibility in results.\n");
    text.push_str("  \"show_files\": ");
    text.push_str(if cfg.show_files { "true" } else { "false" });
    text.push_str(",\n");
    text.push_str("  \"show_folders\": ");
    text.push_str(if cfg.show_folders { "true" } else { "false" });
    text.push_str(",\n\n");

    text.push_str("  // Search mode default: all | apps | files | actions | clipboard\n");
    text.push_str("  \"search_mode_default\": ");
    text.push_str(&json_string(match cfg.search_mode_default {
        SearchMode::All => "all",
        SearchMode::Apps => "apps",
        SearchMode::Files => "files",
        SearchMode::Actions => "actions",
        SearchMode::Clipboard => "clipboard",
    }));
    text.push_str(",\n");
    text.push_str(
        "  // Enable query operators like kind:, modified:, created:, AND/OR/NOT and -term\n",
    );
    text.push_str("  \"search_dsl_enabled\": ");
    text.push_str(if cfg.search_dsl_enabled {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n");
    text.push_str("  // Delay query execution while typing for smoother UI updates\n");
    text.push_str("  \"search_query_results_with_delay\": ");
    text.push_str(if cfg.search_query_results_with_delay {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n");
    text.push_str("  // Typing delay in milliseconds (valid range: 10..2000)\n");
    text.push_str("  \"search_delay_time_ms\": ");
    text.push_str(&cfg.search_delay_time_ms.to_string());
    text.push_str(",\n");
    text.push_str("  // Enable command mode uninstall actions (e.g. > uninstall appname)\n");
    text.push_str("  \"uninstall_actions_enabled\": ");
    text.push_str(if cfg.uninstall_actions_enabled {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n\n");
    text.push_str("  // Web search in command mode (press > then type your query)\n");
    text.push_str("  // Default is google for most users.\n");
    text.push_str(
        "  // Options: google | duckduckgo | bing | brave | startpage | ecosia | yahoo | custom\n",
    );
    text.push_str("  \"web_search_provider\": ");
    text.push_str(&json_string(match cfg.web_search_provider {
        WebSearchProvider::Duckduckgo => "duckduckgo",
        WebSearchProvider::Google => "google",
        WebSearchProvider::Bing => "bing",
        WebSearchProvider::Brave => "brave",
        WebSearchProvider::Startpage => "startpage",
        WebSearchProvider::Ecosia => "ecosia",
        WebSearchProvider::Yahoo => "yahoo",
        WebSearchProvider::Custom => "custom",
    }));
    text.push_str(",\n");
    text.push_str("  // Used only when provider is custom. Must include {query}.\n");
    text.push_str("  // Example: \"https://example.com/search?q={query}\"\n");
    text.push_str("  \"web_search_custom_template\": ");
    text.push_str(&json_string(&cfg.web_search_custom_template));
    text.push_str(",\n\n");

    text.push_str("  // Clipboard history provider settings\n");
    text.push_str("  \"clipboard_enabled\": ");
    text.push_str(if cfg.clipboard_enabled {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n");
    text.push_str("  // Retention in minutes (valid range: 5..43200)\n");
    text.push_str("  \"clipboard_retention_minutes\": ");
    text.push_str(&cfg.clipboard_retention_minutes.to_string());
    text.push_str(",\n");
    text.push_str(
        "  // Substring patterns that should be skipped when capturing clipboard entries\n",
    );
    text.push_str("  \"clipboard_exclude_sensitive_patterns\": [\n");
    for (idx, pattern) in cfg.clipboard_exclude_sensitive_patterns.iter().enumerate() {
        text.push_str("    ");
        text.push_str(&json_string(pattern));
        if idx + 1 != cfg.clipboard_exclude_sensitive_patterns.len() {
            text.push(',');
        }
        text.push('\n');
    }
    text.push_str("  ],\n\n");

    text.push_str("  // Plugin SDK settings\n");
    text.push_str("  \"plugins_enabled\": ");
    text.push_str(if cfg.plugins_enabled { "true" } else { "false" });
    text.push_str(",\n");
    text.push_str("  // Keep safe mode true to prevent plugin command execution.\n");
    text.push_str("  \"plugins_safe_mode\": ");
    text.push_str(if cfg.plugins_safe_mode {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n");
    text.push_str("  // Game Mode: suppress the launcher hotkey while a likely game/fullscreen app is active.\n");
    text.push_str("  \"game_mode_enabled\": ");
    text.push_str(if cfg.game_mode_enabled {
        "true"
    } else {
        "false"
    });
    text.push_str(",\n");
    text.push_str("  \"plugin_paths\": [\n");
    for (idx, path) in cfg.plugin_paths.iter().enumerate() {
        text.push_str("    ");
        text.push_str(&json_string(&path.to_string_lossy()));
        if idx + 1 != cfg.plugin_paths.len() {
            text.push(',');
        }
        text.push('\n');
    }
    text.push_str("  ],\n\n");

    text.push_str("  // Runtime performance targets\n");
    text.push_str("  // cache trim after hide in milliseconds (valid range: 100..10000)\n");
    text.push_str("  \"idle_cache_trim_ms\": ");
    text.push_str(&cfg.idle_cache_trim_ms.to_string());
    text.push_str(",\n");
    text.push_str("  // active memory target in MB (valid range: 20..512)\n");
    text.push_str("  \"active_memory_target_mb\": ");
    text.push_str(&cfg.active_memory_target_mb.to_string());
    text.push_str(",\n");
    text.push_str("  // Maximum indexed file/folder items retained in database discovery pass\n");
    text.push_str("  \"index_max_items_total\": ");
    text.push_str(&cfg.index_max_items_total.to_string());
    text.push_str(",\n");
    text.push_str("  // Maximum indexed file/folder items retained per discovery root\n");
    text.push_str("  \"index_max_items_per_root\": ");
    text.push_str(&cfg.index_max_items_per_root.to_string());
    text.push_str(",\n");
    text.push_str("  // Runtime candidate budget for per-query file/folder retrieval\n");
    text.push_str("  \"index_max_items_per_query_seed\": ");
    text.push_str(&cfg.index_max_items_per_query_seed.to_string());
    text.push('\n');
    text.push_str("}\n");

    std::fs::write(path, text)?;
    Ok(())
}

fn write_user_template_toml(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    let roots_section = toml_path_array_section(&cfg.discovery_roots);
    let excluded_roots_section = toml_path_array_section(&cfg.discovery_exclude_roots);
    let plugin_paths_section = toml_path_array_section(&cfg.plugin_paths);
    let clipboard_patterns_section =
        toml_string_array_section(&cfg.clipboard_exclude_sensitive_patterns);

    let mut text = String::new();
    text.push_str("# Nex config (TOML format).\n");
    text.push_str("#\n");
    text.push_str("# How to use this file:\n");
    text.push_str("# - Edit values and save.\n");
    text.push_str("# - Most settings apply automatically within about 1 second.\n");
    text.push_str("# - Restart required after changing hotkey or index_db_path.\n");
    text.push_str("# - Strings must be in quotes (example: hotkey = \"Ctrl+Space\").\n");
    text.push_str("# - Use double backslashes for Windows paths (C:\\\\Users\\\\Admin\\\\...).\n");
    text.push_str("# - true/false and numbers are not quoted.\n");
    text.push_str("# - This is the active config. Legacy config.json is kept only as backup.\n");
    text.push_str("#\n");
    text.push_str("# Quick setup:\n");
    text.push_str("# 1) Keep exactly ONE hotkey value.\n");
    text.push_str("# 2) Save file.\n");
    text.push_str("# 3) Restart only if you changed hotkey/index_db_path.\n");
    text.push_str("#\n");
    text.push_str("# Safer Windows-friendly hotkeys you can use:\n");
    for option in &cfg.hotkey_recommended {
        text.push_str("# hotkey = ");
        text.push_str(&json_string(option));
        text.push('\n');
    }
    text.push_str("# Avoid common OS-reserved shortcuts like Win+..., Alt+Tab, Ctrl+Esc.\n");
    text.push_str("hotkey = ");
    text.push_str(&json_string(&cfg.hotkey));
    text.push_str("\n\n");

    text.push_str("# Start Nex automatically when you sign in (true/false)\n");
    text.push_str("launch_at_startup = ");
    text.push_str(if cfg.launch_at_startup {
        "true"
    } else {
        "false"
    });
    text.push_str("\n\n");

    text.push_str("# Number of results shown per query (valid range: 5..100)\n");
    text.push_str("max_results = ");
    text.push_str(&cfg.max_results.to_string());
    text.push_str("\n\n");

    text.push_str("# Folders scanned for local files.\n");
    text.push_str("discovery_roots = ");
    text.push_str(&roots_section);
    text.push_str("\n\n");
    text.push_str("# Folders excluded from local-file discovery.\n");
    text.push_str("# Nex also skips common system/temp/dev-noise paths automatically\n");
    text.push_str("# (for example: Windows, Program Files, AppData, node_modules, .git, __pycache__).\n");
    text.push_str("# Built-in exclusions affect file/folder indexing only, not app discovery.\n");
    text.push_str("discovery_exclude_roots = ");
    text.push_str(&excluded_roots_section);
    text.push_str("\n\n");

    text.push_str("# Use Windows Search index for file/folder discovery when available.\n");
    text.push_str("windows_search_enabled = ");
    text.push_str(if cfg.windows_search_enabled {
        "true"
    } else {
        "false"
    });
    text.push('\n');
    text.push_str("# Fall back to direct filesystem scan when Windows Search is unavailable.\n");
    text.push_str("windows_search_fallback_filesystem = ");
    text.push_str(if cfg.windows_search_fallback_filesystem {
        "true"
    } else {
        "false"
    });
    text.push_str("\n\n");

    text.push_str("# Toggle file and folder visibility in results.\n");
    text.push_str("show_files = ");
    text.push_str(if cfg.show_files { "true" } else { "false" });
    text.push('\n');
    text.push_str("show_folders = ");
    text.push_str(if cfg.show_folders { "true" } else { "false" });
    text.push_str("\n\n");

    text.push_str("# Search mode default: all | apps | files | actions | clipboard\n");
    text.push_str("search_mode_default = ");
    text.push_str(&json_string(match cfg.search_mode_default {
        SearchMode::All => "all",
        SearchMode::Apps => "apps",
        SearchMode::Files => "files",
        SearchMode::Actions => "actions",
        SearchMode::Clipboard => "clipboard",
    }));
    text.push('\n');
    text.push_str(
        "# Enable query operators like kind:, modified:, created:, AND/OR/NOT and -term\n",
    );
    text.push_str("search_dsl_enabled = ");
    text.push_str(if cfg.search_dsl_enabled {
        "true"
    } else {
        "false"
    });
    text.push('\n');
    text.push_str("# Delay query execution while typing for smoother UI updates\n");
    text.push_str("search_query_results_with_delay = ");
    text.push_str(if cfg.search_query_results_with_delay {
        "true"
    } else {
        "false"
    });
    text.push('\n');
    text.push_str("# Typing delay in milliseconds (valid range: 10..2000)\n");
    text.push_str("search_delay_time_ms = ");
    text.push_str(&cfg.search_delay_time_ms.to_string());
    text.push('\n');
    text.push_str("# Enable command mode uninstall actions (e.g. > uninstall appname)\n");
    text.push_str("uninstall_actions_enabled = ");
    text.push_str(if cfg.uninstall_actions_enabled {
        "true"
    } else {
        "false"
    });
    text.push_str("\n\n");

    text.push_str("# Web search in command mode (press > then type your query)\n");
    text.push_str("# Default is google for most users.\n");
    text.push_str(
        "# Options: google | duckduckgo | bing | brave | startpage | ecosia | yahoo | custom\n",
    );
    text.push_str("web_search_provider = ");
    text.push_str(&json_string(match cfg.web_search_provider {
        WebSearchProvider::Duckduckgo => "duckduckgo",
        WebSearchProvider::Google => "google",
        WebSearchProvider::Bing => "bing",
        WebSearchProvider::Brave => "brave",
        WebSearchProvider::Startpage => "startpage",
        WebSearchProvider::Ecosia => "ecosia",
        WebSearchProvider::Yahoo => "yahoo",
        WebSearchProvider::Custom => "custom",
    }));
    text.push('\n');
    text.push_str("# Used only when provider is custom. Must include {query}.\n");
    text.push_str("# Example: \"https://example.com/search?q={query}\"\n");
    text.push_str("web_search_custom_template = ");
    text.push_str(&json_string(&cfg.web_search_custom_template));
    text.push_str("\n\n");

    text.push_str("# Clipboard history provider settings\n");
    text.push_str("clipboard_enabled = ");
    text.push_str(if cfg.clipboard_enabled {
        "true"
    } else {
        "false"
    });
    text.push('\n');
    text.push_str("# Retention in minutes (valid range: 5..43200)\n");
    text.push_str("clipboard_retention_minutes = ");
    text.push_str(&cfg.clipboard_retention_minutes.to_string());
    text.push('\n');
    text.push_str("# Substring patterns skipped when capturing clipboard entries\n");
    text.push_str("clipboard_exclude_sensitive_patterns = ");
    text.push_str(&clipboard_patterns_section);
    text.push_str("\n\n");

    text.push_str("# Plugin SDK settings\n");
    text.push_str("plugins_enabled = ");
    text.push_str(if cfg.plugins_enabled { "true" } else { "false" });
    text.push('\n');
    text.push_str("# Keep safe mode true to prevent plugin command execution.\n");
    text.push_str("plugins_safe_mode = ");
    text.push_str(if cfg.plugins_safe_mode {
        "true"
    } else {
        "false"
    });
    text.push('\n');
    text.push_str("# Game Mode: suppress the launcher hotkey while a likely game/fullscreen app is active.\n");
    text.push_str("game_mode_enabled = ");
    text.push_str(if cfg.game_mode_enabled {
        "true"
    } else {
        "false"
    });
    text.push('\n');
    text.push_str("plugin_paths = ");
    text.push_str(&plugin_paths_section);
    text.push_str("\n\n");

    text.push_str("# Runtime performance targets\n");
    text.push_str("# cache trim after hide in milliseconds (valid range: 100..10000)\n");
    text.push_str("idle_cache_trim_ms = ");
    text.push_str(&cfg.idle_cache_trim_ms.to_string());
    text.push('\n');
    text.push_str("# active memory target in MB (valid range: 20..512)\n");
    text.push_str("active_memory_target_mb = ");
    text.push_str(&cfg.active_memory_target_mb.to_string());
    text.push('\n');
    text.push_str("# Maximum indexed file/folder items retained in discovery pass\n");
    text.push_str("index_max_items_total = ");
    text.push_str(&cfg.index_max_items_total.to_string());
    text.push('\n');
    text.push_str("# Maximum indexed file/folder items retained per discovery root\n");
    text.push_str("index_max_items_per_root = ");
    text.push_str(&cfg.index_max_items_per_root.to_string());
    text.push('\n');
    text.push_str("# Runtime candidate budget for per-query file/folder retrieval\n");
    text.push_str("index_max_items_per_query_seed = ");
    text.push_str(&cfg.index_max_items_per_query_seed.to_string());
    text.push('\n');

    std::fs::write(path, text)?;
    Ok(())
}

pub fn validate(cfg: &Config) -> Result<(), String> {
    if cfg.max_results < 5 || cfg.max_results > 100 {
        return Err("max_results out of range".into());
    }

    if cfg.index_db_path.as_os_str().is_empty() {
        return Err("index_db_path is required".into());
    }

    if cfg.config_path.as_os_str().is_empty() {
        return Err("config_path is required".into());
    }

    if cfg.hotkey.trim().is_empty() {
        return Err("hotkey is required".into());
    }

    if cfg.clipboard_retention_minutes < 5 || cfg.clipboard_retention_minutes > 43_200 {
        return Err("clipboard_retention_minutes out of range".into());
    }

    if cfg.idle_cache_trim_ms < 100 || cfg.idle_cache_trim_ms > 10_000 {
        return Err("idle_cache_trim_ms out of range".into());
    }

    if cfg.active_memory_target_mb < 20 || cfg.active_memory_target_mb > 512 {
        return Err("active_memory_target_mb out of range".into());
    }
    if cfg.search_delay_time_ms < 10 || cfg.search_delay_time_ms > 2_000 {
        return Err("search_delay_time_ms out of range".into());
    }

    if cfg.index_max_items_total < 10_000 || cfg.index_max_items_total > 2_000_000 {
        return Err("index_max_items_total out of range".into());
    }

    if cfg.index_max_items_per_root < 1_000 || cfg.index_max_items_per_root > 1_000_000 {
        return Err("index_max_items_per_root out of range".into());
    }

    if cfg.index_max_items_per_query_seed < 250 || cfg.index_max_items_per_query_seed > 200_000 {
        return Err("index_max_items_per_query_seed out of range".into());
    }

    if cfg.index_max_items_per_root > cfg.index_max_items_total {
        return Err("index_max_items_per_root must be <= index_max_items_total".into());
    }

    if cfg.web_search_provider == WebSearchProvider::Custom {
        let template = cfg.web_search_custom_template.trim();
        if template.is_empty() {
            return Err(
                "web_search_custom_template is required when web_search_provider=custom".into(),
            );
        }
        if !template.contains("{query}") {
            return Err("web_search_custom_template must include {query} placeholder".into());
        }
    }

    if cfg
        .discovery_roots
        .iter()
        .any(|root| root.as_os_str().is_empty())
    {
        return Err("discovery_roots contains an empty path".into());
    }

    if cfg
        .discovery_exclude_roots
        .iter()
        .any(|root| root.as_os_str().is_empty())
    {
        return Err("discovery_exclude_roots contains an empty path".into());
    }

    if cfg
        .plugin_paths
        .iter()
        .any(|path| path.as_os_str().is_empty())
    {
        return Err("plugin_paths contains an empty path".into());
    }

    if cfg
        .clipboard_exclude_sensitive_patterns
        .iter()
        .any(|pattern| pattern.trim().is_empty())
    {
        return Err("clipboard_exclude_sensitive_patterns contains an empty pattern".into());
    }

    crate::settings::validate_hotkey(&cfg.hotkey)
        .map_err(|error| format!("hotkey is invalid: {error}"))?;

    if cfg.version == 0 {
        return Err("version must be >= 1".into());
    }

    Ok(())
}

fn write_atomic(path: &Path, encoded: &str) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_path = parent.join(format!(".nex-config-{ts}.tmp"));
    let backup_path = parent.join(".nex-config.backup");

    std::fs::write(&temp_path, encoded)?;

    if backup_path.exists() {
        let _ = std::fs::remove_file(&backup_path);
    }
    if path.exists() {
        std::fs::rename(path, &backup_path)?;
    }

    match std::fs::rename(&temp_path, path) {
        Ok(()) => {
            if backup_path.exists() {
                let _ = std::fs::remove_file(&backup_path);
            }
            Ok(())
        }
        Err(error) => {
            if backup_path.exists() {
                let _ = std::fs::rename(&backup_path, path);
            }
            let _ = std::fs::remove_file(&temp_path);
            Err(ConfigError::Io(error))
        }
    }
}

fn json5_path_array_section(paths: &[PathBuf]) -> String {
    let body = paths
        .iter()
        .map(|path| format!("    {}", json_string(&path.to_string_lossy())))
        .collect::<Vec<_>>()
        .join(",\n");

    if body.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n{body}\n  ]")
    }
}

fn toml_path_array_section(paths: &[PathBuf]) -> String {
    let values = paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    toml_string_array_section(&values)
}

fn toml_string_array_section(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".to_string();
    }

    let body = values
        .iter()
        .map(|value| format!("  {}", json_string(value)))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{body},\n]")
}

fn default_discovery_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(profile_root) = windows_user_profile_root() {
            return vec![profile_root];
        }
    }

    Vec::new()
}

fn default_discovery_exclude_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(profile_root) = windows_user_profile_root() {
            return vec![
                profile_root.join("AppData").join("Local").join("Temp"),
                profile_root
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("Windows")
                    .join("INetCache"),
            ];
        }
    }

    Vec::new()
}

#[cfg(target_os = "windows")]
fn windows_user_profile_root() -> Option<PathBuf> {
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let trimmed = user_profile.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let home_drive = std::env::var("HOMEDRIVE").ok();
    let home_path = std::env::var("HOMEPATH").ok();
    if let (Some(drive), Some(path)) = (home_drive, home_path) {
        let combined = format!("{}{}", drive.trim(), path.trim());
        let trimmed = combined.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    None
}

fn default_for_path(path: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.config_path = path.to_path_buf();
    if cfg.index_db_path == Config::default().index_db_path {
        cfg.index_db_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("index.sqlite3");
    }
    cfg
}

fn apply_migrations(cfg: &mut Config, raw: &str) -> bool {
    let mut changed = false;
    let source_version = cfg.version.max(1);

    if cfg.version < CURRENT_CONFIG_VERSION {
        cfg.version = CURRENT_CONFIG_VERSION;
        changed = true;
    }

    if source_version < 2 {
        let had_idle_key = raw_has_key(raw, "idle_cache_trim_ms");
        let had_active_mem_key = raw_has_key(raw, "active_memory_target_mb");
        if !had_idle_key || cfg.idle_cache_trim_ms == LEGACY_IDLE_CACHE_TRIM_MS_V1 {
            cfg.idle_cache_trim_ms = Config::default().idle_cache_trim_ms;
            changed = true;
        }
        if !had_active_mem_key || cfg.active_memory_target_mb == LEGACY_ACTIVE_MEMORY_TARGET_MB_V1 {
            cfg.active_memory_target_mb = Config::default().active_memory_target_mb;
            changed = true;
        }
    }

    if source_version < 3 {
        if !raw_has_key(raw, "web_search_provider") {
            cfg.web_search_provider = Config::default().web_search_provider;
            changed = true;
        }
        if !raw_has_key(raw, "web_search_custom_template") {
            cfg.web_search_custom_template = Config::default().web_search_custom_template;
            changed = true;
        }
    }

    if source_version < 4 {
        if !raw_has_key(raw, "windows_search_enabled") {
            cfg.windows_search_enabled = Config::default().windows_search_enabled;
            changed = true;
        }
        if !raw_has_key(raw, "windows_search_fallback_filesystem") {
            cfg.windows_search_fallback_filesystem =
                Config::default().windows_search_fallback_filesystem;
            changed = true;
        }
    }

    if source_version < 5 {
        if !raw_has_key(raw, "show_files") {
            cfg.show_files = Config::default().show_files;
            changed = true;
        }
        if !raw_has_key(raw, "show_folders") {
            cfg.show_folders = Config::default().show_folders;
            changed = true;
        }
    }

    if source_version < 6 && !raw_has_key(raw, "uninstall_actions_enabled") {
        cfg.uninstall_actions_enabled = Config::default().uninstall_actions_enabled;
        changed = true;
    }

    if source_version < 7 {
        if !raw_has_key(raw, "index_max_items_total") {
            cfg.index_max_items_total = Config::default().index_max_items_total;
            changed = true;
        }
        if !raw_has_key(raw, "index_max_items_per_root") {
            cfg.index_max_items_per_root = Config::default().index_max_items_per_root;
            changed = true;
        }
        if !raw_has_key(raw, "index_max_items_per_query_seed") {
            cfg.index_max_items_per_query_seed = Config::default().index_max_items_per_query_seed;
            changed = true;
        }
    }

    if source_version < 10 && !raw_has_key(raw, "game_mode_enabled") {
        cfg.game_mode_enabled = Config::default().game_mode_enabled;
        changed = true;
    }

    if source_version < 9 {
        if !raw_has_key(raw, "search_query_results_with_delay") {
            cfg.search_query_results_with_delay = Config::default().search_query_results_with_delay;
            changed = true;
        }
        if !raw_has_key(raw, "search_delay_time_ms") {
            cfg.search_delay_time_ms = Config::default().search_delay_time_ms;
            changed = true;
        }
    }

    if TEMPLATE_REQUIRED_KEYS
        .iter()
        .any(|key| !raw_has_key(raw, key))
    {
        changed = true;
    }

    if raw.contains(LEGACY_APP_DISPLAY_NAME) || raw.contains(LEGACY_APP_DIR_NAME_UNIX) {
        changed = true;
    }

    changed
}

fn persist_migrated_config(
    cfg: &Config,
    path: &Path,
    original_raw: &str,
    source_version: u32,
    source_path: &Path,
) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_ext = source_path
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.trim().is_empty())
        .unwrap_or("txt");
    let backup_path = parent.join(format!(
        "config.v{}-backup-{}.{}",
        source_version.max(1),
        stamp,
        backup_ext
    ));
    std::fs::write(&backup_path, original_raw)?;
    write_user_template(cfg, path)
}

fn raw_has_key(raw: &str, key: &str) -> bool {
    let quoted = format!("\"{key}\"");
    if raw.contains(&quoted) {
        return true;
    }
    let toml_key = format!("{key} =");
    if raw.contains(&toml_key) {
        return true;
    }
    let bare = format!("{key}:");
    raw.contains(&bare)
}

fn parse_text(raw: &str) -> Result<Config, ConfigError> {
    match toml::from_str::<Config>(raw) {
        Ok(cfg) => Ok(cfg),
        Err(toml_err) => match serde_json::from_str::<Config>(raw) {
            Ok(cfg) => Ok(cfg),
            Err(json_err) => match json5::from_str::<Config>(raw) {
                Ok(cfg) => Ok(cfg),
                Err(json5_err) => Err(ConfigError::Parse(format!(
                    "invalid config format. toml error: {toml_err}; json error: {json_err}; json5 error: {json5_err}"
                ))),
            },
        },
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(target_os = "windows")]
fn windows_app_data_dir(app_dir_name: &str) -> Option<PathBuf> {
    if let Ok(app_data) = std::env::var("APPDATA") {
        return Some(PathBuf::from(app_data).join(app_dir_name));
    }

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        return Some(
            PathBuf::from(user_profile)
                .join("AppData")
                .join("Roaming")
                .join(app_dir_name),
        );
    }

    None
}

#[cfg(not(target_os = "windows"))]
fn unix_app_data_dir(app_dir_name: &str) -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join(app_dir_name));
    }

    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".config").join(app_dir_name));
    }

    None
}

fn migrate_legacy_app_data_dir(preferred: PathBuf, legacy: Option<PathBuf>) -> PathBuf {
    let Some(legacy) = legacy else {
        return preferred;
    };

    if legacy == preferred || !legacy.exists() {
        return preferred;
    }

    if !preferred.exists() {
        if let Some(parent) = preferred.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        return match std::fs::rename(&legacy, &preferred) {
            Ok(()) => preferred,
            Err(_) => legacy,
        };
    }

    match move_missing_entries(&legacy, &preferred) {
        Ok(()) => preferred,
        Err(_) => {
            if app_data_dir_has_state(&preferred) || !app_data_dir_has_state(&legacy) {
                preferred
            } else {
                legacy
            }
        }
    }
}

fn move_missing_entries(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if target.exists() {
            continue;
        }
        std::fs::rename(entry.path(), target)?;
    }

    let is_empty = std::fs::read_dir(from)?.next().transpose()?.is_none();
    if is_empty {
        let _ = std::fs::remove_dir(from);
    }
    Ok(())
}

fn app_data_dir_has_state(path: &Path) -> bool {
    [CONFIG_FILE_NAME, LEGACY_CONFIG_FILE_NAME, "index.sqlite3"]
        .iter()
        .any(|file_name| path.join(file_name).exists())
}

fn rewrite_managed_paths_to_current_app_dir(cfg: &mut Config) -> bool {
    let current_dir = stable_app_data_dir();

    #[cfg(target_os = "windows")]
    let legacy_dir = windows_app_data_dir(LEGACY_APP_DIR_NAME_WINDOWS);
    #[cfg(not(target_os = "windows"))]
    let legacy_dir = unix_app_data_dir(LEGACY_APP_DIR_NAME_UNIX);

    let Some(legacy_dir) = legacy_dir else {
        return false;
    };
    if legacy_dir == current_dir {
        return false;
    }

    let mut changed = false;

    if let Some(rebased) = rebase_managed_path(&cfg.index_db_path, &legacy_dir, &current_dir) {
        cfg.index_db_path = rebased;
        changed = true;
    }

    for plugin_path in &mut cfg.plugin_paths {
        if let Some(rebased) = rebase_managed_path(plugin_path, &legacy_dir, &current_dir) {
            *plugin_path = rebased;
            changed = true;
        }
    }

    changed
}

fn rebase_managed_path(path: &Path, from_root: &Path, to_root: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(from_root).ok()?;
    Some(to_root.join(relative))
}
