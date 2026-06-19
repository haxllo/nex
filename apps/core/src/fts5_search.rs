#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{params, Connection};

use crate::model::SearchItem;

pub(crate) struct Fts5Index {
    conn: Connection,
}

impl Fts5Index {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        let conn =
            Connection::open(db_path).map_err(|e| format!("failed to open FTS5 database: {e}"))?;

        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS item_fts5 USING fts5(
                id UNINDEXED,
                title,
                path,
                subtitle,
                kind UNINDEXED,
                extension UNINDEXED,
                tokenize='porter unicode61'
            );",
        )
        .map_err(|e| format!("failed to create FTS5 table: {e}"))?;

        Ok(Self { conn })
    }

    /// Warm the OS page cache by issuing a trivial query.
    pub fn warmup(&self) {
        let _ = self.conn.query_row("SELECT 1", [], |_| Ok(()));
    }

    pub fn search(&self, query_text: &str, limit: usize) -> Result<Vec<SearchItem>, String> {
        if query_text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let fts5_query = build_fts5_query(query_text);
        if fts5_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, path, subtitle, kind, bm25(item_fts5, 0.0, 1.0, 1.0, 1.0)
                 FROM item_fts5
                 WHERE item_fts5 MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| format!("FTS5 prepare error: {e}"))?;

        let results = stmt
            .query_map(params![fts5_query, limit as i64], |row| {
                let bm25: f64 = row.get(5)?;
                // FTS5 bm25 returns negative values (closer to 0 = better).
                // Invert so 0.0 is worst, higher is better. Max typical BM25 ~5.0.
                let normalized = (-bm25).max(0.0);
                let pre_score = ((normalized / 5.0) * 5000.0).round() as i64;
                let mut item = SearchItem::from_owned_with_subtitle(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    0,
                    0,
                );
                item.pre_score = Some(pre_score);
                Ok(item)
            })
            .map_err(|e| format!("FTS5 query error: {e}"))?;

        let items: Vec<_> = results.filter_map(|r| r.ok()).collect();

        Ok(items)
    }

    pub fn index_items(&self, items: &[SearchItem]) -> Result<(), String> {
        self.clear()?;

        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO item_fts5 (id, title, path, subtitle, kind, extension)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| format!("FTS5 insert prepare error: {e}"))?;

        for item in items {
            stmt.execute(params![
                item.id,
                item.title,
                item.path,
                item.subtitle,
                item.kind,
                extract_extension(&item.path),
            ])
            .map_err(|e| format!("FTS5 insert error: {e}"))?;
        }

        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM item_fts5", [])
            .map_err(|e| format!("FTS5 clear error: {e}"))?;
        Ok(())
    }

    pub fn delete_item(&self, item_id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM item_fts5 WHERE id = ?1", params![item_id])
            .map_err(|e| format!("FTS5 delete error: {e}"))?;
        Ok(())
    }

    pub fn upsert_item(&self, item: &SearchItem) -> Result<(), String> {
        self.delete_item(&item.id)?;
        self.conn
            .execute(
                "INSERT INTO item_fts5 (id, title, path, subtitle, kind, extension)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    item.id,
                    item.title,
                    item.path,
                    item.subtitle,
                    item.kind,
                    extract_extension(&item.path),
                ],
            )
            .map_err(|e| format!("FTS5 insert error: {e}"))?;
        Ok(())
    }

    pub fn incremental_sync_items(&self, items: &[SearchItem]) -> Result<(), String> {
        // Collect incoming item IDs
        let incoming_ids: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();

        // Query existing IDs
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM item_fts5")
            .map_err(|e| format!("FTS5 select ids error: {e}"))?;
        let existing_ids: HashSet<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("FTS5 query ids error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        // Wrap deletes + inserts in a single transaction
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("FTS5 begin tx error: {e}"))?;

        // Delete removed items (exist in index but not in incoming set)
        for existing_id in existing_ids.difference(&incoming_ids) {
            self.conn
                .execute("DELETE FROM item_fts5 WHERE id = ?1", params![existing_id])
                .map_err(|e| format!("FTS5 delete error: {e}"))?;
        }

        // Add/update incoming items
        for item in items {
            self.conn
                .execute("DELETE FROM item_fts5 WHERE id = ?1", params![item.id])
                .map_err(|e| format!("FTS5 delete for upsert error: {e}"))?;
            self.conn
                .execute(
                    "INSERT INTO item_fts5 (id, title, path, subtitle, kind, extension)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        item.id,
                        item.title,
                        item.path,
                        item.subtitle,
                        item.kind,
                        extract_extension(&item.path),
                    ],
                )
                .map_err(|e| format!("FTS5 insert error: {e}"))?;
        }

        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| format!("FTS5 commit tx error: {e}"))?;

        Ok(())
    }

    pub fn num_docs(&self) -> Result<u64, String> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM item_fts5", [], |row| row.get(0))
            .map_err(|e| format!("FTS5 count error: {e}"))?;
        Ok(count as u64)
    }

    pub fn optimize(&self) -> Result<(), String> {
        self.conn
            .execute("INSERT INTO item_fts5(item_fts5) VALUES('optimize')", [])
            .map_err(|e| format!("FTS5 optimize error: {e}"))?;
        Ok(())
    }
}

