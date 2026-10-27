use std::fmt::{Display, Formatter};
use std::path::Path;

use rusqlite::{params, Connection};

use crate::config::Config;
use crate::model::SearchItem;

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Db(rusqlite::Error),
}

impl Display for StoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Db(error) => write!(f, "db error: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Db(value)
    }
}

pub fn open_memory() -> Result<Connection, StoreError> {
    let conn = Connection::open_in_memory()?;
    init_schema(&conn)?;
    Ok(conn)
}

pub fn open_file(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(path)?;
    init_schema(&conn)?;
    // WAL mode allows concurrent readers while a background indexer
    // writes to the same database from its own connection. Without
    // this, every connection sharing the file gets SQLITE_BUSY.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // A 1-second busy timeout prevents rapid failures when a
    // background writer briefly holds the commit lock.
    conn.pragma_update(None, "busy_timeout", 1000)?;
    Ok(conn)
}

pub fn open_from_config(cfg: &Config) -> Result<Connection, StoreError> {
    open_file(&cfg.index_db_path)
}

pub fn upsert_item(db: &Connection, item: &SearchItem) -> Result<(), StoreError> {
    db.execute(
        "INSERT INTO item (id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, title=excluded.title, path=excluded.path, subtitle=excluded.subtitle,
         use_count=excluded.use_count, last_accessed_epoch_secs=excluded.last_accessed_epoch_secs,
         launch_count=excluded.launch_count, last_launched_at=excluded.last_launched_at",
        params![
            item.id,
            item.kind,
            item.title,
            item.path,
            item.subtitle,
            item.use_count,
            item.last_accessed_epoch_secs,
            item.launch_count,
            item.last_launched_at,
        ],
    )?;
    Ok(())
}

pub fn get_item(db: &Connection, id: &str) -> Result<Option<SearchItem>, StoreError> {
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at FROM item WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let title: String = row.get(2)?;
        let path: String = row.get(3)?;
        let subtitle: String = row.get(4)?;
        let use_count: u32 = row.get(5)?;
        let last_accessed_epoch_secs: i64 = row.get(6)?;
        let launch_count: u32 = row.get(7)?;
        let last_launched_at: i64 = row.get(8)?;
        Ok(Some(SearchItem::from_owned_with_usage(
            id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at,
        )))
    } else {
        Ok(None)
    }
}

pub fn list_items(db: &Connection) -> Result<Vec<SearchItem>, StoreError> {
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at FROM item ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let title: String = row.get(2)?;
        let path: String = row.get(3)?;
        let subtitle: String = row.get(4)?;
        let use_count: u32 = row.get(5)?;
        let last_accessed_epoch_secs: i64 = row.get(6)?;
        let launch_count: u32 = row.get(7)?;
        let last_launched_at: i64 = row.get(8)?;
        out.push(SearchItem::from_owned_with_usage(
            id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at,
        ));
    }

    Ok(out)
}

pub fn clear_items(db: &Connection) -> Result<(), StoreError> {
    db.execute("DELETE FROM item", [])?;
    Ok(())
}

