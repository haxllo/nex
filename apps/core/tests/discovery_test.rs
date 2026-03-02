use std::time::{SystemTime, UNIX_EPOCH};

use swiftfind_core::core_service::CoreService;
#[cfg(not(target_os = "windows"))]
use swiftfind_core::discovery::StartMenuAppDiscoveryProvider;
use swiftfind_core::discovery::{
    AppProvider, DiscoveryProvider, FileProvider, FileSystemDiscoveryProvider,
};

#[test]
fn app_provider_fixture_is_deterministic() {
    let provider = AppProvider::deterministic_fixture();
    let items = provider.discover().unwrap();

    assert_eq!(provider.provider_name(), "app");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "app-code");
    assert_eq!(items[1].id, "app-term");
}

#[test]
fn file_provider_fixture_is_deterministic() {
    let provider = FileProvider::deterministic_fixture();
    let items = provider.discover().unwrap();

    assert_eq!(provider.provider_name(), "file");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "file-report");
    assert_eq!(items[1].id, "file-notes");
}

#[cfg(not(target_os = "windows"))]
#[test]
fn start_menu_provider_returns_empty_off_windows() {
    let provider = StartMenuAppDiscoveryProvider::default();
    let items = provider.discover().unwrap();
    assert!(items.is_empty());
}

#[test]
fn file_system_provider_discovers_files_in_roots() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let root = std::env::temp_dir().join(format!("swiftfind-discovery-{unique}"));
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();

    let report = nested.join("Q4_Report.xlsx");
    let notes = root.join("Notes.txt");
    std::fs::write(&report, b"report").unwrap();
    std::fs::write(&notes, b"notes").unwrap();

    let provider = FileSystemDiscoveryProvider::new(vec![root.clone()], 4, vec![]);
    let items = provider.discover().unwrap();

    let titles: Vec<String> = items.iter().map(|i| i.title.clone()).collect();
    let kinds: Vec<String> = items.iter().map(|i| i.kind.clone()).collect();
    assert!(titles.contains(&"nested".to_string()));
    assert!(titles.contains(&"Q4_Report.xlsx".to_string()));
    assert!(titles.contains(&"Notes.txt".to_string()));
    assert!(kinds.contains(&"folder".to_string()));

    std::fs::remove_file(&report).unwrap();
    std::fs::remove_file(&notes).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rebuild_index_uses_registered_providers() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let app_path = std::env::temp_dir().join(format!("swiftfind-app-provider-{unique}.tmp"));
    let file_path = std::env::temp_dir().join(format!("swiftfind-file-provider-{unique}.tmp"));
    std::fs::write(&app_path, b"app").unwrap();
    std::fs::write(&file_path, b"file").unwrap();

    let config = swiftfind_core::config::Config::default();
    let db = swiftfind_core::index_store::open_memory().unwrap();

    let service = CoreService::with_connection(config, db)
        .unwrap()
        .with_providers(vec![
            Box::new(AppProvider::from_apps(vec![
                swiftfind_core::model::SearchItem::new(
                    "app-code",
                    "app",
                    "Visual Studio Code",
                    app_path.to_string_lossy().as_ref(),
                ),
            ])),
            Box::new(FileProvider::from_files(vec![
                swiftfind_core::model::SearchItem::new(
                    "file-report",
                    "file",
                    "Q4_Report.xlsx",
                    file_path.to_string_lossy().as_ref(),
                ),
            ])),
        ]);

    let inserted = service.rebuild_index().unwrap();
    assert_eq!(inserted, 2);

    let results = service.search("report", 10).unwrap();
    assert_eq!(results[0].id, "file-report");

    std::fs::remove_file(app_path).unwrap();
    std::fs::remove_file(file_path).unwrap();
}

