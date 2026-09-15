use crate::config::WebBookmark;
use crate::model::{normalize_for_search, SearchItem};

pub(crate) const BOOKMARK_KIND: &str = "bookmark";

pub(crate) fn bookmark_id(url: &str) -> String {
    format!("bookmark:{}", normalize_for_search(url))
}

pub(crate) fn search_bookmarks(bookmarks: &[WebBookmark], query: &str, limit: usize) -> Vec<SearchItem> {
    if limit == 0 {
        return Vec::new();
    }
    let normalized = normalize_for_search(query.trim());
    bookmarks
        .iter()
        .filter(|bookmark| {
            normalized.is_empty()
                || normalize_for_search(&bookmark.title).contains(&normalized)
                || normalize_for_search(&bookmark.url).contains(&normalized)
        })
        .take(limit)
        .map(|bookmark| {
            SearchItem::new(
                &bookmark_id(&bookmark.url),
                BOOKMARK_KIND,
                &bookmark.title,
                &bookmark.url,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_ids_are_stable() {
        assert_eq!(bookmark_id("https://Example.com/"), bookmark_id("https://example.com"));
    }

    #[test]
    fn searches_title_and_url() {
        let bookmarks = vec![WebBookmark::new("Nex", "https://example.com").unwrap()];
        assert_eq!(search_bookmarks(&bookmarks, "nex", 10).len(), 1);
        assert_eq!(search_bookmarks(&bookmarks, "example", 10).len(), 1);
    }
}
