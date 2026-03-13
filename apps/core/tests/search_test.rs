use nex_core::model::SearchItem;
use nex_core::search::SearchFilter;

#[test]
fn typo_query_returns_expected_match() {
    let items = vec![SearchItem::new(
        "1",
        "file",
        "Q4_Report.xlsx",
        "C:\\Q4_Report.xlsx",
    )];

    let results = nex_core::search::search(&items, "q4 reort", 10);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "1");
}

#[test]
fn ranks_better_matches_first() {
    let items = vec![
        SearchItem::new("1", "app", "Code", "C:\\Code.exe"),
        SearchItem::new("2", "app", "Codeium", "C:\\Codeium.exe"),
        SearchItem::new("3", "doc", "Decode Notes", "C:\\DecodeNotes.txt"),
    ];

    let results = nex_core::search::search(&items, "code", 10);

    let ids: Vec<&str> = results.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec!["1", "2", "3"]);
}

#[test]
fn empty_query_returns_no_results() {
    let items = vec![SearchItem::new("1", "app", "Code", "C:\\Code.exe")];

    let results = nex_core::search::search(&items, "   ", 10);

    assert!(results.is_empty());
}

#[test]
fn honors_result_limit() {
    let items = vec![
        SearchItem::new("1", "doc", "Document One", "C:\\Docs\\one.txt"),
        SearchItem::new("2", "doc", "Document Two", "C:\\Docs\\two.txt"),
        SearchItem::new("3", "doc", "Document Three", "C:\\Docs\\three.txt"),
    ];

    let results = nex_core::search::search(&items, "document", 2);

    assert_eq!(results.len(), 2);
}

#[test]
fn recent_item_outranks_older_equivalent() {
    let items = vec![
        SearchItem::new("old", "file", "Report", "C:\\old-report.txt").with_usage(5, 1_000_000),
        SearchItem::new("recent", "file", "Report", "C:\\recent-report.txt")
            .with_usage(5, 2_000_000_000),
    ];

    let results = nex_core::search::search(&items, "report", 10);

    assert_eq!(results[0].id, "recent");
    assert_eq!(results[1].id, "old");
}

#[test]
fn frequency_influences_ties_predictably() {
    let items = vec![
        SearchItem::new("low", "app", "Terminal", "C:\\terminal-low.exe")
            .with_usage(1, 1_800_000_000),
        SearchItem::new("high", "app", "Terminal", "C:\\terminal-high.exe")
            .with_usage(12, 1_800_000_000),
    ];

    let results = nex_core::search::search(&items, "terminal", 10);

    assert_eq!(results[0].id, "high");
    assert_eq!(results[1].id, "low");
}

#[test]
fn apps_then_local_files_then_other_results() {
    let items = vec![
        SearchItem::new(
            "remote",
            "doc",
            "Code Reference",
            "https://example.com/code",
        ),
        SearchItem::new(
            "local",
            "file",
            "Code Notes",
            "C:\\Users\\Admin\\code-notes.txt",
        ),
        SearchItem::new("app", "app", "Code", "C:\\Program Files\\Code\\Code.exe"),
    ];

    let results = nex_core::search::search(&items, "code", 10);
    let ids: Vec<&str> = results.iter().map(|i| i.id.as_str()).collect();

    assert_eq!(ids, vec!["app", "local", "remote"]);
}

#[test]
fn local_file_outranks_network_file_in_same_kind() {
    let items = vec![
        SearchItem::new("network", "file", "Report", "\\\\server\\share\\report.txt"),
        SearchItem::new("local", "file", "Report", "C:\\Reports\\report.txt"),
    ];

    let results = nex_core::search::search(&items, "report", 10);
    let ids: Vec<&str> = results.iter().map(|i| i.id.as_str()).collect();

    assert_eq!(ids, vec!["local", "network"]);
}

#[test]
fn exact_match_outranks_prefix_and_substring() {
    let items = vec![
        SearchItem::new("exact", "app", "Code", "C:\\Code.exe"),
        SearchItem::new("prefix", "app", "CodeRunner", "C:\\CodeRunner.exe"),
        SearchItem::new("substring", "app", "Decode Tool", "C:\\Decode.exe"),
    ];

    let results = nex_core::search::search(&items, "code", 10);
    let ids: Vec<&str> = results.iter().map(|i| i.id.as_str()).collect();

    assert_eq!(ids[0], "exact");
}

