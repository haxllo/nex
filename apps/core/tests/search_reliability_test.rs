//! End-to-end recall checks for configured-root file search: files deep
//! below a root, files created after the initial index, and roots large
//! enough to exceed the in-memory cache.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use nex_core::config::{Config, DiscoveryBackend};
use nex_core::core_service::CoreService;

fn root_config(dir: &Path, root: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.index_db_path = dir.join("index.sqlite3");
    cfg.discovery_roots = vec![root.to_path_buf()];
    cfg.discovery_exclude_roots = Vec::new();
    cfg.file_discovery_backend = DiscoveryBackend::Walkdir;
    cfg.show_files = true;
    cfg.show_folders = true;
    cfg
}

fn indexed_service(cfg: Config) -> CoreService {
    let service = CoreService::new(cfg).unwrap().with_runtime_providers();
    service.rebuild_index().unwrap();
    service
}

fn found(service: &CoreService, query: &str, path: &Path) -> bool {
    let want = path.to_string_lossy().to_ascii_lowercase();
    service
        .search(query, 50)
        .unwrap()
        .iter()
        .any(|r| r.path.to_ascii_lowercase() == want)
}

fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    check()
}

fn deep_path(root: &Path, levels: usize) -> PathBuf {
    let mut dir = root.to_path_buf();
    for i in 0..levels {
        dir = dir.join(format!("level{i}"));
    }
    dir
}

#[test]
fn file_deeper_than_old_depth_limit_is_searchable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profile");
    // The old hardcoded walk depth was 5.
    let dir = deep_path(&root, 9);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("deepquarterly.txt");
    std::fs::write(&file, b"x").unwrap();

    let service = indexed_service(root_config(tmp.path(), &root));
    assert!(found(&service, "deepquarterly", &file));
}

#[test]
fn depth_limit_is_configurable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profile");
    let dir = deep_path(&root, 6);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("toodeep.txt");
    std::fs::write(&file, b"x").unwrap();

    let mut cfg = root_config(tmp.path(), &root);
    cfg.discovery_max_depth = 3;
    let service = indexed_service(cfg);
    assert!(!found(&service, "toodeep", &file));
}

#[test]
fn new_download_appears_in_search_via_watcher() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profile");
    let downloads = root.join("Downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("existing.txt"), b"x").unwrap();

    let service = Arc::new(RwLock::new(indexed_service(root_config(tmp.path(), &root))));
    service
        .read()
        .unwrap()
        .start_file_watchers(&service)
        .unwrap();
    // Let the watcher arm before creating files.
    std::thread::sleep(Duration::from_millis(500));

    let first = downloads.join("message.txt");
    std::fs::write(&first, b"hello").unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || found(
            &service.read().unwrap(),
            "message",
            &first
        )),
        "new file in Downloads should become searchable"
    );

    // A second, later file proves the watcher re-arms after a batch.
    let second = downloads.join("invoice-later.txt");
    std::fs::write(&second, b"hello").unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || found(
            &service.read().unwrap(),
            "invoice",
            &second
        )),
        "watcher must keep reporting after its first batch"
    );

    std::fs::remove_file(&first).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || !found(
            &service.read().unwrap(),
            "message",
            &first
        )),
        "deleted file should leave search results"
    );
}

#[test]
fn large_root_stays_searchable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profile");
    let bulk = root.join("bulk");
    std::fs::create_dir_all(&bulk).unwrap();
    for i in 0..1200 {
        std::fs::write(bulk.join(format!("report-{i:05}.txt")), b"x").unwrap();
    }
    let late_dir = root.join("zz-late");
    std::fs::create_dir_all(&late_dir).unwrap();
    let needle = late_dir.join("final-message.txt");
    std::fs::write(&needle, b"x").unwrap();

    let service = indexed_service(root_config(tmp.path(), &root));

    assert!(found(&service, "message", &needle));
    assert!(found(&service, "report-01199", &bulk.join("report-01199.txt")));
}

#[test]
fn per_root_cap_no_longer_truncates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profile");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..1100 {
        std::fs::write(root.join(format!("cap-{i:05}.txt")), b"x").unwrap();
    }

    let cfg = root_config(tmp.path(), &root);
    let service = indexed_service(cfg);

    assert!(found(&service, "cap-00000", &root.join("cap-00000.txt")));
    assert!(found(&service, "cap-01099", &root.join("cap-01099.txt")));
}
