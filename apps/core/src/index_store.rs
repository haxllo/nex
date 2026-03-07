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
    Ok(conn)
}

pub fn open_from_config(cfg: &Config) -> Result<Connection, StoreError> {
    open_file(&cfg.index_db_path)
}

pub fn upsert_item(db: &Connection, item: &SearchItem) -> Result<(), StoreError> {
    db.execute(
        "INSERT INTO item (id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
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
        ],
    )?;
    Ok(())
}

pub fn get_item(db: &Connection, id: &str) -> Result<Option<SearchItem>, StoreError> {
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs FROM item WHERE id = ?1",
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
        Ok(Some(SearchItem::from_owned_with_subtitle(
            id,
            kind,
            title,
            path,
            subtitle,
            use_count,
            last_accessed_epoch_secs,
        )))
    } else {
        Ok(None)
    }
}

pub fn list_items(db: &Connection) -> Result<Vec<SearchItem>, StoreError> {
    let mut stmt = db.prepare(
        "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs FROM item ORDER BY id",
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
        out.push(SearchItem::from_owned_with_subtitle(
            id,
            kind,
            title,
            path,
            subtitle,
            use_count,
            last_accessed_epoch_secs,
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

    if current_version < 4 {
        conn.pragma_update(None, "user_version", 4_i64)?;
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
