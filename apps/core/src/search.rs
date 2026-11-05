use crate::config::SearchMode;
use crate::model::{normalize_for_search, SearchItem};
use crate::query_dsl::TimeFilterWindow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const SCORE_EXACT: i64 = 30_000;
const SCORE_PREFIX: i64 = 27_000;
const SCORE_SUBSTRING: i64 = 24_000;
const SCORE_FUZZY: i64 = 20_000;

const SOURCE_APP_BONUS: i64 = 700;
const SOURCE_LOCAL_FS_BONUS: i64 = 420;
const SOURCE_ACTION_BONUS: i64 = 350;
const SOURCE_CLIPBOARD_BONUS: i64 = 300;

const WORD_PREFIX_PRIMARY_BOOST: i64 = 210;
const WORD_PREFIX_SECONDARY_BOOST: i64 = 140;
const ACRONYM_EXACT_BOOST: i64 = 290;
const ACRONYM_PREFIX_BOOST: i64 = 190;
const MAX_LEXICAL_SIGNAL_BOOST: i64 = 520;

const APP_INTENT_SHORT_QUERY_BONUS: i64 = 320;
const APP_INTENT_MEDIUM_QUERY_BONUS: i64 = 160;

const TOP_HIT_CONFIDENCE_DELTA_SHORT: i64 = 3_000;
const TOP_HIT_CONFIDENCE_DELTA_MEDIUM: i64 = 3_000;
const TOP_HIT_CONFIDENCE_DELTA_LONG: i64 = 3_000;
const TOP_HIT_APP_PREFERENCE_DELTA_SHORT: i64 = 7_000;
const TOP_HIT_APP_PREFERENCE_DELTA_MEDIUM: i64 = 2_100;
const TOP_HIT_APP_PREFERENCE_DELTA_LONG: i64 = 780;
const TOP_HIT_SOURCE_PREFERENCE_DELTA_SHORT: i64 = 420;
const TOP_HIT_SOURCE_PREFERENCE_DELTA_LONG: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextMatchKind {
    Exact,
    Prefix,
    Substring,
    Fuzzy,
}

impl TextMatchKind {
    fn rank(self) -> u8 {
        match self {
            Self::Exact => 0,
            Self::Prefix => 1,
            Self::Substring => 2,
            Self::Fuzzy => 3,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TextScore {
    score: i64,
    kind: TextMatchKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilter {
    pub mode: SearchMode,
    pub extension_filter: Option<String>,
    pub include_files: bool,
    pub include_folders: bool,
    pub include_groups: Vec<Vec<String>>,
    pub exclude_terms: Vec<String>,
    pub modified_within: Option<TimeFilterWindow>,
    pub created_within: Option<TimeFilterWindow>,
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self {
            mode: SearchMode::All,
            extension_filter: None,
            include_files: true,
            include_folders: true,
            include_groups: Vec::new(),
            exclude_terms: Vec::new(),
            modified_within: None,
            created_within: None,
        }
    }
}

pub fn search(items: &[SearchItem], query: &str, limit: usize) -> Vec<SearchItem> {
    if normalize_for_search(query).is_empty() {
        return Vec::new();
    }
    search_with_filter(items, query, limit, &SearchFilter::default())
}

pub fn search_with_filter(
    items: &[SearchItem],
    query: &str,
    limit: usize,
    filter: &SearchFilter,
) -> Vec<SearchItem> {
    search_with_filter_with_boosts(items, query, limit, filter, None)
}

pub fn search_with_filter_with_boosts(
    items: &[SearchItem],
    query: &str,
    limit: usize,
    filter: &SearchFilter,
    personalization_boosts: Option<&HashMap<String, i64>>,
) -> Vec<SearchItem> {
    search_with_filter_with_boosts_indexed(items, None, query, limit, filter, personalization_boosts)
}

/// Same as [`search_with_filter_with_boosts`], but can use a
/// [`SearchIndex`] to score only the items that can possibly match.
///
/// The index is a character-presence postings index: any item that survives
/// `item_may_match_query` contains every normalised query character, so the
/// shortest per-character posting list is a strict superset of every match
/// (exact, prefix, substring and fuzzy). Scoring that list therefore returns
/// exactly the same results as the full scan, while items whose match is
/// decided elsewhere (`pre_score`, `match_target`) are always included.
pub fn search_with_filter_with_boosts_indexed(
    items: &[SearchItem],
    index: Option<&SearchIndex>,
    query: &str,
    limit: usize,
    filter: &SearchFilter,
    personalization_boosts: Option<&HashMap<String, i64>>,
) -> Vec<SearchItem> {
    if limit == 0 || items.is_empty() {
        return Vec::new();
    }

    let normalized_query = normalize_for_search(query);
    let fast_path = is_default_filter(filter) && !normalized_query.is_empty();
    let app_intent_query = looks_like_app_intent_query(query, &normalized_query, filter.mode);
    let now_epoch_secs = now_epoch_secs();
    // Characters the query needs. Any item whose normalised text lacks one of
    // them cannot match (exact, substring, or fuzzy) — reject with one AND.
    let required_mask = query_presence_requirement(&normalized_query);
    let context = ScoreContext {
        normalized_query: &normalized_query,
        now_epoch_secs,
        app_intent_query,
        fast_path,
        required_mask,
        filter,
        personalization_boosts,
    };
    let mut scored: Vec<ScoredItem<'_>> = match index {
        Some(index) if index.item_count == items.len() => {
            match index.candidate_indices(required_mask) {
                Some((postings, always)) => {
                    let mut scored = score_indices(items, postings, &context);
                    // Items with a pre-score or match target bypass the mask
                    // check; include them unless the chosen posting already
                    // scored them.
                    let extra: Vec<u32> = always
                        .iter()
                        .copied()
                        .filter(|idx| postings.binary_search(idx).is_err())
                        .collect();
                    scored.extend(score_indices(items, &extra, &context));
                    scored
                }
                None => score_items(items, &context),
            }
        }
        _ => score_items(items, &context),
    };

    deduplicate_apps(&mut scored);
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, compare_scored);
        scored.truncate(limit);
    }
    scored.sort_unstable_by(compare_scored);
    apply_top_hit_confidence_guard(&mut scored, &normalized_query, app_intent_query);

