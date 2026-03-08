use crate::model::{normalize_for_search, SearchItem};
use std::cmp::Ordering;
use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const ACTION_UNINSTALL_PREFIX: &str = "__swiftfind_action_uninstall__:";
const UNINSTALL_CACHE_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
struct UninstallEntry {
    token: String,
    display_name: String,
    publisher: String,
    uninstall_command: String,
    fallback_uninstall_command: Option<String>,
}

#[derive(Debug, Default)]
struct UninstallCache {
    loaded_at: Option<Instant>,
    entries: Vec<UninstallEntry>,
}

static UNINSTALL_CACHE: OnceLock<Mutex<UninstallCache>> = OnceLock::new();

pub fn search_uninstall_actions(query: &str, limit: usize) -> Vec<SearchItem> {
    if limit == 0 {
        return Vec::new();
    }
    let Some(search_term) = extract_uninstall_search_term(query) else {
        return Vec::new();
    };

    let entries = match load_cached_entries(false) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    search_uninstall_actions_with_entries(search_term.as_str(), limit, &entries)
}

pub fn has_uninstall_intent(query: &str) -> bool {
    extract_uninstall_search_term(query).is_some()
}

pub fn execute_uninstall_action(action_id: &str) -> Result<(), String> {
    let token = action_id
        .strip_prefix(ACTION_UNINSTALL_PREFIX)
        .ok_or_else(|| "invalid uninstall action id".to_string())?;

    let mut entries = load_cached_entries(false)?;
    let mut entry = entries
        .into_iter()
        .find(|candidate| candidate.token == token);

    if entry.is_none() {
        entries = load_cached_entries(true)?;
        entry = entries
            .into_iter()
            .find(|candidate| candidate.token == token);
    }

    let target = entry.ok_or_else(|| "uninstall target is no longer available".to_string())?;
    execute_entry(&target)
}

pub fn is_display_name_registered(display_name: &str) -> Result<bool, String> {
    let normalized_query = normalize_for_search(display_name);
    if normalized_query.is_empty() {
        return Ok(false);
    }

    let entries = load_cached_entries(true)?;
    Ok(entries
        .iter()
        .map(|entry| normalize_for_search(entry.display_name.as_str()))
        .any(|normalized_name| normalized_name == normalized_query))
}

pub fn publishers_by_display_name() -> Result<HashMap<String, String>, String> {
    let entries = load_cached_entries(false)?;
    let mut out = HashMap::new();
    for entry in entries {
        let key = normalize_for_search(entry.display_name.as_str());
        let publisher = entry.publisher.trim();
        if key.is_empty() || publisher.is_empty() {
            continue;
        }
        out.entry(key).or_insert_with(|| publisher.to_string());
    }
    Ok(out)
}

fn search_uninstall_actions_with_entries(
    search_term: &str,
    limit: usize,
    entries: &[UninstallEntry],
) -> Vec<SearchItem> {
    if limit == 0 || entries.is_empty() {
        return Vec::new();
    }

    let normalized_query = normalize_for_search(search_term);
    let mut ranked: Vec<(i64, &UninstallEntry)> = entries
        .iter()
        .filter_map(|entry| {
            uninstall_entry_score(entry, &normalized_query).map(|score| (score, entry))
        })
        .collect();

    ranked.sort_by(|(left_score, left_entry), (right_score, right_entry)| {
        right_score
            .cmp(left_score)
            .then_with(|| compare_display_name(left_entry, right_entry))
            .then_with(|| left_entry.token.cmp(&right_entry.token))
    });

    ranked
        .into_iter()
        .take(limit)
        .map(|(_, entry)| uninstall_entry_to_action(entry))
        .collect()
}

fn compare_display_name(left: &UninstallEntry, right: &UninstallEntry) -> Ordering {
    left.display_name
        .to_ascii_lowercase()
        .cmp(&right.display_name.to_ascii_lowercase())
}

