#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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
    /// Bumped on every successful commit. A query reloads the reader only when
    /// this moved, so an unchanged index costs no searcher rebuild per
    /// keystroke.
    commit_generation: AtomicU64,
    /// The generation the current searcher was reloaded from.
    loaded_generation: AtomicU64,
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
            commit_generation: AtomicU64::new(0),
            loaded_generation: AtomicU64::new(0),
        })
    }

    pub fn search(&self, query_text: &str, limit: usize) -> Result<Vec<SearchItem>, String> {
        if query_text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let started = Instant::now();
        let (cpu_start_ms, faults_start) = process_cost_counters();

        // Reload only when a commit actually landed, and never wait for one.
        // `reload()` re-opens every segment reader; doing it per keystroke was
        // pure work on an unchanged index, and doing it while the writer holds
        // the lock queued the keystroke behind an in-flight commit - which is
        // exactly the multi-second window a startup cycle creates. Serving the
        // previous searcher for one query costs a slightly stale result set;
        // the next query picks the commit up.
        let generation = self.commit_generation.load(Ordering::Acquire);
        let writer_busy = self.writer.try_lock().is_err();
        let reloaded = !writer_busy && generation != self.loaded_generation.load(Ordering::Relaxed);
        if reloaded {
            self.reader
                .reload()
                .map_err(|e| format!("tantivy reload error: {e}"))?;
            self.loaded_generation.store(generation, Ordering::Relaxed);
        }
        let reload_ms = started.elapsed();

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
        let collect_ms = started.elapsed();

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

        // One line only when a query is slow enough to be felt. Without it the
        // cause of a slow first keystroke can only be guessed at: the four
        // stages below separate page-cache faults (everything equally slow but
        // warm on the retry), a blocked commit (`writer_busy`), a cold searcher
        // rebuild (`reload_ms`) and plain CPU starvation (all stages slow).
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        if total_ms >= 20.0 {
            let (cpu_end_ms, faults_end) = process_cost_counters();
            crate::logging::info(&format!(
                "[nex] tantivy seed slow q={:?} total={:.1}ms reload={:.1}ms collect={:.1}ms docs={:.1}ms hits={} limit={} reloaded={} writer_busy={} gen={} cpu_ms={} faults={}",
                query_text,
                total_ms,
                reload_ms.as_secs_f64() * 1000.0,
                (collect_ms - reload_ms).as_secs_f64() * 1000.0,
                (started.elapsed() - collect_ms).as_secs_f64() * 1000.0,
                results.len(),
                limit,
                reloaded,
                writer_busy,
                generation,
                cpu_end_ms.saturating_sub(cpu_start_ms),
                faults_end.saturating_sub(faults_start),
            ));
        }

        Ok(results)
    }

    /// Record that the on-disk index moved, so the next query reloads.
    fn note_commit(&self) {
        self.commit_generation.fetch_add(1, Ordering::AcqRel);
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
        self.note_commit();
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
        self.note_commit();
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
        self.note_commit();
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
        self.note_commit();
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
        self.note_commit();
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
        self.note_commit();
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
        self.note_commit();
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
        self.note_commit();
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
///
/// This is the difference between a first keystroke and the second one. The
/// segmentation files are mapped, not read, so the pages behind them come off
/// disk on first touch: on the live 222k-document index a prefix query that
/// touches a new region costs 73-271 ms, and 3-4 ms once those pages are
/// resident. Warming by reading the files makes the fault soft, which is why
/// the pass has to run *before* the keystroke and in the process that owns the
/// reader (the indexer), not where the panel lives.
#[derive(Default)]
pub(crate) struct IndexWarmer {
    in_flight: std::sync::atomic::AtomicBool,
    /// A forced pass was asked for while another was running. Dropping it is
    /// how the first keystroke after an idle period goes cold: the show asks
    /// for the forced pass, a background pass happens to be in flight, and the
    /// request vanishes.
    force_pending: std::sync::atomic::AtomicBool,
    files_read: std::sync::atomic::AtomicU64,
    /// Bytes a forced pass may read; 0 means [`FORCED_PASS_MAX_BYTES`]. Only
    /// tests set it, so a test does not have to write 24 MB to prove the slice
    /// ends.
    pass_budget: std::sync::atomic::AtomicU64,
    state: Mutex<WarmState>,
}

#[derive(Default)]
struct WarmState {
    files: std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>)>,
    last_full: Option<std::time::Instant>,
    /// Where the next forced pass resumes in the sorted file list. A forced
    /// pass has to ignore stamps, so without a cursor it would re-read the
    /// whole index on every show.
    forced_cursor: usize,
}