    scored
        .into_iter()
        .take(limit)
        .map(|scored| {
            let mut item = scored.item.clone();
            item.match_tier = Some(scored.match_kind.rank());
            item
        })
        .collect()
}

/// Below this many items, splitting the scan costs more than it saves.
const PARALLEL_SEARCH_MIN_ITEMS: usize = 20_000;
const PARALLEL_INDEX_MIN_ITEMS: usize = 2_000;
const PARALLEL_SEARCH_MAX_THREADS: usize = 8;
const SEARCH_MASK_BITS: usize = 64;

/// Character-presence postings over the cached item set.
///
/// Built when the cache changes. `normalized_search_mask` is the same mask the
/// scorer uses as an O(1) reject, so intersecting (taking) one required
/// character's posting list is always a superset of the items that can match.
pub struct SearchIndex {
    char_postings: Vec<Vec<u32>>,
    always: Vec<u32>,
    item_count: usize,
}

impl SearchIndex {
    pub fn build(items: &[SearchItem]) -> Self {
        let mut char_postings = vec![Vec::new(); SEARCH_MASK_BITS];
        let mut always = Vec::new();
        for (position, item) in items.iter().enumerate() {
            let position = position as u32;
            if item.pre_score.is_some() || item.match_target.is_some() {
                always.push(position);
            }
            let mut mask = item.normalized_search_mask();
            while mask != 0 {
                let bit = mask.trailing_zeros() as usize;
                if let Some(posting) = char_postings.get_mut(bit) {
                    posting.push(position);
                }
                mask &= mask - 1;
            }
        }
        Self {
            char_postings,
            always,
            item_count: items.len(),
        }
    }

    /// The shortest required-character posting plus the always-admitted items.
    /// Returns `None` when the query has no character mask (scan everything).
    fn candidate_indices(&self, required_mask: u64) -> Option<(&[u32], &[u32])> {
        if required_mask == 0 {
            return None;
        }
        let mut chosen: Option<&Vec<u32>> = None;
        let mut mask = required_mask;
        while mask != 0 {
            let bit = mask.trailing_zeros() as usize;
            let posting = self.char_postings.get(bit)?;
            if chosen.is_none_or(|best| posting.len() < best.len()) {
                chosen = Some(posting);
            }
            mask &= mask - 1;
        }
        chosen.map(|posting| (posting.as_slice(), self.always.as_slice()))
    }
}

#[derive(Clone, Copy)]
struct ScoreContext<'a> {
    normalized_query: &'a str,
    now_epoch_secs: i64,
    app_intent_query: bool,
    fast_path: bool,
    required_mask: u64,
    filter: &'a SearchFilter,
    personalization_boosts: Option<&'a HashMap<String, i64>>,
}

/// Score every candidate, on one thread for small sets and chunked across
/// cores for the full index (the 80k-item scan was 60-84 ms per keystroke).
fn score_items<'a>(items: &'a [SearchItem], context: &ScoreContext<'_>) -> Vec<ScoredItem<'a>> {
    if items.len() < PARALLEL_SEARCH_MIN_ITEMS {
        return items
            .iter()
            .filter_map(|item| score_candidate(item, context))
            .collect();
    }
    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .clamp(1, PARALLEL_SEARCH_MAX_THREADS);
    if threads <= 1 {
        return items
            .iter()
            .filter_map(|item| score_candidate(item, context))
            .collect();
    }
    let chunk_len = items.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk_len)
            .map(|chunk| {
                let context = *context;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|item| score_candidate(item, &context))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut scored = Vec::with_capacity(items.len());
        for handle in handles {
            if let Ok(mut chunk) = handle.join() {
                scored.append(&mut chunk);
            }
        }
        scored
    })
}

/// Score only the candidate positions from a [`SearchIndex`], using the same
/// per-item path as the full scan.
fn score_indices<'a>(
    items: &'a [SearchItem],
    indices: &[u32],
    context: &ScoreContext<'_>,
) -> Vec<ScoredItem<'a>> {
    if indices.len() < PARALLEL_INDEX_MIN_ITEMS {
        return indices
            .iter()
            .filter_map(|&index| {
                items
                    .get(index as usize)
                    .and_then(|item| score_candidate(item, context))
            })
            .collect();
    }
    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .clamp(1, PARALLEL_SEARCH_MAX_THREADS);
    if threads <= 1 {
        return indices
            .iter()
            .filter_map(|&index| {
                items
                    .get(index as usize)
                    .and_then(|item| score_candidate(item, context))
            })
            .collect();
    }
    let chunk_len = indices.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = indices
            .chunks(chunk_len)
            .map(|chunk| {
                let context = *context;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|&index| {
                            items
                                .get(index as usize)
                                .and_then(|item| score_candidate(item, &context))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut scored = Vec::with_capacity(indices.len());
        for handle in handles {
            if let Ok(mut chunk) = handle.join() {
                scored.append(&mut chunk);
            }
        }
        scored
    })
}