fn uninstall_entry_to_action(entry: &UninstallEntry) -> SearchItem {
    let id = format!("{ACTION_UNINSTALL_PREFIX}{}", entry.token);
    let subtitle = if entry.publisher.trim().is_empty() {
        "Installed application".to_string()
    } else {
        format!("{} application", entry.publisher.trim())
    };
    SearchItem::new(
        &id,
        "action",
        &format!("Uninstall {}", entry.display_name.trim()),
        &subtitle,
    )
}

fn uninstall_entry_score(entry: &UninstallEntry, normalized_query: &str) -> Option<i64> {
    if normalized_query.is_empty() {
        return Some(100);
    }

    let normalized_name = normalize_for_search(entry.display_name.as_str());
    let normalized_publisher = normalize_for_search(entry.publisher.as_str());

    if normalized_name == normalized_query {
        return Some(20_000);
    }
    if normalized_name.starts_with(normalized_query) {
        return Some(16_000 - (normalized_name.len() as i64 - normalized_query.len() as i64).abs());
    }
    if normalized_name.contains(normalized_query) {
        return Some(12_000 - (normalized_name.len() as i64 - normalized_query.len() as i64).abs());
    }
    if normalized_publisher.contains(normalized_query) {
        return Some(
            8_000 - (normalized_publisher.len() as i64 - normalized_query.len() as i64).abs(),
        );
    }

    None
}

fn extract_uninstall_search_term(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let first = parts.next()?.to_ascii_lowercase();
    if !matches!(
        first.as_str(),
        "uninstall" | "remove" | "delete" | "del" | "rm" | "u"
    ) {
        return None;
    }

    Some(parts.collect::<Vec<_>>().join(" ").trim().to_string())
}

fn load_cached_entries(force_refresh: bool) -> Result<Vec<UninstallEntry>, String> {
    let cache = UNINSTALL_CACHE.get_or_init(|| Mutex::new(UninstallCache::default()));
    let mut state = cache
        .lock()
        .map_err(|_| "uninstall cache lock poisoned".to_string())?;

    let stale = state
        .loaded_at
        .map(|loaded| loaded.elapsed() >= UNINSTALL_CACHE_TTL)
        .unwrap_or(true);

    if force_refresh || stale {
        state.entries = load_entries()?;
        state.loaded_at = Some(Instant::now());
    }

    Ok(state.entries.clone())
}

fn load_entries() -> Result<Vec<UninstallEntry>, String> {
    #[cfg(target_os = "windows")]
    {
        return load_entries_windows();
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(Vec::new())
    }
}