#[test]
fn runtime_providers_use_configured_roots() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("swiftfind-runtime-roots-{unique}"));
    std::fs::create_dir_all(&root).unwrap();
    let file_path = root.join("RuntimeDoc.txt");
    std::fs::write(&file_path, b"runtime").unwrap();

    let mut config = swiftfind_core::config::Config::default();
    config.discovery_roots = vec![root.clone()];
    // Ensure this test root is not filtered by default exclude roots (which may include %TEMP%).
    config.discovery_exclude_roots = vec![];

    let db = swiftfind_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(config, db)
        .unwrap()
        .with_runtime_providers();

    let inserted = service.rebuild_index().unwrap();
    assert!(inserted >= 1);

    let results = service.search("runtimedoc", 10).unwrap();
    assert!(!results.is_empty());

    std::fs::remove_file(&file_path).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn runtime_providers_respect_show_files_and_folders_toggles() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("swiftfind-runtime-hidden-roots-{unique}"));
    std::fs::create_dir_all(&root).unwrap();
    let file_path = root.join("HiddenDoc.txt");
    std::fs::write(&file_path, b"runtime").unwrap();

    let mut config = swiftfind_core::config::Config::default();
    config.discovery_roots = vec![root.clone()];
    config.discovery_exclude_roots = vec![];
    config.show_files = false;
    config.show_folders = false;

    let db = swiftfind_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(config, db)
        .unwrap()
        .with_runtime_providers();

    let _ = service.rebuild_index().unwrap();
    let results = service.search("hiddendoc", 10).unwrap();
    assert!(results.is_empty());

    std::fs::remove_file(&file_path).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn runtime_providers_prune_existing_file_entries_when_disabled() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("swiftfind-runtime-prune-roots-{unique}"));
    std::fs::create_dir_all(&root).unwrap();

    let mut config = swiftfind_core::config::Config::default();
    config.discovery_roots = vec![root.clone()];
    config.discovery_exclude_roots = vec![];
    config.show_files = false;
    config.show_folders = false;

    let db = swiftfind_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(config, db)
        .unwrap()
        .with_runtime_providers();

    let stale_path = root.join("StaleDoc.txt");
    std::fs::write(&stale_path, b"stale").unwrap();

    service
        .upsert_item(&swiftfind_core::model::SearchItem::new(
            "file:stale-doc",
            "file",
            "StaleDoc.txt",
            stale_path.to_string_lossy().as_ref(),
        ))
        .unwrap();

    let before = service.search("staledoc", 10).unwrap();
    assert!(!before.is_empty());

    let _ = service.rebuild_index().unwrap();
    let after = service.search("staledoc", 10).unwrap();
    assert!(after.is_empty());

    std::fs::remove_file(&stale_path).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn file_system_provider_excludes_configured_roots() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let root = std::env::temp_dir().join(format!("swiftfind-exclude-roots-{unique}"));
    let included = root.join("include");
    let excluded = root.join("exclude");
    std::fs::create_dir_all(&included).unwrap();
    std::fs::create_dir_all(&excluded).unwrap();

    let keep_file = included.join("Keep.txt");
    let skip_file = excluded.join("Skip.txt");
    std::fs::write(&keep_file, b"keep").unwrap();
    std::fs::write(&skip_file, b"skip").unwrap();

    let provider = FileSystemDiscoveryProvider::new(vec![root.clone()], 6, vec![excluded.clone()]);
    let items = provider.discover().unwrap();

    let paths: Vec<String> = items.iter().map(|i| i.path.clone()).collect();
    assert!(paths.iter().any(|p| p.ends_with("Keep.txt")));
    assert!(!paths.iter().any(|p| p.ends_with("Skip.txt")));

    std::fs::remove_file(&keep_file).unwrap();
    std::fs::remove_file(&skip_file).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn file_system_provider_honors_item_caps() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let root = std::env::temp_dir().join(format!("swiftfind-cap-roots-{unique}"));
    std::fs::create_dir_all(&root).unwrap();

    for idx in 0..10 {
        let file = root.join(format!("Cap-{idx}.txt"));
        std::fs::write(file, b"cap").unwrap();
    }

    let provider =
        FileSystemDiscoveryProvider::new(vec![root.clone()], 6, vec![]).with_index_limits(6, 4);
    let items = provider.discover().unwrap();
    assert!(items.len() <= 6);

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn runtime_provider_reconfigure_applies_new_roots() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let root_a = std::env::temp_dir().join(format!("swiftfind-recfg-a-{unique}"));
    let root_b = std::env::temp_dir().join(format!("swiftfind-recfg-b-{unique}"));
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let file_a = root_a.join("AlphaRoot.txt");
    let file_b = root_b.join("BetaRoot.txt");
    std::fs::write(&file_a, b"a").unwrap();
    std::fs::write(&file_b, b"b").unwrap();

    let mut cfg_a = swiftfind_core::config::Config::default();
    cfg_a.discovery_roots = vec![root_a.clone()];
    cfg_a.discovery_exclude_roots = vec![];
    cfg_a.windows_search_enabled = false;

    let db = swiftfind_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(cfg_a.clone(), db)
        .unwrap()
        .with_runtime_providers();

    let _ = service.rebuild_index().unwrap();
    let before = service.search("alpharoot", 20).unwrap();
    assert!(!before.is_empty());

    let mut cfg_b = cfg_a.clone();
    cfg_b.discovery_roots = vec![root_b.clone()];
    service.reconfigure_runtime_providers(&cfg_b).unwrap();
    let _ = service.rebuild_index().unwrap();

    let after_a = service.search("alpharoot", 20).unwrap();
    let after_b = service.search("betaroot", 20).unwrap();
    assert!(after_a.is_empty());
    assert!(!after_b.is_empty());

    std::fs::remove_file(&file_a).unwrap();
    std::fs::remove_file(&file_b).unwrap();
    std::fs::remove_dir_all(&root_a).unwrap();
    std::fs::remove_dir_all(&root_b).unwrap();
}
