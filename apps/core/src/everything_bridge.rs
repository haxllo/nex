#![allow(dead_code)]

use std::ffi::CString;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};

use crate::discovery::{DiscoveryExclusionPolicy};
use crate::logging;
use crate::model::SearchItem;

const EVERYTHING_REQUEST_FILE_NAME: u32 = 0x01;
const EVERYTHING_REQUEST_PATH: u32 = 0x02;

/// The voidtools Everything SDK stores all query/result state in
/// process-global mutable storage (`Everything_SetSearchW`,
/// `Everything_QueryW`, `Everything_GetResultFullPathNameW`, ...).
/// Any two `EverythingBridge` instances — and there can be more than
/// one, e.g. the startup indexer thread and the background refresh
/// thread each build their own `CoreService` — share that global
/// state, so concurrent SDK calls corrupt each other's results.
///
/// This mutex serializes every SDK touch across all bridges in the
/// process, which is what makes the `Send + Sync` impl below sound.
static SDK_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct EverythingBridge {
    lib: HMODULE,
    fns: EverythingFns,
}

// SAFETY: `lib` (HMODULE) and `fns` (function pointers) are themselves
// plain immutable data and safe to share across threads. The dangerous
// part — the SDK's process-global mutable state — is guarded by
// `SDK_LOCK`, which every method acquires before calling any SDK
// function. So a shared `&EverythingBridge` cannot race.
unsafe impl Send for EverythingBridge {}
unsafe impl Sync for EverythingBridge {}

impl Drop for EverythingBridge {
    fn drop(&mut self) {
        if !self.lib.is_null() {
            unsafe { FreeLibrary(self.lib); }
        }
    }
}

#[derive(Clone, Copy)]
struct EverythingFns {
    set_search_w: unsafe extern "system" fn(*const u16),
    set_match_path: unsafe extern "system" fn(i32),
    set_max: unsafe extern "system" fn(u32),
    set_sort: unsafe extern "system" fn(u32),
    set_request_flags: unsafe extern "system" fn(u32),
    query_w: unsafe extern "system" fn(i32) -> i32,
    get_num_results: unsafe extern "system" fn() -> u32,
    get_result_full_path_name_w: unsafe extern "system" fn(u32, *mut u16, u32) -> u32,
    is_folder_result: unsafe extern "system" fn(u32) -> i32,
    get_major_version: unsafe extern "system" fn() -> u32,
}

impl EverythingBridge {
    pub(crate) fn detect() -> Option<Self> {
        let mut attempts: Vec<String> = Vec::new();

        // Probe absolute candidate paths first (registry + Program Files
        // + tool dirs). Only if none resolve do we fall back to a
        // search-order load — and even then via LoadLibraryExW with
        // LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, which restricts the search
        // to safe system directories rather than the unsafe default
        // order (cwd, application dir, ...). This avoids the DLL
        // planting surface of a bare `LoadLibraryW("Everything64.dll")`.
        for path in candidate_dll_paths() {
            attempts.push(format!("{}", path.display()));
            if let Some(bridge) = try_load_dll(&path) {
                let major = unsafe { (bridge.fns.get_major_version)() };
                logging::info(&format!(
                    "[nex] everything_bridge detected version={major} path={} (probed location)",
                    path.display()
                ));
                return Some(bridge);
            }
        }

        for dll_name in &["Everything64.dll", "Everything32.dll"] {
            let wide = to_wide(dll_name);
            let lib = unsafe {
                LoadLibraryExW(
                    wide.as_ptr(),
                    core::ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )
            };
            attempts.push(format!("LoadLibraryExW({dll_name}, SEARCH_DEFAULT_DIRS)"));
            if lib.is_null() {
                continue;
            }
            if let Some(fns) = unsafe { load_functions(lib) } {
                let major = unsafe { (fns.get_major_version)() };
                logging::info(&format!(
                    "[nex] everything_bridge detected version={major} dll={dll_name} (search path)"
                ));
                return Some(Self { lib, fns });
            }
            unsafe { FreeLibrary(lib); }
        }

        logging::info(&format!(
            "[nex] everything_bridge not available (tried {} candidate location(s); set file_discovery_backend=\"walkdir\" to skip)",
            attempts.len()
        ));
        None
    }

