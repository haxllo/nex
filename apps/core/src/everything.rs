//! Everything SDK integration for instant file/folder search.
//!
//! Provides [`EverythingSearchProvider`] which implements [`DiscoveryProvider`]
//! using [voidtools Everything](https://www.voidtools.com/) via its SDK DLL.
//!
//! If the Everything DLL cannot be loaded (Everything not installed) the
//! provider silently returns an empty result set. Everything is an opt-in
//! feature controlled by `everything_search_enabled` in config.

#![cfg(target_os = "windows")]

use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use std::path::PathBuf;

use libloading::{Library, Symbol};

use crate::discovery::{DiscoveryProvider, ProviderError};
use crate::model::SearchItem;

// ---------------------------------------------------------------------------
// Everything SDK constants
// ---------------------------------------------------------------------------

const EVERYTHING_REQUEST_FILE_NAME: u32 = 0x0000_0001;
const EVERYTHING_REQUEST_PATH: u32 = 0x0000_0002;
const EVERYTHING_REQUEST_DATE_MODIFIED: u32 = 0x0000_0008;
const EVERYTHING_REQUEST_SIZE: u32 = 0x0000_0010;
const EVERYTHING_REQUEST_ATTRIBUTES: u32 = 0x0000_0040;
const EVERYTHING_REQUEST_DATE_CREATED: u32 = 0x0000_0080;
const EVERYTHING_SORT_DATE_MODIFIED_DESCENDING: u32 = 0x0000_000C;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

const SDK_RESULT_CAP: u32 = 200_000;

// Error codes from Everything SDK
#[allow(dead_code)]
const EVERYTHING_ERROR_OK: u32 = 0;
#[allow(dead_code)]
const EVERYTHING_ERROR_MEMORY: u32 = 1;
#[allow(dead_code)]
const EVERYTHING_ERROR_IPC: u32 = 2;
#[allow(dead_code)]
const EVERYTHING_ERROR_REGISTERCLASSEX: u32 = 3;
#[allow(dead_code)]
const EVERYTHING_ERROR_CREATEWINDOW: u32 = 4;
#[allow(dead_code)]
const EVERYTHING_ERROR_CREATETHREAD: u32 = 5;
#[allow(dead_code)]
const EVERYTHING_ERROR_INVALIDINDEX: u32 = 6;
#[allow(dead_code)]
const EVERYTHING_ERROR_INVALIDCALL: u32 = 7;

// ---------------------------------------------------------------------------
// Everything SDK function type aliases (__stdcall = `extern "system"`)
// ---------------------------------------------------------------------------

type EverythingSetMatchPath = unsafe extern "system" fn(i32);
type EverythingSetSearchW = unsafe extern "system" fn(*const u16);
type EverythingSetRequestFlags = unsafe extern "system" fn(u32);
type EverythingSetMax = unsafe extern "system" fn(u32);
type EverythingSetSort = unsafe extern "system" fn(u32);
type EverythingQueryW = unsafe extern "system" fn(i32) -> i32;
type EverythingGetNumResults = unsafe extern "system" fn() -> u32;
type EverythingGetResultFileNameW = unsafe extern "system" fn(u32) -> *const u16;
type EverythingGetResultPathW = unsafe extern "system" fn(u32) -> *const u16;
type EverythingGetResultAttributes = unsafe extern "system" fn(u32) -> u32;
type EverythingGetLastError = unsafe extern "system" fn() -> u32;
type EverythingReset = unsafe extern "system" fn();

/// Cached Everything SDK function pointers, resolved once from the loaded DLL.
struct EverythingFunctions {
    set_match_path: EverythingSetMatchPath,
    set_search: EverythingSetSearchW,
    set_request_flags: EverythingSetRequestFlags,
    set_max_results: EverythingSetMax,
    set_sort: EverythingSetSort,
    query_fn: EverythingQueryW,
    get_num_results: EverythingGetNumResults,
    get_result_file_name: EverythingGetResultFileNameW,
    get_result_path: EverythingGetResultPathW,
    get_result_attributes: EverythingGetResultAttributes,
    get_last_error: EverythingGetLastError,
    reset_fn: EverythingReset,
}

