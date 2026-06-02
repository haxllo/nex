#![allow(dead_code)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, FuzzyTermQuery, Occur};
use tantivy::schema::*;
use tantivy::{doc, query::Query, Index, IndexReader, IndexWriter, TantivyDocument};

use crate::model::SearchItem;

pub(crate) struct TantivyFields {
    pub id: Field,
    pub title: Field,
    pub path: Field,
    pub subtitle: Field,
    pub kind: Field,
    pub extension: Field,
    pub use_count: Field,
    pub last_accessed_epoch_secs: Field,
}

pub(crate) struct TantivyIndex {
    index_path: PathBuf,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    fields: TantivyFields,
    schema: Schema,
}

impl TantivyIndex {
    pub fn open(index_path: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(index_path)
            .map_err(|e| format!("failed to create tantivy directory: {e}"))?;

        let mut schema_builder = Schema::builder();

        let id = schema_builder.add_text_field("id", STRING | STORED);
        let title = schema_builder.add_text_field("title", TEXT | STORED);
        let path = schema_builder.add_text_field("path", STRING | STORED);
        let subtitle = schema_builder.add_text_field("subtitle", TEXT);
        let kind = schema_builder.add_text_field("kind", STRING);
        let extension = schema_builder.add_text_field("extension", STRING);
        let use_count = schema_builder.add_i64_field("use_count", STORED);
        let last_accessed_epoch_secs =
            schema_builder.add_i64_field("last_accessed_epoch_secs", STORED);

        let schema = schema_builder.build();

        let directory = MmapDirectory::open(index_path)
            .map_err(|e| format!("failed to open tantivy directory: {e}"))?;
        let index = Index::open_or_create(directory, schema.clone())
            .map_err(|e| format!("failed to open/create tantivy index: {e}"))?;

        let reader = index
            .reader()
            .map_err(|e| format!("failed to create tantivy reader: {e}"))?;

        let writer = index
            .writer(50_000_000)
            .map_err(|e| format!("failed to create tantivy writer: {e}"))?;

        Ok(Self {
            index_path: index_path.to_path_buf(),
            reader,
            writer: Mutex::new(writer),
            fields: TantivyFields {
                id,
                title,
                path,
                subtitle,
                kind,
                extension,
                use_count,
                last_accessed_epoch_secs,
            },
            schema,
        })
    }

