use crate::config::{self, Config};
use crate::overlay_ipc_max_bytes;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
    #[serde(default, rename = "quickLaunchMaxItems")]
    quick_launch_max_items: Option<u8>,
    #[serde(default, rename = "quickLaunchAutoFill")]
    quick_launch_auto_fill: Option<bool>,
    #[serde(default, rename = "indexMaxItemsTotal")]
    index_max_items_total: Option<u32>,
    #[serde(default, rename = "indexMaxItemsPerRoot")]
    index_max_items_per_root: Option<u32>,
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
    #[serde(default, rename = "panelBackground")]
    panel_background: Option<String>,
    #[serde(default, rename = "viewMode")]
    view_mode: Option<String>,
    #[serde(default, rename = "discoveryRoots")]
    discovery_roots: Option<Vec<String>>,
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
    if let Some(v) = cfg_obj.index_max_items_per_root {
        if !(1_000..=1_000_000).contains(&v) {
            return Err(format!(
                "indexMaxItemsPerRoot must be between 1000 and 1000000, got {v}"
            ));
        }
        cfg.index_max_items_per_root = v;
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
    if let Some(s) = cfg_obj.panel_background.as_deref() {
        cfg.panel_background = config::PanelBackground::parse(s)
            .ok_or_else(|| format!("unknown panelBackground: {s:?}"))?;
    }
    if let Some(s) = cfg_obj.view_mode.as_deref() {
        cfg.view_mode =
            config::ViewMode::parse(s).ok_or_else(|| format!("unknown viewMode: {s:?}"))?;
    }
    if let Some(roots) = cfg_obj.discovery_roots.as_deref() {
        cfg.discovery_roots = normalize_discovery_roots(roots)?;
    }
    crate::config::validate(&cfg).map_err(|e| format!("invalid settings: {e}"))?;
    Ok(cfg)
}

/// Validate roots coming from the settings page. Paths are kept as typed
/// (no `\\?\` canonicalisation) so the config stays readable.
pub(crate) fn normalize_discovery_roots(raw: &[String]) -> Result<Vec<PathBuf>, String> {
    if raw.is_empty() {
        return Err("at least one discovery root is required".into());
    }
    let mut roots = Vec::with_capacity(raw.len());
    let mut seen = HashSet::with_capacity(raw.len());
    for value in raw {
        // Explorer's "Copy as path" includes quotes; accept the paste as-is.
        let trimmed = value
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .trim();
        if trimmed.is_empty() {
            return Err("discovery root cannot be empty".into());
        }
        let path = PathBuf::from(trimmed);
        if !path.is_absolute() {
            return Err(format!(
                "discovery root must be an absolute folder path: {trimmed}"
            ));
        }
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(format!("discovery root is not a folder: {trimmed}")),
            Err(error) => {
                return Err(format!("cannot read discovery root '{trimmed}': {error}"))
            }
        }
        if seen.insert(discovery_root_key(&path)) {
            roots.push(path);
        }
    }
    Ok(roots)
}

fn discovery_root_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    trimmed.to_lowercase()
}

pub(crate) fn save(cfg: &Config) -> Result<(), String> {
    let path = std::path::PathBuf::from(&cfg.config_path);
    config::save_to_path(cfg, &path).map_err(|e| format!("{e}"))
}


pub(crate) fn build(cfg: &Config, theme: &str) -> String {
    serde_json::json!({
        "gridView": cfg.grid_view,
        "maxResults": cfg.max_results,
        "quickLaunchMaxItems": cfg.quick_launch.max_items,
        "quickLaunchAutoFill": cfg.quick_launch.auto_fill,
        "indexMaxItemsTotal": cfg.index_max_items_total,
        "indexMaxItemsPerRoot": cfg.index_max_items_per_root,
        "hotkey": cfg.hotkey,
        "theme": theme,
        "showFiles": cfg.show_files,
        "showFolders": cfg.show_folders,
        "launchAtStartup": cfg.launch_at_startup,
        "searchModeDefault": cfg.search_mode_default.as_str(),
        "searchDslEnabled": cfg.search_dsl_enabled,
        "webSearchProvider": cfg.web_search_provider.as_str(),
        "panelBackground": cfg.panel_background.as_str(),
        "viewMode": cfg.view_mode.as_str(),
        "discoveryRoots": cfg
            .discovery_roots
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "version": env!("CARGO_PKG_VERSION"),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nex-settings-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn save_body(roots: &[String]) -> String {
        serde_json::json!({
            "t": "save",
            "cfg": { "discoveryRoots": roots },
        })
        .to_string()
    }

    #[test]
    fn save_replaces_discovery_roots() {
        let dir = unique_dir("roots");
        let base = Config::default();
        let updated = apply(&base, &save_body(&[dir.to_string_lossy().into_owned()]))
            .expect("an existing absolute folder is valid");
        assert_eq!(updated.discovery_roots, vec![dir.clone()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_rejects_empty_relative_and_missing_roots() {
        let base = Config::default();
        assert!(apply(&base, &save_body(&[])).is_err());
        assert!(apply(&base, &save_body(&[r"relative\folder".to_string()])).is_err());
        assert!(apply(
            &base,
            &save_body(&[r"Z:\definitely-missing-nex-test".to_string()])
        )
        .is_err());
    }

    #[test]
    fn save_deduplicates_roots() {
        let dir = unique_dir("dupe");
        let text = dir.to_string_lossy().into_owned();
        let updated = apply(&Config::default(), &save_body(&[text.clone(), text]))
            .expect("valid roots");
        assert_eq!(updated.discovery_roots.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_sets_per_root_item_limit() {
        let raw = r#"{"t":"save","cfg":{"indexMaxItemsPerRoot":150000}}"#;
        let updated = apply(&Config::default(), raw).expect("valid limit");
        assert_eq!(updated.index_max_items_per_root, 150_000);
    }

    #[test]
    fn save_rejects_per_root_above_total() {
        let raw = r#"{"t":"save","cfg":{"indexMaxItemsPerRoot":600000}}"#;
        assert!(apply(&Config::default(), raw).is_err());
    }

    #[test]
    fn save_accepts_explorer_copy_as_path_quotes() {
        let dir = unique_dir("quoted");
        let quoted = format!("\"{}\"", dir.to_string_lossy());
        let updated =
            apply(&Config::default(), &save_body(&[quoted])).expect("quoted path is accepted");
        assert_eq!(updated.discovery_roots, vec![dir.clone()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_exposes_discovery_roots_to_the_page() {
        let dir = unique_dir("build");
        let mut cfg = Config::default();
        cfg.discovery_roots = vec![dir.clone()];
        let snapshot: serde_json::Value =
            serde_json::from_str(&build(&cfg, "dark")).expect("settings snapshot is JSON");
        assert_eq!(
            snapshot["discoveryRoots"][0].as_str(),
            Some(dir.to_string_lossy().as_ref())
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