fn score_candidate<'a>(
    item: &'a SearchItem,
    context: &ScoreContext<'_>,
) -> Option<ScoredItem<'a>> {
    if !matches_visibility(item, context.filter) {
        return None;
    }
    if !item_may_match_query(item, context.required_mask) {
        return None;
    }
    let personalization_boost = context
        .personalization_boosts
        .and_then(|boosts| boosts.get(item.id.as_str()))
        .copied()
        .unwrap_or(0);
    let score = if context.fast_path {
        score_item_fast(
            item,
            context.normalized_query,
            context.now_epoch_secs,
            context.app_intent_query,
            personalization_boost,
        )
    } else {
        score_item(
            item,
            context.normalized_query,
            context.now_epoch_secs,
            context.filter,
            context.app_intent_query,
            personalization_boost,
        )
    };
    score.map(|score| ScoredItem {
        source_rank: source_rank(item),
        score: score.score,
        match_kind: score.kind,
        title_len: item.normalized_title().len(),
        item,
    })
}
/// Presence bits the query requires of an item's normalised search text.
/// Empty when the query has no normalised characters.
fn query_presence_requirement(normalized_query: &str) -> u64 {
    let mut required = 0_u64;
    for ch in normalized_query.chars() {
        if let Some(bit) = crate::model::mask_bit_for_char(ch) {
            required |= 1_u64 << bit;
        }
    }
    required
}

/// Cheap O(1) reject used before any lexical scan. Items whose match is
/// decided elsewhere (Tantivy pre-scores, `match_target` overrides) are
/// always admitted so their semantics are untouched.
#[inline]
fn item_may_match_query(item: &SearchItem, required_mask: u64) -> bool {
    if required_mask == 0 || item.pre_score.is_some() || item.match_target.is_some() {
        return true;
    }
    item.normalized_search_mask() & required_mask == required_mask
}

#[derive(Debug, Clone, Copy)]
struct ScoredItem<'a> {
    source_rank: u8,
    score: i64,
    match_kind: TextMatchKind,
    title_len: usize,
    item: &'a SearchItem,
}

fn compare_scored(a: &ScoredItem<'_>, b: &ScoredItem<'_>) -> Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| a.match_kind.rank().cmp(&b.match_kind.rank()))
        .then_with(|| a.source_rank.cmp(&b.source_rank))
        .then_with(|| a.title_len.cmp(&b.title_len))
        .then_with(|| a.item.normalized_title().cmp(b.item.normalized_title()))
        .then_with(|| a.item.id.cmp(&b.item.id))
}

fn score_item_fast(
    item: &SearchItem,
    normalized_query: &str,
    now_epoch_secs: i64,
    app_intent_query: bool,
    personalization_boost: i64,
) -> Option<TextScore> {
    let text_score = if let Some(pre_score) = item.pre_score {
        // BM25 pre-score: derive tier from threshold, add within-tier bonus.
        let (tier, base) = if pre_score >= 3_000 {
            (TextMatchKind::Prefix, SCORE_PREFIX)
        } else {
            (TextMatchKind::Substring, SCORE_SUBSTRING)
        };
        let within_tier_bonus = pre_score.min(2_500);
        TextScore {
            score: base + within_tier_bonus,
            kind: tier,
        }
    } else {
        // Common case is `match_target: None`; borrow the precomputed
        // normalized title directly instead of cloning it into a String.
        let normalized_target;
        let title_for_match: &str = match item.match_target.as_deref() {
            Some(target) => {
                normalized_target = crate::model::normalize_for_search(target);
                normalized_target.as_str()
            }
            None => item.normalized_title(),
        };
        score_text(title_for_match, normalized_query)?
    };
    let lexical_signal_bonus = word_boundary_and_acronym_bonus(&item.title, normalized_query);
    let app_intent_bonus = app_intent_bonus(item, app_intent_query, normalized_query.len());
    let source_bonus = source_bonus(item);
    let recency_bonus = recency_bonus(item.last_accessed_epoch_secs, now_epoch_secs);
    let frequency_bonus = frequency_bonus(item.use_count);

    Some(TextScore {
        score: text_score.score
            + lexical_signal_bonus
            + app_intent_bonus
            + source_bonus
            + recency_bonus
            + frequency_bonus
            + personalization_boost,
        kind: text_score.kind,
    })
}