pub fn delete_item(db: &Connection, id: &str) -> Result<(), StoreError> {
    db.execute("DELETE FROM item WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn get_meta(db: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    let mut stmt = db.prepare("SELECT value FROM index_meta WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? {
        let value: String = row.get(0)?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

pub fn set_meta(db: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    db.execute(
        "INSERT INTO index_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn record_query_selection(
    db: &Connection,
    query_norm: &str,
    mode: &str,
    item_id: &str,
    selected_at_epoch_secs: i64,
) -> Result<(), StoreError> {
    db.execute(
        "INSERT INTO item_query_memory (query_norm, mode, item_id, selected_count, last_selected_epoch_secs)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT(query_norm, mode, item_id) DO UPDATE SET
         selected_count = MIN(item_query_memory.selected_count + 1, 1000),
         last_selected_epoch_secs = excluded.last_selected_epoch_secs",
        params![query_norm, mode, item_id, selected_at_epoch_secs],
    )?;
    Ok(())
}

/// Get Quick Launch items: pinned apps first, then top by launch_count.
/// Returns (id, kind, title, path, subtitle, icon_path, is_pinned) tuples.
pub fn get_quick_launch_items(
    db: &Connection,
    pinned_paths: &[String],
    max_items: usize,
    auto_fill: bool,
) -> Result<Vec<(String, String, String, String, String, String, bool)>, StoreError> {
    let mut result = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // First: add pinned apps (in the order specified by the user)
    for pinned_path in pinned_paths {
        if result.len() >= max_items {
            break;
        }
        let trimmed = pinned_path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let item = if let Ok(url) = crate::config::normalize_bookmark_url(trimmed) {
            Some((
                crate::bookmarks::bookmark_id(&url),
                "bookmark".to_string(),
                url.clone(),
                url,
                String::new(),
            ))
        } else {
            find_item_by_path_or_title(db, trimmed)?
        };
        if let Some((id, kind, title, path, subtitle)) = item {
            if seen_ids.insert(id.clone()) {
                let icon_path = path.clone();
                result.push((id, kind, title, path, subtitle, icon_path, true));
            }
        }
    }

    if !auto_fill {
        return Ok(result);
    }

    if result.len() >= max_items {
        return Ok(result);
    }

    // Two signals, both must surface: what gets clicked most, then what was
    // clicked last. Overlap is dropped, so the recent pass only adds new
    // items instead of repeating the frequent ones.
    let mut seen_ids = seen_ids;
    let passes = [
        ("launch_count DESC, last_launched_at DESC", usize::div_ceil(max_items - result.len(), 2)),
        ("last_launched_at DESC", max_items - result.len()),
    ];
    for (order_by, limit) in passes {
        if result.len() >= max_items {
            break;
        }
        let sql = format!(
            "SELECT id, kind, title, path, subtitle FROM item
             WHERE kind = 'app' AND launch_count > 0
             ORDER BY {order_by}
             LIMIT ?1"
        );
        let mut stmt = db.prepare(&sql)?;
        let mut rows = stmt.query(params![limit as i64])?;
        while let Some(row) = rows.next()? {
            if result.len() >= max_items {
                break;
            }
            let id: String = row.get(0)?;
            if seen_ids.insert(id.clone()) {
                let kind: String = row.get(1)?;
                let title: String = row.get(2)?;
                let path: String = row.get(3)?;
                let subtitle: String = row.get(4)?;
                let icon_path = path.clone();
                result.push((id, kind, title, path, subtitle, icon_path, false));
            }
        }
    }

    // Cold start: nothing pinned and no launch history yet (fresh index, or
    // a reset). Show the first indexed apps so the idle panel is never
    // blank; the usage passes take over as soon as anything is opened.
    // A list that already has pins or history is never padded alphabetically.
    if result.is_empty() {
        let mut stmt = db.prepare(
            "SELECT id, kind, title, path, subtitle FROM item
             WHERE kind = 'app'
             ORDER BY title COLLATE NOCASE
             LIMIT ?1",
        )?;
        let mut rows = stmt.query(params![max_items as i64])?;
        while let Some(row) = rows.next()? {
            if result.len() >= max_items {
                break;
            }
            let id: String = row.get(0)?;
            if seen_ids.insert(id.clone()) {
                let kind: String = row.get(1)?;
                let title: String = row.get(2)?;
                let path: String = row.get(3)?;
                let subtitle: String = row.get(4)?;
                let icon_path = path.clone();
                result.push((id, kind, title, path, subtitle, icon_path, false));
            }
        }
    }

    Ok(result)
}

/// List every indexed app (id, title, path, subtitle), alphabetical.
/// Backs the overlay's "Show all apps" full-index section.
pub fn get_all_apps(
    db: &Connection,
) -> Result<Vec<(String, String, String, String)>, StoreError> {
    let mut stmt = db.prepare(
        "SELECT id, title, path, subtitle FROM item
         WHERE kind = 'app'
         ORDER BY title COLLATE NOCASE",
    )?;
    let mut rows = stmt.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        result.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
    }
    Ok(result)
}

/// Find an item by path or title (case-insensitive).
pub fn find_item_by_path_or_title(
    db: &Connection,
    query: &str,
) -> Result<Option<(String, String, String, String, String)>, StoreError> {
    // Try exact path match first
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle FROM item WHERE path = ?1 LIMIT 1",
    )?;
    let mut rows = stmt.query(params![query])?;
    if let Some(row) = rows.next()? {
        return Ok(Some((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        )));
    }

    // Try case-insensitive path match
    let normalized_query = query.replace('/', "\\").to_ascii_lowercase();
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle FROM item WHERE LOWER(REPLACE(path, '/', '\\')) = ?1 LIMIT 1",
    )?;
    let mut rows = stmt.query(params![normalized_query])?;
    if let Some(row) = rows.next()? {
        return Ok(Some((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        )));
    }

    // Try case-insensitive title match
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle FROM item WHERE LOWER(title) = LOWER(?1) LIMIT 1",
    )?;
    let mut rows = stmt.query(params![query])?;
    if let Some(row) = rows.next()? {
        return Ok(Some((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        )));
    }

    Ok(None)
}

