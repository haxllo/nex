use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn rejects_max_results_out_of_range() {
    let cfg = nex_core::config::Config {
        max_results: 200,
        ..Default::default()
    };
    assert!(nex_core::config::validate(&cfg).is_err());
}

#[test]
fn accepts_default_config() {
    let cfg = nex_core::config::Config::default();
    assert_eq!(cfg.max_results, 20);
    assert_eq!(cfg.version, nex_core::config::CURRENT_CONFIG_VERSION);
    assert_eq!(cfg.hotkey, "Ctrl+Space");
    assert!(!cfg.launch_at_startup);
    assert!(!cfg.hotkey_help.trim().is_empty());
    assert!(!cfg.hotkey_recommended.is_empty());
    assert_eq!(
        cfg.web_search_provider,
        nex_core::config::WebSearchProvider::Google
    );
    assert!(cfg.windows_search_enabled);
    assert!(cfg.windows_search_fallback_filesystem);
    assert_eq!(cfg.index_max_items_total, 120_000);
    assert_eq!(cfg.index_max_items_per_root, 40_000);
    assert_eq!(cfg.index_max_items_per_query_seed, 5_000);
    assert!(!cfg.game_mode_enabled);
    assert!(cfg.search_query_results_with_delay);
    assert_eq!(cfg.search_delay_time_ms, 90);
    assert!(
        cfg.index_db_path.to_string_lossy().contains("nex")
            || cfg.index_db_path.to_string_lossy().contains("Nex")
    );
    assert!(!cfg.show_files);
    assert!(!cfg.show_folders);
    assert!(cfg.uninstall_actions_enabled);
    assert!(
        cfg.config_path.to_string_lossy().contains("nex")
            || cfg.config_path.to_string_lossy().contains("Nex")
    );
    assert!(nex_core::config::validate(&cfg).is_ok());
}

#[test]
fn opens_index_store_from_config_path() {
    let mut cfg = nex_core::config::Config::default();
    cfg.index_db_path = std::env::temp_dir()
        .join("swiftfind")
        .join("cfg-open.sqlite3");

    let db = nex_core::index_store::open_from_config(&cfg).unwrap();
    let item =
        nex_core::model::SearchItem::new("cfg-1", "app", "Terminal", "C:\\Terminal.exe");
    nex_core::index_store::upsert_item(&db, &item).unwrap();

    let got = nex_core::index_store::get_item(&db, "cfg-1").unwrap();
    assert!(got.is_some());

    drop(db);
    std::fs::remove_file(&cfg.index_db_path).unwrap();
}

#[test]
fn loads_default_when_config_file_missing() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_path = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("missing-config-{unique}.json"));

    let cfg = nex_core::config::load(Some(&config_path)).unwrap();

    assert_eq!(cfg.config_path, config_path);
    assert_eq!(cfg.max_results, 20);
    assert_eq!(cfg.version, nex_core::config::CURRENT_CONFIG_VERSION);
}

#[test]
fn saves_and_reloads_config_file() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_path = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("save-reload-{unique}.json"));

    let mut cfg = nex_core::config::Config::default();
    cfg.config_path = config_path.clone();
    cfg.max_results = 33;
    cfg.hotkey = "Ctrl+Space".to_string();
    cfg.launch_at_startup = true;
    cfg.discovery_roots = vec![std::env::temp_dir().join("root-a")];
    cfg.discovery_exclude_roots = vec![std::env::temp_dir().join("root-a").join("exclude")];

    nex_core::config::save(&cfg).unwrap();
    let loaded = nex_core::config::load(Some(&config_path)).unwrap();

    assert_eq!(loaded.max_results, 33);
    assert_eq!(loaded.hotkey, "Ctrl+Space");
    assert!(loaded.launch_at_startup);
    assert_eq!(loaded.discovery_roots.len(), 1);
    assert_eq!(loaded.discovery_exclude_roots.len(), 1);
    assert_eq!(
        loaded.version,
        nex_core::config::CURRENT_CONFIG_VERSION
    );

    std::fs::remove_file(&config_path).unwrap();
}