fn score_item(
    item: &SearchItem,
    normalized_query: &str,
    now_epoch_secs: i64,
    filter: &SearchFilter,
    app_intent_query: bool,
    personalization_boost: i64,
) -> Option<TextScore> {
    if !matches_mode(item, filter.mode) {
        return None;
    }
    if let Some(extension) = &filter.extension_filter {
        if !matches_extension_filter(item, extension) {
            return None;
        }
    }
    if !matches_term_filters(item, filter) {
        return None;
    }
    if !matches_time_filters(item, filter, now_epoch_secs) {
        return None;
    }

    let text_score = if let Some(pre_score) = item.pre_score {
        // BM25 pre-score: derive tier from threshold, add within-tier bonus.
        let (tier, base) = if pre_score >= 3_000 {
            (TextMatchKind::Prefix, SCORE_PREFIX)
        } else {
            (TextMatchKind::Substring, SCORE_SUBSTRING)
        };
        let within_tier_bonus = pre_score.min(2_500);
        TextScore {
            score: base + within_tier_bonus,
            kind: tier,
        }
    } else if normalized_query.is_empty() {
        TextScore {
            score: 0,
            kind: TextMatchKind::Substring,
        }
    } else {
        let normalized_target;
        let title_for_match: &str = match item.match_target.as_deref() {
            Some(target) => {
                normalized_target = crate::model::normalize_for_search(target);
                normalized_target.as_str()
            }
            None => item.normalized_title(),
        };
        score_text(title_for_match, normalized_query).or_else(|| {
            score_text(item.normalized_search_text(), normalized_query).map(|text_score| {
                TextScore {
                    score: text_score.score - 1_500,
                    kind: text_score.kind,
                }
            })
        })?
    };
    let lexical_signal_bonus = word_boundary_and_acronym_bonus(&item.title, normalized_query);
    let app_intent_bonus = app_intent_bonus(item, app_intent_query, normalized_query.len());
    let source_bonus = source_bonus(item);
    let mode_bonus = mode_bonus(item, filter.mode);
    let recency_bonus = recency_bonus(item.last_accessed_epoch_secs, now_epoch_secs);
    let frequency_bonus = frequency_bonus(item.use_count);

    Some(TextScore {
        score: text_score.score
            + lexical_signal_bonus
            + app_intent_bonus
            + source_bonus
            + mode_bonus
            + recency_bonus
            + frequency_bonus
            + personalization_boost,
        kind: text_score.kind,
    })
}

fn score_text(normalized_title: &str, query: &str) -> Option<TextScore> {
    if normalized_title.is_empty() || query.is_empty() {
        return None;
    }

    let length_penalty = (normalized_title.len() as i64 - query.len() as i64).abs();
    let compact_bonus = (query.len() as i64) * 45;

    if normalized_title == query {
        return Some(TextScore {
            score: SCORE_EXACT + compact_bonus - length_penalty,
            kind: TextMatchKind::Exact,
        });
    }

    if normalized_title.starts_with(query) {
        return Some(TextScore {
            score: SCORE_PREFIX + compact_bonus - length_penalty,
            kind: TextMatchKind::Prefix,
        });
    }

    if let Some(position) = normalized_title.find(query) {
        let position_penalty = (position as i64) * 3;
        return Some(TextScore {
            score: SCORE_SUBSTRING + compact_bonus - position_penalty - length_penalty,
            kind: TextMatchKind::Substring,
        });
    }

    let (start_penalty, gap_penalty) = subsequence_penalties(normalized_title, query)?;
    Some(TextScore {
        score: SCORE_FUZZY + compact_bonus - gap_penalty * 8 - start_penalty - length_penalty,
        kind: TextMatchKind::Fuzzy,
    })
}

fn recency_bonus(last_accessed_epoch_secs: i64, now_epoch_secs: i64) -> i64 {
    if last_accessed_epoch_secs <= 0 || now_epoch_secs <= 0 {
        return 0;
    }

    let age_secs = if last_accessed_epoch_secs >= now_epoch_secs {
        0
    } else {
        now_epoch_secs - last_accessed_epoch_secs
    };
    match age_secs {
        0..=3_600 => 260,             // within 1 hour
        3_601..=86_400 => 220,        // within 1 day
        86_401..=604_800 => 170,      // within 7 days
        604_801..=2_592_000 => 110,   // within 30 days
        2_592_001..=7_776_000 => 60,  // within 90 days
        7_776_001..=31_536_000 => 25, // within 1 year
        _ => 0,
    }
}

fn frequency_bonus(use_count: u32) -> i64 {
    ((use_count as i64) * 18).clamp(0, 220)
}

fn mode_bonus(item: &SearchItem, mode: SearchMode) -> i64 {
    match mode {
        SearchMode::All => 0,
        SearchMode::Apps if item.kind.eq_ignore_ascii_case("app") => 550,
        SearchMode::Files if item.kind.eq_ignore_ascii_case("file") => 550,
        SearchMode::Folders if item.kind.eq_ignore_ascii_case("folder") => 550,
        SearchMode::Actions if item.kind.eq_ignore_ascii_case("action") => 550,
        SearchMode::Clipboard if item.kind.eq_ignore_ascii_case("clipboard") => 550,
        _ => -2_500,
    }
}

fn source_rank(item: &SearchItem) -> u8 {
    if item.kind.eq_ignore_ascii_case("app") {
        return 0;
    }

    if item.kind.eq_ignore_ascii_case("action") {
        return 1;
    }

    if (item.kind.eq_ignore_ascii_case("file") || item.kind.eq_ignore_ascii_case("folder"))
        && is_local_path(&item.path)
    {
        return 2;
    }

    if item.kind.eq_ignore_ascii_case("clipboard") {
        return 3;
    }

    4
}

fn source_bonus(item: &SearchItem) -> i64 {
    match source_rank(item) {
        0 => SOURCE_APP_BONUS,
        1 => SOURCE_ACTION_BONUS,
        2 => SOURCE_LOCAL_FS_BONUS,
        3 => SOURCE_CLIPBOARD_BONUS,
        _ => 0,
    }
}

