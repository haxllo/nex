use crate::config::WebBookmark;
use crate::model::{normalize_for_search, SearchItem};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const BOOKMARK_KIND: &str = "bookmark";

pub(crate) fn display_title(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.trim().to_string());
    host.trim_start_matches("www.")
        .split('.')
        .next()
        .unwrap_or(url)
        .to_string()
        .chars()
        .enumerate()
        .map(|(index, ch)| if index == 0 { ch.to_ascii_uppercase() } else { ch })
        .collect()
}

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

#[cfg(target_os = "windows")]
pub(crate) fn favicon_cache_path(url: &str) -> PathBuf {
    let hash = xxhash_rust::xxh3::xxh3_64(url.as_bytes());
    crate::config::stable_app_data_dir().join("bookmark-icons").join(format!("{hash:016x}.png"))
}

pub(crate) fn favicon_url(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|_| "invalid bookmark URL".to_string())?;
    parsed
        .join("/favicon.ico")
        .map(|icon_url| icon_url.into())
        .map_err(|_| "bookmark URL has no favicon endpoint".to_string())
}

#[cfg(target_os = "windows")]
pub(crate) fn download_favicon(url: &str) -> Result<PathBuf, String> {
    let icon_url = favicon_url(url)?;
    let path = favicon_cache_path(url);
    if path.is_file() {
        return Ok(path);
    }
    let response = ureq::get(&icon_url)
        .set("Accept", "image/ico,image/png,image/*;q=0.8")
        .set("User-Agent", "Nex/2")
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .map_err(|e| format!("favicon request failed: {e}"))?;
    if response.status() != 200 {
        return Err(format!("favicon request returned {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(512 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("favicon read failed: {e}"))?;
    if bytes.is_empty() || bytes.len() >= 512 * 1024 {
        return Err("favicon response is empty or too large".into());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("favicon cache dir failed: {e}"))?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).map_err(|e| format!("favicon cache write failed: {e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("favicon cache replace failed: {e}"))?;
    Ok(path)
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

    #[test]
    fn favicon_endpoint_preserves_the_site_origin() {
        assert_eq!(
            favicon_url("https://example.com:8443/a/page?query=value").unwrap(),
            "https://example.com:8443/favicon.ico"
        );
    }

    #[test]
    fn bookmarks_reject_urls_with_embedded_credentials() {
        assert!(WebBookmark::new("Example", "https://user:password@example.com").is_err());
    }
}
