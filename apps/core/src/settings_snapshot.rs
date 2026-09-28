use crate::config::{self, Config};
use crate::overlay_ipc_max_bytes;

/// Strict schema for the settings-page save payload. Unknown fields are
/// rejected (the page must not smuggle keys past review by adding them
/// to the cfg object), and every value is range-checked before it
/// touches `Config`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsSaveCfg {
    #[serde(default)]
    hotkey: Option<String>,
    #[serde(default, rename = "gridView")]
    grid_view: Option<bool>,
    #[serde(default, rename = "maxResults")]
    max_results: Option<u16>,
    #[serde(default, rename = "quickLaunchEnabled")]
    quick_launch_enabled: Option<bool>,
    #[serde(default, rename = "quickLaunchMaxItems")]
    quick_launch_max_items: Option<u8>,
    #[serde(default, rename = "quickLaunchAutoFill")]
    quick_launch_auto_fill: Option<bool>,
    #[serde(default, rename = "indexMaxItemsTotal")]
    index_max_items_total: Option<u32>,
    #[serde(default, rename = "showFiles")]
    show_files: Option<bool>,
    #[serde(default, rename = "showFolders")]
    show_folders: Option<bool>,
    #[serde(default, rename = "launchAtStartup")]
    launch_at_startup: Option<bool>,
    #[serde(default, rename = "searchModeDefault")]
    search_mode_default: Option<String>,
    #[serde(default, rename = "searchDslEnabled")]
    search_dsl_enabled: Option<bool>,
    #[serde(default, rename = "webSearchProvider")]
    web_search_provider: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsSaveBody {
    #[serde(rename = "t")]
    tag: String,
    cfg: SettingsSaveCfg,
}

pub(crate) fn apply(base: &Config, raw: &str) -> Result<Config, String> {
    if raw.len() > overlay_ipc_max_bytes() {
        return Err(format!(
            "settings body exceeds {} bytes",
            overlay_ipc_max_bytes()
        ));
    }
    let body: SettingsSaveBody =
        serde_json::from_str(raw).map_err(|e| format!("bad settings save: {e}"))?;
    if body.tag != "save" {
        return Err("settings save must carry {\"t\":\"save\"}".into());
    }
    let cfg_obj = body.cfg;
    let mut cfg = base.clone();
    if let Some(hotkey) = cfg_obj.hotkey.as_deref() {
        cfg.hotkey = crate::settings::validate_hotkey(hotkey)
            .map_err(|e| format!("invalid hotkey: {e}"))?;
    }
    if let Some(v) = cfg_obj.grid_view {
        cfg.grid_view = v;
    }
    if let Some(v) = cfg_obj.max_results {
        crate::settings::validate_max_results(v)
            .map_err(|e| format!("invalid maxResults: {e}"))?;
        cfg.max_results = v;
    }
    if let Some(v) = cfg_obj.quick_launch_enabled {
        cfg.quick_launch.enabled = v;
    }
    if let Some(v) = cfg_obj.quick_launch_max_items {
        if !(3..=12).contains(&v) {
            return Err(format!(
                "quickLaunchMaxItems must be between 3 and 12, got {v}"
            ));
        }
        cfg.quick_launch.max_items = v;
    }
    if let Some(v) = cfg_obj.quick_launch_auto_fill {
        cfg.quick_launch.auto_fill = v;
    }
    if let Some(v) = cfg_obj.index_max_items_total {
        if !(10_000..=2_000_000).contains(&v) {
            return Err(format!(
                "indexMaxItemsTotal must be between 10000 and 2000000, got {v}"
            ));
        }
        cfg.index_max_items_total = v;
    }
    if let Some(v) = cfg_obj.show_files {
        cfg.show_files = v;
    }
    if let Some(v) = cfg_obj.show_folders {
        cfg.show_folders = v;
    }
    if let Some(v) = cfg_obj.launch_at_startup {
        cfg.launch_at_startup = v;
    }
    if let Some(s) = cfg_obj.search_mode_default.as_deref() {
        cfg.search_mode_default = config::SearchMode::parse(s)
            .ok_or_else(|| format!("unknown searchModeDefault: {s:?}"))?;
    }
    if let Some(v) = cfg_obj.search_dsl_enabled {
        cfg.search_dsl_enabled = v;
    }
    if let Some(s) = cfg_obj.web_search_provider.as_deref() {
        cfg.web_search_provider = config::WebSearchProvider::parse(s)
            .ok_or_else(|| format!("unknown webSearchProvider: {s:?}"))?;
    }
    crate::config::validate(&cfg).map_err(|e| format!("invalid settings: {e}"))?;
    Ok(cfg)
}

pub(crate) fn save(cfg: &Config) -> Result<(), String> {
    let path = std::path::PathBuf::from(&cfg.config_path);
    config::save_to_path(cfg, &path).map_err(|e| format!("{e}"))
}


pub(crate) fn build(cfg: &Config, theme: &str) -> String {
    serde_json::json!({
        "gridView": cfg.grid_view,
        "maxResults": cfg.max_results,
        "quickLaunchEnabled": cfg.quick_launch.enabled,
        "quickLaunchMaxItems": cfg.quick_launch.max_items,
        "quickLaunchAutoFill": cfg.quick_launch.auto_fill,
        "indexMaxItemsTotal": cfg.index_max_items_total,
        "hotkey": cfg.hotkey,
        "theme": theme,
        "showFiles": cfg.show_files,
        "showFolders": cfg.show_folders,
        "launchAtStartup": cfg.launch_at_startup,
        "searchModeDefault": cfg.search_mode_default.as_str(),
        "searchDslEnabled": cfg.search_dsl_enabled,
        "webSearchProvider": cfg.web_search_provider.as_str(),
        "version": env!("CARGO_PKG_VERSION"),
    })
    .to_string()
}