fn apply_top_hit_confidence_guard(
    scored: &mut [ScoredItem<'_>],
    normalized_query: &str,
    app_intent_query: bool,
) {
    if scored.len() < 2 || normalized_query.is_empty() {
        return;
    }

    let lead = scored[0];
    let runner_up = scored[1];
    let score_delta = lead.score.saturating_sub(runner_up.score);
    let query_len = normalized_query.len();
    let confidence_delta = match query_len {
        0..=2 => TOP_HIT_CONFIDENCE_DELTA_SHORT,
        3..=5 => TOP_HIT_CONFIDENCE_DELTA_MEDIUM,
        _ => TOP_HIT_CONFIDENCE_DELTA_LONG,
    };

    let stronger_runner_up_match =
        runner_up.match_kind.rank() < lead.match_kind.rank() && score_delta <= confidence_delta;

    let app_runner_up_preferred = app_intent_query
        && !lead.item.kind.eq_ignore_ascii_case("app")
        && runner_up.item.kind.eq_ignore_ascii_case("app")
        && score_delta
            <= match query_len {
                0..=2 => TOP_HIT_APP_PREFERENCE_DELTA_SHORT,
                3..=5 => TOP_HIT_APP_PREFERENCE_DELTA_MEDIUM,
                _ => TOP_HIT_APP_PREFERENCE_DELTA_LONG,
            };

    let stronger_source_runner_up = lead.match_kind.rank() == runner_up.match_kind.rank()
        && source_rank(runner_up.item) < source_rank(lead.item)
        && score_delta
            <= if query_len <= 2 {
                TOP_HIT_SOURCE_PREFERENCE_DELTA_SHORT
            } else {
                TOP_HIT_SOURCE_PREFERENCE_DELTA_LONG
            };

    if stronger_runner_up_match || app_runner_up_preferred || stronger_source_runner_up {
        scored.swap(0, 1);
    }
}

/// Allocation-free lexical signal: word-prefix and acronym bonuses.
///
/// The previous implementation built a `Vec<String>` of word tokens plus an
/// `acronym` `String` for **every candidate on every keystroke** — roughly
/// 2 heap allocations per item, so ~240k for a 120k-item corpus. This version
/// scans the title exactly once with zero heap traffic, which is the single
/// largest per-query win in the ranking pass.
fn word_boundary_and_acronym_bonus(title: &str, normalized_query: &str) -> i64 {
    if title.trim().is_empty() || normalized_query.is_empty() {
        return 0;
    }

    let mut scan = LexicalScan::new(normalized_query);
    let mut previous_was_lower = false;

    for ch in title.chars() {
        if !ch.is_alphanumeric() {
            scan.close_word();
            previous_was_lower = false;
            continue;
        }

        let is_upper = ch.is_uppercase();
        if scan.word_open && is_upper && previous_was_lower {
            scan.close_word();
        }
        if !scan.word_open {
            scan.start_word();
        }

        for lower in ch.to_lowercase() {
            scan.push_char(lower);
        }
        previous_was_lower = ch.is_lowercase();
    }
    scan.close_word();

    scan.bonus()
}

/// Single-pass scanner that reproduces `normalized_word_tokens` semantics
/// (alphanumeric runs, split before an uppercase that follows a lowercase)
/// without allocating. `progress` tracks how many bytes of the query the
/// current word has consumed; `usize::MAX` marks a word that already failed.
struct LexicalScan<'a> {
    query: &'a str,
    query_len: usize,
    query_char_len: usize,
    progress: usize,
    word_open: bool,
    word_count: usize,
    acronym_pending: bool,
    acronym_ok: bool,
    acronym: [char; 64],
    first_word_prefix: bool,
    other_word_prefix: bool,
}

impl<'a> LexicalScan<'a> {
    fn new(query: &'a str) -> Self {
        let query_char_len = query.chars().count();
        Self {
            query,
            query_len: query.len(),
            query_char_len,
            progress: 0,
            word_open: false,
            word_count: 0,
            acronym_pending: false,
            acronym_ok: query_char_len <= 64,
            acronym: ['\0'; 64],
            first_word_prefix: false,
            other_word_prefix: false,
        }
    }

    fn start_word(&mut self) {
        self.word_open = true;
        self.word_count += 1;
        self.progress = 0;
        self.acronym_pending = true;
    }

    fn push_char(&mut self, lower: char) {
        if self.acronym_pending {
            self.acronym_pending = false;
            if self.acronym_ok && self.word_count <= 64 {
                self.acronym[self.word_count - 1] = lower;
            }
        }
        if self.progress < self.query_len {
            match self.query[self.progress..].chars().next() {
                Some(expected) if expected == lower => self.progress += expected.len_utf8(),
                _ => self.progress = usize::MAX,
            }
        }
    }

    fn close_word(&mut self) {
        if !self.word_open {
            return;
        }
        self.word_open = false;
        if self.progress == self.query_len {
            if self.word_count == 1 {
                self.first_word_prefix = true;
            } else {
                self.other_word_prefix = true;
            }
        }
    }

    fn bonus(&self) -> i64 {
        let mut bonus = 0_i64;
        if self.first_word_prefix {
            bonus += WORD_PREFIX_PRIMARY_BOOST;
        } else if self.other_word_prefix {
            bonus += WORD_PREFIX_SECONDARY_BOOST;
        }

        if self.query_len >= 2 {
            let stored = self.word_count.min(self.acronym.len());
            let mut query_chars = self.query.chars();
            let mut matched = 0_usize;
            let mut all_match = true;
            for index in 0..stored {
                match query_chars.next() {
                    Some(expected) => {
                        if self.acronym[index] != expected {
                            all_match = false;
                            break;
                        }
                        matched += 1;
                    }
                    None => break,
                }
            }
            if all_match && matched == self.query_char_len {
                if self.word_count == self.query_char_len {
                    bonus += ACRONYM_EXACT_BOOST;
                } else if self.word_count > self.query_char_len {
                    bonus += ACRONYM_PREFIX_BOOST;
                }
            }
        }

        bonus.clamp(0, MAX_LEXICAL_SIGNAL_BOOST)
    }
}

