use std::fmt::{Display, Formatter};
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

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

/// Read-only connection for diagnostics: no schema migration and no write
/// lock, so probing never contends with a running instance.
pub fn open_read_only(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "busy_timeout", 1000)?;
    Ok(conn)
}

pub fn open_from_config(cfg: &Config) -> Result<Connection, StoreError> {
    open_file(&cfg.index_db_path)
}

const UPSERT_ITEM_SQL: &str =
    "INSERT INTO item (id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, title=excluded.title, path=excluded.path, subtitle=excluded.subtitle,
         use_count=excluded.use_count, last_accessed_epoch_secs=excluded.last_accessed_epoch_secs,
         launch_count=excluded.launch_count, last_launched_at=excluded.last_launched_at";

pub fn upsert_item(db: &Connection, item: &SearchItem) -> Result<(), StoreError> {
    db.execute(
        UPSERT_ITEM_SQL,
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

/// Write many items in one transaction. The FTS triggers fire per row, so the
/// search index stays identical to the single-row path.
pub fn upsert_items(db: &Connection, items: &[SearchItem]) -> Result<(), StoreError> {
    if items.is_empty() {
        return Ok(());
    }
    db.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(), StoreError> {
        let mut stmt = db.prepare_cached(UPSERT_ITEM_SQL)?;
        for item in items {
            stmt.execute(params![
                item.id,
                item.kind,
                item.title,
                item.path,
                item.subtitle,
                item.use_count,
                item.last_accessed_epoch_secs,
                item.launch_count,
                item.last_launched_at,
            ])?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            db.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(error)
        }
    }
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

/// Delete many ids in one transaction.
pub fn delete_items(db: &Connection, ids: &[String]) -> Result<(), StoreError> {
    if ids.is_empty() {
        return Ok(());
    }
    db.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(), StoreError> {
        let mut stmt = db.prepare_cached("DELETE FROM item WHERE id = ?1")?;
        for id in ids {
            stmt.execute(params![id])?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            db.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

const HOT_APPS_SQL: &str =
    "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at
     FROM item WHERE kind = 'app' ORDER BY title COLLATE NOCASE";
const HOT_OTHERS_SQL: &str =
    "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at
     FROM item WHERE kind != 'app'
     ORDER BY use_count DESC, last_accessed_epoch_secs DESC, last_launched_at DESC, id ASC
     LIMIT ?1";

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchItem> {
    let id: String = row.get(0)?;
    let kind: String = row.get(1)?;
    let title: String = row.get(2)?;
    let path: String = row.get(3)?;
    let subtitle: String = row.get(4)?;
    let use_count: u32 = row.get(5)?;
    let last_accessed_epoch_secs: i64 = row.get(6)?;
    let launch_count: u32 = row.get(7)?;
    let last_launched_at: i64 = row.get(8)?;
    Ok(SearchItem::from_owned_with_usage(
        id,
        kind,
        title,
        path,
        subtitle,
        use_count,
        last_accessed_epoch_secs,
        launch_count,
        last_launched_at,
    ))
}

/// Ids owned by the given kinds, for provider-scoped stale pruning.
pub fn list_item_ids_for_kinds(
    db: &Connection,
    kinds: &[&str],
) -> Result<Vec<String>, StoreError> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(kinds.len())
        .collect::<Vec<_>>()
        .join(",");
    let mut stmt = db.prepare(&format!(
        "SELECT id FROM item WHERE kind IN ({placeholders})"
    ))?;
    let mut rows = stmt.query(rusqlite::params_from_iter(kinds.iter().copied()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Delete every row whose path is `prefix` or below it, and return the ids.
pub fn delete_items_under_prefix(db: &Connection, prefix: &str) -> Result<Vec<String>, StoreError> {
    let like = format!("{}%", escape_like(prefix));
    let mut ids = Vec::new();
    {
        let mut stmt = db.prepare("SELECT id FROM item WHERE path LIKE ?1 ESCAPE '!'")?;
        let mut rows = stmt.query(params![like])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
        }
    }
    for chunk in ids.chunks(500) {
        delete_items(db, chunk)?;
    }
    Ok(ids)
}

/// Start a discovery scan: the seen table records every id the pass touched
/// so the prune can remove rows that are no longer discovered.
pub fn scan_begin(db: &Connection) -> Result<(), StoreError> {
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS rebuild_seen (id TEXT PRIMARY KEY) WITHOUT ROWID;
         DELETE FROM rebuild_seen;",
    )?;
    Ok(())
}

/// Record a batch of discovered ids in the current scan.
pub fn scan_mark(db: &Connection, ids: &[String]) -> Result<(), StoreError> {
    if ids.is_empty() {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("INSERT OR IGNORE INTO rebuild_seen (id) VALUES (?1)")?;
        for id in ids {
            stmt.execute(params![id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Delete rows of the given kinds that the current scan never saw, and return
/// their ids for the search backends and the UI publish log.
pub fn scan_prune(db: &Connection, kinds: &[&str]) -> Result<Vec<String>, StoreError> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(kinds.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id FROM item WHERE kind IN ({placeholders})
         AND id NOT IN (SELECT id FROM rebuild_seen)"
    );
    let mut ids = Vec::new();
    {
        let mut stmt = db.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(kinds.iter().copied()))?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
        }
    }
    for chunk in ids.chunks(500) {
        delete_items(db, chunk)?;
    }
    Ok(ids)
}

/// Drop the scan seen table.
pub fn scan_end(db: &Connection) -> Result<(), StoreError> {
    db.execute_batch("DROP TABLE IF EXISTS rebuild_seen")?;
    Ok(())
}

/// One page of store rows ordered by id, for a full-store stale scan.
/// Pass the last id of the previous page to continue; `None` starts over.
pub fn list_item_page(
    db: &Connection,
    after_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchItem>, StoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    const ITEM_COLUMNS: &str =
        "SELECT id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, launch_count, last_launched_at FROM item";
    let mut out = Vec::with_capacity(limit);
    match after_id {
        Some(after) => {
            let mut stmt = db.prepare(&format!(
                "{ITEM_COLUMNS} WHERE id > ?1 ORDER BY id LIMIT ?2"
            ))?;
            let mut rows = stmt.query(params![after, limit as i64])?;
            while let Some(row) = rows.next()? {
                out.push(row_to_item(row)?);
            }
        }
        None => {
            let mut stmt = db.prepare(&format!("{ITEM_COLUMNS} ORDER BY id LIMIT ?1"))?;
            let mut rows = stmt.query(params![limit as i64])?;
            while let Some(row) = rows.next()? {
                out.push(row_to_item(row)?);
            }
        }
    }
    Ok(out)
}

/// All apps plus the highest-use/most-recent non-app rows, capped at `limit`.
/// Matches `core_service::bounded_ui_items` without reading the full store.
pub fn list_hot_items(db: &Connection, limit: usize) -> Result<Vec<SearchItem>, StoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut stmt = db.prepare(HOT_APPS_SQL)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        out.push(row_to_item(row)?);
    }
    if out.len() >= limit {
        return Ok(out);
    }
    let remaining = limit - out.len();
    let mut stmt = db.prepare(HOT_OTHERS_SQL)?;
    let mut rows = stmt.query(params![remaining as i64])?;
    while let Some(row) = rows.next()? {
        out.push(row_to_item(row)?);
    }
    Ok(out)
}

/// Escape `%`, `_` and the escape char itself for a `LIKE ... ESCAPE '!'`.
pub(crate) fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '!') {
            out.push('!');
        }
        out.push(ch);
    }
    out
}

/// Ids of file/folder rows strictly beneath `folder_path` (case-insensitive,
/// either separator).
pub fn list_item_ids_under_path(db: &Connection, folder_path: &str) -> Result<Vec<String>, StoreError> {
    let base = folder_path
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    if base.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("{}\\%", escape_like(&base));
    let mut stmt = db.prepare(
        "SELECT id FROM item
         WHERE kind IN ('file', 'folder') AND path LIKE ?1 ESCAPE '!'",
    )?;
    let mut rows = stmt.query(params![pattern])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Substring recall over file/folder rows: every whitespace-separated term
/// must appear in the title or path. Used when Tantivy and the compacted
/// in-memory cache cannot produce enough candidates, so any indexed row
/// stays reachable.
///
/// Terms of 3+ characters go through the title trigram index (~1 ms); the
/// unindexed `LIKE` scan (~40 ms at 120k rows) is only used while the
/// index backfill is still running or when every term is shorter.
pub fn search_file_folder_items(
    db: &Connection,
    query: &str,
    include_files: bool,
    include_folders: bool,
    limit: usize,
) -> Result<Vec<SearchItem>, StoreError> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.replace('/', "\\").to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .take(8)
        .collect();
    if terms.is_empty() || limit == 0 || !(include_files || include_folders) {
        return Ok(Vec::new());
    }
    let mut kinds = Vec::new();
    if include_files {
        kinds.push("'file'");
    }
    if include_folders {
        kinds.push("'folder'");
    }
    let trigram_terms: Vec<&String> = terms.iter().filter(|t| t.chars().count() >= 3).collect();
    let use_fts = !trigram_terms.is_empty() && !title_fts_backfill_pending(db)?;

    let mut values: Vec<String> = Vec::with_capacity(terms.len() + 1);
    let mut sql = if use_fts {
        // Candidate if the title contains any long term; the LIKE filters
        // below then require every term in the title or path.
        let match_expr = trigram_terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        values.push(match_expr);
        format!(
            "SELECT i.id, i.kind, i.title, i.path, i.subtitle, i.use_count, i.last_accessed_epoch_secs, i.launch_count, i.last_launched_at
             FROM item_title_fts f JOIN item i ON i.rowid = f.rowid
             WHERE item_title_fts MATCH ?1 AND i.kind IN ({})",
            kinds.join(", ")
        )
    } else {
        format!(
            "SELECT i.id, i.kind, i.title, i.path, i.subtitle, i.use_count, i.last_accessed_epoch_secs, i.launch_count, i.last_launched_at
             FROM item i WHERE i.kind IN ({})",
            kinds.join(", ")
        )
    };
    let first_term_param = values.len() + 1;
    for term in &terms {
        values.push(format!("%{}%", escape_like(term)));
        // LIKE is ASCII case-insensitive, matching `to_ascii_lowercase` above.
        sql.push_str(&format!(
            " AND (i.title LIKE ?{n} ESCAPE '!' OR i.path LIKE ?{n} ESCAPE '!')",
            n = values.len()
        ));
    }
    // Title matches first, then shorter (shallower) paths.
    sql.push_str(&format!(
        " ORDER BY (i.title LIKE ?{first_term_param} ESCAPE '!') DESC, LENGTH(i.path) ASC LIMIT {limit}"
    ));
    let mut stmt = db.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(values.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SearchItem::from_owned_with_usage(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
        ));
    }
    Ok(out)
}

/// Count of file/folder rows at or beneath `root` (case-insensitive).
pub fn count_file_folder_items_under_root(
    db: &Connection,
    root: &str,
) -> Result<(u64, u64), StoreError> {
    let base = root.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    if base.is_empty() {
        return Ok((0, 0));
    }
    let pattern = format!("{}\\%", escape_like(&base));
    let (files, folders): (i64, i64) = db.query_row(
        "SELECT
            COALESCE(SUM(kind = 'file'), 0),
            COALESCE(SUM(kind = 'folder'), 0)
         FROM item
         WHERE kind IN ('file', 'folder')
           AND LOWER(REPLACE(path, '/', '\\')) LIKE ?1 ESCAPE '!'",
        params![pattern],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((files.max(0) as u64, folders.max(0) as u64))
}

pub fn count_items(db: &Connection) -> Result<u64, StoreError> {
    let n: i64 = db.query_row("SELECT COUNT(*) FROM item", [], |row| row.get(0))?;
    Ok(n.max(0) as u64)
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

    // First: add every pinned app (in the order specified by the user).
    // `max_items` only limits auto-filled suggestions; explicit pins are
    // always shown, even when their count exceeds that limit.
    for pinned_path in pinned_paths {
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
        (QUICK_LAUNCH_FREQUENT_SQL, usize::div_ceil(max_items - result.len(), 2)),
        (QUICK_LAUNCH_RECENT_SQL, max_items - result.len()),
    ];
    for (sql, limit) in passes {
        if result.len() >= max_items {
            break;
        }
        let mut stmt = db.prepare_cached(sql)?;
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

/// Find an item by path or title (case-insensitive). Runs on every overlay
/// show (pinned Quick Launch items), so every step must hit an index
/// (`idx_item_path_nocase`, `idx_item_title_nocase`, `idx_item_app_launch`);
/// unindexed scans cost ~25 ms per pin at 120k rows.
pub fn find_item_by_path_or_title(
    db: &Connection,
    query: &str,
) -> Result<Option<(String, String, String, String, String)>, StoreError> {
    let read = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(String, String, String, String, String)> {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
    };

    // Path match, ASCII case-insensitive like the previous LOWER() lookup;
    // an exact-case row wins when several differ only by case.
    let mut stmt = db.prepare_cached(FIND_BY_PATH_SQL)?;
    if let Some(found) = stmt.query_row(params![query], read).optional()? {
        return Ok(Some(found));
    }
    let backslashed = query.replace('/', "\\");
    if backslashed != query {
        if let Some(found) = stmt.query_row(params![backslashed], read).optional()? {
            return Ok(Some(found));
        }
    }
    // Stored paths with forward slashes: only app rows (the partial index
    // keeps this to a few hundred rows).
    let mut stmt = db.prepare_cached(FIND_APP_BY_NORMALIZED_PATH_SQL)?;
    if let Some(found) = stmt
        .query_row(params![backslashed.to_ascii_lowercase()], read)
        .optional()?
    {
        return Ok(Some(found));
    }

    let mut stmt = db.prepare_cached(FIND_BY_TITLE_SQL)?;
    Ok(stmt.query_row(params![query], read).optional()?)
}

const FIND_BY_PATH_SQL: &str = "SELECT id, kind, title, path, subtitle FROM item
     WHERE path = ?1 COLLATE NOCASE ORDER BY path = ?1 DESC LIMIT 1";
const FIND_APP_BY_NORMALIZED_PATH_SQL: &str = "SELECT id, kind, title, path, subtitle FROM item
     WHERE kind = 'app' AND LOWER(REPLACE(path, '/', '\\')) = ?1 LIMIT 1";
const FIND_BY_TITLE_SQL: &str = "SELECT id, kind, title, path, subtitle FROM item
     WHERE title = ?1 COLLATE NOCASE LIMIT 1";
const QUICK_LAUNCH_FREQUENT_SQL: &str = "SELECT id, kind, title, path, subtitle FROM item
     WHERE kind = 'app' AND launch_count > 0
     ORDER BY launch_count DESC, last_launched_at DESC
     LIMIT ?1";
const QUICK_LAUNCH_RECENT_SQL: &str = "SELECT id, kind, title, path, subtitle FROM item
     WHERE kind = 'app' AND launch_count > 0
     ORDER BY last_launched_at DESC
     LIMIT ?1";

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
        if current_version < 6 {
            migration_v6(conn)?;
        }
        if current_version < 7 {
            migration_v7(conn)?;
        }

        if current_version < 7 {
            conn.pragma_update(None, "user_version", 7_i64)?;
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

/// Cover the bounded-UI snapshot ordering (apps are fetched separately).
fn migration_v7(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_item_hot
             ON item(use_count DESC, last_accessed_epoch_secs DESC, last_launched_at DESC, id)
             WHERE kind != 'app';",
    )?;
    Ok(())
}

/// Add launch_count and last_launched_at columns for Quick Launch feature.
/// Indexes for the per-show Quick Launch lookups (see
/// `find_item_by_path_or_title`). The app index is partial so it never
/// competes with file/folder scans.
///
/// Also adds a trigram index over titles for substring recall
/// (`search_file_folder_items`). It is keyed by `item.rowid` and kept in
/// sync by triggers; rows that predate the migration are filled in by
/// `backfill_title_fts_batch` in the background so opening a large store
/// never blocks on it.
fn migration_v6(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_item_path_nocase ON item(path COLLATE NOCASE);
         CREATE INDEX IF NOT EXISTS idx_item_title_nocase ON item(title COLLATE NOCASE);
         CREATE INDEX IF NOT EXISTS idx_item_app_launch
             ON item(launch_count, last_launched_at) WHERE kind = 'app';
         CREATE VIRTUAL TABLE IF NOT EXISTS item_title_fts USING fts5(
             title, content='', contentless_delete=1, tokenize='trigram');
         CREATE TRIGGER IF NOT EXISTS item_title_fts_ai AFTER INSERT ON item BEGIN
             INSERT OR REPLACE INTO item_title_fts(rowid, title) VALUES (new.rowid, new.title);
         END;
         CREATE TRIGGER IF NOT EXISTS item_title_fts_ad AFTER DELETE ON item BEGIN
             DELETE FROM item_title_fts WHERE rowid = old.rowid;
         END;
         CREATE TRIGGER IF NOT EXISTS item_title_fts_au AFTER UPDATE OF title ON item
             WHEN old.title IS NOT new.title BEGIN
             INSERT OR REPLACE INTO item_title_fts(rowid, title) VALUES (new.rowid, new.title);
         END;
         INSERT INTO index_meta (key, value)
             SELECT 'title_fts_backfill_upto', CAST(COALESCE(MAX(rowid), 0) AS TEXT) FROM item
             WHERE true
             ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
    )?;
    Ok(())
}

const TITLE_FTS_BACKFILL_KEY: &str = "title_fts_backfill_upto";

/// True while rows that predate the title index are still being indexed.
pub fn title_fts_backfill_pending(db: &Connection) -> Result<bool, StoreError> {
    Ok(get_meta(db, TITLE_FTS_BACKFILL_KEY)?
        .and_then(|v| v.parse::<i64>().ok())
        .is_some_and(|upto| upto > 0))
}

/// Index up to `batch` pre-migration rows (highest rowids first). Returns
/// true once the backfill is complete. Small batches keep each call short
/// so the shared connection is never held for long.
pub fn backfill_title_fts_batch(db: &Connection, batch: usize) -> Result<bool, StoreError> {
    let upto = match get_meta(db, TITLE_FTS_BACKFILL_KEY)?.and_then(|v| v.parse::<i64>().ok()) {
        Some(upto) if upto > 0 => upto,
        _ => return Ok(true),
    };
    let low = upto.saturating_sub(batch.max(1) as i64).max(0);
    db.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        db.execute(
            "INSERT OR REPLACE INTO item_title_fts(rowid, title)
             SELECT rowid, title FROM item WHERE rowid > ?1 AND rowid <= ?2",
            params![low, upto],
        )?;
        set_meta(db, TITLE_FTS_BACKFILL_KEY, &low.to_string())?;
        Ok::<(), StoreError>(())
    })();
    match result {
        Ok(()) => {
            db.execute_batch("COMMIT")?;
            Ok(low == 0)
        }
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

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

    fn plan(db: &Connection, sql: &str) -> String {
        let mut stmt = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt
            .query_map(params!["x"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.join(" | ")
    }

    #[test]
    fn batch_upsert_and_delete_keep_store_consistent() {
        let db = open_memory().unwrap();
        let a = SearchItem::new(
            "file:c:\\temp\\a.txt",
            "file",
            "a.txt",
            "C:\\temp\\a.txt",
        );
        let b = SearchItem::new(
            "file:c:\\temp\\b.txt",
            "file",
            "b.txt",
            "C:\\temp\\b.txt",
        );

        upsert_items(&db, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(count_items(&db).unwrap(), 2);
        assert!(get_item(&db, &a.id).unwrap().is_some());

        upsert_items(&db, &[a.clone()]).unwrap();
        assert_eq!(count_items(&db).unwrap(), 2);

        delete_items(&db, &[a.id.clone()]).unwrap();
        assert_eq!(count_items(&db).unwrap(), 1);
        assert!(get_item(&db, &a.id).unwrap().is_none());
        assert!(get_item(&db, &b.id).unwrap().is_some());
    }

    #[test]
    fn list_item_ids_for_kinds_filters_and_handles_empty() {
        let db = open_memory().unwrap();
        upsert_item(&db, &SearchItem::new("app1", "app", "App", "C:\\app.exe")).unwrap();
        upsert_item(&db, &SearchItem::new("file1", "file", "File", "C:\\file.txt")).unwrap();
        upsert_item(&db, &SearchItem::new("folder1", "folder", "Folder", "C:\\folder"))
            .unwrap();
        let mut files = list_item_ids_for_kinds(&db, &["file", "folder"]).unwrap();
        files.sort();
        assert_eq!(files, vec!["file1".to_string(), "folder1".to_string()]);
        assert_eq!(
            list_item_ids_for_kinds(&db, &["app"]).unwrap(),
            vec!["app1".to_string()]
        );
        assert!(list_item_ids_for_kinds(&db, &[]).unwrap().is_empty());
    }

    #[test]
    fn hot_items_match_the_bounded_ui_policy_and_use_the_index() {
        let db = open_memory().unwrap();
        for index in 0..5 {
            upsert_item(
                &db,
                &SearchItem::new(
                    &format!("app{index}"),
                    "app",
                    &format!("App {index}"),
                    "C:\\app.exe",
                ),
            )
            .unwrap();
        }
        for index in 0..20 {
            let mut item = SearchItem::new(
                &format!("file{index}"),
                "file",
                &format!("File {index}"),
                &format!("C:\\file{index}.txt"),
            );
            item.use_count = index as u32;
            upsert_item(&db, &item).unwrap();
        }

        let mut expected: Vec<String> = crate::core_service::bounded_ui_items(&list_items(&db).unwrap())
            .into_iter()
            .map(|item| item.id)
            .collect();
        let mut hot: Vec<String> = list_hot_items(&db, crate::core_service::UI_CACHE_MAX_ITEMS)
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect();
        expected.sort();
        hot.sort();
        assert_eq!(hot, expected);

        let plan = plan(&db, HOT_OTHERS_SQL);
        assert!(plan.contains("idx_item_hot"), "hot query plan: {plan}");
    }

    #[test]
    fn quick_launch_lookups_use_indexes() {
        let db = open_memory().unwrap();
        for sql in [
            FIND_BY_PATH_SQL,
            FIND_APP_BY_NORMALIZED_PATH_SQL,
            FIND_BY_TITLE_SQL,
            QUICK_LAUNCH_FREQUENT_SQL,
            QUICK_LAUNCH_RECENT_SQL,
        ] {
            let plan = plan(&db, sql);
            assert!(plan.contains("USING INDEX"), "full scan for {sql}: {plan}");
        }
    }

    #[test]
    fn find_item_by_path_or_title_matches_case_and_separators() {
        let db = open_memory().unwrap();
        upsert_item(&db, &app("a", "Alpha", 0, 0)).unwrap();
        let exact = "C:\\Program Files\\Alpha\\Alpha.exe";
        assert_eq!(find_item_by_path_or_title(&db, exact).unwrap().unwrap().0, "a");
        assert_eq!(
            find_item_by_path_or_title(&db, "c:/program files/alpha/ALPHA.EXE").unwrap().unwrap().0,
            "a"
        );
        assert_eq!(find_item_by_path_or_title(&db, "alpha").unwrap().unwrap().0, "a");
        assert!(find_item_by_path_or_title(&db, "C:\\missing.exe").unwrap().is_none());
    }

    fn file(name: &str, dir: &str) -> SearchItem {
        let path = format!("C:\\data\\{dir}\\{name}");
        SearchItem::new(&format!("file:{}", path.to_ascii_lowercase()), "file", name, &path)
    }

    #[test]
    fn title_index_recall_matches_substrings_and_tracks_changes() {
        let db = open_memory().unwrap();
        upsert_item(&db, &file("annual-Report-2024.pdf", "finance")).unwrap();
        upsert_item(&db, &file("notes.txt", "reports")).unwrap();
        assert!(!title_fts_backfill_pending(&db).unwrap());

        let hits = search_file_folder_items(&db, "report", true, true, 10).unwrap();
        assert_eq!(hits.len(), 1, "title match only: {hits:?}");
        // Extra terms may match the path.
        let hits = search_file_folder_items(&db, "finance report", true, true, 10).unwrap();
        assert_eq!(hits.len(), 1);
        let hits = search_file_folder_items(&db, "notes reports", true, true, 10).unwrap();
        assert_eq!(hits.len(), 1);

        delete_item(&db, &file("annual-Report-2024.pdf", "finance").id).unwrap();
        assert!(search_file_folder_items(&db, "report", true, true, 10).unwrap().is_empty());
    }

    #[test]
    fn title_index_backfills_rows_from_before_the_upgrade() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("BEGIN").unwrap();
        for migrate in [migration_v1, migration_v2, migration_v3, migration_v4, migration_v5] {
            migrate(&db).unwrap();
        }
        db.pragma_update(None, "user_version", 5_i64).unwrap();
        db.execute_batch("COMMIT").unwrap();
        for i in 0..2_500 {
            upsert_item(&db, &file(&format!("legacy-{i:04}.log"), "old")).unwrap();
        }
        upsert_item(&db, &file("quarterly-message.txt", "old")).unwrap();

        init_schema(&db).unwrap();
        assert!(title_fts_backfill_pending(&db).unwrap());
        // Recall still works through the LIKE fallback meanwhile.
        assert_eq!(search_file_folder_items(&db, "message", true, true, 5).unwrap().len(), 1);

        let mut steps = 0;
        while !backfill_title_fts_batch(&db, 1_000).unwrap() {
            steps += 1;
            assert!(steps < 10);
        }
        assert!(!title_fts_backfill_pending(&db).unwrap());
        assert_eq!(search_file_folder_items(&db, "message", true, true, 5).unwrap().len(), 1);
        assert_eq!(search_file_folder_items(&db, "legacy-0042", true, true, 5).unwrap().len(), 1);
    }

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
