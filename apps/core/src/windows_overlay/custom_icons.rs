use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

// ==================== EMBEDDED PNG DATA ====================

const SEARCH_PNG: &[u8] = include_bytes!("../../../assets/icons/search-48.png");
const FOLDER_PNG: &[u8] = include_bytes!("../../../assets/icons/folder-64.png");
const FILE_PNG: &[u8] = include_bytes!("../../../assets/icons/file-64.png");
const IMAGE_PNG: &[u8] = include_bytes!("../../../assets/icons/image-64.png");
const VIDEO_PNG: &[u8] = include_bytes!("../../../assets/icons/video-64.png");
const MUSIC_PNG: &[u8] = include_bytes!("../../../assets/icons/music-64.png");
const DOCUMENT_PNG: &[u8] = include_bytes!("../../../assets/icons/document-64.png");
const CODE_PNG: &[u8] = include_bytes!("../../../assets/icons/code-64.png");
const ARCHIVE_PNG: &[u8] = include_bytes!("../../../assets/icons/archive-folder-64.png");
const PDF_PNG: &[u8] = include_bytes!("../../../assets/icons/pdf-64.png");
const INTERNET_PNG: &[u8] = include_bytes!("../../../assets/icons/internet-64.png");
const SETTINGS_PNG: &[u8] = include_bytes!("../../../assets/icons/settings-64.png");
const SYNC_PNG: &[u8] = include_bytes!("../../../assets/icons/settings-64.png");
const RESTART_PNG: &[u8] = include_bytes!("../../../assets/icons/info-64.png");
const CLIPBOARD_PNG: &[u8] = include_bytes!("../../../assets/icons/clipboard-64.png");

// ==================== FFI ====================

// IUnknown::Release trampoline for IStream cleanup
type ReleaseFn = unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32;

#[link(name = "shlwapi")]
extern "system" {
    fn SHCreateMemStream(pInit: *const u8, cbInit: u32) -> *mut std::ffi::c_void;
}

#[link(name = "gdiplus")]
extern "system" {
    fn GdipLoadImageFromStream(stream: isize, image: *mut isize) -> i32;
    fn GdipDisposeImage(image: isize) -> i32;
}

fn png_to_gdiplus_image(png_data: &[u8]) -> Option<isize> {
    unsafe {
        let stream = SHCreateMemStream(png_data.as_ptr(), png_data.len() as u32);
        if stream.is_null() {
            return None;
        }
        let mut image = 0isize;
        let status = GdipLoadImageFromStream(stream as isize, &mut image);
        // Release IStream (GdipLoadImageFromStream copies data internally)
        let vtbl = stream as *mut *mut usize;
        let release: ReleaseFn = std::mem::transmute(*(*vtbl).add(2));
        release(stream);
        if status == 0 && image != 0 {
            Some(image)
        } else {
            if image != 0 {
                GdipDisposeImage(image);
            }
            None
        }
    }
}

// ==================== CACHE ====================

type ImageCache = HashMap<String, isize>;
static CACHE: LazyLock<Mutex<ImageCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn classify_kind(path: &str) -> &'static str {
    let raw_ext = match path.rfind('.') {
        Some(pos) => &path[pos + 1..],
        None => return "file",
    };
    let ext = raw_ext.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif"
        | "heic" | "heif" | "avif" => "image",
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "m4v" | "mpg" | "mpeg" | "flv" | "3gp"
        | "rm" | "vob" => "video",
        "mp3" | "wav" | "flac" | "m4a" | "ogg" | "wma" | "aac" | "opus" | "aiff" | "alac" => {
            "music"
        }
        "pdf" => "pdf",
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "tbz2" | "cab"
        | "iso" | "dmg" | "lzh" | "arj" => "archive",
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf"
        | "csv" | "tsv" => "document",
        "rs" | "py" | "js" | "ts" | "html" | "css" | "json" | "toml" | "yaml" | "yml" | "xml"
        | "sh" | "bat" | "ps1" | "c" | "cpp" | "h" | "hpp" | "go" | "java" | "kt" | "swift"
        | "rb" | "php" | "pl" | "lua" | "r" | "dart" | "scala" | "sql" | "graphql" | "proto"
        | "tex" | "md" | "rst" | "cmake" | "makefile" | "dockerfile" | "tf" | "conf" | "ini"
        | "cfg" => "code",
        _ => "file",
    }
}

fn kind_png_data(kind: &str) -> Option<&'static [u8]> {
    match kind {
        "folder" => Some(FOLDER_PNG),
        "file" => Some(FILE_PNG),
        "image" => Some(IMAGE_PNG),
        "video" => Some(VIDEO_PNG),
        "music" => Some(MUSIC_PNG),
        "document" => Some(DOCUMENT_PNG),
        "code" => Some(CODE_PNG),
        "archive" => Some(ARCHIVE_PNG),
        "pdf" => Some(PDF_PNG),
        "clipboard" => Some(CLIPBOARD_PNG),
        "search" => Some(SEARCH_PNG),
        _ => None,
    }
}

fn action_png_data(title: &str) -> Option<&'static [u8]> {
    let lower = title.to_ascii_lowercase();
    if lower.contains("clipboard") {
        Some(CLIPBOARD_PNG)
    } else if lower.contains("web") || lower.contains("search") {
        Some(INTERNET_PNG)
    } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer") {
        Some(SETTINGS_PNG)
    } else if lower.contains("restart") || lower.contains("quit") {
        Some(RESTART_PNG)
    } else if lower.contains("rebuild")
        || lower.contains("index")
        || lower.contains("refresh")
        || lower.contains("sync")
    {
        Some(SYNC_PNG)
    } else {
        None
    }
}

fn load_and_cache(key: &str, data: &[u8]) -> Option<isize> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(&handle) = cache.get(key) {
        return if handle == 0 { None } else { Some(handle) };
    }
    let handle = png_to_gdiplus_image(data).unwrap_or(0);
    cache.insert(key.into(), handle);
    if handle == 0 {
        None
    } else {
        Some(handle)
    }
}

pub(crate) fn custom_icon_for_kind(kind: &str) -> Option<isize> {
    let key = format!("kind:{kind}");
    let data = kind_png_data(kind)?;
    load_and_cache(&key, data)
}

pub(crate) fn custom_icon_for_action(title: &str) -> Option<isize> {
    let lower = title.to_ascii_lowercase();
    let cat = if lower.contains("web") || lower.contains("search") {
        "action:web"
    } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer") {
        "action:settings"
    } else if lower.contains("restart") || lower.contains("quit") {
        "action:restart"
    } else if lower.contains("rebuild")
        || lower.contains("index")
        || lower.contains("refresh")
        || lower.contains("sync")
    {
        "action:sync"
    } else {
        return None;
    };
    let data = action_png_data(title)?;
    load_and_cache(cat, data)
}

pub(crate) fn search_icon() -> Option<isize> {
    custom_icon_for_kind("search")
}

pub(crate) fn destroy_all() {
    let mut cache = CACHE.lock().unwrap();
    for (_, &handle) in cache.iter() {
        if handle != 0 {
            unsafe {
                GdipDisposeImage(handle);
            }
        }
    }
    cache.clear();
}