impl EverythingFunctions {
    unsafe fn resolve(lib: &Library) -> Result<Self, String> {
        Ok(Self {
            set_match_path: *lib.get(b"Everything_SetMatchPath")
                .map_err(|e| format!("{e}"))?,
            set_search: *lib.get(b"Everything_SetSearchW")
                .map_err(|e| format!("{e}"))?,
            set_request_flags: *lib.get(b"Everything_SetRequestFlags")
                .map_err(|e| format!("{e}"))?,
            set_max_results: *lib.get(b"Everything_SetMax")
                .map_err(|e| format!("{e}"))?,
            set_sort: *lib.get(b"Everything_SetSort")
                .map_err(|e| format!("{e}"))?,
            query_fn: *lib.get(b"Everything_QueryW")
                .map_err(|e| format!("{e}"))?,
            get_num_results: *lib.get(b"Everything_GetNumResults")
                .map_err(|e| format!("{e}"))?,
            get_result_file_name: *lib.get(b"Everything_GetResultFileNameW")
                .map_err(|e| format!("{e}"))?,
            get_result_path: *lib.get(b"Everything_GetResultPathW")
                .map_err(|e| format!("{e}"))?,
            get_result_attributes: *lib.get(b"Everything_GetResultAttributes")
                .map_err(|e| format!("{e}"))?,
            get_last_error: *lib.get(b"Everything_GetLastError")
                .map_err(|e| format!("{e}"))?,
            reset_fn: *lib.get(b"Everything_Reset")
                .map_err(|e| format!("{e}"))?,
        })
    }
}

fn everything_functions_cached() -> Result<&'static EverythingFunctions, String> {
    static FUNCS: OnceLock<Result<EverythingFunctions, String>> = OnceLock::new();
    match FUNCS.get_or_init(|| {
        let lib = load_everything_dll_cached()
            .ok_or_else(|| "Everything DLL not loaded".to_string())?;
        unsafe { EverythingFunctions::resolve(lib) }
    }) {
        Ok(funcs) => Ok(funcs),
        Err(e) => Err(e.clone()),
    }
}

// ---------------------------------------------------------------------------
// EverythingSearchProvider
// ---------------------------------------------------------------------------

/// A [`DiscoveryProvider`] backed by the Everything search engine.
///
/// Uses the Everything SDK DLL (`Everything64.dll` or `Everything32.dll`) to
/// query the Everything index for files and folders under the configured
/// discovery roots. Results are reported as `SearchItem` entries with kind
/// `"file"` or `"folder"`.
pub struct EverythingSearchProvider {
    search_roots: Vec<PathBuf>,
    excluded_root_entries: Vec<String>,
    show_files: bool,
    show_folders: bool,
}

impl EverythingSearchProvider {
    pub fn new(
        search_roots: Vec<PathBuf>,
        exclude_roots: &[PathBuf],
        show_files: bool,
        show_folders: bool,
    ) -> Self {
        // Pre-normalize excluded roots for fast path matching during filtering.
        let excluded_root_entries: Vec<String> = exclude_roots
            .iter()
            .filter_map(|root| {
                let normalized = root.to_string_lossy().replace('/', "\\");
                if normalized.trim().is_empty() {
                    None
                } else {
                    Some(normalized.trim().to_ascii_lowercase())
                }
            })
            .collect();

        Self {
            search_roots,
            excluded_root_entries,
            show_files,
            show_folders,
        }
    }

    /// Returns `true` when `path` (already lowercased) lives under any
    /// configured exclusion root.
    fn is_excluded(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        self.excluded_root_entries.iter().any(|root| {
            lower == *root
                || (lower.starts_with(root)
                    && (root.ends_with('\\') || lower[root.len()..].starts_with('\\')))
        })
    }

