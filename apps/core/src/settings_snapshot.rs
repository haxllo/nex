use crate::{config::{self, Config}, overlay::model::Theme};

pub(crate) fn apply(base: &Config, raw: &str) -> Result<Config, String> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| format!("bad json: {e}"))?;
    let cfg_obj = v.get("cfg").ok_or("missing cfg")?;
    let mut cfg = base.clone();
    let get_bool = |k: &str, cur: bool| cfg_obj.get(k).and_then(|x| x.as_bool()).unwrap_or(cur);
    let get_u64 = |k: &str, cur: u64| cfg_obj.get(k).and_then(|x| x.as_u64()).unwrap_or(cur);
    let get_str = |k: &str| cfg_obj.get(k).and_then(|x| x.as_str()).map(String::from);
    cfg.hotkey = get_str("hotkey").unwrap_or(cfg.hotkey);
    cfg.grid_view = get_bool("gridView", cfg.grid_view);
    cfg.max_results = get_u64("maxResults", cfg.max_results as u64) as u16;
    cfg.quick_launch.enabled = get_bool("quickLaunchEnabled", cfg.quick_launch.enabled);
    cfg.quick_launch.max_items = get_u64("quickLaunchMaxItems", cfg.quick_launch.max_items as u64) as u8;
    cfg.quick_launch.auto_fill = get_bool("quickLaunchAutoFill", cfg.quick_launch.auto_fill);
    cfg.index_max_items_total = get_u64("indexMaxItemsTotal", cfg.index_max_items_total as u64) as u32;
    cfg.show_files = get_bool("showFiles", cfg.show_files);
    cfg.show_folders = get_bool("showFolders", cfg.show_folders);
    cfg.launch_at_startup = get_bool("launchAtStartup", cfg.launch_at_startup);
    if let Some(s) = get_str("searchModeDefault") {
        if let Some(mode) = config::SearchMode::parse(&s) {
            cfg.search_mode_default = mode;
        }
    }
    cfg.search_dsl_enabled = get_bool("searchDslEnabled", cfg.search_dsl_enabled);
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
    })
    .to_string()
}