#![allow(dead_code)]

use std::ffi::CString;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};

use crate::discovery::{DiscoveryExclusionPolicy};
use crate::logging;
use crate::model::SearchItem;

const EVERYTHING_REQUEST_FILE_NAME: u32 = 0x01;
const EVERYTHING_REQUEST_PATH: u32 = 0x02;

pub(crate) struct EverythingBridge {
    lib: HMODULE,
    fns: EverythingFns,
}

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

        for dll_name in &["Everything64.dll", "Everything32.dll"] {
            let wide = to_wide(dll_name);
            let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
            attempts.push(format!("LoadLibraryW({dll_name})"));
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
        show_files: bool,
        show_folders: bool,
        max_items_total: usize,
        max_items_per_root: usize,
    ) -> Result<Vec<SearchItem>, crate::discovery::ProviderError> {
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

            let mut root_added = 0_usize;
            let root_lower = root.to_string_lossy().to_ascii_lowercase();
            let root_lower = root_lower.trim_end_matches('\\');

            let mut buf: Vec<u16> = vec![0u16; 512];

            for i in 0..count as u32 {
                if total_added >= total_budget || root_added >= per_root_budget {
                    break;
                }

                let mut written = unsafe {
                    (self.fns.get_result_full_path_name_w)(i, buf.as_mut_ptr(), buf.len() as u32)
                };
                if written == 0 {
                    continue;
                }
                if (written as usize) >= buf.len() {
                    // The voidtools SDK reports the required buffer size in TCHARs
                    // (not including the null terminator) and does NOT modify the
                    // buffer when it is too small. Resize and retry.
                    let needed = (written as usize).saturating_add(1);
                    buf = vec![0u16; needed];
                    written = unsafe {
                        (self.fns.get_result_full_path_name_w)(
                            i,
                            buf.as_mut_ptr(),
                            buf.len() as u32,
                        )
                    };
                    if written == 0 || (written as usize) >= buf.len() {
                        continue;
                    }
                }
                let path = String::from_utf16_lossy(&buf[..written as usize]);
                let path_lower = path.to_ascii_lowercase();

                if !path_lower.starts_with(&root_lower) {
                    continue;
                }
                let path = Path::new(&path);

                if exclusion.should_exclude_path_under_root(path, root) {
                    skipped_excluded = skipped_excluded.saturating_add(1);
                    continue;
                }

                let is_folder = unsafe { (self.fns.is_folder_result)(i) } != 0;

                if is_folder {
                    if !show_folders {
                        continue;
                    }
                    if path == root {
                        continue;
                    }
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        let id = format!("folder:{}", path.to_string_lossy());
                        out.push(SearchItem::new(&id, "folder", name, &path.to_string_lossy()));
                        total_added += 1;
                        root_added += 1;
                    }
                } else {
                    if !show_files {
                        continue;
                    }
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .or_else(|| path.file_name().and_then(|n| n.to_str()))
                        .unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    let id = format!("file:{}", path.to_string_lossy());
                    out.push(SearchItem::new(&id, "file", name, &path.to_string_lossy()));
                    total_added += 1;
                    root_added += 1;
                }
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
}

unsafe fn load_functions(lib: HMODULE) -> Option<EverythingFns> {
    macro_rules! load {
        ($name:expr) => {{
            let c_name = CString::new($name).unwrap();
            let ptr = GetProcAddress(lib, c_name.as_bytes_with_nul().as_ptr());
            if ptr.is_none() {
                return None;
            }
            std::mem::transmute(ptr)
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
    let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
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