    /// Probe whether the Everything service (Everything.exe) is actually
    /// running. The DLL is always loadable as long as it exists on disk, but
    /// IPC calls into it succeed only when Everything.exe is alive.
    ///
    /// Heuristic: `Everything_GetMajorVersion()` returns 0 when the service
    /// is not running (no IPC response) and a non-zero version when it is.
    /// We also send a no-op `Everything_QueryW("")` and check whether the
    /// SDK accepted it; some versions return 0 for both. The version probe
    /// is the strongest signal on its own.
    pub(crate) fn is_service_running(&self) -> bool {
        // Hold SDK_LOCK for the whole probe so the no-op query cannot
        // clobber an in-flight discover() on another thread.
        let _guard = SDK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let major = unsafe { (self.fns.get_major_version)() };
        if major == 0 {
            return false;
        }
        // Belt-and-braces: also try a no-op query. If both look healthy, the
        // service is up. If only the query fails we still consider it down
        // because the IPC channel is unreliable.
        unsafe {
            (self.fns.set_search_w)(to_wide("").as_ptr());
            let ok = (self.fns.query_w)(0);
            ok != 0
        }
    }

    pub(crate) fn discover(
        &self,
        roots: &[PathBuf],
        exclusion: &DiscoveryExclusionPolicy,
        max_depth: usize,
        show_files: bool,
        show_folders: bool,
        max_items_total: usize,
        max_items_per_root: usize,
    ) -> Result<Vec<SearchItem>, crate::discovery::ProviderError> {
        // The Everything SDK uses process-global query/result state.
        // Serialize the entire discover() so a concurrent
        // is_service_running() or a second discover() on another bridge
        // can't interleave and corrupt the result set.
        let _guard = SDK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            (self.fns.set_match_path)(1);
            (self.fns.set_sort)(1);
            (self.fns.set_request_flags)(
                EVERYTHING_REQUEST_FILE_NAME | EVERYTHING_REQUEST_PATH,
            );
        }

        let empty = to_wide("");
        unsafe { (self.fns.set_search_w)(empty.as_ptr()); }

        let ok = unsafe { (self.fns.query_w)(1) };
        if ok == 0 {
            return Err(crate::discovery::ProviderError::new(format!(
                "Everything_QueryW failed: {}",
                unsafe { GetLastError() }
            )));
        }

        let count = unsafe { (self.fns.get_num_results)() } as usize;
        if count == 0 {
            return Ok(Vec::new());
        }

        let total_budget = max_items_total.max(1);
        let per_root_budget = max_items_per_root.max(1).min(total_budget);
        let mut out = Vec::with_capacity(total_budget.min(count));
        let mut total_added = 0_usize;
        let mut skipped_excluded = 0_usize;
        let mut buf: Vec<u16> = vec![0u16; 512];