fn build_fts5_query(user_text: &str) -> String {
    let terms: Vec<String> = user_text
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}*", escape_fts5_term(t)))
        .collect();

    if terms.is_empty() {
        return String::new();
    }

    terms.join(" AND ")
}

fn escape_fts5_term(term: &str) -> String {
    term.replace('"', "")
        .replace('^', "")
        .replace('(', "")
        .replace(')', "")
        .replace(':', "")
}

fn extract_extension(path: &str) -> &str {
    let filename = path
        .rsplit(std::path::MAIN_SEPARATOR)
        .next()
        .unwrap_or(path);
    match filename.rfind('.') {
        Some(pos) => &filename[pos + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_temp_index() -> (Fts5Index, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("test_fts5.sqlite3");
        let index = Fts5Index::open(&db_path).expect("open FTS5 index");
        (index, dir)
    }

    #[test]
    fn test_fts5_search_basic() {
        let (index, _dir) = open_temp_index();

        let items = vec![
            SearchItem::new("1", "app", "Hello World", "/usr/bin/hello"),
            SearchItem::new("2", "app", "Firefox Browser", "/usr/bin/firefox"),
        ];
        index.index_items(&items).unwrap();

        let results = index.search("hello", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_fts5_search_prefix() {
        let (index, _dir) = open_temp_index();

        let items = vec![
            SearchItem::new("1", "app", "Hello World", "/usr/bin/hello"),
            SearchItem::new("2", "app", "Firefox Browser", "/usr/bin/firefox"),
        ];
        index.index_items(&items).unwrap();

        let results = index.search("hel", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_fts5_search_empty_query() {
        let (index, _dir) = open_temp_index();
        let results = index.search("", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_fts5_index_rebuild() {
        let (index, _dir) = open_temp_index();

        let items = vec![SearchItem::new("1", "app", "Alpha", "/alpha")];
        index.index_items(&items).unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        index.clear().unwrap();
        assert_eq!(index.num_docs().unwrap(), 0);

        let items2 = vec![SearchItem::new("2", "app", "Beta", "/beta")];
        index.index_items(&items2).unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        let results = index.search("beta", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "2");
    }

    #[test]
    fn test_fts5_delete_item() {
        let (index, _dir) = open_temp_index();

        index
            .index_items(&[
                SearchItem::new("1", "app", "Keep", "/keep"),
                SearchItem::new("2", "app", "Delete Me", "/delete"),
            ])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        index.delete_item("2").unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        let results = index.search("delete", 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_fts5_incremental_sync_basic() {
        let (index, _dir) = open_temp_index();

        // Start with items [A, B]
        index
            .index_items(&[
                SearchItem::new("a", "app", "Alpha", "/a"),
                SearchItem::new("b", "app", "Beta", "/b"),
            ])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        // Incremental sync to [A, C] — B deleted, C added, A untouched
        let new_items = vec![
            SearchItem::new("a", "app", "Alpha Updated", "/a"),
            SearchItem::new("c", "app", "Charlie", "/c"),
        ];
        index.incremental_sync_items(&new_items).unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        // B should not be found
        let results = index.search("beta", 10).unwrap();
        assert_eq!(results.len(), 0);

        // C should be found
        let results = index.search("charlie", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "c");

        // A should still be found
        let results = index.search("alpha", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_fts5_incremental_sync_empty_index() {
        let (index, _dir) = open_temp_index();

        let items = vec![
            SearchItem::new("1", "app", "Hello", "/hello"),
            SearchItem::new("2", "app", "World", "/world"),
        ];
        index.incremental_sync_items(&items).unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        let results = index.search("hello", 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_fts5_incremental_sync_empty_list() {
        let (index, _dir) = open_temp_index();

        index
            .index_items(&[SearchItem::new("1", "app", "Hello", "/hello")])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        let empty: Vec<SearchItem> = vec![];
        index.incremental_sync_items(&empty).unwrap();
        assert_eq!(index.num_docs().unwrap(), 0);
    }
}