const FULL_REWARM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// A show-triggered pass may ignore the unchanged-file bookkeeping, because a
/// trimmed page cache leaves lengths and mtimes identical. Throttled so a user
/// hammering the hotkey does not re-read the index per show.
const FORCED_REWARM_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// How much a single forced pass may read.
///
/// The pass runs while the panel is being shown, on the same machine and the
/// same cold page cache as the reveal. Reading a 4.6 GB index in one go there
/// was 120 MB of I/O against a first paint; a slice keeps the warm cheap and
/// the cursor covers the rest over the next few shows.
const FORCED_PASS_MAX_BYTES: u64 = 24 * 1024 * 1024;

impl IndexWarmer {
    pub(crate) fn warm_in_background(self: &std::sync::Arc<Self>, index_dir: PathBuf) {
        self.spawn_warm(index_dir, false);
    }

    /// Warm even when every file looks unchanged. Called from the show path:
    /// the OS can drop mapped pages while the panel sits idle, and handing out
    /// identical (length, mtime) stamps to a warmer that trusts them is exactly
    /// how the first keystroke after an idle period stays slow.
    pub(crate) fn warm_in_background_forced(self: &std::sync::Arc<Self>, index_dir: PathBuf) {
        self.spawn_warm(index_dir, true);
    }

    /// Files read by warm passes so far. Test and log evidence that a pass did
    /// real work, since a pass that skips everything is otherwise invisible.
    pub(crate) fn files_read(&self) -> u64 {
        self.files_read.load(std::sync::atomic::Ordering::Acquire)
    }

