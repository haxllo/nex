use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

// ==================== EMBEDDED PNG DATA ====================

const SEARCH_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-search-54.png");
const FOLDER_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-archive-folder-72.png");
const FILE_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-file-72.png");
const CINEMA_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-cinema-72.png");
const IMAGE_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-image-file-72.png");
const INTERNET_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-internet-72.png");
const SETTINGS_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-settings-72.png");
const SYNC_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-synchronize-72.png");
const RESTART_PNG: &[u8] = include_bytes!("../../../assets/icons/icons8-restart-72.png");

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
            if image != 0 { GdipDisposeImage(image); }
            None
        }
    }
}

// ==================== CACHE ====================

type ImageCache = HashMap<String, isize>;
static CACHE: LazyLock<Mutex<ImageCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn kind_png_data(kind: &str) -> Option<&'static [u8]> {
    match kind {
        "folder" => Some(FOLDER_PNG),
        "file" => Some(FILE_PNG),
        "video" | "cinema" => Some(CINEMA_PNG),
        "image" | "photo" => Some(IMAGE_PNG),
        "search" => Some(SEARCH_PNG),
        _ => None,
    }
}

fn action_png_data(title: &str) -> Option<&'static [u8]> {
    let lower = title.to_ascii_lowercase();
    if lower.contains("web") || lower.contains("search") {
        Some(INTERNET_PNG)
    } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer") {
        Some(SETTINGS_PNG)
    } else if lower.contains("restart") || lower.contains("quit") {
        Some(RESTART_PNG)
    } else if lower.contains("rebuild") || lower.contains("index") || lower.contains("refresh")
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
    if handle == 0 { None } else { Some(handle) }
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
    } else if lower.contains("rebuild") || lower.contains("index") || lower.contains("refresh")
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
            unsafe { GdipDisposeImage(handle); }
        }
    }
    cache.clear();
}