#[test]
fn loads_partial_config_with_migration_safe_defaults() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_path = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("partial-{unique}.json"));

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    std::fs::write(
        &config_path,
        r#"{ "max_results": 25, "config_path": "placeholder" }"#,
    )
    .unwrap();

    let loaded = nex_core::config::load(Some(&config_path)).unwrap();

    assert_eq!(loaded.max_results, 25);
    assert_eq!(
        loaded.version,
        nex_core::config::CURRENT_CONFIG_VERSION
    );
    assert_eq!(loaded.hotkey, "Ctrl+Space");
    assert!(!loaded.launch_at_startup);
    assert_eq!(loaded.config_path, config_path);
    assert!(!loaded.index_db_path.as_os_str().is_empty());
    assert!(!loaded.hotkey_help.trim().is_empty());
    assert!(!loaded.hotkey_recommended.is_empty());

    if Path::new(&loaded.config_path).exists() {
        std::fs::remove_file(&loaded.config_path).unwrap();
    }
}

#[test]
fn writes_user_template_with_comments_and_loads_it() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_path = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("template-{unique}.json"));

    let mut cfg = nex_core::config::Config::default();
    cfg.config_path = config_path.clone();

    nex_core::config::write_user_template(&cfg, &config_path).unwrap();
    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(raw.contains("// Nex config (comments are allowed)."));
    assert!(raw.contains("\"hotkey\": \"Ctrl+Space\""));
    assert!(raw.contains("// \"hotkey\": \"Ctrl+Alt+Space\""));
    assert!(!raw.contains("\"index_db_path\""));
    assert!(raw.contains("\"discovery_exclude_roots\":"));
    assert!(raw.contains("\"windows_search_enabled\": true"));
    assert!(raw.contains("\"windows_search_fallback_filesystem\": true"));
    assert!(raw.contains("\"show_files\": false"));
    assert!(raw.contains("\"show_folders\": false"));
    assert!(raw.contains("\"uninstall_actions_enabled\": true"));
    assert!(raw.contains("\"web_search_provider\": \"google\""));
    assert!(raw.contains("\"index_max_items_total\":"));
    assert!(raw.contains("\"index_max_items_per_root\":"));
    assert!(raw.contains("\"index_max_items_per_query_seed\":"));
    assert!(raw.contains("\"game_mode_enabled\": false"));
    assert!(raw.contains("\"search_query_results_with_delay\": true"));
    assert!(raw.contains("\"search_delay_time_ms\": 90"));
    if cfg.discovery_exclude_roots.is_empty() {
        assert!(raw.contains("\"discovery_exclude_roots\": []"));
    } else {
        for exclude_root in &cfg.discovery_exclude_roots {
            let encoded =
                serde_json::to_string(&exclude_root.to_string_lossy().to_string()).unwrap();
            assert!(
                raw.contains(&encoded),
                "template should include discovery_exclude_roots path {encoded}"
            );
        }
    }

    let loaded = nex_core::config::load(Some(&config_path)).unwrap();
    assert_eq!(loaded.hotkey, cfg.hotkey);
    assert_eq!(loaded.max_results, cfg.max_results);
    assert_eq!(loaded.discovery_exclude_roots, cfg.discovery_exclude_roots);

    std::fs::remove_file(&config_path).unwrap();
}

#[test]
fn rejects_custom_web_provider_without_query_placeholder() {
    let mut cfg = nex_core::config::Config::default();
    cfg.web_search_provider = nex_core::config::WebSearchProvider::Custom;
    cfg.web_search_custom_template = "https://example.com/search".to_string();
    let err = nex_core::config::validate(&cfg).expect_err("custom template should fail");
    assert!(err.contains("{query}"));
}

