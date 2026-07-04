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
         use_count=excluded.use_count, last_accessed_epoch_secs=excluded.last_accessed_epoch_secs",
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

/// Record an app launch for Quick Launch usage tracking.
pub fn record_launch(
    db: &Connection,
    item_id: &str,
    launched_at_epoch_secs: i64,
) -> Result<(), StoreError> {
    db.execute(
        "UPDATE item SET
         launch_count = launch_count + 1,
         last_launched_at = ?2
         WHERE id = ?1",
        params![item_id, launched_at_epoch_secs],
    )?;
    Ok(())
}

/// Get Quick Launch items: pinned apps first, then top by launch_count.
/// Returns (id, kind, title, path, subtitle, icon_path, is_pinned) tuples.
pub fn get_quick_launch_items(
    db: &Connection,
    pinned_paths: &[String],
    max_items: usize,
) -> Result<Vec<(String, String, String, String, String, String, bool)>, StoreError> {
    let mut result = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // First: add pinned apps (in the order specified by the user)
    for pinned_path in pinned_paths {
        let trimmed = pinned_path.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Try to find by path first, then by title
        let item = find_item_by_path_or_title(db, trimmed)?;
        if let Some((id, kind, title, path, subtitle)) = item {
            if seen_ids.insert(id.clone()) {
                let icon_path = path.clone();
                result.push((id, kind, title, path, subtitle, icon_path, true));
            }
        }
    }

    // If pinned items exist, ONLY show pinned items (no auto-fill)
    if !result.is_empty() {
        return Ok(result);
    }

    // No pinned items: auto-fill from usage
    let remaining = max_items.saturating_sub(result.len());
    if remaining > 0 {
        // First try apps with launch_count > 0
        let mut stmt = db.prepare(
            "SELECT id, kind, title, path, subtitle FROM item
             WHERE kind = 'app' AND launch_count > 0
             ORDER BY launch_count DESC, last_launched_at DESC
             LIMIT ?1",
        )?;
        let mut rows = stmt.query(params![remaining as i64])?;
        while let Some(row) = rows.next()? {
            if result.len() >= max_items {
                break;
            }
            let id: String = row.get(0)?;
            if !seen_ids.contains(&id) {
                let kind: String = row.get(1)?;
                let title: String = row.get(2)?;
                let path: String = row.get(3)?;
                let subtitle: String = row.get(4)?;
                let icon_path = path.clone();
                result.push((id, kind, title, path, subtitle, icon_path, false));
            }
        }

        // If still not enough, fill with any apps (alphabetical)
        let still_remaining = max_items.saturating_sub(result.len());
        if still_remaining > 0 {
            let mut stmt = db.prepare(
                "SELECT id, kind, title, path, subtitle FROM item
                 WHERE kind = 'app'
                 ORDER BY title ASC
                 LIMIT ?1",
            )?;
            let mut rows = stmt.query(params![still_remaining as i64])?;
            while let Some(row) = rows.next()? {
                if result.len() >= max_items {
                    break;
                }
                let id: String = row.get(0)?;
                if !seen_ids.insert(id.clone()) {
                    continue;
                }
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

    Ok(())
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
