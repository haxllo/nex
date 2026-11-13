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
    pub title_norm: Field,
    pub path: Field,
    pub subtitle: Field,
    pub kind: Field,
    pub extension: Field,
    pub use_count: Field,
    pub last_accessed_epoch_secs: Field,
}

/// Why `TantivyIndex::open` failed. `LockBusy` means another process (a
/// running Nex instance) owns the writer: the index is healthy and must not
/// be reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TantivyOpenError {
    LockBusy(String),
    Other(String),
}

impl std::fmt::Display for TantivyOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LockBusy(msg) => write!(f, "tantivy index locked by another writer: {msg}"),
            Self::Other(msg) => f.write_str(msg),
        }
    }
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
    pub fn open(index_path: &Path) -> Result<Self, TantivyOpenError> {
        std::fs::create_dir_all(index_path).map_err(|e| {
            TantivyOpenError::Other(format!("failed to create tantivy directory: {e}"))
        })?;

        let mut schema_builder = Schema::builder();

        let id = schema_builder.add_text_field("id", STRING | STORED);
        let title = schema_builder.add_text_field("title", TEXT | STORED);
        let title_norm = schema_builder.add_text_field("title_norm", TEXT);
        let path = schema_builder.add_text_field("path", STRING | STORED);
        let subtitle = schema_builder.add_text_field("subtitle", TEXT | STORED);
        let kind = schema_builder.add_text_field("kind", STRING | STORED);
        let extension = schema_builder.add_text_field("extension", STRING);
        let use_count = schema_builder.add_i64_field("use_count", STORED);
        let last_accessed_epoch_secs =
            schema_builder.add_i64_field("last_accessed_epoch_secs", STORED);

        let schema = schema_builder.build();

        let other = TantivyOpenError::Other;
        let directory = MmapDirectory::open(index_path)
            .map_err(|e| other(format!("failed to open tantivy directory: {e}")))?;
        let index = Index::open_or_create(directory, schema.clone())
            .map_err(|e| other(format!("failed to open/create tantivy index: {e}")))?;

        let reader = index
            .reader()
            .map_err(|e| other(format!("failed to create tantivy reader: {e}")))?;

        let writer = index.writer(16_000_000).map_err(|e| match e {
            tantivy::TantivyError::LockFailure(..) => TantivyOpenError::LockBusy(e.to_string()),
            other_err => other(format!("failed to create tantivy writer: {other_err}")),
        })?;

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
                title_norm,
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

    pub fn search(&self, query_text: &str, limit: usize) -> Result<Vec<SearchItem>, String> {
        if query_text.trim().is_empty() {
            return Ok(Vec::new());
        }

        self.reader
            .reload()
            .map_err(|e| format!("tantivy reload error: {e}"))?;

        let searcher = self.reader.searcher();
        let query = build_prefix_query(
            &searcher.index(),
            query_text,
            &[
                self.fields.title_norm,
                self.fields.path,
                self.fields.subtitle,
            ],
            self.fields.title_norm,
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

        // Commit buffered mutations before delete_all_documents: it removes
        // committed segments, so pending documents could otherwise survive
        // a full replacement.
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        *self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))? = 0;

        writer
            .delete_all_documents()
            .map_err(|e| format!("tantivy delete all error: {e}"))?;

        for item in items {
            writer
                .add_document(doc!(
                    self.fields.id => item.id.as_str(),
                    self.fields.title => item.title.as_str(),
                    self.fields.title_norm => crate::model::normalize_for_search(item.title.as_str()),
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
        *self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))? = 0;

        // Reclaim disk + mmap RSS from the prior segments that
        // `delete_all_documents` logically removed but the file
        // handles stay resident until GC runs. Without this the
        // index directory grows unboundedly on every reindex.
        collect_garbage(&writer);

        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        *self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))? = 0;

        writer
            .delete_all_documents()
            .map_err(|e| format!("tantivy delete all error: {e}"))?;
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        *self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))? = 0;
        collect_garbage(&writer);

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
                self.fields.title_norm => crate::model::normalize_for_search(item.title.as_str()),
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

    /// Upsert a whole batch with one writer lock. Used by the discovery
    /// rebuild so per-item lock churn does not dominate the write phase.
    pub fn upsert_items(&self, items: &[SearchItem]) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }
        let writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        for item in items {
            writer.delete_term(tantivy::Term::from_field_text(self.fields.id, &item.id));
            writer
                .add_document(doc!(
                    self.fields.id => item.id.as_str(),
                    self.fields.title => item.title.as_str(),
                    self.fields.title_norm => crate::model::normalize_for_search(item.title.as_str()),
                    self.fields.path => item.path.as_str(),
                    self.fields.subtitle => item.subtitle.as_str(),
                    self.fields.kind => item.kind.as_str(),
                    self.fields.extension => extract_extension(&item.path),
                    self.fields.use_count => item.use_count as i64,
                    self.fields.last_accessed_epoch_secs => item.last_accessed_epoch_secs,
                ))
                .map_err(|e| format!("tantivy add document error: {e}"))?;
        }
        drop(writer);

        self.maybe_commit_and_gc()
    }

    pub fn incremental_sync_items(&self, items: &[SearchItem]) -> Result<(), String> {
        // Publish buffered upserts/deletes before using the reader to compute
        // the existing-ID set; otherwise an uncommitted document can escape
        // the diff and be committed again by this sync after it was removed.
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        *self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))? = 0;
        drop(writer);

        // Phase 1: collect existing document IDs (reader only, no writer lock).
        let existing_ids = self.live_ids()?;

        // Phase 2: Compute diff (no lock needed)
        let incoming_ids: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();

        // Phase 3: Lock writer and apply changes — delete removed,
        // add/update incoming, commit, and GC. This is fast because
        // the expensive scan already happened above.
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;

        // Delete removed items (exist in index but not in incoming set)
        for existing_id in existing_ids.difference(&incoming_ids) {
            writer.delete_term(tantivy::Term::from_field_text(self.fields.id, existing_id));
        }

        // Add/update incoming items
        for item in items {
            writer.delete_term(tantivy::Term::from_field_text(self.fields.id, &item.id));
            writer
                .add_document(doc!(
                    self.fields.id => item.id.as_str(),
                    self.fields.title => item.title.as_str(),
                    self.fields.title_norm => crate::model::normalize_for_search(item.title.as_str()),
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
        collect_garbage(&writer);

        Ok(())
    }

    /// IDs of all committed live documents. Iterates every document, so
    /// keep it off interactive paths. A failed reload leaves a stale
    /// searcher whose view is missing live docs, which would silently skip
    /// their deletions — fail instead.
    fn live_ids(&self) -> Result<HashSet<String>, String> {
        self.reader
            .reload()
            .map_err(|e| format!("tantivy reload error: {e}"))?;
        let reader = self.reader.searcher();
        let mut existing_ids: HashSet<String> = HashSet::new();
        for (segment_ord, segment_reader) in reader.segment_readers().iter().enumerate() {
            let max_doc = segment_reader.max_doc();
            let alive_bitset = segment_reader.alive_bitset();
            for doc_id in 0u32..max_doc {
                // After deletions, live doc IDs can exist at positions
                // >= num_docs(). Iterate max_doc() and skip deleted docs
                // via the alive bitset to avoid missing live entries.
                if let Some(ref bitset) = alive_bitset {
                    if !bitset.is_alive(doc_id) {
                        continue;
                    }
                }
                let doc_address = tantivy::DocAddress::new(segment_ord as u32, doc_id);
                let doc: TantivyDocument = reader
                    .doc::<TantivyDocument>(doc_address)
                    .map_err(|e| format!("tantivy doc retrieval error: {e}"))?;
                if let Some(id_val) = doc.get_first(self.fields.id).and_then(|v| v.as_str()) {
                    existing_ids.insert(id_val.to_string());
                }
            }
        }
        Ok(existing_ids)
    }

    /// Bring the index in line with `items` by adding only missing documents
    /// and deleting extra ones; unlike `incremental_sync_items`, existing
    /// documents are not rewritten. Returns (added, removed).
    pub fn reconcile_items(&self, items: &[SearchItem]) -> Result<(usize, usize), String> {
        self.commit_pending()?;
        let existing_ids = self.live_ids()?;
        let incoming_ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();

        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;
        let mut removed = 0usize;
        for existing_id in &existing_ids {
            if !incoming_ids.contains(existing_id.as_str()) {
                writer.delete_term(tantivy::Term::from_field_text(self.fields.id, existing_id));
                removed += 1;
            }
        }
        let mut added = 0usize;
        for item in items {
            if existing_ids.contains(&item.id) {
                continue;
            }
            writer
                .add_document(doc!(
                    self.fields.id => item.id.as_str(),
                    self.fields.title => item.title.as_str(),
                    self.fields.title_norm => crate::model::normalize_for_search(item.title.as_str()),
                    self.fields.path => item.path.as_str(),
                    self.fields.subtitle => item.subtitle.as_str(),
                    self.fields.kind => item.kind.as_str(),
                    self.fields.extension => extract_extension(&item.path),
                    self.fields.use_count => item.use_count as i64,
                    self.fields.last_accessed_epoch_secs => item.last_accessed_epoch_secs,
                ))
                .map_err(|e| format!("tantivy add document error: {e}"))?;
            added += 1;
        }
        if added > 0 || removed > 0 {
            writer
                .commit()
                .map_err(|e| format!("tantivy commit error: {e}"))?;
            collect_garbage(&writer);
        }
        Ok((added, removed))
    }

    /// Commit buffered upserts/deletes without GC. Returns false when
    /// nothing was pending.
    pub fn commit_pending(&self) -> Result<bool, String> {
        let mut count = self
            .write_count
            .lock()
            .map_err(|e| format!("tantivy write_count lock error: {e}"))?;
        if *count == 0 {
            return Ok(false);
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        *count = 0;
        Ok(true)
    }

    pub fn flush(&self) -> Result<(), String> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| format!("tantivy writer lock error: {e}"))?;
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        collect_garbage(&writer);
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

    /// Commit pending writes and wait for background merges so no segment
    /// is left half-written when the index is dropped or the process exits.
    pub fn close(self) -> Result<(), String> {
        let mut writer = self
            .writer
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer
            .commit()
            .map_err(|e| format!("tantivy commit error: {e}"))?;
        collect_garbage(&writer);
        writer
            .wait_merging_threads()
            .map_err(|e| format!("tantivy merge wait error: {e}"))
    }

    /// Document count without creating a writer, so diagnostics can probe
    /// the live index while Nex holds the writer lock.
    pub fn read_only_doc_count(index_path: &Path) -> Result<u64, String> {
        let directory = MmapDirectory::open(index_path)
            .map_err(|e| format!("failed to open tantivy directory: {e}"))?;
        let index =
            Index::open(directory).map_err(|e| format!("failed to open tantivy index: {e}"))?;
        let reader = index
            .reader()
            .map_err(|e| format!("failed to create tantivy reader: {e}"))?;
        Ok(reader.searcher().num_docs())
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

/// Keeps index files in the OS page cache without touching the show path:
/// warming runs on a background thread and only re-reads files that are
/// new or changed (segments are immutable), plus a full pass every few
/// minutes in case the cache was trimmed while idle.
#[derive(Default)]
pub(crate) struct IndexWarmer {
    in_flight: std::sync::atomic::AtomicBool,
    state: Mutex<WarmState>,
}

#[derive(Default)]
struct WarmState {
    files: std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>)>,
    last_full: Option<std::time::Instant>,
}

const FULL_REWARM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

impl IndexWarmer {
    pub(crate) fn warm_in_background(self: &std::sync::Arc<Self>, index_dir: PathBuf) {
        use std::sync::atomic::Ordering;
        if self.in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = std::sync::Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("nex-index-warm".into())
            .spawn(move || {
                this.warm(&index_dir);
                this.in_flight.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.in_flight.store(false, Ordering::Release);
        }
    }

    fn warm(&self, index_dir: &Path) {
        let Ok(entries) = std::fs::read_dir(index_dir) else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .last_full
            .is_none_or(|at| at.elapsed() >= FULL_REWARM_INTERVAL)
        {
            state.files.clear();
            state.last_full = Some(std::time::Instant::now());
        }
        let mut seen = HashSet::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || path.extension().is_some_and(|e| e == "tmp" || e == "lock") {
                continue;
            }
            let stamp = (meta.len(), meta.modified().ok());
            seen.insert(path.clone());
            if state.files.get(&path) == Some(&stamp) {
                continue;
            }
            // The data is dropped at once; the OS page cache keeps the pages
            // that tantivy's mmap reads later.
            if std::fs::read(&path).is_ok() {
                state.files.insert(path, stamp);
            }
        }
        state.files.retain(|path, _| seen.contains(path));
    }
}

fn collect_garbage(writer: &IndexWriter) {
    match writer.garbage_collect_files().wait() {
        Ok(result) => {
            if !result.failed_to_delete_files.is_empty() {
                // Normal on Windows while a reader still maps the file;
                // retried on the next GC.
                crate::logging::info(&format!(
                    "[nex] Tantivy GC deferred {} file(s) still in use",
                    result.failed_to_delete_files.len()
                ));
            }
        }
        Err(e) => crate::logging::warn(&format!("[nex] Tantivy GC failed: {e}")),
    }
}

fn build_prefix_query(
    _index: &Index,
    query_text: &str,
    fields: &[Field],
    norm_field: Field,
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
        let word = if fields.iter().any(|&f| f == norm_field) {
            crate::model::normalize_for_search(terms[0])
        } else {
            terms[0].to_string()
        };
        for &field in fields {
            let term = if field == norm_field {
                tantivy::Term::from_field_text(field, &word)
            } else {
                tantivy::Term::from_field_text(field, terms[0])
            };
            subqueries.push((
                Occur::Should,
                Box::new(FuzzyTermQuery::new_prefix(term, 0, false)),
            ));
        }
        return Ok(Box::new(BooleanQuery::new(subqueries)));
    }

    let mut all_subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    for word in &terms {
        let normalized_word = if fields.iter().any(|&f| f == norm_field) {
            crate::model::normalize_for_search(word)
        } else {
            word.to_string()
        };
        let mut word_subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for &field in fields {
            let term = if field == norm_field {
                tantivy::Term::from_field_text(field, &normalized_word)
            } else {
                tantivy::Term::from_field_text(field, word)
            };
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

    /// Serialises the tests in this module.
    ///
    /// Each test opens its own index, so the files are not shared - but every
    /// `TantivyIndex` still commits through a Tantivy writer, and the whole
    /// crate's test threads compete for the same small number of CPUs. Under
    /// full-suite load a commit followed by `collect_garbage` can take long
    /// enough that a reader never observes the new generation inside any
    /// reasonable deadline, which is why a different tantivy test failed on
    /// each run while every one of them passed in isolation.
    ///
    /// A module-local mutex keeps this module's heavy index work from running
    /// alongside itself without serialising the rest of the suite.
    static TANTIVY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Hold for the whole test: `let _guard = tantivy_guard();`
    fn tantivy_guard() -> std::sync::MutexGuard<'static, ()> {
        TANTIVY_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wait for `predicate` to hold, allowing for the fact that a commit is not
    /// instantly observable through a reader.
    ///
    /// `num_docs()` and `search()` each call `reader.reload()` separately, and
    /// `index_items` finishes with `collect_garbage`, which on Windows defers
    /// deletion while a reader still maps a file (see the comment there). A
    /// single read straight after a write can therefore observe the previous
    /// generation - which is what made this suite fail on a different test each
    /// run. Polling to a deadline tests the property that actually matters
    /// ("the stale document eventually disappears") without asserting a timing
    /// guarantee the code does not make.
    fn settles(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if predicate() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn open_temp_index() -> (TantivyIndex, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = TantivyIndex::open(dir.path()).expect("open tantivy index");
        (index, dir)
    }

    #[test]
    fn second_open_reports_lock_busy_not_corruption() {
        let _serial = tantivy_guard();
        let (index, dir) = open_temp_index();
        index
            .index_items(&[SearchItem::new("1", "file", "alpha.txt", "C:\\a\\alpha.txt")])
            .unwrap();
        match TantivyIndex::open(dir.path()) {
            Err(TantivyOpenError::LockBusy(_)) => {}
            Err(other) => panic!("expected LockBusy, got {other}"),
            Ok(_) => panic!("second writer must not open"),
        }
        // Read-only probing works while the writer is held.
        assert_eq!(TantivyIndex::read_only_doc_count(dir.path()).unwrap(), 1);
    }

    #[test]
    fn close_commits_pending_writes_and_releases_lock() {
        let _serial = tantivy_guard();
        let (index, dir) = open_temp_index();
        index
            .upsert_item(&SearchItem::new("1", "file", "beta.txt", "C:\\b\\beta.txt"))
            .unwrap();
        index.close().unwrap();
        let reopened = TantivyIndex::open(dir.path()).expect("lock released after close");
        assert_eq!(reopened.num_docs().unwrap(), 1);
    }

    #[test]
    fn test_tantivy_search_basic() {
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
        let (index, _dir) = open_temp_index();
        let results = index.search("", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_tantivy_index_rebuild() {
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
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
        let _serial = tantivy_guard();
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
    fn test_tantivy_incremental_sync_removes_uncommitted_upsert() {
        let _serial = tantivy_guard();
        let (index, _dir) = open_temp_index();
        let stale = SearchItem::new("stale", "file", "StaleDoc.txt", "/tmp/StaleDoc.txt");

        index.upsert_item(&stale).unwrap();
        index.incremental_sync_items(&[]).unwrap();

        assert!(
            settles(|| index.num_docs().unwrap_or(1) == 0),
            "the stale document was still counted"
        );
        assert!(
            settles(|| index.search("staledoc", 10).unwrap_or_default().is_empty()),
            "the stale document was still searchable"
        );
    }

    #[test]
    fn test_tantivy_full_sync_removes_uncommitted_upsert() {
        let _serial = tantivy_guard();
        let (index, _dir) = open_temp_index();
        let stale = SearchItem::new("stale", "file", "StaleDoc.txt", "/tmp/StaleDoc.txt");

        index.upsert_item(&stale).unwrap();
        index.index_items(&[]).unwrap();

        assert!(
            settles(|| index.num_docs().unwrap_or(1) == 0),
            "the stale document was still counted"
        );
        assert!(
            settles(|| index.search("staledoc", 10).unwrap_or_default().is_empty()),
            "the stale document was still searchable"
        );
    }

    #[test]
    fn test_tantivy_clear_removes_uncommitted_upsert() {
        let _serial = tantivy_guard();
        let (index, _dir) = open_temp_index();
        let stale = SearchItem::new("stale", "file", "StaleDoc.txt", "/tmp/StaleDoc.txt");

        index.upsert_item(&stale).unwrap();
        index.clear().unwrap();

        assert!(
            settles(|| index.num_docs().unwrap_or(1) == 0),
            "the stale document was still counted"
        );
        assert!(
            settles(|| index.search("staledoc", 10).unwrap_or_default().is_empty()),
            "the stale document was still searchable"
        );
    }

    #[test]
    fn test_tantivy_deferred_commit_and_flush() {
        let _serial = tantivy_guard();
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