#[test]
fn migrates_legacy_config_and_preserves_user_values() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_dir = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("migrate-{unique}"));
    let config_path = config_dir.join("config.json");

    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        &config_path,
        r#"{
  "version": 1,
  "hotkey": "Ctrl+Alt+P",
  "max_results": 33,
  "launch_at_startup": true,
  "idle_cache_trim_ms": 1200,
  "active_memory_target_mb": 80,
  "discovery_roots": ["C:\\Users\\Admin"]
}"#,
    )
    .unwrap();

    let loaded = nex_core::config::load(Some(&config_path)).unwrap();
    assert_eq!(
        loaded.version,
        nex_core::config::CURRENT_CONFIG_VERSION
    );
    assert_eq!(loaded.hotkey, "Ctrl+Alt+P");
    assert_eq!(loaded.max_results, 33);
    assert!(loaded.launch_at_startup);
    assert_eq!(loaded.idle_cache_trim_ms, 900);
    assert_eq!(loaded.active_memory_target_mb, 72);
    assert!(loaded.windows_search_enabled);
    assert!(loaded.windows_search_fallback_filesystem);
    assert!(!loaded.show_files);
    assert!(!loaded.show_folders);
    assert!(loaded.uninstall_actions_enabled);
    assert_eq!(loaded.index_max_items_total, 120_000);
    assert_eq!(loaded.index_max_items_per_root, 40_000);
    assert_eq!(loaded.index_max_items_per_query_seed, 5_000);
    assert!(!loaded.game_mode_enabled);
    assert!(loaded.search_query_results_with_delay);
    assert_eq!(loaded.search_delay_time_ms, 90);

    let updated_raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(updated_raw.contains("\"hotkey\": \"Ctrl+Alt+P\""));
    assert!(updated_raw.contains("\"max_results\": 33"));
    assert!(updated_raw.contains("\"idle_cache_trim_ms\": 900"));
    assert!(updated_raw.contains("\"active_memory_target_mb\": 72"));
    assert!(updated_raw.contains("\"windows_search_enabled\": true"));
    assert!(updated_raw.contains("\"windows_search_fallback_filesystem\": true"));
    assert!(updated_raw.contains("\"show_files\": false"));
    assert!(updated_raw.contains("\"show_folders\": false"));
    assert!(updated_raw.contains("\"uninstall_actions_enabled\": true"));
    assert!(updated_raw.contains("\"index_max_items_total\": 120000"));
    assert!(updated_raw.contains("\"index_max_items_per_root\": 40000"));
    assert!(updated_raw.contains("\"index_max_items_per_query_seed\": 5000"));
    assert!(updated_raw.contains("\"game_mode_enabled\": false"));
    assert!(updated_raw.contains("\"search_query_results_with_delay\": true"));
    assert!(updated_raw.contains("\"search_delay_time_ms\": 90"));

    let backups: Vec<_> = std::fs::read_dir(&config_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("config.v1-backup-"))
                .unwrap_or(false)
        })
        .collect();
    assert!(!backups.is_empty());

    for backup in backups {
        let _ = std::fs::remove_file(backup);
    }
    std::fs::remove_file(&config_path).unwrap();
    std::fs::remove_dir_all(&config_dir).unwrap();
}

#[test]
fn legacy_fullscreen_flag_does_not_enable_game_mode_on_migration() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_dir = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("migrate-game-mode-{unique}"));
    let config_path = config_dir.join("config.toml");

    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        &config_path,
        r#"
version = 9
hotkey = "Ctrl+Space"
ignore_hotkeys_on_fullscreen = true
"#,
    )
    .unwrap();

    let loaded = nex_core::config::load(Some(&config_path)).unwrap();
    assert_eq!(
        loaded.version,
        nex_core::config::CURRENT_CONFIG_VERSION
    );
    assert!(!loaded.game_mode_enabled);

    let updated_raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(updated_raw.contains("game_mode_enabled = false"));
    assert!(!updated_raw.contains("ignore_hotkeys_on_fullscreen"));

    std::fs::remove_file(&config_path).unwrap();
    std::fs::remove_dir_all(&config_dir).unwrap();
}
