#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::merge_policy::LogMergePolicy;
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
    write_count: Mutex<u32>,
    commit_threshold: u32,
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
            .writer(16_000_000)
            .map_err(|e| format!("failed to create tantivy writer: {e}"))?;

        let mut merge_policy = LogMergePolicy::default();
        merge_policy.set_min_num_segments(3);
        merge_policy.set_level_log_size(5.0);
        merge_policy.set_del_docs_ratio_before_merge(0.5);
        writer.set_merge_policy(Box::new(merge_policy));

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
            write_count: Mutex::new(0),
            commit_threshold: 500,
        })
    }

    /// Pre-warm ALL Tantivy segment files into the OS page cache so the
    /// user's first keystroke for ANY letter pays zero page-fault latency
    /// for term dictionaries, posting lists, document store, or index
    /// metadata.  Tantivy's `MmapDirectory` maps these files — the OS
    /// Cache Manager shares pages between `ReadFile` and `CreateFileMapping`
    /// on Windows, so a sequential pre-read populates the same pages the
    /// mmap accesses later.
    ///
    /// Typical index size: 10–30 MB for 50k documents.  Sequential SSD
    /// read completes in 50–100 ms and runs synchronously during the show
    /// animation (~160 ms), so the user perceives no delay.
    pub fn warmup(&self) {
        let _ = self.reader.reload();
        let dir = self.index_path.as_path();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e != "tmp") {
                    // Allocate falls out of scope immediately — the OS
                    // page cache retains the data for subsequent mmap
                    // accesses.
                    let _ = std::fs::read(&path);
                }
            }
        }
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

        // Reclaim disk + mmap RSS from the prior segments that
        // `delete_all_documents` logically removed but the file
        // handles stay resident until GC runs. Without this the
        // index directory grows unboundedly on every reindex.
        let _ = writer.garbage_collect_files().wait();

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
        let _ = writer.garbage_collect_files().wait();

        Ok(())
    }

    pub fn delete_item(&self, item_id: &str) -> Result<(), String> {
        let writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        writer.delete_term(tantivy::Term::from_field_text(self.fields.id, item_id));
        drop(writer);

        self.maybe_commit_and_gc()
    }

    pub fn upsert_item(&self, item: &SearchItem) -> Result<(), String> {
        let writer = self
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
        drop(writer);

        self.maybe_commit_and_gc()
    }

    pub fn incremental_sync_items(&self, items: &[SearchItem]) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        // Collect existing document IDs from the current index
        self.reader.reload().ok();
        let reader = self.reader.searcher();
        let mut existing_ids: HashSet<String> = HashSet::new();
        for (segment_ord, segment_reader) in reader.segment_readers().iter().enumerate() {
            for doc_id in 0u32..segment_reader.num_docs() as u32 {
                let doc_address = tantivy::DocAddress::new(segment_ord as u32, doc_id);
                let doc: TantivyDocument = reader
                    .doc::<TantivyDocument>(doc_address)
                    .map_err(|e| format!("tantivy doc retrieval error: {e}"))?;
                if let Some(id_val) = doc.get_first(self.fields.id).and_then(|v| v.as_str()) {
                    existing_ids.insert(id_val.to_string());
                }
            }
        }

        // Collect incoming item IDs
        let incoming_ids: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();

        // Delete removed items (exist in index but not in incoming set)
        for existing_id in existing_ids.difference(&incoming_ids) {
            writer
                .delete_term(tantivy::Term::from_field_text(self.fields.id, existing_id));
        }

        // Add/update incoming items
        for item in items {
            writer
                .delete_term(tantivy::Term::from_field_text(self.fields.id, &item.id));
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
        let _ = writer.garbage_collect_files().wait();

        Ok(())
    }

    pub fn flush(&self) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        let _ = writer.garbage_collect_files().wait();
        let mut count = self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))?;
        *count = 0;
        Ok(())
    }

    fn maybe_commit_and_gc(&self) -> Result<(), String> {
        let mut count = self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))?;
        *count += 1;
        if *count >= self.commit_threshold {
            drop(count);
            self.flush()
        } else {
            Ok(())
        }
    }

    pub fn num_docs(&self) -> Result<u64, String> {
        self.reader
            .reload()
            .map_err(|e| format!("tantivy reload error: {e}"))?;
        let searcher = self.reader.searcher();
        Ok(searcher.num_docs() as u64)
    }

    pub fn mem_usage_bytes(&self) -> usize {
        let arena = 16_000_000;
        let reader = self.reader.searcher();
        let segment_mem = reader.segment_readers().len() * 2_000_000;
        arena + segment_mem
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
        index.flush().unwrap(); // flush deferred commit
        assert_eq!(index.num_docs().unwrap(), 1);

        let results = index.search("delete", 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_tantivy_incremental_sync_basic() {
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

        // A should still be found, with updated title
        let results = index.search("alpha", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_tantivy_incremental_sync_empty_index() {
        let (index, _dir) = open_temp_index();

        // Sync items into empty index
        let items = vec![
            SearchItem::new("1", "app", "Hello", "/hello"),
            SearchItem::new("2", "app", "World", "/world"),
        ];
        index.incremental_sync_items(&items).unwrap();
        assert_eq!(index.num_docs().unwrap(), 2);

        let results = index.search("hello", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_tantivy_incremental_sync_empty_list() {
        let (index, _dir) = open_temp_index();

        index
            .index_items(&[SearchItem::new("1", "app", "Hello", "/hello")])
            .unwrap();
        assert_eq!(index.num_docs().unwrap(), 1);

        // Sync with empty list — removes all
        let empty: Vec<SearchItem> = vec![];
        index.incremental_sync_items(&empty).unwrap();
        assert_eq!(index.num_docs().unwrap(), 0);
    }

    #[test]
    fn test_tantivy_deferred_commit_and_flush() {
        let (index, _dir) = open_temp_index();

        // Add item with deferred commit (upsert_item doesn't commit immediately)
        index
            .upsert_item(&SearchItem::new("d1", "app", "Deferred", "/d1"))
            .unwrap();

        // Search shouldn't see it yet (not committed)
        let results = index.search("deferred", 10).unwrap();
        assert_eq!(results.len(), 0);

        // Flush to persist
        index.flush().unwrap();

        // Now search should see it
        let results = index.search("deferred", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "d1");
        assert_eq!(index.num_docs().unwrap(), 1);

        // Delete with deferred commit
        index.delete_item("d1").unwrap();
        // Search still sees it (not committed yet)
        assert_eq!(index.num_docs().unwrap(), 1);

        // Flush to persist the delete
        index.flush().unwrap();
        assert_eq!(index.num_docs().unwrap(), 0);
    }
}