    /// Execute an Everything SDK query after resolving all function pointers.
    ///
    /// Returns `Ok(items)` on success or `Err(msg)` explaining why the query
    /// could not be performed. Errors from this method are treated as
    /// transient / environment-specific and never propagate as [`ProviderError`].
    unsafe fn try_query(&self, lib: &Library) -> Result<Vec<SearchItem>, String> {
        // -------------------------------------------------------------------
        // Resolve all required function pointers
        // -------------------------------------------------------------------
        let set_search: Symbol<EverythingSetSearchW> =
            resolve_symbol(lib, b"Everything_SetSearchW")?;
        let set_match_path: Symbol<EverythingSetMatchPath> =
            resolve_symbol(lib, b"Everything_SetMatchPath")?;
        let set_request_flags: Symbol<EverythingSetRequestFlags> =
            resolve_symbol(lib, b"Everything_SetRequestFlags")?;
        let set_max_results: Symbol<EverythingSetMax> =
            resolve_symbol(lib, b"Everything_SetMax")?;
        let set_sort: Symbol<EverythingSetSort> = resolve_symbol(lib, b"Everything_SetSort")?;
        let query_fn: Symbol<EverythingQueryW> = resolve_symbol(lib, b"Everything_QueryW")?;
        let get_num_results: Symbol<EverythingGetNumResults> =
            resolve_symbol(lib, b"Everything_GetNumResults")?;
        let get_result_file_name: Symbol<EverythingGetResultFileNameW> =
            resolve_symbol(lib, b"Everything_GetResultFileNameW")?;
        let get_result_path: Symbol<EverythingGetResultPathW> =
            resolve_symbol(lib, b"Everything_GetResultPathW")?;
        let get_result_attributes: Symbol<EverythingGetResultAttributes> =
            resolve_symbol(lib, b"Everything_GetResultAttributes")?;
        let get_last_error: Symbol<EverythingGetLastError> =
            resolve_symbol(lib, b"Everything_GetLastError")?;
        let reset_fn: Symbol<EverythingReset> = resolve_symbol(lib, b"Everything_Reset")?;

        // -------------------------------------------------------------------
        // Configure search
        // -------------------------------------------------------------------
        set_request_flags(
            EVERYTHING_REQUEST_FILE_NAME
                | EVERYTHING_REQUEST_PATH
                | EVERYTHING_REQUEST_DATE_MODIFIED
                | EVERYTHING_REQUEST_SIZE
                | EVERYTHING_REQUEST_ATTRIBUTES
                | EVERYTHING_REQUEST_DATE_CREATED,
        );
        set_max_results(SDK_RESULT_CAP);
        set_sort(EVERYTHING_SORT_DATE_MODIFIED_DESCENDING);
        set_match_path(1); // match against full path for bare quoted path filters

        // Push root-path filtering into the Everything query so the SDK
        // returns far fewer results than the full 200K cap.
        // e.g.  "C:\Users\Admin" | "D:\Projects"
        let search_str = build_root_filter_query(&self.search_roots);
        let search_wide = to_wide_everything(&search_str);
        set_search(search_wide.as_ptr());

        // -------------------------------------------------------------------
        // Execute query (bWait = TRUE)
        // -------------------------------------------------------------------
        if query_fn(1) == 0 {
            let code = get_last_error();
            let msg = everything_error_label(code);
            crate::logging::warn(&format!(
                "[nex] everything_query_failed error=\"{msg}\" code={code}"
            ));
            reset_fn();
            return Ok(Vec::new());
        }

        let total_results = get_num_results();
        if total_results == 0 {
            crate::logging::info("[nex] everything_query returned 0 results");
            reset_fn();
            return Ok(Vec::new());
        }

        // -------------------------------------------------------------------
        // Normalise search roots for client-side filtering
        // -------------------------------------------------------------------
        let normalized_roots: Vec<String> = self
            .search_roots
            .iter()
            .map(|root| {
                root.to_string_lossy()
                    .replace('/', "\\")
                    .to_ascii_lowercase()
            })
            .collect();

        // -------------------------------------------------------------------
        // Iterate results, filter by roots and exclusions
        // -------------------------------------------------------------------
        let mut items: Vec<SearchItem> = Vec::new();
        for i in 0..total_results {
            let name_ptr = get_result_file_name(i);
            let path_ptr = get_result_path(i);

            if name_ptr.is_null() || path_ptr.is_null() {
                continue;
            }

            let name = wide_to_string(name_ptr);
            let full_path = wide_to_string(path_ptr);

            if name.is_empty() || full_path.is_empty() {
                continue;
            }

            let lower_path = full_path.to_ascii_lowercase();

            // Must be under at least one configured search root.
            let under_root = normalized_roots.iter().any(|root| {
                lower_path == *root
                    || (lower_path.starts_with(root)
                        && (root.ends_with('\\') || lower_path[root.len()..].starts_with('\\')))
            });
            if !under_root {
                continue;
            }

            // Check exclusion policy.
            if self.is_excluded(&lower_path) {
                continue;
            }

            // Determine kind from file attributes.
            let attrs = get_result_attributes(i);
            let is_directory = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;

            if is_directory && !self.show_folders {
                continue;
            }
            if !is_directory && !self.show_files {
                continue;
            }

            let kind = if is_directory { "folder" } else { "file" };
            let id = format!("{kind}:{full_path}");

            items.push(SearchItem::new(&id, kind, &name, &full_path));
        }

        // Clean up Everything internal state.
        reset_fn();

        crate::logging::info(&format!(
            "[nex] everything_discovery provider=everything total={} filtered={} roots={}",
            total_results,
            items.len(),
            normalized_roots.len(),
        ));

        Ok(items)
    }
}

