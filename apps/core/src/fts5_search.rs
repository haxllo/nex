#![allow(dead_code)]

use std::path::Path;

use rusqlite::{params, Connection};

use crate::model::SearchItem;

pub(crate) struct Fts5Index {
    conn: Connection,
}

impl Fts5Index {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("failed to open FTS5 database: {e}"))?;

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
                "SELECT id, title, path, subtitle, kind
                 FROM item_fts5
                 WHERE item_fts5 MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| format!("FTS5 prepare error: {e}"))?;

        let results = stmt
            .query_map(params![fts5_query, limit as i64], |row| {
                Ok(SearchItem::from_owned_with_subtitle(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    0,
                    0,
                ))
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
            .execute(
                "DELETE FROM item_fts5 WHERE id = ?1",
                params![item_id],
            )
            .map_err(|e| format!("FTS5 delete error: {e}"))?;
        Ok(())
    }

    pub fn num_docs(&self) -> Result<u64, String> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM item_fts5", [], |row| row.get(0))
            .map_err(|e| format!("FTS5 count error: {e}"))?;
        Ok(count as u64)
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
    let filename = path.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or(path);
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
}
