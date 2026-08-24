use crate::{config::{self, Config}, overlay::model::Theme};

pub(crate) fn apply(base: &Config, raw: &str) -> Result<Config, String> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| format!("bad json: {e}"))?;
    let cfg_obj = v.get("cfg").ok_or("missing cfg")?;
    let mut cfg =base.clone();
    let get_bool = |k: &str, cur:bool| cfg_obj.get(k).and_then(|x| x.as_bool()).unwrap_or(cur);
    let get_u64 = |k: &str, cur: u64| cfg_obj.get(k).and_then(|x| x.as_u64()).unwrap_or(cur);
    cfg.hotkey = cfg_obj.get("hotkey").and_then(|x| x.as_str()).map(String::from).unwrap_or(cfg.hotkey.clone());
    cfg.grid_view =get_bool("gridView", cfg.grid_view);
    cfg.max_results = get_u64("maxResults", cfg.max_results as u64) as u16;
    cfg.quick_launch.enabled = get_bool("quickLaunchEnabled", cfg.quick_launch.enabled);
    cfg.quick_launch.max_items = get_u64("quickLaunchMaxItems", cfg.quick_launch.max_items as u64) as u8;
    cfg.index_max_items_total = get_u64("indexMaxItemsTotal", cfg.index_max_items_total as u64) as u32;
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
        "indexMaxItemsTotal": cfg.index_max_items_total,
        "hotkey": cfg.hotkey,
        "theme": theme,
    })
    .to_string()
}