impl DiscoveryProvider for EverythingSearchProvider {
    fn provider_name(&self) -> &'static str {
        "everything"
    }

    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError> {
        if !self.show_files && !self.show_folders {
            return Ok(Vec::new());
        }

        if self.search_roots.is_empty() {
            return Ok(Vec::new());
        }

        // -----------------------------------------------------------------------
        // Load the Everything SDK DLL
        // -----------------------------------------------------------------------
        let lib = match load_everything_dll() {
            Ok(lib) => lib,
            Err(load_msg) => {
                crate::logging::info(&format!("[nex] everything_sdk load={load_msg}"));
                return Ok(Vec::new());
            }
        };

        // -----------------------------------------------------------------------
        // Try to run the Everything query. If the DLL can't provide the expected
        // symbols (e.g. it was loaded but is not the SDK version), log and
        // gracefully return empty rather than failing the entire index rebuild.
        // -----------------------------------------------------------------------
        match unsafe { self.try_query(&lib) } {
            Ok(items) => Ok(items),
            Err(msg) => {
                crate::logging::warn(&format!("[nex] everything_api {msg}"));
                Ok(Vec::new())
            }
        }
    }

    /// Everything's index is real-time, so we always return `None` to ensure
    /// every re-index cycle re-queries without relying on a stale stamp.
    fn change_stamp(&self) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Try to load the Everything SDK DLL from well-known locations.
///
/// Looks for `Everything64.dll` first (64-bit), then `Everything32.dll`.
///
/// The hot-path caller should prefer [`load_everything_dll_cached`] which
/// caches the result after the first attempt.
fn load_everything_dll() -> Result<Library, String> {
    // Attempt 1: standard DLL name — may be found via PATH / app-local.
    if let Ok(lib) = unsafe { Library::new("Everything64.dll") } {
        return Ok(lib);
    }

    // Attempt 2: 32-bit fallback.
    if let Ok(lib) = unsafe { Library::new("Everything32.dll") } {
        return Ok(lib);
    }

    // Attempt 3: check next to the running executable (bundled DLL).
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            for name in &["Everything64.dll", "Everything32.dll"] {
                let try_path = exe_dir.join(name);
                if let Some(s) = try_path.to_str() {
                    if let Ok(lib) = unsafe { Library::new(s) } {
                        return Ok(lib);
                    }
                }
            }
        }
    }

    // Attempt 4: detect Everything install path from registry.
    if let Some(install_dir) = everything_registry_dir() {
        for path in &[
            format!(r"{install_dir}\Everything64.dll"),
            format!(r"{install_dir}\Everything32.dll"),
            format!(r"{install_dir}\dll\Everything64.dll"),
            format!(r"{install_dir}\dll\Everything32.dll"),
        ] {
            if let Ok(lib) = unsafe { Library::new(path.as_str()) } {
                return Ok(lib);
            }
        }
    }

    // Attempt 5: search common install directories and the SDK dll/ subdirectory.
    for base in &[
        r"C:\Program Files\Everything",
        r"C:\Program Files (x86)\Everything",
    ] {
        for path in &[
            format!(r"{base}\Everything64.dll"),
            format!(r"{base}\Everything32.dll"),
            format!(r"{base}\dll\Everything64.dll"),
            format!(r"{base}\dll\Everything32.dll"),
        ] {
            if let Ok(lib) = unsafe { Library::new(path.as_str()) } {
                return Ok(lib);
            }
        }
    }

    Err("Everything64.dll not found in PATH, app directory, or common install directories".into())
}

