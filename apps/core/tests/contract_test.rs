use std::time::{SystemTime, UNIX_EPOCH};

use nex_core::contract::{CoreRequest, CoreResponse, LaunchRequest, SearchRequest};
use nex_core::core_service::CoreService;

/// A config that owns its index instead of reading the developer's.
///
/// `Config::default()` resolves `index_db_path` to the live
/// `%APPDATA%/Nex/index.sqlite3`, and `CoreService` opens the Tantivy index
/// beside it, so a default config searches the real index: this test asked
/// for "code" and got five hits from the machine instead of the one item it
/// inserted. The database handed to `with_connection` is in memory, but the
/// search index is not, so it has to be redirected here.
fn isolated_config(unique: u128) -> nex_core::config::Config {
    let temp = std::env::temp_dir();
    let mut config = nex_core::config::Config::default();
    config.config_path = temp.join(format!("nex-contract-cfg-{unique}.toml"));
    config.index_db_path = temp.join(format!("nex-contract-index-{unique}.sqlite3"));
    config.clipboard_enabled = false;
    config.plugins_enabled = false;
    config
}

#[test]
fn serializes_and_deserializes_search_request() {
    let request = CoreRequest::Search(SearchRequest {
        query: "code".to_string(),
        limit: Some(5),
    });

    let encoded = serde_json::to_string(&request).unwrap();
    let decoded: CoreRequest = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, request);
}

#[test]
fn handles_search_command_and_serializes_response() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let item_path = std::env::temp_dir().join(format!("nex-contract-search-{unique}.tmp"));
    std::fs::write(&item_path, b"ok").unwrap();

    let config = isolated_config(unique);
    let db = nex_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(config, db).unwrap();

    service
        .upsert_item(&nex_core::model::SearchItem::new(
            "s1",
            "app",
            "Visual Studio Code",
            item_path.to_string_lossy().as_ref(),
        ))
        .unwrap();

    let response = service
        .handle_command(CoreRequest::Search(SearchRequest {
            query: "code".into(),
            limit: Some(5),
        }))
        .unwrap();

    match response {
        CoreResponse::Search(payload) => {
            assert_eq!(payload.results.len(), 1);
            assert_eq!(payload.results[0].id, "s1");

            let encoded = serde_json::to_string(&CoreResponse::Search(payload)).unwrap();
            let decoded: CoreResponse = serde_json::from_str(&encoded).unwrap();
            assert!(matches!(decoded, CoreResponse::Search(_)));
        }
        _ => panic!("expected search response"),
    }

    std::fs::remove_file(&item_path).unwrap();
}

#[test]
fn handles_launch_command_by_path() {
    // Nothing may open on the machine running the tests.
    nex_core::action_executor::set_shell_launches_suppressed(true);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let launch_path = std::env::temp_dir().join(format!("nex-contract-launch-{unique}.txt"));
    std::fs::write(&launch_path, b"ok").unwrap();

    let config = isolated_config(unique);
    let db = nex_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(config, db).unwrap();

    let response = service
        .handle_command(CoreRequest::Launch(LaunchRequest {
            id: None,
            path: Some(launch_path.to_string_lossy().to_string()),
        }))
        .unwrap();

    std::fs::remove_file(&launch_path).unwrap();

    assert_eq!(
        response,
        CoreResponse::Launch(nex_core::contract::LaunchResponse { launched: true })
    );
}
