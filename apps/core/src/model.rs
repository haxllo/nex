#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub path: String,
    pub subtitle: String,
    pub use_count: u32,
    pub last_accessed_epoch_secs: i64,
    normalized_title: String,
    normalized_search_text: String,
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
        let normalized_title = normalize_for_search(&title);
        let normalized_search_text = normalize_for_search(&format!("{title} {path} {subtitle}"));
        Self {
            id,
            kind,
            title,
            path,
            subtitle,
            use_count,
            last_accessed_epoch_secs,
            normalized_title,
            normalized_search_text,
        }
    }

    pub fn with_usage(mut self, use_count: u32, last_accessed_epoch_secs: i64) -> Self {
        self.use_count = use_count;
        self.last_accessed_epoch_secs = last_accessed_epoch_secs;
        self
    }

    pub fn with_subtitle(mut self, subtitle: &str) -> Self {
        self.subtitle = subtitle.to_string();
        self.normalized_search_text =
            normalize_for_search(&format!("{} {} {}", self.title, self.path, self.subtitle));
        self
    }

    pub fn normalized_title(&self) -> &str {
        &self.normalized_title
    }

    pub fn normalized_search_text(&self) -> &str {
        &self.normalized_search_text
    }
}

pub fn normalize_for_search(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}
