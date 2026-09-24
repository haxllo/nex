#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub path: String,
    pub subtitle: String,
    pub use_count: u32,
    pub last_accessed_epoch_secs: i64,
    pub launch_count: u32,
    pub last_launched_at: i64,
    pub pre_score: Option<i64>,
    /// Match-tier bucket (0=Exact, 1=Prefix, 2=Substring, 3=Fuzzy).
    /// Set by the scoring pipeline; `None` when not scored lexically.
    pub match_tier: Option<u8>,
    /// Override string used by `score_text` for lexical matching.
    /// When set, scoring uses this string instead of `title`.
    /// Used by uninstall actions to score against the original app name.
    pub match_target: Option<String>,
    normalized_title: String,
    normalized_search_text: String,
    /// 64-bit character-presence bitmap of `normalized_search_text`.
    /// Bits 0..25 = a-z, 26..35 = 0-9, bit 36 = "any other alphanumeric".
    /// Used as an O(1) *reject* filter before any subsequence scan: if the
    /// query needs a character the item does not contain at all, no fuzzy
    /// match is possible.
    normalized_search_mask: u64,
}

impl SearchItem {
    pub fn new(id: &str, kind: &str, title: &str, path: &str) -> Self {
        Self::from_owned(
            id.to_string(),
            kind.to_string(),
            title.to_string(),
            path.to_string(),
            0,
            0,
        )
    }

    pub fn from_owned(
        id: String,
        kind: String,
        title: String,
        path: String,
        use_count: u32,
        last_accessed_epoch_secs: i64,
    ) -> Self {
        Self::from_owned_with_subtitle(
            id,
            kind,
            title,
            path,
            String::new(),
            use_count,
            last_accessed_epoch_secs,
        )
    }

    pub fn from_owned_with_subtitle(
        id: String,
        kind: String,
        title: String,
        path: String,
        subtitle: String,
        use_count: u32,
        last_accessed_epoch_secs: i64,
    ) -> Self {
        Self::from_owned_with_usage(
            id, kind, title, path, subtitle, use_count, last_accessed_epoch_secs, 0, 0,
        )
    }

    pub fn from_owned_with_usage(
        id: String,
        kind: String,
        title: String,
        path: String,
        subtitle: String,
        use_count: u32,
        last_accessed_epoch_secs: i64,
        launch_count: u32,
        last_launched_at: i64,
    ) -> Self {
        let normalized_title = normalize_for_search(&title);
        let normalized_search_text = normalize_for_search(&format!("{title} {path} {subtitle}"));
        let normalized_search_mask = char_presence_mask(&normalized_search_text);
        Self {
            id,
            kind,
            title,
            path,
            subtitle,
            use_count,
            last_accessed_epoch_secs,
            launch_count,
            last_launched_at,
            pre_score: None,
            match_tier: None,
            match_target: None,
            normalized_title,
            normalized_search_text,
            normalized_search_mask,
        }
    }

    pub fn with_pre_score(mut self, pre_score: i64) -> Self {
        self.pre_score = Some(pre_score);
        self
    }

    pub fn with_match_tier(mut self, tier: u8) -> Self {
        self.match_tier = Some(tier);
        self
    }

    pub fn with_match_target(mut self, target: &str) -> Self {
        self.match_target = Some(target.to_string());
        self
    }

    pub fn with_usage(mut self, use_count: u32, last_accessed_epoch_secs: i64) -> Self {
        self.use_count = use_count;
        self.last_accessed_epoch_secs = last_accessed_epoch_secs;
        self
    }

    pub fn with_launch_usage(mut self, launch_count: u32, last_launched_at: i64) -> Self {
        self.launch_count = launch_count;
        self.last_launched_at = last_launched_at;
        self
    }

    pub fn with_subtitle(mut self, subtitle: &str) -> Self {
        self.subtitle = subtitle.to_string();
        self.normalized_search_text =
            normalize_for_search(&format!("{} {} {}", self.title, self.path, self.subtitle));
        self.normalized_search_mask = char_presence_mask(&self.normalized_search_text);
        self
    }

    pub fn normalized_title(&self) -> &str {
        &self.normalized_title
    }

    pub fn normalized_search_text(&self) -> &str {
        &self.normalized_search_text
    }

    /// Character-presence bitmap over `normalized_search_text`.
    pub fn normalized_search_mask(&self) -> u64 {
        self.normalized_search_mask
    }
}

/// Bit index for a normalised-search character. `None` means the character
/// cannot be represented in the bitmap and callers must not reject on it.
pub fn mask_bit_for_char(ch: char) -> Option<u32> {
    match ch {
        'a'..='z' => Some((ch as u32) - ('a' as u32)),
        '0'..='9' => Some(26 + (ch as u32) - ('0' as u32)),
        _ => None,
    }
}

/// Build the presence bitmap for a normalised string. Unmappable characters
/// set [`MASK_OTHER_BIT`] so a query containing one is never falsely rejected.
pub const MASK_OTHER_BIT: u32 = 36;

pub fn char_presence_mask(normalized: &str) -> u64 {
    let mut mask = 0_u64;
    for ch in normalized.chars() {
        match mask_bit_for_char(ch) {
            Some(bit) => mask |= 1_u64 << bit,
            None => mask |= 1_u64 << MASK_OTHER_BIT,
        }
    }
    mask
}

pub fn normalize_for_search(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}