        for root in roots {
            if total_added >= total_budget {
                break;
            }
            if !root.exists() {
                continue;
            }
            if exclusion.should_exclude_path_under_root(root, root) {
                skipped_excluded = skipped_excluded.saturating_add(1);
                continue;
            }

            let root_prefix = normalized_root_prefix(root);
            let root_budget = per_root_budget.min(total_budget - total_added);

            // Pass 1: record only (result index, depth) for every eligible
            // entry so a root with millions of matches stays cheap in memory.
            let mut eligible: Vec<EligibleResult> = Vec::new();
            let mut histogram: Vec<usize> = Vec::new();
            for i in 0..count as u32 {
                let Some(path) = self.result_path(i, &mut buf) else {
                    continue;
                };
                let Some(relative) = relative_under_root(&path, &root_prefix) else {
                    continue;
                };
                let depth = relative_depth(relative);
                if max_depth != 0 && depth > max_depth {
                    continue;
                }

                let is_folder = unsafe { (self.fns.is_folder_result)(i) } != 0;
                if is_folder {
                    if !show_folders || depth == 0 {
                        continue;
                    }
                } else if !show_files {
                    continue;
                }

                // Rebuild the path from the configured root so component
                // exclusions apply even when its casing differs from the
                // index's.
                let exclusion_path = if relative.is_empty() {
                    root.clone()
                } else {
                    root.join(relative)
                };
                if exclusion.should_exclude_entry_under_root(&exclusion_path, root, is_folder) {
                    skipped_excluded = skipped_excluded.saturating_add(1);
                    continue;
                }

                if histogram.len() <= depth {
                    histogram.resize(depth + 1, 0);
                }
                histogram[depth] += 1;
                eligible.push(EligibleResult {
                    index: i,
                    depth: depth.min(u16::MAX as usize) as u16,
                    is_folder,
                });
            }

            // Pass 2: when the root exceeds its budget keep the shallowest
            // entries (ties at the cutoff depth in Everything's order)
            // rather than an arbitrary name-ordered sample.
            let (cutoff_depth, mut cutoff_slots) = shallow_first_cutoff(&histogram, root_budget);
            for result in &eligible {
                let depth = result.depth as usize;
                if depth > cutoff_depth {
                    continue;
                }
                if depth == cutoff_depth {
                    if cutoff_slots == 0 {
                        continue;
                    }
                    cutoff_slots -= 1;
                }
                let Some(path) = self.result_path(result.index, &mut buf) else {
                    continue;
                };
                let path = Path::new(&path);
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.is_empty() {
                    continue;
                }
                // Lowercase id: matches walk (`discover_filesystem_walk`)
                // and watcher (`path_to_search_item`) so an Everything
                // scan upserts the same row instead of leaving a ghost
                // duplicate. Title is the full file name, like the walk.
                let kind = if result.is_folder { "folder" } else { "file" };
                let id = format!("{kind}:{}", path.to_string_lossy().to_ascii_lowercase());
                out.push(SearchItem::new(&id, kind, name, &path.to_string_lossy()));
                total_added += 1;
            }

            if eligible.len() > per_root_budget {
                crate::logging::warn(&format!(
                    "[nex] discovery_cap provider=filesystem backend=everything root={} per_root_cap={per_root_budget} eligible={} reached=true; deepest entries were not indexed (raise index_max_items_per_root)",
                    root.display(),
                    eligible.len()
                ));
            }
        }

        if total_added >= total_budget {
            logging::info(&format!(
                "[nex] discovery_cap provider=filesystem backend=everything total_cap={total_budget} reached=true"
            ));
        }
        if skipped_excluded > 0 {
            logging::info(&format!(
                "[nex] discovery_exclusion provider=filesystem backend=everything skipped={skipped_excluded}"
            ));
        }

        Ok(out)
    }

    /// Full path of result `index`, growing `buf` when the SDK reports a
    /// longer path. Callers must hold `SDK_LOCK`.
    fn result_path(&self, index: u32, buf: &mut Vec<u16>) -> Option<String> {
        let mut written = unsafe {
            (self.fns.get_result_full_path_name_w)(index, buf.as_mut_ptr(), buf.len() as u32)
        };
        if written == 0 {
            return None;
        }
        if (written as usize) >= buf.len() {
            // The voidtools SDK reports the required buffer size in TCHARs
            // (not including the null terminator) and does NOT modify the
            // buffer when it is too small. Resize and retry.
            let needed = (written as usize).saturating_add(1);
            *buf = vec![0u16; needed];
            written = unsafe {
                (self.fns.get_result_full_path_name_w)(index, buf.as_mut_ptr(), buf.len() as u32)
            };
            if written == 0 || (written as usize) >= buf.len() {
                return None;
            }
        }
        Some(String::from_utf16_lossy(&buf[..written as usize]))
    }
}

struct EligibleResult {
    index: u32,
    depth: u16,
    is_folder: bool,
}

/// Root as a backslash path without trailing separators (`G:\` -> `G:`),
/// the form `relative_under_root` expects.
fn normalized_root_prefix(root: &Path) -> String {
    root.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string()
}