#[cfg(target_os = "windows")]
fn execute_entry(entry: &UninstallEntry) -> Result<(), String> {
    match launch_uninstall_command(
        entry.display_name.as_str(),
        entry.uninstall_command.as_str(),
    ) {
        Ok(()) => Ok(()),
        Err(primary_error) => {
            if let Some(fallback_command) = entry.fallback_uninstall_command.as_deref() {
                match launch_uninstall_command(entry.display_name.as_str(), fallback_command) {
                    Ok(()) => Ok(()),
                    Err(fallback_error) => {
                        Err(format!("{primary_error}; fallback_error={fallback_error}"))
                    }
                }
            } else {
                Err(primary_error)
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn execute_entry(_entry: &UninstallEntry) -> Result<(), String> {
    Err("uninstall actions are only supported on Windows".to_string())
}

#[cfg(target_os = "windows")]
fn load_entries_windows() -> Result<Vec<UninstallEntry>, String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY,
    };

    let mut entries = Vec::new();
    collect_entries_from_uninstall_key("hkcu", HKEY_CURRENT_USER, 0, &mut entries)?;
    collect_entries_from_uninstall_key("hklm", HKEY_LOCAL_MACHINE, 0, &mut entries)?;
    collect_entries_from_uninstall_key(
        "hklm32",
        HKEY_LOCAL_MACHINE,
        KEY_WOW64_32KEY,
        &mut entries,
    )?;

    let mut seen = HashSet::new();
    entries.retain(|entry| {
        let key = format!(
            "{}|{}|{}|{}",
            normalize_for_search(entry.display_name.as_str()),
            normalize_for_search(entry.publisher.as_str()),
            normalize_for_search(entry.uninstall_command.as_str()),
            normalize_for_search(
                entry
                    .fallback_uninstall_command
                    .as_deref()
                    .unwrap_or_default()
            ),
        );
        seen.insert(key)
    });

    entries.sort_by(compare_display_name);
    Ok(entries)
}

#[cfg(target_os = "windows")]
fn collect_entries_from_uninstall_key(
    hive_label: &str,
    root: windows_sys::Win32::System::Registry::HKEY,
    view_flags: u32,
    out: &mut Vec<UninstallEntry>,
) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryInfoKeyW, HKEY, KEY_READ,
    };

    const UNINSTALL_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

    let mut uninstall_root: HKEY = std::ptr::null_mut();
    let uninstall_subkey_wide = to_wide(UNINSTALL_SUBKEY);
    let open_status = unsafe {
        RegOpenKeyExW(
            root,
            uninstall_subkey_wide.as_ptr(),
            0,
            KEY_READ | view_flags,
            &mut uninstall_root,
        )
    };

    if open_status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if open_status != ERROR_SUCCESS {
        return Err(format!(
            "failed to open uninstall key ({hive_label}) with code {open_status}"
        ));
    }

    let mut subkey_count = 0_u32;
    let mut max_subkey_len = 0_u32;
    let info_status = unsafe {
        RegQueryInfoKeyW(
            uninstall_root,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut subkey_count,
            &mut max_subkey_len,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if info_status != ERROR_SUCCESS {
        unsafe {
            RegCloseKey(uninstall_root);
        }
        return Err(format!(
            "failed to inspect uninstall key ({hive_label}) with code {info_status}"
        ));
    }

    let mut name_buffer = vec![0_u16; max_subkey_len as usize + 2];
    for index in 0..subkey_count {
        let mut name_len = max_subkey_len + 1;
        let enum_status = unsafe {
            RegEnumKeyExW(
                uninstall_root,
                index,
                name_buffer.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };

        if enum_status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if enum_status != ERROR_SUCCESS {
            continue;
        }

        let subkey_name = String::from_utf16_lossy(&name_buffer[..name_len as usize]);
        if let Some(entry) =
            read_uninstall_entry(hive_label, uninstall_root, view_flags, &subkey_name)?
        {
            out.push(entry);
        }
    }

    unsafe {
        RegCloseKey(uninstall_root);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn read_uninstall_entry(
    hive_label: &str,
    uninstall_root: windows_sys::Win32::System::Registry::HKEY,
    view_flags: u32,
    subkey_name: &str,
) -> Result<Option<UninstallEntry>, String> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, KEY_READ};

    let subkey_wide = to_wide(subkey_name);
    let mut app_key: HKEY = std::ptr::null_mut();
    let open_status = unsafe {
        RegOpenKeyExW(
            uninstall_root,
            subkey_wide.as_ptr(),
            0,
            KEY_READ | view_flags,
            &mut app_key,
        )
    };

    if open_status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if open_status != ERROR_SUCCESS {
        return Err(format!(
            "failed to open uninstall item key ({hive_label}:{subkey_name}) with code {open_status}"
        ));
    }

    let display_name = read_reg_string_value(app_key, "DisplayName");
    let publisher = read_reg_string_value(app_key, "Publisher").unwrap_or_default();
    let quiet_uninstall = read_reg_string_value(app_key, "QuietUninstallString");
    let uninstall = read_reg_string_value(app_key, "UninstallString");
    let release_type = read_reg_string_value(app_key, "ReleaseType").unwrap_or_default();
    let parent_key = read_reg_string_value(app_key, "ParentKeyName").unwrap_or_default();
    let system_component = read_reg_dword_value(app_key, "SystemComponent").unwrap_or(0);

    unsafe {
        RegCloseKey(app_key);
    }

    let Some(display_name) = display_name else {
        return Ok(None);
    };
    let uninstall_candidates = dedupe_uninstall_candidates(quiet_uninstall, uninstall);
    let mut usable_commands = uninstall_candidates
        .into_iter()
        .filter(|candidate| is_uninstall_command_usable(candidate.as_str()))
        .collect::<Vec<_>>();
    if display_name.trim().is_empty() || usable_commands.is_empty() {
        return Ok(None);
    }
    if system_component == 1 {
        return Ok(None);
    }
    if !parent_key.trim().is_empty() {
        return Ok(None);
    }
    if looks_like_update_entry(display_name.as_str(), release_type.as_str()) {
        return Ok(None);
    }

    let uninstall_command = usable_commands.remove(0);
    let fallback_uninstall_command = usable_commands.into_iter().next();

    Ok(Some(UninstallEntry {
        token: format!("{hive_label}:{subkey_name}"),
        display_name: display_name.trim().to_string(),
        publisher: publisher.trim().to_string(),
        uninstall_command: uninstall_command.trim().to_string(),
        fallback_uninstall_command,
    }))
}

#[cfg(target_os = "windows")]
fn read_reg_string_value(
    key: windows_sys::Win32::System::Registry::HKEY,
    value_name: &str,
) -> Option<String> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{RegQueryValueExW, REG_EXPAND_SZ, REG_SZ};

    let value_name_wide = to_wide(value_name);
    let mut value_type = 0_u32;
    let mut size = 0_u32;
    let query_status = unsafe {
        RegQueryValueExW(
            key,
            value_name_wide.as_ptr(),
            std::ptr::null(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if query_status == ERROR_FILE_NOT_FOUND || query_status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
        return None;
    }

    let mut buffer = vec![0_u8; size as usize];
    let read_status = unsafe {
        RegQueryValueExW(
            key,
            value_name_wide.as_ptr(),
            std::ptr::null(),
            &mut value_type,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    if read_status != ERROR_SUCCESS {
        return None;
    }

    let mut wide = Vec::with_capacity(buffer.len() / 2);
    for chunk in buffer.chunks_exact(2) {
        wide.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    while wide.last().copied() == Some(0) {
        wide.pop();
    }
    let value = String::from_utf16_lossy(&wide).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(target_os = "windows")]
fn read_reg_dword_value(
    key: windows_sys::Win32::System::Registry::HKEY,
    value_name: &str,
) -> Option<u32> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{RegQueryValueExW, REG_DWORD};

    let value_name_wide = to_wide(value_name);
    let mut value_type = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut value = 0_u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            value_name_wide.as_ptr(),
            std::ptr::null(),
            &mut value_type,
            &mut value as *mut u32 as *mut u8,
            &mut size,
        )
    };

    if status == ERROR_FILE_NOT_FOUND || status != ERROR_SUCCESS {
        return None;
    }
    if value_type != REG_DWORD {
        return None;
    }
    Some(value)
}

#[cfg(target_os = "windows")]
fn looks_like_update_entry(display_name: &str, release_type: &str) -> bool {
    let display_lower = display_name.to_ascii_lowercase();
    let release_lower = release_type.to_ascii_lowercase();

    release_lower.contains("update")
        || release_lower.contains("hotfix")
        || release_lower.contains("security")
        || display_lower.starts_with("update for ")
        || display_lower.starts_with("security update for ")
        || display_lower.contains("hotfix")
}

#[cfg(target_os = "windows")]
fn dedupe_uninstall_candidates(
    quiet_uninstall: Option<String>,
    uninstall: Option<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for candidate in [quiet_uninstall, uninstall].into_iter().flatten() {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

#[cfg(target_os = "windows")]
fn is_uninstall_command_usable(command: &str) -> bool {
    let Ok((program_raw, _)) = split_uninstall_command(command) else {
        return false;
    };
    let program = expand_environment_strings(program_raw.as_str());
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return false;
    }

    let has_path_hint = trimmed.contains('\\')
        || trimmed.contains('/')
        || trimmed.contains(':')
        || trimmed.starts_with('.');
    if !has_path_hint {
        return true;
    }

    std::path::Path::new(trimmed).exists()
}

#[cfg(target_os = "windows")]
fn launch_uninstall_command(display_name: &str, command: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let (program_raw, parameters_raw) = split_uninstall_command(command)?;
    let program = expand_environment_strings(program_raw.as_str());
    let mut parameters = expand_environment_strings(parameters_raw.as_str());
    if is_msiexec_program(program.as_str()) {
        parameters = rewrite_msiexec_install_to_uninstall(parameters.as_str());
    }

    let program_wide = to_wide(program.as_str());
    let parameters_wide = to_wide(parameters.as_str());
    let parameters_ptr = if parameters.trim().is_empty() {
        std::ptr::null()
    } else {
        parameters_wide.as_ptr()
    };

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            program_wide.as_ptr(),
            parameters_ptr,
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result > 32 {
        return Ok(());
    }

    let shell_hint = match result {
        0 => "out_of_memory_or_resources",
        2 => "file_not_found",
        3 => "path_not_found",
        5 => "access_denied",
        8 => "out_of_memory",
        26 => "sharing_violation",
        27 => "association_incomplete",
        28 => "dde_timeout",
        29 => "dde_failed",
        30 => "dde_busy",
        31 => "no_association",
        _ => "unknown_shell_error",
    };
    let fallback_err = launch_uninstall_via_cmd(command).err().unwrap_or_default();
    Err(format!(
        "failed to launch uninstall command for '{}' (shell_code={result}:{shell_hint}); fallback={fallback_err}",
        display_name,
    ))
}

#[cfg(target_os = "windows")]
fn launch_uninstall_via_cmd(raw_command: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let status = Command::new("cmd.exe")
        .args(["/C", raw_command.trim()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("cmd fallback spawn failed: {error}"))?;

    if status.id() == 0 {
        return Err("cmd fallback returned invalid process id".to_string());
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn split_uninstall_command(command: &str) -> Result<(String, String), String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err("uninstall command is empty".to_string());
    }

    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end_quote) = rest.find('"') {
            let executable = rest[..end_quote].trim();
            if executable.is_empty() {
                return Err("uninstall command executable is empty".to_string());
            }
            let args = rest[end_quote + 1..].trim().to_string();
            return Ok((executable.to_string(), args));
        }
    }

    if let Some((program, args)) = split_unquoted_path_with_extension(trimmed) {
        return Ok((program, args));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let executable = parts.next().unwrap_or_default().trim();
    if executable.is_empty() {
        return Err("uninstall command executable is empty".to_string());
    }
    let args = parts.next().unwrap_or_default().trim().to_string();

    Ok((executable.to_string(), args))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn split_unquoted_path_with_extension(command: &str) -> Option<(String, String)> {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();
    for extension in [".exe", ".msi", ".cmd", ".bat", ".com"] {
        let mut offset = 0;
        while let Some(relative) = lower[offset..].find(extension) {
            let end = offset + relative + extension.len();
            let boundary = if end >= lower.len() {
                true
            } else {
                lower.as_bytes()[end].is_ascii_whitespace()
            };
            if boundary {
                let executable = trimmed[..end].trim();
                if !executable.is_empty() {
                    let args = trimmed[end..].trim().to_string();
                    return Some((executable.to_string(), args));
                }
            }
            offset = end;
            if offset >= lower.len() {
                break;
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn is_msiexec_program(executable: &str) -> bool {
    let normalized = executable.replace('/', "\\");
    let file_name = normalized
        .rsplit('\\')
        .next()
        .unwrap_or(normalized.as_str());
    file_name.eq_ignore_ascii_case("msiexec") || file_name.eq_ignore_ascii_case("msiexec.exe")
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn rewrite_msiexec_install_to_uninstall(parameters: &str) -> String {
    let trimmed = parameters.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("/x") || lower.contains("-x") {
        return trimmed.to_string();
    }

    let bytes = trimmed.as_bytes();
    if bytes.len() < 2 {
        return trimmed.to_string();
    }

    for index in 0..(bytes.len() - 1) {
        let current = bytes[index];
        let next = bytes[index + 1];
        if !matches!(current, b'/' | b'-') {
            continue;
        }
        if !matches!(next, b'i' | b'I') {
            continue;
        }

        let prev_ok = index == 0 || bytes[index - 1].is_ascii_whitespace();
        let next_ok = index + 2 >= bytes.len()
            || bytes[index + 2].is_ascii_whitespace()
            || bytes[index + 2] == b'{'
            || bytes[index + 2] == b'"';
        if !prev_ok || !next_ok {
            continue;
        }

        let mut rewritten = trimmed.as_bytes().to_vec();
        rewritten[index + 1] = b'X';
        return String::from_utf8_lossy(&rewritten).to_string();
    }

    trimmed.to_string()
}

#[cfg(target_os = "windows")]
fn expand_environment_strings(input: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;

    if !input.contains('%') {
        return input.to_string();
    }

    let input_wide = to_wide(input);
    let needed = unsafe { ExpandEnvironmentStringsW(input_wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return input.to_string();
    }

    let mut output = vec![0_u16; needed as usize];
    let written =
        unsafe { ExpandEnvironmentStringsW(input_wide.as_ptr(), output.as_mut_ptr(), needed) };
    if written == 0 {
        return input.to_string();
    }

    while output.last().copied() == Some(0) {
        output.pop();
    }
    String::from_utf16_lossy(&output)
}

#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        extract_uninstall_search_term, rewrite_msiexec_install_to_uninstall,
        search_uninstall_actions_with_entries, split_unquoted_path_with_extension, UninstallEntry,
    };

    #[test]
    fn parses_uninstall_intent_prefixes() {
        assert_eq!(
            extract_uninstall_search_term("uninstall Discord"),
            Some("Discord".to_string())
        );
        assert_eq!(
            extract_uninstall_search_term("remove VLC"),
            Some("VLC".to_string())
        );
        assert_eq!(
            extract_uninstall_search_term("u Notepad"),
            Some("Notepad".to_string())
        );
        assert_eq!(extract_uninstall_search_term("delete"), Some(String::new()));
    }

    #[test]
    fn ignores_non_uninstall_queries() {
        assert_eq!(extract_uninstall_search_term("discord"), None);
        assert_eq!(extract_uninstall_search_term("open uninstall menu"), None);
        assert_eq!(extract_uninstall_search_term(""), None);
    }

    #[test]
    fn ranks_uninstall_actions_by_name_match_strength() {
        let entries = vec![
            UninstallEntry {
                token: "1".to_string(),
                display_name: "Discord".to_string(),
                publisher: "Discord Inc.".to_string(),
                uninstall_command: "C:\\Tools\\uninstall_discord.exe".to_string(),
                fallback_uninstall_command: None,
            },
            UninstallEntry {
                token: "2".to_string(),
                display_name: "Visual Studio Code".to_string(),
                publisher: "Microsoft".to_string(),
                uninstall_command: "C:\\Tools\\uninstall_vscode.exe".to_string(),
                fallback_uninstall_command: None,
            },
            UninstallEntry {
                token: "3".to_string(),
                display_name: "Codium".to_string(),
                publisher: "VSCodium".to_string(),
                uninstall_command: "C:\\Tools\\uninstall_codium.exe".to_string(),
                fallback_uninstall_command: None,
            },
        ];

        let results = search_uninstall_actions_with_entries("dis", 10, &entries);
        assert_eq!(results[0].title, "Uninstall Discord");
    }

    #[test]
    fn rewrites_msiexec_install_switch_when_needed() {
        assert_eq!(
            rewrite_msiexec_install_to_uninstall("/I {1234-5678}"),
            "/X {1234-5678}"
        );
        assert_eq!(
            rewrite_msiexec_install_to_uninstall("/X {1234-5678}"),
            "/X {1234-5678}"
        );
    }

    #[test]
    fn splits_unquoted_executable_path_with_spaces() {
        let parsed = split_unquoted_path_with_extension(
            "C:\\Program Files\\Example App\\uninstall.exe /SILENT",
        )
        .expect("should parse executable and args");
        assert_eq!(parsed.0, "C:\\Program Files\\Example App\\uninstall.exe");
        assert_eq!(parsed.1, "/SILENT");
    }
}