    /// True when no warm pass is running.
    #[cfg(test)]
    pub(crate) fn is_idle_for_test(&self) -> bool {
        !self.in_flight.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Cap the bytes a forced pass reads, so a test can exercise the slice.
    #[cfg(test)]
    pub(crate) fn set_pass_budget_for_test(&self, bytes: u64) {
        self.pass_budget
            .store(bytes, std::sync::atomic::Ordering::Release);
    }

    /// Pretend the last full pass is old, so a forced pass is not throttled.
    #[cfg(test)]
    pub(crate) fn rewind_throttle_for_test(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.last_full = None;
    }

    fn spawn_warm(self: &std::sync::Arc<Self>, index_dir: PathBuf, force: bool) {
        use std::sync::atomic::Ordering;
        if self.in_flight.swap(true, Ordering::AcqRel) {
            // Keep the request instead of dropping it: a pass is already
            // running, so it will pick this up on the way out.
            if force {
                self.force_pending.store(true, Ordering::Release);
            }
            return;
        }
        let this = std::sync::Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("nex-index-warm".into())
            .spawn(move || {
                let mut force = force;
                loop {
                    this.warm(&index_dir, force);
                    if !this.force_pending.swap(false, Ordering::AcqRel) {
                        break;
                    }
                    force = true;
                }
                this.in_flight.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.in_flight.store(false, Ordering::Release);
        }
    }

    fn warm(&self, index_dir: &Path, force: bool) {
        let Ok(entries) = std::fs::read_dir(index_dir) else {
            return;
        };
        // Sorted so the forced pass resumes at the same place twice: the
        // directory order is not stable enough to carry a cursor across passes.
        let mut candidates: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || path.extension().is_some_and(|e| e == "tmp" || e == "lock") {
                continue;
            }
            candidates.push((path, meta.len(), meta.modified().ok()));
        }
        candidates.sort_by(|a, b| a.0.cmp(&b.0));

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let due = state
            .last_full
            .is_none_or(|at| at.elapsed() >= FULL_REWARM_INTERVAL);
        let forced_due = force
            && state
                .last_full
                .is_none_or(|at| at.elapsed() >= FORCED_REWARM_MIN_INTERVAL);
        if due || forced_due {
            state.last_full = Some(std::time::Instant::now());
        }
        let count = candidates.len();
        // The stamp-driven pass only touches what changed, so it has no budget
        // and no order. The forced pass ignores stamps and starts where the
        // previous one stopped.
        let start = if forced_due && count > 0 {
            state.forced_cursor % count
        } else {
            0
        };
        let cap = match self.pass_budget.load(std::sync::atomic::Ordering::Acquire) {
            0 => FORCED_PASS_MAX_BYTES,
            bytes => bytes,
        };
        let mut budget = if forced_due { cap } else { u64::MAX };
        let mut seen = HashSet::new();
        let mut read = 0_u64;
        let mut bytes = 0_u64;
        for step in 0..count {
            let index = (start + step) % count;
            let (path, len, mtime) = &candidates[index];
            let stamp = (*len, *mtime);
            seen.insert(path.clone());
            if !forced_due && state.files.get(path) == Some(&stamp) {
                continue;
            }
            // One file may exceed the slice on its own (a postings file can be
            // tens of megabytes); reading it is the only way the pass makes
            // progress, so the first file is always allowed.
            if forced_due && read > 0 && *len > budget {
                continue;
            }
            // The data is dropped at once; the OS page cache keeps the pages
            // that tantivy's mmap reads later.
            if std::fs::read(path).is_ok() {
                state.files.insert(path.clone(), stamp);
                read += 1;
                bytes += *len;
                budget = budget.saturating_sub(*len);
                if forced_due {
                    state.forced_cursor = (index + 1) % count;
                }
            }
        }
        state.files.retain(|path, _| seen.contains(path));
        if read > 0 {
            self.files_read
                .fetch_add(read, std::sync::atomic::Ordering::AcqRel);
            crate::logging::info(&format!(
                "[nex] index warm: read {read} file(s) ({:.1} MB){} into the page cache",
                bytes as f64 / (1024.0 * 1024.0),
                if forced_due { " (forced slice)" } else { "" }
            ));
        }
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

/// Process CPU milliseconds and cumulative page-fault count, for telling a
/// slow query apart by cause: high `faults` with `cpu_ms` close to the wall
/// time is page-cache work, `cpu_ms` close to the wall time with few faults is
/// scoring, and `cpu_ms` far below it is disk I/O or CPU starvation.
#[cfg(target_os = "windows")]
fn process_cost_counters() -> (u64, u32) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let filetime_to_u64 = |ft: &FILETIME| ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let cpu_ms = unsafe {
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ) != 0
        {
            (filetime_to_u64(&kernel) + filetime_to_u64(&user)) / 10_000
        } else {
            0
        }
    };
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let faults = unsafe {
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) != 0 {
            counters.PageFaultCount
        } else {
            0
        }
    };
    (cpu_ms, faults)
}