/// The part of `path` below `root_prefix`, or `None` when `path` is outside
/// it. Matches only on a path-separator boundary (`g:\games` does not own
/// `g:\games2\x`); the root itself yields an empty string.
fn relative_under_root<'a>(path: &'a str, root_prefix: &str) -> Option<&'a str> {
    let path = path.trim_end_matches(['\\', '/']);
    let head = path.get(..root_prefix.len())?;
    if !head.eq_ignore_ascii_case(root_prefix) {
        return None;
    }
    let rest = &path[root_prefix.len()..];
    if rest.is_empty() {
        return Some(rest);
    }
    rest.strip_prefix(['\\', '/'])
}

/// Number of path components in a `relative_under_root` result.
fn relative_depth(relative: &str) -> usize {
    relative
        .split(['\\', '/'])
        .filter(|component| !component.is_empty())
        .count()
}

/// Splits `budget` over a per-depth entry histogram, shallowest first.
/// Returns `(depth, slots)`: keep every entry shallower than `depth`, plus
/// the first `slots` entries at `depth`; deeper entries are dropped. When
/// everything fits, `depth` is `histogram.len()`.
fn shallow_first_cutoff(histogram: &[usize], budget: usize) -> (usize, usize) {
    let mut remaining = budget;
    for (depth, &count) in histogram.iter().enumerate() {
        if count > remaining {
            return (depth, remaining);
        }
        remaining -= count;
    }
    (histogram.len(), remaining)
}

unsafe fn load_functions(lib: HMODULE) -> Option<EverythingFns> {
    macro_rules! load {
        ($name:expr) => {{
            let c_name = CString::new($name).unwrap();
            // SAFETY: lib is valid HMODULE from LoadLibraryW, function name is NUL-terminated
            let ptr = unsafe { GetProcAddress(lib, c_name.as_bytes_with_nul().as_ptr()) };
            if ptr.is_none() {
                return None;
            }
            // SAFETY: ptr is a valid function pointer matching the target type
            unsafe { std::mem::transmute(ptr) }
        }};
    }

    Some(EverythingFns {
        set_search_w: load!("Everything_SetSearchW"),
        set_match_path: load!("Everything_SetMatchPath"),
        set_max: load!("Everything_SetMax"),
        set_sort: load!("Everything_SetSort"),
        set_request_flags: load!("Everything_SetRequestFlags"),
        query_w: load!("Everything_QueryW"),
        get_num_results: load!("Everything_GetNumResults"),
        get_result_full_path_name_w: load!("Everything_GetResultFullPathNameW"),
        is_folder_result: load!("Everything_IsFolderResult"),
        get_major_version: load!("Everything_GetMajorVersion"),
    })
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = OsStr::new(s).encode_wide().collect();
    v.push(0);
    v
}

fn try_load_dll(path: &Path) -> Option<EverythingBridge> {
    if !path.exists() {
        return None;
    }
    let wide = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    // Absolute path with LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: the path is
    // fully qualified so no search order applies, but the flag keeps the
    // DLL's own dependency resolution (if any) off the unsafe default
    // search order.
    let lib = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            core::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    };
    if lib.is_null() {
        return None;
    }
    let fns = unsafe { load_functions(lib) };
    if let Some(fns) = fns {
        Some(EverythingBridge { lib, fns })
    } else {
        unsafe { FreeLibrary(lib); }
        None
    }
}