/// Cached variant used by the hot path ([`live_everything_search`]).
///
/// Only attempts to load the DLL once; subsequent calls return the cached
/// result (success or failure) instantly, avoiding expensive LoadLibrary /
/// registry / filesystem checks on every keystroke.
fn load_everything_dll_cached() -> Option<&'static Library> {
    static LOADED: OnceLock<Result<Library, String>> = OnceLock::new();
    match LOADED.get_or_init(load_everything_dll) {
        Ok(lib) => Some(lib),
        Err(_) => None,
    }
}

/// Safely resolve a symbol from the loaded library.
unsafe fn resolve_symbol<'a, T>(lib: &'a Library, name: &'a [u8]) -> Result<Symbol<'a, T>, String> {
    lib.get(name)
        .map_err(|e| format!("failed to resolve {}: {e}", String::from_utf8_lossy(name)))
}

/// Convert a null-terminated UTF-16 (WideChar) string pointer to a Rust String.
unsafe fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    if len == 0 {
        return String::new();
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    String::from_utf16_lossy(slice)
}

/// Look up the Everything install directory from the Windows registry.
///
/// Checks `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\Everything.exe`
/// which is set by the Everything installer and points to `Everything.exe`.
fn everything_registry_dir() -> Option<String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
    };

    const APP_PATHS: &[u16] = &[
        'S' as u16,
        'O' as u16,
        'F' as u16,
        'T' as u16,
        'W' as u16,
        'A' as u16,
        'R' as u16,
        'E' as u16,
        '\\' as u16,
        'M' as u16,
        'i' as u16,
        'c' as u16,
        'r' as u16,
        'o' as u16,
        's' as u16,
        'o' as u16,
        'f' as u16,
        't' as u16,
        '\\' as u16,
        'W' as u16,
        'i' as u16,
        'n' as u16,
        'd' as u16,
        'o' as u16,
        'w' as u16,
        's' as u16,
        '\\' as u16,
        'C' as u16,
        'u' as u16,
        'r' as u16,
        'r' as u16,
        'e' as u16,
        'n' as u16,
        't' as u16,
        'V' as u16,
        'e' as u16,
        'r' as u16,
        's' as u16,
        'i' as u16,
        'o' as u16,
        'n' as u16,
        '\\' as u16,
        'A' as u16,
        'p' as u16,
        'p' as u16,
        ' ' as u16,
        'P' as u16,
        'a' as u16,
        't' as u16,
        'h' as u16,
        's' as u16,
        '\\' as u16,
        'E' as u16,
        'v' as u16,
        'e' as u16,
        'r' as u16,
        'y' as u16,
        't' as u16,
        'h' as u16,
        'i' as u16,
        'n' as u16,
        'g' as u16,
        '.' as u16,
        'e' as u16,
        'x' as u16,
        'e' as u16,
        0,
    ];

    unsafe {
        let mut key = null_mut();
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            APP_PATHS.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return None;
        }

        let mut buf = [0u16; 520];
        let mut len = (buf.len() * 2) as u32;
        let result = RegGetValueW(
            key,
            std::ptr::null(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut len,
        );
        RegCloseKey(key);

        if result != ERROR_SUCCESS {
            return None;
        }

        let null_pos = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let path = String::from_utf16_lossy(&buf[..null_pos]);
        if path.is_empty() {
            return None;
        }

        let path = path.replace('/', "\\");
        if let Some(last_backslash) = path.rfind('\\') {
            Some(path[..last_backslash].to_string())
        } else {
            Some(path)
        }
    }
}

/// Probe whether the Everything SDK DLL can be loaded.
///
/// Returns `Ok(true)` if the DLL loads or `Ok(false)` with a description
/// explaining why.
pub fn probe_everything_sdk() -> Result<bool, String> {
    match load_everything_dll() {
        Ok(_) => Ok(true),
        Err(msg) => {
            // The DLL wasn't found. Check if Everything is installed via the
            // service or registry as a helpful diagnostic.
            let install_hint = check_everything_installed();
            Err(format!("{msg}. {install_hint}"))
        }
    }
}