#[test]
fn deterministic_order_does_not_depend_on_input_order() {
    let forward = vec![
        SearchItem::new("b-id", "app", "Terminal", "C:\\term-b.exe"),
        SearchItem::new("a-id", "app", "Terminal", "C:\\term-a.exe"),
        SearchItem::new("c-id", "app", "Terminal", "C:\\term-c.exe"),
    ];
    let mut reversed = forward.clone();
    reversed.reverse();

    let forward_ids: Vec<String> = nex_core::search::search(&forward, "term", 10)
        .into_iter()
        .map(|item| item.id)
        .collect();
    let reversed_ids: Vec<String> = nex_core::search::search(&reversed, "term", 10)
        .into_iter()
        .map(|item| item.id)
        .collect();

    assert_eq!(forward_ids, vec!["a-id", "b-id", "c-id"]);
    assert_eq!(reversed_ids, forward_ids);
}

#[test]
fn word_boundary_boost_promotes_whole_word_match() {
    let items = vec![
        SearchItem::new("compact", "app", "Superstudio", "C:\\Superstudio.exe"),
        SearchItem::new("spaced", "app", "Visual Studio Code", "C:\\VSCode.exe"),
    ];

    let results = nex_core::search::search(&items, "studio", 10);
    assert_eq!(results[0].id, "spaced");
}

#[test]
fn acronym_boost_promotes_expected_match() {
    let items = vec![
        SearchItem::new("acronym", "app", "Git Kraken", "C:\\GitKraken.exe"),
        SearchItem::new("fuzzy", "app", "Gecko", "C:\\Gecko.exe"),
    ];

    let results = nex_core::search::search(&items, "gk", 10);
    assert_eq!(results[0].id, "acronym");
}

#[test]
fn short_plain_query_prefers_app_top_hit() {
    let items = vec![
        SearchItem::new("file-exact", "file", "V", "C:\\Users\\Admin\\v.txt"),
        SearchItem::new(
            "app-prefix",
            "app",
            "Vivaldi",
            "C:\\Program Files\\Vivaldi\\vivaldi.exe",
        ),
    ];

    let results = nex_core::search::search(&items, "v", 10);
    assert_eq!(results[0].id, "app-prefix");
}

#[test]
fn extension_filter_matches_only_requested_extension() {
    let items = vec![
        SearchItem::new("txt", "file", "Todo", "C:\\Docs\\todo.txt"),
        SearchItem::new("md", "file", "Readme", "C:\\Docs\\readme.md"),
        SearchItem::new("app", "app", "Markdown Tool", "C:\\Apps\\mdtool.exe"),
    ];

    let filter = SearchFilter {
        extension_filter: Some("md".to_string()),
        ..SearchFilter::default()
    };
    let results = nex_core::search::search_with_filter(&items, "read", 10, &filter);
    let ids: Vec<&str> = results.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["md"]);
}

#[test]
fn visibility_filter_can_hide_files() {
    let items = vec![
        SearchItem::new("file", "file", "Readme.md", "C:\\Docs\\Readme.md"),
        SearchItem::new("folder", "folder", "Docs", "C:\\Docs"),
        SearchItem::new("app", "app", "Docs Viewer", "C:\\Apps\\viewer.exe"),
    ];

    let filter = SearchFilter {
        include_files: false,
        include_folders: true,
        ..SearchFilter::default()
    };
    let results = nex_core::search::search_with_filter(&items, "doc", 10, &filter);
    let ids: Vec<&str> = results.iter().map(|item| item.id.as_str()).collect();
    assert!(!ids.contains(&"file"));
    assert!(ids.contains(&"folder"));
    assert!(ids.contains(&"app"));
}

#[test]
fn visibility_filter_can_hide_folders() {
    let items = vec![
        SearchItem::new("file", "file", "Readme.md", "C:\\Docs\\Readme.md"),
        SearchItem::new("folder", "folder", "Docs", "C:\\Docs"),
        SearchItem::new("app", "app", "Docs Viewer", "C:\\Apps\\viewer.exe"),
    ];

    let filter = SearchFilter {
        include_files: true,
        include_folders: false,
        ..SearchFilter::default()
    };
    let results = nex_core::search::search_with_filter(&items, "doc", 10, &filter);
    let ids: Vec<&str> = results.iter().map(|item| item.id.as_str()).collect();
    assert!(ids.contains(&"file"));
    assert!(!ids.contains(&"folder"));
    assert!(ids.contains(&"app"));
}