fn candidate_dll_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    let mut push = |p: PathBuf| {
        if seen.insert(p.clone()) {
            paths.push(p);
        }
    };

    for install_dir in registry_install_dirs() {
        push(install_dir.join("Everything64.dll"));
        push(install_dir.join("Everything32.dll"));
    }

    let program_files = std::env::var("ProgramFiles").ok();
    let program_files_x86 = std::env::var("ProgramFiles(x86)").ok();
    let program_w6432 = std::env::var("ProgramW6432").ok();
    let local_app_data = std::env::var("LOCALAPPDATA").ok();

    if let Some(pf) = program_files.as_deref() {
        let dir = PathBuf::from(pf).join("Everything");
        push(dir.join("Everything64.dll"));
        push(dir.join("Everything32.dll"));
    }
    if let Some(pf) = program_files_x86.as_deref() {
        let dir = PathBuf::from(pf).join("Everything");
        push(dir.join("Everything64.dll"));
        push(dir.join("Everything32.dll"));
    }
    if let Some(pf) = program_w6432.as_deref() {
        let dir = PathBuf::from(pf).join("Everything");
        push(dir.join("Everything64.dll"));
        push(dir.join("Everything32.dll"));
    }
    if let Some(local) = local_app_data.as_deref() {
        for sub in ["Everything", "Programs\\Everything", "Programs\\Everything-beta"] {
            let dir = PathBuf::from(local).join(sub);
            push(dir.join("Everything64.dll"));
            push(dir.join("Everything32.dll"));
        }
    }

    push(PathBuf::from("C:\\Tools\\Everything\\Everything64.dll"));
    push(PathBuf::from("D:\\Tools\\Everything\\Everything64.dll"));

    paths
}

fn registry_install_dirs() -> Vec<PathBuf> {
    let probes: [RegistryProbe; 6] = [
        RegistryProbe {
            hkey: HKEY_LOCAL_MACHINE,
            subkey: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Everything_is1",
        },
        RegistryProbe {
            hkey: HKEY_LOCAL_MACHINE,
            subkey: r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Everything_is1",
        },
        RegistryProbe {
            hkey: HKEY_CURRENT_USER,
            subkey: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Everything_is1",
        },
        RegistryProbe {
            hkey: HKEY_LOCAL_MACHINE,
            subkey: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Everything",
        },
        RegistryProbe {
            hkey: HKEY_LOCAL_MACHINE,
            subkey: r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Everything",
        },
        RegistryProbe {
            hkey: HKEY_CURRENT_USER,
            subkey: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Everything",
        },
    ];

    let mut dirs: Vec<PathBuf> = Vec::new();
    for probe in &probes {
        if let Some(dir) = read_install_dir(probe.hkey, probe.subkey) {
            dirs.push(dir);
        }
    }
    dirs
}

struct RegistryProbe {
    hkey: *mut std::ffi::c_void,
    subkey: &'static str,
}