    pub fn search(&self, query_text: &str, limit: usize) -> Result<Vec<SearchItem>, String> {
        if query_text.trim().is_empty() {
            return Ok(Vec::new());
        }

        self.reader.reload().ok();

        let searcher = self.reader.searcher();
        let query = build_prefix_query(
            &searcher.index(),
            query_text,
            &[self.fields.title, self.fields.path, self.fields.subtitle],
        )
        .map_err(|e| format!("tantivy query build error: {e}"))?;

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| format!("tantivy search error: {e}"))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| format!("tantivy doc retrieval error: {e}"))?;

            let id = doc
                .get_first(self.fields.id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let title = doc
                .get_first(self.fields.title)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let path_value = doc
                .get_first(self.fields.path)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let subtitle = doc
                .get_first(self.fields.subtitle)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let kind = doc
                .get_first(self.fields.kind)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let use_count = doc
                .get_first(self.fields.use_count)
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as u32;
            let last_accessed = doc
                .get_first(self.fields.last_accessed_epoch_secs)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            // Map BM25 (~0..5) to 0..5000 range for pre_score
            let pre_score = ((score / 5.0) * 5000.0).round() as i64;

            results.push(
                SearchItem::from_owned_with_subtitle(
                    id,
                    kind,
                    title,
                    path_value,
                    subtitle,
                    use_count,
                    last_accessed,
                )
                .with_pre_score(pre_score),
            );
        }

        Ok(results)
    }

    pub fn index_items(&self, items: &[SearchItem]) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        writer
            .delete_all_documents()
            .map_err(|e| format!("tantivy delete all error: {e}"))?;

        for item in items {
            writer
                .add_document(doc!(
                    self.fields.id => item.id.as_str(),
                    self.fields.title => item.title.as_str(),
                    self.fields.path => item.path.as_str(),
                    self.fields.subtitle => item.subtitle.as_str(),
                    self.fields.kind => item.kind.as_str(),
                    self.fields.extension => extract_extension(&item.path),
                    self.fields.use_count => item.use_count as i64,
                    self.fields.last_accessed_epoch_secs => item.last_accessed_epoch_secs,
                ))
                .map_err(|e| format!("tantivy add document error: {e}"))?;
        }

        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;

        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        writer
            .delete_all_documents()
            .map_err(|e| format!("tantivy delete all error: {e}"))?;
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;

        Ok(())
    }

    pub fn delete_item(&self, item_id: &str) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        writer.delete_term(tantivy::Term::from_field_text(self.fields.id, item_id));
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;

        Ok(())
    }

    pub fn upsert_item(&self, item: &SearchItem) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        writer.delete_term(tantivy::Term::from_field_text(self.fields.id, &item.id));

        writer
            .add_document(doc!(
                self.fields.id => item.id.as_str(),
                self.fields.title => item.title.as_str(),
                self.fields.path => item.path.as_str(),
                self.fields.subtitle => item.subtitle.as_str(),
                self.fields.kind => item.kind.as_str(),
                self.fields.extension => extract_extension(&item.path),
                self.fields.use_count => item.use_count as i64,
                self.fields.last_accessed_epoch_secs => item.last_accessed_epoch_secs,
            ))
            .map_err(|e| format!("tantivy add document error: {e}"))?;

        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;

        Ok(())
    }

    pub fn num_docs(&self) -> Result<u64, String> {
        self.reader
            .reload()
            .map_err(|e| format!("tantivy reload error: {e}"))?;
        let searcher = self.reader.searcher();
        Ok(searcher.num_docs() as u64)
    }
}

fn build_prefix_query(
    _index: &Index,
    query_text: &str,
    fields: &[Field],
) -> Result<Box<dyn Query>, String> {
    let terms: Vec<&str> = query_text
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return Ok(Box::new(tantivy::query::AllQuery));
    }

    if terms.len() == 1 {
        let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for &field in fields {
            let term = tantivy::Term::from_field_text(field, terms[0]);
            subqueries.push((
                Occur::Should,
                Box::new(FuzzyTermQuery::new_prefix(term, 0, false)),
            ));
        }
        return Ok(Box::new(BooleanQuery::new(subqueries)));
    }

    let mut all_subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    for word in &terms {
        let mut word_subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for &field in fields {
            let term = tantivy::Term::from_field_text(field, word);
            word_subqueries.push((
                Occur::Should,
                Box::new(FuzzyTermQuery::new_prefix(term, 0, false)),
            ));
        }
        all_subqueries.push((Occur::Must, Box::new(BooleanQuery::new(word_subqueries))));
    }
    Ok(Box::new(BooleanQuery::new(all_subqueries)))
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

    fn open_temp_index() -> (TantivyIndex, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = TantivyIndex::open(dir.path()).expect("open tantivy index");
        (index, dir)
    }

    #[test]
    fn test_tantivy_search_basic() {
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
    fn test_tantivy_search_prefix() {
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
    fn test_tantivy_search_empty_query() {
        let (index, _dir) = open_temp_index();
        let results = index.search("", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_tantivy_index_rebuild() {
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
    fn test_tantivy_incremental_update() {
        let (index, _dir) = open_temp_index();

        index
            .index_items(&[SearchItem::new("1", "app", "Hello", "/hello")])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        index
            .index_items(&[
                SearchItem::new("1", "app", "Hello Updated", "/hello"),
                SearchItem::new("2", "app", "World", "/world"),
            ])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        let results = index.search("world", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "2");
    }

    #[test]
    fn test_tantivy_delete_item() {
        let (index, _dir) = open_temp_index();

        index
            .index_items(&[
                SearchItem::new("1", "app", "Keep", "/keep"),
                SearchItem::new("2", "app", "Delete", "/delete"),
            ])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        index.delete_item("2").unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        let results = index.search("delete", 10).unwrap();
        assert_eq!(results.len(), 0);
    }
}