fn app_intent_bonus(
    item: &SearchItem,
    app_intent_query: bool,
    normalized_query_len: usize,
) -> i64 {
    if !app_intent_query || normalized_query_len == 0 {
        return 0;
    }

    if item.kind.eq_ignore_ascii_case("app") {
        if normalized_query_len <= 2 {
            APP_INTENT_SHORT_QUERY_BONUS
        } else if normalized_query_len <= 4 {
            APP_INTENT_MEDIUM_QUERY_BONUS
        } else {
            0
        }
    } else {
        0
    }
}

fn looks_like_app_intent_query(raw_query: &str, normalized_query: &str, mode: SearchMode) -> bool {
    if normalized_query.is_empty() || matches!(mode, SearchMode::Files) {
        return false;
    }

    let trimmed = raw_query.trim();
    if trimmed.is_empty() || trimmed.starts_with('>') || trimmed.starts_with('@') {
        return false;
    }

    !(trimmed.contains('\\')
        || trimmed.contains('/')
        || trimmed.contains(':')
        || trimmed.contains('.')
        || trimmed.contains('*')
        || trimmed.contains('?'))
}

fn is_default_filter(filter: &SearchFilter) -> bool {
    filter.mode == SearchMode::All
        && filter.extension_filter.is_none()
        && filter.include_files
        && filter.include_folders
        && filter.include_groups.is_empty()
        && filter.exclude_terms.is_empty()
        && filter.modified_within.is_none()
        && filter.created_within.is_none()
}

fn matches_mode(item: &SearchItem, mode: SearchMode) -> bool {
    match mode {
        SearchMode::All => true,
        SearchMode::Apps => item.kind.eq_ignore_ascii_case("app"),
        SearchMode::Files => item.kind.eq_ignore_ascii_case("file"),
        SearchMode::Folders => item.kind.eq_ignore_ascii_case("folder"),
        SearchMode::Actions => item.kind.eq_ignore_ascii_case("action"),
        SearchMode::Clipboard => item.kind.eq_ignore_ascii_case("clipboard"),
    }
}

fn matches_visibility(item: &SearchItem, filter: &SearchFilter) -> bool {
    if item.kind.eq_ignore_ascii_case("file") && !filter.include_files {
        return false;
    }
    if item.kind.eq_ignore_ascii_case("folder") && !filter.include_folders {
        return false;
    }
    true
}

fn matches_extension_filter(item: &SearchItem, extension_filter: &str) -> bool {
    let normalized = extension_filter
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if item.kind.eq_ignore_ascii_case("folder") || item.kind.eq_ignore_ascii_case("action") {
        return false;
    }

    let path = item.path.trim();
    if path.is_empty() {
        return false;
    }

    let ext = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_default();
    !ext.is_empty() && ext == normalized
}

fn matches_term_filters(item: &SearchItem, filter: &SearchFilter) -> bool {
    let haystack = item.normalized_search_text();
    if filter
        .exclude_terms
        .iter()
        .any(|term| !term.is_empty() && haystack.contains(term))
    {
        return false;
    }

    if filter.include_groups.is_empty() {
        return true;
    }

    filter.include_groups.iter().any(|group| {
        group
            .iter()
            .all(|term| term.is_empty() || haystack.contains(term))
    })
}

fn matches_time_filters(item: &SearchItem, filter: &SearchFilter, now_epoch_secs: i64) -> bool {
    if filter.modified_within.is_none() && filter.created_within.is_none() {
        return true;
    }

    let path = Path::new(item.path.trim());
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };

    if let Some(window) = filter.modified_within {
        let Some(modified_secs) = meta
            .modified()
            .ok()
            .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
            .map(|v| v.as_secs() as i64)
        else {
            return false;
        };
        if !within_window(modified_secs, now_epoch_secs, window) {
            return false;
        }
    }

    if let Some(window) = filter.created_within {
        let Some(created_secs) = meta
            .created()
            .ok()
            .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
            .map(|v| v.as_secs() as i64)
        else {
            return false;
        };
        if !within_window(created_secs, now_epoch_secs, window) {
            return false;
        }
    }

    true
}

fn within_window(value_secs: i64, now_secs: i64, window: TimeFilterWindow) -> bool {
    if value_secs <= 0 || now_secs <= 0 || value_secs > now_secs {
        return false;
    }
    let age = now_secs - value_secs;
    match window {
        TimeFilterWindow::Today => age <= 24 * 60 * 60,
        TimeFilterWindow::Week => age <= 7 * 24 * 60 * 60,
        TimeFilterWindow::Month => age <= 31 * 24 * 60 * 60,
    }
}

fn is_local_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains("://") {
        return false;
    }
    if trimmed.starts_with("\\\\") {
        return false;
    }

    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return true;
    }

    trimmed.starts_with('/')
}