fn read_install_dir(hkey: *mut std::ffi::c_void, subkey: &str) -> Option<PathBuf> {
    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_w: Vec<u16> = "InstallLocation"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut buffer = [0u16; 1024];
    let mut buffer_size: u32 = (buffer.len() * 2) as u32;
    let mut value_type: u32 = 0;

    let status = unsafe {
        RegGetValueW(
            hkey,
            subkey_w.as_ptr(),
            value_w.as_ptr(),
            RRF_RT_REG_SZ,
            &mut value_type,
            buffer.as_mut_ptr() as *mut std::ffi::c_void,
            &mut buffer_size,
        )
    };
    if status != 0 {
        return None;
    }
    if buffer_size < 2 {
        return None;
    }
    let char_count = (buffer_size as usize).saturating_sub(2) / 2;
    let path_str = String::from_utf16_lossy(&buffer[..char_count]);
    let path = PathBuf::from(path_str.trim());
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_includes_program_files() {
        let paths = candidate_dll_paths();
        let has_pf = paths.iter().any(|p| {
            p.to_string_lossy().contains("Everything64.dll")
                && p.to_string_lossy().contains("Everything")
        });
        assert!(has_pf, "expected at least one candidate containing Everything64.dll");
    }

    #[test]
    fn candidate_paths_are_deduped() {
        let paths = candidate_dll_paths();
        let mut seen = std::collections::HashSet::new();
        for p in &paths {
            assert!(seen.insert(p.clone()), "duplicate candidate path: {}", p.display());
        }
    }

    #[test]
    fn try_load_dll_returns_none_for_missing() {
        let path = PathBuf::from("C:\\nonexistent\\missing.dll");
        assert!(try_load_dll(&path).is_none());
    }

    #[test]
    fn registry_install_dirs_does_not_panic() {
        let dirs = registry_install_dirs();
        for dir in &dirs {
            assert!(!dir.as_os_str().is_empty());
        }
    }

    #[test]
    fn shallow_first_cutoff_keeps_everything_when_budget_fits() {
        assert_eq!(shallow_first_cutoff(&[0, 3, 5], 10), (3, 2));
        assert_eq!(shallow_first_cutoff(&[], 4), (0, 4));
    }

    #[test]
    fn shallow_first_cutoff_splits_at_first_depth_that_overflows() {
        assert_eq!(shallow_first_cutoff(&[0, 3, 5, 9], 10), (3, 2));
        assert_eq!(shallow_first_cutoff(&[0, 3, 50, 9], 10), (2, 7));
        assert_eq!(shallow_first_cutoff(&[0, 50, 3], 10), (1, 10));
    }

    #[test]
    fn shallow_first_cutoff_exact_fit_leaves_no_slots_below() {
        assert_eq!(shallow_first_cutoff(&[0, 4, 6, 1], 10), (3, 0));
        assert_eq!(shallow_first_cutoff(&[0, 4, 6], 10), (3, 0));
        assert_eq!(shallow_first_cutoff(&[5, 5], 0), (0, 0));
    }

    #[test]
    fn relative_under_root_requires_separator_boundary() {
        let root = normalized_root_prefix(Path::new(r"g:\games"));
        assert_eq!(relative_under_root(r"G:\Games\a\b.txt", &root), Some(r"a\b.txt"));
        assert_eq!(relative_under_root(r"G:\Games", &root), Some(""));
        assert_eq!(relative_under_root(r"G:\Games2\x", &root), None);
        assert_eq!(relative_under_root(r"G:\Gam", &root), None);
        assert_eq!(relative_under_root(r"D:\Games\x", &root), None);
    }

    #[test]
    fn relative_under_root_handles_drive_roots_and_trailing_separators() {
        for root in [r"G:\", "G:", "g:/"] {
            let root = normalized_root_prefix(Path::new(root));
            assert!(root.eq_ignore_ascii_case("G:"));
            assert_eq!(relative_under_root(r"G:\", &root), Some(""));
            assert_eq!(relative_under_root(r"G:\top", &root), Some("top"));
            assert_eq!(relative_under_root(r"G:\top\sub\", &root), Some(r"top\sub"));
            assert_eq!(relative_under_root(r"H:\top", &root), None);
        }
        let slashed = normalized_root_prefix(Path::new("G:/Data/Docs/"));
        assert_eq!(relative_under_root(r"G:\Data\Docs\a.txt", &slashed), Some("a.txt"));
    }

    #[test]
    fn relative_depth_counts_components_below_root() {
        assert_eq!(relative_depth(""), 0);
        assert_eq!(relative_depth("a"), 1);
        assert_eq!(relative_depth(r"a\b\c.txt"), 3);
        assert_eq!(relative_depth("a/b"), 2);
    }

    #[test]
    fn path_decode_resizes_buffer_when_required_size_exceeds_capacity() {
        // Reproduce the slice math used in `discover` to confirm the
        // buffer-too-small path stays in-bounds. The voidtools SDK
        // returns the required TCHAR count (not including the null
        // terminator) and leaves the buffer untouched when the caller's
        // buffer is too small — so we have to grow the buffer and retry
        // before reading.
        let mut buf: Vec<u16> = vec![0u16; 4];
        let long_path = r"C:\Users\admin\AppData\Local\Programs\Microsoft VS Code\Code.exe";
        let wide: Vec<u16> = long_path.encode_utf16().collect();
        let required = wide.len() as u32; // SDK return value

        // First call sees the buffer-too-small case.
        assert!(required as usize >= buf.len());
        let needed = required as usize + 1;
        buf = vec![0u16; needed];
        for (idx, ch) in wide.iter().enumerate() {
            buf[idx] = *ch;
        }
        buf[wide.len()] = 0;

        // Now the slice is safe.
        let decoded = String::from_utf16_lossy(&buf[..required as usize]);
        assert_eq!(decoded, long_path);
    }
}