/// Quick check: is the Everything service/process running?
fn check_everything_installed() -> &'static str {
    let common_paths = [
        r"C:\Program Files\Everything\Everything.exe",
        r"C:\Program Files (x86)\Everything\Everything.exe",
    ];
    if common_paths
        .iter()
        .any(|p| std::path::Path::new(p).exists())
    {
        "Everything is installed but the SDK DLL was not found. Download Everything-SDK.zip from https://www.voidtools.com/support/everything/sdk/ and extract Everything64.dll (or Everything32.dll) into the Nex install directory or next to Everything.exe. Keep Everything.exe running in the background."
    } else {
        "Everything does not appear to be installed. Download from https://www.voidtools.com/. After installing, also download Everything-SDK.zip from https://www.voidtools.com/support/everything/sdk/ and extract Everything64.dll next to Everything.exe."
    }
}

/// Perform a live Everything SDK search at query time.
///
/// Returns matched [`SearchItem`] results filtered to the configured discovery
/// roots. Returns `None` when the Everything DLL could not be loaded (caller
/// should fall back to the indexed cache).
///
/// Unlike [`EverythingSearchProvider::discover`] which uses a wildcard query,
/// this function passes the user's text directly to Everything's search engine
/// for instant ranked results.
pub fn live_everything_search(
    query: &str,
    roots: &[PathBuf],
    exclude_roots: &[PathBuf],
    show_files: bool,
    show_folders: bool,
    max_results: u32,
) -> Option<Vec<SearchItem>> {
    let trimmed = query.trim();
    if trimmed.is_empty() || roots.is_empty() || (!show_files && !show_folders) {
        return None;
    }

    let funcs = match everything_functions_cached() {
        Ok(funcs) => funcs,
        Err(_) => return None,
    };

    let excluded: Vec<String> = exclude_roots
        .iter()
        .filter_map(|root| {
            let normalized = root.to_string_lossy().replace('/', "\\");
            if normalized.trim().is_empty() {
                None
            } else {
                Some(normalized.trim().to_ascii_lowercase())
            }
        })
        .collect();

    // Run Everything query on a background thread with a timeout to avoid
    // freezing the UI thread if Everything's IPC is slow or hangs.
    let (tx, rx) = mpsc::channel();
    let query_owned = trimmed.to_string();
    let roots_owned: Vec<PathBuf> = roots.to_vec();
    let excluded_owned = excluded.clone();

    // funcs is `&'static EverythingFunctions` — safe to pass to a thread
    std::thread::spawn(move || {
        let result = unsafe {
            try_live_query(
                funcs,
                &query_owned,
                &roots_owned,
                &excluded_owned,
                show_files,
                show_folders,
                max_results,
            )
        };
        let _ = tx.send(result);
    });

    // Try non-blocking first — Everything usually responds in <1ms once IPC is warm.
    if let Ok(Ok(items)) = rx.try_recv() {
        return Some(items);
    }
    // Give a short window for cold-start IPC or slow queries.
    match rx.recv_timeout(Duration::from_millis(50)) {
        Ok(Ok(items)) => Some(items),
        Ok(Err(msg)) => {
            crate::logging::warn(&format!("[nex] everything_live_query {msg}"));
            None
        }
        Err(_) => {
            crate::logging::warn("[nex] everything_live_query timed out after 500ms");
            None
        }
    }
}