fn subsequence_penalties(haystack: &str, needle: &str) -> Option<(i64, i64)> {
    let mut next_start = 0;
    let mut start_penalty: Option<i64> = None;
    let mut previous_position: Option<usize> = None;
    let mut gap_penalty = 0_i64;

    for needle_char in needle.chars() {
        let mut found: Option<(usize, usize)> = None;
        for (offset, hay_char) in haystack[next_start..].char_indices() {
            if hay_char == needle_char {
                let absolute = next_start + offset;
                found = Some((absolute, hay_char.len_utf8()));
                break;
            }
        }

        let (position, char_len) = found?;
        if start_penalty.is_none() {
            start_penalty = Some(position as i64);
        }
        if let Some(previous) = previous_position {
            gap_penalty += position.saturating_sub(previous + 1) as i64;
        }
        previous_position = Some(position);
        next_start = position + char_len;
    }

    Some((start_penalty.unwrap_or(0), gap_penalty))
}

fn deduplicate_apps(scored: &mut Vec<ScoredItem<'_>>) {
    // File-only corpora (the common case for indexed searches) have nothing
    // to dedupe — skip the HashMap + Vec<bool> allocations entirely.
    if !scored.iter().any(|s| s.item.kind.eq_ignore_ascii_case("app")) {
        return;
    }

    let mut winners: HashMap<(String, String), usize> = HashMap::new();
    let mut remove = vec![false; scored.len()];

    for (idx, s) in scored.iter().enumerate() {
        if !s.item.kind.eq_ignore_ascii_case("app") {
            continue;
        }
        let key = (
            normalize_for_search(&s.item.title),
            deduplication_base_name(s.item),
        );
        if let Some(&current_winner) = winners.get(&key) {
            if compare_scored(s, &scored[current_winner]).is_lt() {
                remove[current_winner] = true;
                winners.insert(key, idx);
            } else {
                remove[idx] = true;
            }
        } else {
            winners.insert(key, idx);
        }
    }

    *scored = scored
        .drain(..)
        .enumerate()
        .filter_map(|(idx, item)| (!remove[idx]).then_some(item))
        .collect();
}

fn deduplication_base_name(item: &SearchItem) -> String {
    if item
        .path
        .split(['\\', '/'])
        .any(|part| part.eq_ignore_ascii_case("uninstall"))
    {
        return normalize_for_search(&item.title);
    }
    extract_base_name(&item.path)
}

fn extract_base_name(path: &str) -> String {
    let filename = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path);
    // Strip extension (e.g., "firefox.exe" -> "firefox")
    let stem = if let Some(pos) = filename.rfind('.') {
        &filename[..pos]
    } else {
        filename
    };
    normalize_for_search(stem)
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod candidate_index_tests {
    use super::*;
    use crate::config::SearchMode;
    use std::time::Instant;

    fn item(id: &str, title: &str, path: &str) -> SearchItem {
        SearchItem::new(id, "file", title, path)
    }

    fn ids(items: &[SearchItem]) -> Vec<String> {
        items.iter().map(|item| item.id.clone()).collect()
    }

    fn compare(items: &[SearchItem], query: &str, filter: &SearchFilter) {
        let index = SearchIndex::build(items);
        let full = search_with_filter_with_boosts(items, query, 20, filter, None);
        let indexed = search_with_filter_with_boosts_indexed(
            items,
            Some(&index),
            query,
            20,
            filter,
            None,
        );
        assert_eq!(
            ids(&full),
            ids(&indexed),
            "indexed search must match the full scan for {query:?}"
        );
    }

    #[test]
    fn candidate_index_matches_full_scan() {
        let items = vec![
            item("f1", "document-1.txt", r"C:\Data\document-1.txt"),
            item("f2", "report-final.pdf", r"C:\Docs\report-final.pdf"),
            item("f3", "alpha.txt", r"C:\misc\alpha.txt"),
            item("f4", "zeta.log", r"C:\logs\zeta.log"),
            item("f5", "readme.md", r"C:\README.md"),
        ];
        let mut pre = item("f6", "unrelated.bin", r"C:\unrelated.bin").with_pre_score(4000);
        pre.use_count = 3;
        let mut target = item("f7", "some.exe", r"C:\some.exe");
        target.match_target = Some("special target".into());

        let mut items = items;
        items.push(pre);
        items.push(target);

        let default_filter = SearchFilter::default();
        for query in [
            "a", "al", "alpha", "report", "port", "doc-1", "zzz", "x", "special", "",
        ] {
            compare(&items, query, &default_filter);
        }

        let files_only = SearchFilter {
            mode: SearchMode::Files,
            include_folders: false,
            ..SearchFilter::default()
        };
        compare(&items, "report", &files_only);
        compare(&items, "a", &files_only);

        let extension = SearchFilter {
            extension_filter: Some("txt".into()),
            ..SearchFilter::default()
        };
        compare(&items, "document", &extension);
    }

    #[test]
    fn candidate_index_matches_full_scan_on_a_larger_set() {
        let items: Vec<SearchItem> = (0..2_000)
            .map(|index| {
                item(
                    &format!("file:{index}"),
                    &format!("file-{index}.txt"),
                    &format!("C:\\data\\file-{index}.txt"),
                )
            })
            .collect();
        let default_filter = SearchFilter::default();
        for query in ["1", "4", "file", "file-19", "xyz", "document"] {
            compare(&items, query, &default_filter);
        }
    }

    #[test]
    #[ignore = "manual latency benchmark"]
    fn bench_candidate_index_80k() {
        let items: Vec<SearchItem> = (0..80_000)
            .map(|index| {
                item(
                    &format!("file:c:\\data\\file{index}.txt"),
                    &format!("document-{index}.txt"),
                    &format!("C:\\Data\\file{index}.txt"),
                )
            })
            .collect();
        let build_started = Instant::now();
        let index = SearchIndex::build(&items);
        eprintln!("index build 80k: {} ms", build_started.elapsed().as_millis());
        let filter = SearchFilter::default();
        for query in ["d", "do", "document-4", "ument-4"] {
            let started = Instant::now();
            let full = search_with_filter_with_boosts(&items, query, 20, &filter, None);
            let full_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let indexed = search_with_filter_with_boosts_indexed(
                &items,
                Some(&index),
                query,
                20,
                &filter,
                None,
            );
            let indexed_ms = started.elapsed().as_millis();
            assert_eq!(ids(&full), ids(&indexed));
            eprintln!("query={query:?} full_ms={full_ms} indexed_ms={indexed_ms}");
        }
    }
}

