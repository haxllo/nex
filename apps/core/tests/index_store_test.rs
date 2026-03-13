use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn inserts_and_reads_search_item() {
    let db = nex_core::index_store::open_memory().unwrap();
    let item = nex_core::model::SearchItem::new("1", "app", "Code", "C:\\Code.exe")
        .with_usage(3, 1_700_000_000);

    nex_core::index_store::upsert_item(&db, &item).unwrap();
    let got = nex_core::index_store::get_item(&db, "1")
        .unwrap()
        .unwrap();

    assert_eq!(got.title, "Code");
    assert_eq!(got.use_count, 3);
    assert_eq!(got.last_accessed_epoch_secs, 1_700_000_000);
}

#[test]
fn persists_items_across_reopen() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("persist-test-{unique}.sqlite3"));

    {
        let db = nex_core::index_store::open_file(&db_path).unwrap();
        let item = nex_core::model::SearchItem::new(
            "persist-1",
            "file",
            "Report",
            "C:\\Report.xlsx",
        )
        .with_usage(7, 1_800_000_000);
        nex_core::index_store::upsert_item(&db, &item).unwrap();
    }

    let reopened = nex_core::index_store::open_file(&db_path).unwrap();
    let got = nex_core::index_store::get_item(&reopened, "persist-1")
        .unwrap()
        .unwrap();

    assert_eq!(got.title, "Report");
    assert_eq!(got.use_count, 7);
    assert_eq!(got.last_accessed_epoch_secs, 1_800_000_000);

    drop(reopened);
    std::fs::remove_file(&db_path).unwrap();
}

#[test]
fn persists_meta_values_across_reopen() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir()
        .join("swiftfind")
        .join(format!("meta-test-{unique}.sqlite3"));

    {
        let db = nex_core::index_store::open_file(&db_path).unwrap();
        nex_core::index_store::set_meta(&db, "provider_stamp:filesystem", "abc123").unwrap();
    }

    let reopened = nex_core::index_store::open_file(&db_path).unwrap();
    let value = nex_core::index_store::get_meta(&reopened, "provider_stamp:filesystem")
        .unwrap()
        .unwrap();
    assert_eq!(value, "abc123");

    drop(reopened);
    std::fs::remove_file(&db_path).unwrap();
}

#[test]
fn stores_and_reads_query_selection_memory() {
    let db = nex_core::index_store::open_memory().unwrap();
    nex_core::index_store::record_query_selection(&db, "vim", "all", "app:a", 100).unwrap();
    nex_core::index_store::record_query_selection(&db, "vim", "all", "app:a", 200).unwrap();
    nex_core::index_store::record_query_selection(&db, "vim", "all", "app:b", 150).unwrap();

    let rows = nex_core::index_store::list_query_selections(&db, "vim", "all", 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "app:a");
    assert_eq!(rows[0].1, 2);
    assert_eq!(rows[0].2, 200);
}
