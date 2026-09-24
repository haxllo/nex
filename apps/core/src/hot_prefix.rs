//! Predictive hot-prefix pre-fetch.
//!
//! Every search starts with a first keystroke, and that keystroke is the
//! most expensive one in the whole interaction: a single character matches
//! a very large share of the corpus, so the ranking pass has to score
//! (and then partially sort) far more items than any longer query does.
//! Measured on a 20k-item corpus in a debug build, a one-character prefix
//! query costs ~80-100 ms p95 versus ~2.5 ms for a distinctive multi-token
//! query — see `tests/perf/hot_path_bench_test.rs`.
//!
//! Instead of reacting to the first keystroke, this module pre-computes the
//! ranked results for the small, fixed set of possible first characters
//! (`a`-`z`, `0`-`9`) *before* the user types, and serves them straight from
//! memory. The work is identical to what the on-demand path would compute,
//! so results are byte-for-byte the same; it simply happens earlier, off the
//! interactive path.
//!
//! Memory cost is bounded: at most 36 entries of `HOT_PREFIX_RESULT_LIMIT`
//! lightweight `SearchItem`s (titles/paths are shared strings already held
//! by the cache), i.e. well under 1 MB even at the limit.
//!
//! Correctness: any mutation of the item set (reindex, provider refresh,
//! file-watcher upsert, stale prune, or a launch that changes usage counts)
//! bumps a generation counter, which invalidates the whole cache. The
//! generation is checked on both insert and lookup, so a stale entry can
//! never be served.

use std::collections::HashMap;

use crate::model::SearchItem;

/// How many ranked results to keep per first character. The overlay renders
/// far fewer rows than this; the extra headroom lets a slightly larger
/// configured `max_results` still be served entirely from cache.
pub(crate) const HOT_PREFIX_RESULT_LIMIT: usize = 64;

#[derive(Default)]
pub(crate) struct HotPrefixCache {
    entries: HashMap<String, Vec<SearchItem>>,
    generation: u64,
    built: bool,
}

impl HotPrefixCache {
    /// Returns cached results only when they were produced from the current
    /// generation of the item set.
    pub(crate) fn lookup(&self, query: &str, generation: u64) -> Option<&[SearchItem]> {
        if !self.built || self.generation != generation {
            return None;
        }
        self.entries.get(query).map(Vec::as_slice)
    }

    pub(crate) fn replace(&mut self, generation: u64, entries: HashMap<String, Vec<SearchItem>>) {
        self.entries = entries;
        self.generation = generation;
        self.built = true;
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.built = false;
    }

    /// True when the cache holds entries built from `generation`.
    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.built && self.generation == generation
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The fixed set of first characters a user can type. Normalised queries are
/// lowercase alphanumeric, so these cover every possible first keystroke.
pub(crate) fn prefetch_queries() -> Vec<String> {
    let mut queries: Vec<String> = ('a'..='z').map(|c| c.to_string()).collect();
    queries.extend(('0'..='9').map(|c| c.to_string()));
    queries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> SearchItem {
        SearchItem::new(id, "app", id, &format!("C:\\{id}.exe"))
    }

    #[test]
    fn covers_every_first_keystroke() {
        let queries = prefetch_queries();
        assert_eq!(queries.len(), 36);
        assert!(queries.contains(&"a".to_string()));
        assert!(queries.contains(&"z".to_string()));
        assert!(queries.contains(&"0".to_string()));
        assert!(queries.contains(&"9".to_string()));
    }

    #[test]
    fn lookup_is_generation_gated() {
        let mut cache = HotPrefixCache::default();
        assert!(cache.lookup("f", 1).is_none());

        let mut entries = HashMap::new();
        entries.insert("f".to_string(), vec![item("f1")]);
        cache.replace(1, entries);

        assert_eq!(cache.lookup("f", 1).unwrap().len(), 1);
        // A generation bump invalidates everything.
        assert!(cache.lookup("f", 2).is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn clear_drops_built_flag() {
        let mut cache = HotPrefixCache::default();
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), vec![item("a1")]);
        cache.replace(7, entries);
        assert!(cache.lookup("a", 7).is_some());
        cache.clear();
        assert!(cache.lookup("a", 7).is_none());
    }
}