#[cfg(not(target_os = "windows"))]
fn process_cost_counters() -> (u64, u32) {
    (0, 0)
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
    pub(crate) fn tantivy_guard() -> std::sync::MutexGuard<'static, ()> {
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

    pub(crate) fn open_temp_index() -> (TantivyIndex, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = TantivyIndex::open(dir.path()).expect("open tantivy index");
        (index, dir)
    }

    #[test]
    fn search_reloads_only_when_a_commit_landed() {
        let _serial = tantivy_guard();
        let (index, _dir) = open_temp_index();
        index
            .index_items(&[SearchItem::new("1", "app", "Alpha", "/a")])
            .unwrap();

        assert_eq!(index.search("alpha", 10).unwrap().len(), 1);
        let loaded = index.loaded_generation.load(Ordering::Relaxed);
        assert_eq!(
            loaded,
            index.commit_generation.load(Ordering::Relaxed),
            "the first query must reflect the committed state"
        );

        // A query with nothing committed must not rebuild the searcher.
        assert_eq!(index.search("alpha", 10).unwrap().len(), 1);
        assert_eq!(
            index.loaded_generation.load(Ordering::Relaxed),
            loaded,
            "an unchanged index must not reload per keystroke"
        );

        // Once a commit lands, the next query picks it up.
        index
            .index_items(&[
                SearchItem::new("1", "app", "Alpha", "/a"),
                SearchItem::new("2", "app", "Alphabet", "/b"),
            ])
            .unwrap();
        assert_eq!(index.search("alpha", 10).unwrap().len(), 2);
        assert!(
            index.loaded_generation.load(Ordering::Relaxed) > loaded,
            "a commit must be picked up by the next query"
        );
    }

    #[test]
    fn search_serves_the_previous_searcher_while_the_writer_holds_the_lock() {
        let _serial = tantivy_guard();
        let (index, _dir) = open_temp_index();
        index
            .index_items(&[SearchItem::new("1", "app", "Alpha", "/a")])
            .unwrap();
        assert_eq!(index.search("alpha", 10).unwrap().len(), 1);

        // Stand in for an in-flight commit: a keystroke must be answered from
        // the current searcher, not queued behind the writer.
        let writer = index.writer.lock().unwrap();
        let started = std::time::Instant::now();
        let results = index.search("alpha", 10).unwrap();
        let elapsed = started.elapsed();
        drop(writer);

        assert_eq!(results.len(), 1, "the previous searcher still answers");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "a query must not wait for the writer, took {elapsed:?}"
        );
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

    /// The first keystroke after a restart or an idle period is the slow one
    /// because the mapped segment files are cold: measured on a 222k-document
    /// index, the first query that touches a region costs 73-271 ms and 3-4 ms
    /// once the pages are resident. The warmer is what prevents that, so it has
    /// to keep working when a trimmed cache leaves the files byte-identical.
    #[test]
    fn warm_pass_reads_files_and_skips_unchanged_ones() {
        let _serial = tantivy_guard();
        let (index, dir) = open_temp_index();
        index
            .index_items(&[SearchItem::new("1", "app", "Alpha", "/a")])
            .unwrap();
        drop(index);

        let warmer = std::sync::Arc::new(IndexWarmer::default());
        warmer.warm_in_background(dir.path().to_path_buf());
        assert!(
            settles(|| warmer.files_read() > 0),
            "the first pass must read the segment files"
        );

        assert!(
            settles(|| warmer.is_idle_for_test()),
            "the first pass must finish before the next one starts"
        );

        // Nothing changed, no force: a second pass has nothing to do.
        let after_first = warmer.files_read();
        warmer.warm_in_background(dir.path().to_path_buf());
        assert!(
            settles(|| warmer.is_idle_for_test()),
            "the second pass must finish"
        );
        assert_eq!(
            warmer.files_read(),
            after_first,
            "an unforced pass must not re-read unchanged files"
        );

        // A show-triggered pass must read them again all the same.
        warmer.rewind_throttle_for_test();
        warmer.warm_in_background_forced(dir.path().to_path_buf());
        assert!(
            settles(|| warmer.files_read() > after_first),
            "a forced pass must re-read files whose length and mtime are unchanged"
        );
    }

    /// A forced pass ignores file stamps, so without a limit it re-reads the
    /// whole index on every show - 23 files and ~120 MB on the live index,
    /// while the panel is painting. It reads a slice instead and resumes where
    /// it stopped, so repeated shows still cover everything.
    #[test]
    fn forced_pass_reads_a_slice_and_resumes_where_it_stopped() {
        let _serial = tantivy_guard();
        let dir = tempfile::tempdir().unwrap();
        // Four files of 1 KiB, one budget of 1 KiB: one file per pass.
        for name in ["a.idx", "b.idx", "c.idx", "d.idx"] {
            std::fs::write(dir.path().join(name), vec![7_u8; 1024]).unwrap();
        }
        let warmer = std::sync::Arc::new(IndexWarmer::default());
        warmer.set_pass_budget_for_test(1024);

        warmer.warm_in_background_forced(dir.path().to_path_buf());
        assert!(
            settles(|| warmer.files_read() == 1),
            "a forced pass must stop at the slice, not read every file: got {}",
            warmer.files_read()
        );

        // Every later pass picks up where the previous one stopped, so four
        // shows warm all four files even though no pass reads more than one.
        for expected in 2..=4 {
            warmer.rewind_throttle_for_test();
            assert!(
                settles(|| warmer.is_idle_for_test()),
                "the previous pass must finish before the next one"
            );
            warmer.warm_in_background_forced(dir.path().to_path_buf());
            assert!(
                settles(|| warmer.files_read() == expected),
                "pass {expected} must read the next file: got {}",
                warmer.files_read()
            );
        }
    }
}