/// Internal: execute an Everything SDK query with the given search text and
/// filters, using the cached function pointers from `everything_functions_cached()`.
unsafe fn try_live_query(
    funcs: &EverythingFunctions,
    query: &str,
    roots: &[PathBuf],
    excluded: &[String],
    show_files: bool,
    show_folders: bool,
    max_results: u32,
) -> Result<Vec<SearchItem>, String> {
    // Build combined query: user_text + root filter
    // e.g.  needle "C:\Users\Admin" | "D:\Projects"
    let root_filter = build_root_filter_query(roots);
    let search_str = if root_filter.is_empty() {
        query.to_string()
    } else {
        format!("{} {}", query, root_filter)
    };
    let search_wide = to_wide_everything(&search_str);

    (funcs.set_request_flags)(
        EVERYTHING_REQUEST_FILE_NAME
            | EVERYTHING_REQUEST_PATH
            | EVERYTHING_REQUEST_DATE_MODIFIED
            | EVERYTHING_REQUEST_SIZE
            | EVERYTHING_REQUEST_ATTRIBUTES
            | EVERYTHING_REQUEST_DATE_CREATED,
    );
    (funcs.set_max_results)(max_results);
    (funcs.set_sort)(EVERYTHING_SORT_DATE_MODIFIED_DESCENDING);
    (funcs.set_match_path)(1); // match against full path for bare quoted path filters
    (funcs.set_search)(search_wide.as_ptr());

    if (funcs.query_fn)(1) == 0 {
        let code = (funcs.get_last_error)();
        let msg = everything_error_label(code);
        (funcs.reset_fn)();
        return Err(format!("Everything_QueryW failed: {msg} (code={code})"));
    }

    let total_results = (funcs.get_num_results)();
    if total_results == 0 {
        (funcs.reset_fn)();
        return Ok(Vec::new());
    }

    // Normalize roots for client-side filtering
    let normalized_roots: Vec<String> = roots
        .iter()
        .map(|root| {
            root.to_string_lossy()
                .replace('/', "\\")
                .to_ascii_lowercase()
        })
        .collect();

    let limit = max_results.min(total_results);
    let mut items: Vec<SearchItem> = Vec::new();

    for i in 0..limit {
        let name_ptr = (funcs.get_result_file_name)(i);
        let path_ptr = (funcs.get_result_path)(i);
        if name_ptr.is_null() || path_ptr.is_null() {
            continue;
        }

        let name = wide_to_string(name_ptr);
        let full_path = wide_to_string(path_ptr);
        if name.is_empty() || full_path.is_empty() {
            continue;
        }

        let lower_path = full_path.to_ascii_lowercase();

        // Must be under at least one configured search root
        let under_root = normalized_roots.iter().any(|root| {
            lower_path == *root
                || (lower_path.starts_with(root)
                    && (root.ends_with('\\') || lower_path[root.len()..].starts_with('\\')))
        });
        if !under_root {
            continue;
        }

        // Check exclusion policy
        if path_is_excluded(&lower_path, excluded) {
            continue;
        }

        let attrs = (funcs.get_result_attributes)(i);
        let is_directory = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;

        if is_directory && !show_folders {
            continue;
        }
        if !is_directory && !show_files {
            continue;
        }

        let kind = if is_directory { "folder" } else { "file" };
        let id = format!("{kind}:{full_path}");
        items.push(SearchItem::new(&id, kind, &name, &full_path));
    }

    (funcs.reset_fn)();
    Ok(items)
}

/// Returns `true` when `lower_path` lives under any configured exclusion root.
fn path_is_excluded(lower_path: &str, excluded: &[String]) -> bool {
    excluded.iter().any(|root| {
        lower_path == *root
            || (lower_path.starts_with(root)
                && (root.ends_with('\\') || lower_path[root.len()..].starts_with('\\')))
    })
}

/// Return a human-readable label for an Everything error code.
fn everything_error_label(code: u32) -> &'static str {
    match code {
        EVERYTHING_ERROR_OK => "ok",
        EVERYTHING_ERROR_MEMORY => "out_of_memory",
        EVERYTHING_ERROR_IPC => "ipc_failed",
        EVERYTHING_ERROR_REGISTERCLASSEX => "register_class_ex_failed",
        EVERYTHING_ERROR_CREATEWINDOW => "create_window_failed",
        EVERYTHING_ERROR_CREATETHREAD => "create_thread_failed",
        EVERYTHING_ERROR_INVALIDINDEX => "invalid_index",
        EVERYTHING_ERROR_INVALIDCALL => "invalid_call",
        _ => "unknown",
    }
}