pub fn list_query_selections(
    db: &Connection,
    query_norm: &str,
    mode: &str,
    limit: usize,
) -> Result<Vec<(String, u32, i64)>, StoreError> {
    if query_norm.trim().is_empty() || mode.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut stmt = db.prepare(
        "SELECT item_id, selected_count, last_selected_epoch_secs
         FROM item_query_memory
         WHERE query_norm = ?1 AND mode = ?2
         ORDER BY selected_count DESC, last_selected_epoch_secs DESC
         LIMIT ?3",
    )?;
    let mut rows = stmt.query(params![query_norm, mode, limit as i64])?;

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?, row.get(2)?));
    }
    Ok(out)
}

fn init_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let current_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

        if current_version < 1 {
            migration_v1(conn)?;
        }
        if current_version < 2 {
            migration_v2(conn)?;
        }
        if current_version < 3 {
            migration_v3(conn)?;
        }
        if current_version < 4 {
            migration_v4(conn)?;
        }
        if current_version < 5 {
            migration_v5(conn)?;
        }

        if current_version < 5 {
            conn.pragma_update(None, "user_version", 5_i64)?;
        }

        Ok::<(), StoreError>(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn migration_v1(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS item (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            path TEXT NOT NULL,
            subtitle TEXT NOT NULL DEFAULT '',
            use_count INTEGER NOT NULL DEFAULT 0,
            last_accessed_epoch_secs INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )?;

    Ok(())
}

fn migration_v2(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS index_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;
    Ok(())
}

fn migration_v3(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS item_query_memory (
            query_norm TEXT NOT NULL,
            mode TEXT NOT NULL,
            item_id TEXT NOT NULL,
            selected_count INTEGER NOT NULL DEFAULT 0,
            last_selected_epoch_secs INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(query_norm, mode, item_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_item_query_memory_lookup
         ON item_query_memory(query_norm, mode, selected_count DESC, last_selected_epoch_secs DESC)",
        [],
    )?;
    Ok(())
}

fn migration_v4(conn: &Connection) -> Result<(), StoreError> {
    let mut has_subtitle = false;
    let mut stmt = conn.prepare("PRAGMA table_info(item)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let column_name: String = row.get(1)?;
        if column_name.eq_ignore_ascii_case("subtitle") {
            has_subtitle = true;
            break;
        }
    }

    if !has_subtitle {
        conn.execute(
            "ALTER TABLE item ADD COLUMN subtitle TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

/// Add launch_count and last_launched_at columns for Quick Launch feature.
fn migration_v5(conn: &Connection) -> Result<(), StoreError> {
    let mut has_launch_count = false;
    let mut has_last_launched_at = false;
    let mut stmt = conn.prepare("PRAGMA table_info(item)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let column_name: String = row.get(1)?;
        if column_name.eq_ignore_ascii_case("launch_count") {
            has_launch_count = true;
        }
        if column_name.eq_ignore_ascii_case("last_launched_at") {
            has_last_launched_at = true;
        }
    }

    if !has_launch_count {
        conn.execute(
            "ALTER TABLE item ADD COLUMN launch_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !has_last_launched_at {
        conn.execute(
            "ALTER TABLE item ADD COLUMN last_launched_at INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SearchItem;

    fn app(id: &str, title: &str, launches: u32, last: i64) -> SearchItem {
        SearchItem::from_owned_with_usage(
            id.into(),
            "app".into(),
            title.into(),
            format!("C:\\Program Files\\{title}\\{title}.exe"),
            String::new(),
            0,
            0,
            launches,
            last,
        )
    }

    #[test]
    fn quick_launch_auto_fill_mixes_frequent_and_recent() {
        let db = open_memory().unwrap();
        // a: most clicked, oldest click. b: clicked often, newer. c: newest
        // click. d: never launched — must not appear.
        upsert_item(&db, &app("a", "Alpha", 9, 1_000)).unwrap();
        upsert_item(&db, &app("b", "Bravo", 5, 2_000)).unwrap();
        upsert_item(&db, &app("c", "Charlie", 1, 9_000)).unwrap();
        upsert_item(&db, &app("d", "Delta", 0, 0)).unwrap();

        let items = get_quick_launch_items(&db, &[], 4, true).unwrap();
        let titles: Vec<&str> = items.iter().map(|item| item.2.as_str()).collect();
        assert_eq!(titles.len(), 3);
        assert_eq!(titles[0], "Alpha", "most clicked leads");
        assert!(titles.contains(&"Charlie"), "newest click is listed");
        assert!(!titles.contains(&"Delta"), "unused apps stay out");
        let unique: std::collections::HashSet<_> = titles.iter().collect();
        assert_eq!(unique.len(), titles.len(), "no duplicates between passes");
    }

    #[test]
    fn quick_launch_cold_start_lists_indexed_apps() {
        let db = open_memory().unwrap();
        upsert_item(&db, &app("a", "Zulu", 0, 0)).unwrap();
        upsert_item(&db, &app("b", "Alpha", 0, 0)).unwrap();
        let items = get_quick_launch_items(&db, &[], 6, true).unwrap();
        let titles: Vec<&str> = items.iter().map(|item| item.2.as_str()).collect();
        assert_eq!(titles, vec!["Alpha", "Zulu"], "alphabetical cold start");
        assert!(items.iter().all(|item| !item.6), "fallback items are not pinned");
    }

    #[test]
    fn cold_start_stays_off_once_something_is_pinned() {
        let db = open_memory().unwrap();
        upsert_item(&db, &app("a", "Alpha", 0, 0)).unwrap();
        upsert_item(&db, &app("b", "Bravo", 0, 0)).unwrap();
        let items = get_quick_launch_items(
            &db,
            &["C:\\Program Files\\Alpha\\Alpha.exe".to_string()],
            6,
            true,
        )
        .unwrap();
        let titles: Vec<&str> = items.iter().map(|item| item.2.as_str()).collect();
        assert_eq!(titles, vec!["Alpha"], "no alphabetical tail beside a pin");
    }

    #[test]
    fn quick_launch_pinned_leads_and_autofill_does_not_repeat_it() {
        let db = open_memory().unwrap();
        upsert_item(&db, &app("a", "Alpha Beta", 9, 1_000)).unwrap();
        let items = get_quick_launch_items(&db, &["Alpha Beta".to_string()], 3, true).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].6, "explicitly pinned item reports is_pinned");
    }
}