#[cfg(test)]
mod lexical_scan_tests {
    use super::*;

    #[test]
    #[ignore = "manual latency benchmark"]
    fn bench_indexed_search_80k() {
        let items: Vec<SearchItem> = (0..80_000)
            .map(|index| {
                SearchItem::new(
                    &format!("file:c:\\data\\file{index}.txt"),
                    "file",
                    &format!("document-{index}.txt"),
                    &format!("C:\\Data\\file{index}.txt"),
                )
            })
            .collect();
        let filter = SearchFilter::default();
        for query in ["d", "do", "document-4", "ument-4"] {
            let started = std::time::Instant::now();
            let results = search_with_filter(&items, query, 20, &filter);
            eprintln!(
                "query={query:?} elapsed_ms={} results={}",
                started.elapsed().as_millis(),
                results.len()
            );
        }
    }

    /// Reference implementation mirroring the original allocating scanner.
    /// Kept verbatim so the allocation-free rewrite can be proven equivalent.
    fn reference_normalized_word_tokens(title: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut current = String::new();
        let mut previous_was_lower = false;

        for ch in title.chars() {
            if !ch.is_alphanumeric() {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                previous_was_lower = false;
                continue;
            }

            let is_upper = ch.is_uppercase();
            if !current.is_empty() && is_upper && previous_was_lower {
                words.push(std::mem::take(&mut current));
            }

            for lower in ch.to_lowercase() {
                current.push(lower);
            }
            previous_was_lower = ch.is_lowercase();
        }

        if !current.is_empty() {
            words.push(current);
        }

        words
    }

    fn reference_bonus(title: &str, normalized_query: &str) -> i64 {
        if title.trim().is_empty() || normalized_query.is_empty() {
            return 0;
        }

        let words = reference_normalized_word_tokens(title);
        if words.is_empty() {
            return 0;
        }

        let mut bonus = 0_i64;
        if words
            .first()
            .is_some_and(|word| word.starts_with(normalized_query))
        {
            bonus += WORD_PREFIX_PRIMARY_BOOST;
        } else if words
            .iter()
            .skip(1)
            .any(|word| word.starts_with(normalized_query))
        {
            bonus += WORD_PREFIX_SECONDARY_BOOST;
        }

        let acronym: String = words
            .iter()
            .filter_map(|word| word.chars().next())
            .collect();
        if normalized_query.len() >= 2 {
            if acronym == normalized_query {
                bonus += ACRONYM_EXACT_BOOST;
            } else if acronym.starts_with(normalized_query) {
                bonus += ACRONYM_PREFIX_BOOST;
            }
        }

        bonus.clamp(0, MAX_LEXICAL_SIGNAL_BOOST)
    }

    #[test]
    fn allocation_free_scanner_matches_reference() {
        let titles = [
            "Visual Studio Code",
            "Windows Terminal",
            "Q4_Report.xlsx",
            "Document_00042.txt",
            "HTC One M8",
            "mySQLWorkbench",
            "git-bash.exe",
            "  spaced out  ",
            "ALLCAPS",
            "a",
            "...",
            "",
            "Nex",
            "win10_notes_v2.md",
            "camelCaseWordsHere",
            "snake_case_name",
            "12345",
        ];
        let queries = [
            "a", "ab", "vs", "wte", "q4", "doc", "nte", "z", "q4reort", "msw", "htcom",
            "win", "camel", "snake", "12", "al", "", "my", "g", "ccw", "nex", "vs", "h8",
        ];

        for title in titles {
            for query in queries {
                assert_eq!(
                    word_boundary_and_acronym_bonus(title, query),
                    reference_bonus(title, query),
                    "divergence for title={title:?} query={query:?}"
                );
            }
        }
    }

    #[test]
    fn scanner_handles_boundaries_and_acronyms() {
        assert_eq!(word_boundary_and_acronym_bonus("", "a"), 0);
        assert_eq!(word_boundary_and_acronym_bonus("   ", "a"), 0);
        assert_eq!(word_boundary_and_acronym_bonus("alpha", ""), 0);
        // first-word prefix
        assert_eq!(word_boundary_and_acronym_bonus("Alpha", "al"), WORD_PREFIX_PRIMARY_BOOST);
        // secondary-word prefix only
        assert_eq!(word_boundary_and_acronym_bonus("foo bar", "ba"), WORD_PREFIX_SECONDARY_BOOST);
        // acronym exact (needs >= 2 chars)
        assert_eq!(
            word_boundary_and_acronym_bonus("Visual Studio Code", "vsc"),
            ACRONYM_EXACT_BOOST
        );
        // acronym prefix with extra words
        assert_eq!(
            word_boundary_and_acronym_bonus("Visual Studio Code", "vs"),
            ACRONYM_PREFIX_BOOST
        );
    }
}