/// Build an Everything search query that restricts results to the given
/// root directories using recursive path matching. Returns an empty string
/// (unrestricted query) when no roots are configured, which causes
/// Everything to return its full index.
///
/// Uses bare quoted paths instead of `parent:` because `parent:` is
/// non-recursive — it only returns immediate children. Bare paths match
/// files at any depth under the root.
///
/// Example:  "C:\Users\Admin" | "D:\Projects"
fn build_root_filter_query(roots: &[PathBuf]) -> String {
    if roots.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    for root in roots {
        let normalized = root.to_string_lossy().replace('/', "\\");
        // Skip malformed or empty roots
        if normalized.trim().is_empty() {
            continue;
        }
        // Escape any embedded quotes in the path
        let escaped = normalized.trim().replace('"', "\"\"");
        parts.push(format!("\"{escaped}\""));
    }
    if parts.is_empty() {
        return String::new();
    }
    parts.join(" | ")
}

/// Convert a UTF-8 string to a null-terminated UTF-16 wide string.
fn to_wide_everything(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn provider_name_is_everything() {
        let provider = EverythingSearchProvider::new(vec![PathBuf::from(r"C:\")], &[], true, true);
        assert_eq!(provider.provider_name(), "everything");
    }

    #[test]
    fn empty_discovery_roots_returns_no_items_without_dll() {
        let provider = EverythingSearchProvider::new(vec![], &[], true, true);
        // Without the DLL, discover silently returns empty vec
        let results = provider.discover().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn is_excluded_matches_exact_root() {
        let provider = EverythingSearchProvider::new(
            vec![PathBuf::from(r"C:\Users")],
            &[PathBuf::from(r"C:\Users\Admin\AppData")],
            true,
            false,
        );
        assert!(provider.is_excluded(r"c:\users\admin\appdata"));
        assert!(provider.is_excluded(r"C:\Users\Admin\AppData\Local\Temp"));
    }

    #[test]
    fn is_excluded_rejects_non_matching_paths() {
        let provider = EverythingSearchProvider::new(
            vec![PathBuf::from(r"C:\Users")],
            &[PathBuf::from(r"C:\Users\Admin\AppData")],
            true,
            false,
        );
        assert!(!provider.is_excluded(r"C:\Users\Admin\Documents"));
        assert!(!provider.is_excluded(r"C:\Users\Public"));
    }

    #[test]
    fn is_excluded_without_exclusions_never_excludes() {
        let provider = EverythingSearchProvider::new(vec![PathBuf::from(r"C:\")], &[], true, true);
        assert!(!provider.is_excluded(r"C:\Windows"));
        assert!(!provider.is_excluded(r"C:\Users"));
    }

    #[test]
    fn no_items_when_both_show_files_and_show_folders_false() {
        let provider =
            EverythingSearchProvider::new(vec![PathBuf::from(r"C:\")], &[], false, false);
        // Early return without loading DLL
        let results = provider.discover().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn is_excluded_matches_subdirectory_deeply() {
        let provider = EverythingSearchProvider::new(
            vec![PathBuf::from(r"D:\Data")],
            &[PathBuf::from(r"D:\Data\Temp")],
            true,
            true,
        );
        assert!(provider.is_excluded(r"d:\data\temp\sub\nested\file.txt"));
        assert!(!provider.is_excluded(r"D:\Data\Work\project"));
    }

    #[test]
    fn normalized_roots_use_backslash() {
        let provider = EverythingSearchProvider::new(
            vec![PathBuf::from("C:/Users")],
            &[PathBuf::from("C:/Users/Admin/AppData")],
            true,
            true,
        );
        // Forward-slash inputs get normalized to backslash during construction
        assert!(provider.is_excluded(r"C:\Users\Admin\AppData\Local"));
    }

    #[test]
    fn everything_error_label_returns_known_labels() {
        assert_eq!(everything_error_label(0), "ok");
        assert_eq!(everything_error_label(1), "out_of_memory");
        assert_eq!(everything_error_label(2), "ipc_failed");
        assert_eq!(everything_error_label(3), "register_class_ex_failed");
        assert_eq!(everything_error_label(4), "create_window_failed");
        assert_eq!(everything_error_label(5), "create_thread_failed");
        assert_eq!(everything_error_label(6), "invalid_index");
        assert_eq!(everything_error_label(7), "invalid_call");
        assert_eq!(everything_error_label(99), "unknown");
    }
}
