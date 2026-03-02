#[cfg(target_os = "windows")]
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::model::SearchItem;

const DEFAULT_INDEX_MAX_ITEMS_TOTAL: usize = 120_000;
const DEFAULT_INDEX_MAX_ITEMS_PER_ROOT: usize = 40_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    message: String,
}

impl ProviderError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for ProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProviderError {}

pub trait DiscoveryProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError>;
    fn change_stamp(&self) -> Option<String> {
        None
    }
}

pub struct AppProvider {
    apps: Vec<SearchItem>,
}

impl AppProvider {
    pub fn from_apps(apps: Vec<SearchItem>) -> Self {
        Self { apps }
    }

    pub fn deterministic_fixture() -> Self {
        Self {
            apps: vec![
                SearchItem::new(
                    "app-code",
                    "app",
                    "Visual Studio Code",
                    "C:\\Program Files\\Microsoft VS Code\\Code.exe",
                ),
                SearchItem::new(
                    "app-term",
                    "app",
                    "Windows Terminal",
                    "C:\\Program Files\\WindowsApps\\Terminal.exe",
                ),
            ],
        }
    }
}

impl DiscoveryProvider for AppProvider {
    fn provider_name(&self) -> &'static str {
        "app"
    }

    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError> {
        Ok(self.apps.clone())
    }
}

pub struct FileProvider {
    files: Vec<SearchItem>,
}

impl FileProvider {
    pub fn from_files(files: Vec<SearchItem>) -> Self {
        Self { files }
    }

    pub fn deterministic_fixture() -> Self {
        Self {
            files: vec![
                SearchItem::new(
                    "file-report",
                    "file",
                    "Q4_Report.xlsx",
                    "C:\\Users\\Admin\\Documents\\Q4_Report.xlsx",
                ),
                SearchItem::new(
                    "file-notes",
                    "file",
                    "Meeting Notes.txt",
                    "C:\\Users\\Admin\\Documents\\Meeting Notes.txt",
                ),
            ],
        }
    }
}

impl DiscoveryProvider for FileProvider {
    fn provider_name(&self) -> &'static str {
        "file"
    }

    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError> {
        Ok(self.files.clone())
    }
}

pub struct StartMenuAppDiscoveryProvider {
    roots: Vec<PathBuf>,
}

impl Default for StartMenuAppDiscoveryProvider {
    fn default() -> Self {
        Self {
            roots: default_start_menu_roots(),
        }
    }
}

impl StartMenuAppDiscoveryProvider {
    pub fn with_roots(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }
}

impl DiscoveryProvider for StartMenuAppDiscoveryProvider {
    fn provider_name(&self) -> &'static str {
        "start-menu-apps"
    }

    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = &self.roots;
            Ok(Vec::new())
        }

        #[cfg(target_os = "windows")]
        {
            let mut items = Vec::new();
            for root in &self.roots {
                items.extend(discover_start_menu_root(root)?);
            }
            if let Ok(system_apps) = discover_start_apps() {
                items.extend(system_apps);
            }
            Ok(dedupe_apps_by_title(items))
        }
    }

    fn change_stamp(&self) -> Option<String> {
        // Bump when Start menu discovery/filtering behavior changes so incremental
        // rebuilds do not keep stale cached app entries.
        const START_MENU_DISCOVERY_SCHEMA_VERSION: &str = "2";
        Some(format!(
            "v{START_MENU_DISCOVERY_SCHEMA_VERSION};{}",
            roots_change_stamp(&self.roots)
        ))
    }
}

pub struct FileSystemDiscoveryProvider {
    roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    max_depth: usize,
    windows_search_enabled: bool,
    windows_search_fallback_filesystem: bool,
    show_files: bool,
    show_folders: bool,
    max_items_total: usize,
    max_items_per_root: usize,
}

impl FileSystemDiscoveryProvider {
    pub fn new(roots: Vec<PathBuf>, max_depth: usize, excluded_roots: Vec<PathBuf>) -> Self {
        Self::with_options(roots, max_depth, excluded_roots, true, true, true, true)
    }

    pub fn with_windows_search_options(
        roots: Vec<PathBuf>,
        max_depth: usize,
        excluded_roots: Vec<PathBuf>,
        windows_search_enabled: bool,
        windows_search_fallback_filesystem: bool,
    ) -> Self {
        Self::with_options(
            roots,
            max_depth,
            excluded_roots,
            windows_search_enabled,
            windows_search_fallback_filesystem,
            true,
            true,
        )
    }

    pub fn with_options(
        roots: Vec<PathBuf>,
        max_depth: usize,
        excluded_roots: Vec<PathBuf>,
        windows_search_enabled: bool,
        windows_search_fallback_filesystem: bool,
        show_files: bool,
        show_folders: bool,
    ) -> Self {
        Self {
            roots,
            excluded_roots,
            max_depth,
            windows_search_enabled,
            windows_search_fallback_filesystem,
            show_files,
            show_folders,
            max_items_total: DEFAULT_INDEX_MAX_ITEMS_TOTAL,
            max_items_per_root: DEFAULT_INDEX_MAX_ITEMS_PER_ROOT,
        }
    }

    pub fn with_index_limits(mut self, max_items_total: usize, max_items_per_root: usize) -> Self {
        let total = max_items_total.max(1);
        let per_root = max_items_per_root.max(1).min(total);
        self.max_items_total = total;
        self.max_items_per_root = per_root;
        self
    }
}

impl DiscoveryProvider for FileSystemDiscoveryProvider {
    fn provider_name(&self) -> &'static str {
        "filesystem"
    }

    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError> {
        if !self.show_files && !self.show_folders {
            return Ok(Vec::new());
        }

        #[cfg(target_os = "windows")]
        if self.windows_search_enabled {
            match discover_windows_search_items(
                &self.roots,
                &self.excluded_roots,
                self.show_files,
                self.show_folders,
                self.max_items_total,
                self.max_items_per_root,
            ) {
                Ok(items) if !items.is_empty() => return Ok(items),
                Ok(_) if !self.windows_search_fallback_filesystem => return Ok(Vec::new()),
                Ok(_) => {}
                Err(error) if !self.windows_search_fallback_filesystem => return Err(error),
                Err(_) => {}
            }
        }

        discover_filesystem_walk(
            &self.roots,
            &self.excluded_roots,
            self.max_depth,
            self.show_files,
            self.show_folders,
            self.max_items_total,
            self.max_items_per_root,
        )
    }

    fn change_stamp(&self) -> Option<String> {
        let mut stamp = String::new();
        stamp.push_str("roots=");
        stamp.push_str(&roots_change_stamp(&self.roots));
        stamp.push_str(";exclude=");
        stamp.push_str(&roots_change_stamp(&self.excluded_roots));
        stamp.push_str(";depth=");
        stamp.push_str(&self.max_depth.to_string());
        stamp.push_str(";windows_search=");
        stamp.push_str(if self.windows_search_enabled {
            "enabled"
        } else {
            "disabled"
        });
        stamp.push_str(";fallback=");
        stamp.push_str(if self.windows_search_fallback_filesystem {
            "filesystem"
        } else {
            "none"
        });
        stamp.push_str(";show_files=");
        stamp.push_str(if self.show_files { "true" } else { "false" });
        stamp.push_str(";show_folders=");
        stamp.push_str(if self.show_folders { "true" } else { "false" });
        stamp.push_str(";cap_total=");
        stamp.push_str(&self.max_items_total.to_string());
        stamp.push_str(";cap_per_root=");
        stamp.push_str(&self.max_items_per_root.to_string());
        Some(stamp)
    }
}

fn discover_filesystem_walk(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    max_depth: usize,
    show_files: bool,
    show_folders: bool,
    max_items_total: usize,
    max_items_per_root: usize,
) -> Result<Vec<SearchItem>, ProviderError> {
    let mut out = Vec::new();
    let excluded = normalized_exclusion_roots(excluded_roots);
    let total_budget = max_items_total.max(1);
    let per_root_budget = max_items_per_root.max(1).min(total_budget);
    let mut total_added = 0_usize;

    for root in roots {
        if total_added >= total_budget {
            break;
        }
        if !root.exists() {
            continue;
        }

        let mut root_added = 0_usize;
        for entry in walkdir::WalkDir::new(root)
            .max_depth(max_depth)
            .into_iter()
            .filter_entry(|entry| !is_path_under_any_excluded_root(entry.path(), &excluded))
            .filter_map(Result::ok)
        {
            if total_added >= total_budget || root_added >= per_root_budget {
                break;
            }
            let path = entry.path();
            if path.is_dir() {
                if !show_folders {
                    continue;
                }
                if path == root {
                    continue;
                }

                let folder_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());

                let id = format!("folder:{}", path.to_string_lossy());
                out.push(SearchItem::new(
                    &id,
                    "folder",
                    &folder_name,
                    &path.to_string_lossy(),
                ));
                total_added += 1;
                root_added += 1;
                continue;
            }

            if !path.is_file() {
                continue;
            }
            if !show_files {
                continue;
            }

            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());

            let id = format!("file:{}", path.to_string_lossy());
            out.push(SearchItem::new(
                &id,
                "file",
                &file_name,
                &path.to_string_lossy(),
            ));
            total_added += 1;
            root_added += 1;
        }
    }

    if total_added >= total_budget {
        crate::logging::info(&format!(
            "[swiftfind-core] discovery_cap provider=filesystem total_cap={} reached=true",
            total_budget
        ));
    }

    Ok(out)
}

fn roots_change_stamp(roots: &[PathBuf]) -> String {
    let mut parts = Vec::with_capacity(roots.len());
    for root in roots {
        let normalized = normalize_root_for_stamp(root);
        let (exists, modified_secs, child_count, child_latest_secs) = quick_path_fingerprint(root);
        parts.push(format!(
            "{normalized}:{exists}:{modified_secs}:{child_count}:{child_latest_secs}"
        ));
    }
    parts.join("|")
}

fn normalize_root_for_stamp(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn quick_path_fingerprint(path: &Path) -> (u8, u64, usize, u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0, 0, 0);
    };
    let root_modified_secs = modified_secs(&meta);
    let mut child_count = 0_usize;
    let mut child_latest_secs = 0_u64;

    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                child_count += 1;
                if let Ok(child_meta) = entry.metadata() {
                    child_latest_secs = child_latest_secs.max(modified_secs(&child_meta));
                }
            }
        }
    }

    (1, root_modified_secs, child_count, child_latest_secs)
}

fn modified_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn normalized_exclusion_roots(excluded_roots: &[PathBuf]) -> Vec<String> {
    excluded_roots
        .iter()
        .filter_map(|root| normalize_path_for_compare(root).filter(|v| !v.is_empty()))
        .collect()
}

fn is_path_under_any_excluded_root(path: &Path, excluded_roots: &[String]) -> bool {
    let Some(path_norm) = normalize_path_for_compare(path) else {
        return false;
    };
    excluded_roots.iter().any(|root| {
        path_norm == *root
            || (path_norm.starts_with(root) && path_norm[root.len()..].starts_with('\\'))
    })
}

fn normalize_path_for_compare(path: &Path) -> Option<String> {
    let mut value = path.to_string_lossy().replace('/', "\\");
    while value.ends_with('\\') {
        value.pop();
    }
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(target_os = "windows")]
fn default_start_menu_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(program_data) = std::env::var("ProgramData") {
        roots.push(
            PathBuf::from(program_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }

    if let Ok(app_data) = std::env::var("APPDATA") {
        roots.push(
            PathBuf::from(app_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }

    roots
}

#[cfg(not(target_os = "windows"))]
fn default_start_menu_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn discover_start_menu_root(root: &Path) -> Result<Vec<SearchItem>, ProviderError> {
    let mut items = Vec::new();

    if !root.exists() {
        return Ok(items);
    }

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        if ext != "lnk" && ext != "exe" {
            continue;
        }
        if ext == "lnk" {
            if let Some(shortcut_target) = resolve_shortcut_target_for_discovery(path) {
                if should_exclude_non_app_start_reference(title.as_str(), shortcut_target.as_str())
                {
                    continue;
                }
            }
        }
        if ext == "lnk" && !shortcut_has_launch_target(path) {
            continue;
        }

        if is_documentation_like_start_entry_title(&title) {
            continue;
        }
        let id = format!("app:{}", path.to_string_lossy());

        items.push(SearchItem::new(&id, "app", &title, &path.to_string_lossy()));
    }

    Ok(items)
}

#[cfg(target_os = "windows")]
fn discover_start_apps() -> Result<Vec<SearchItem>, ProviderError> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let script = r#"
$ErrorActionPreference = 'Stop'
Get-StartApps | ForEach-Object {
  $name = [string]$_.Name
  $appId = [string]$_.AppID
  if (-not [string]::IsNullOrWhiteSpace($name) -and -not [string]::IsNullOrWhiteSpace($appId)) {
    "{0}`t{1}" -f $name.Trim(), $appId.Trim()
  }
}
"#;
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .creation_flags(CREATE_NO_WINDOW);

    let output = command
        .output()
        .map_err(|error| ProviderError::new(format!("Get-StartApps invocation failed: {error}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ProviderError::new(format!(
            "Get-StartApps failed (status={}): {}",
            output.status,
            if stderr.is_empty() {
                "no stderr"
            } else {
                stderr.as_str()
            }
        )));
    }

    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((name, app_id)) = line.split_once('\t') else {
            continue;
        };

        let title = name.trim();
        let app_id = app_id.trim();
        if title.is_empty() || app_id.is_empty() {
            continue;
        }
        if should_exclude_non_app_start_reference(title, app_id) {
            continue;
        }
        if is_documentation_like_start_entry_title(title) {
            continue;
        }

        let path = format!("shell:AppsFolder\\{app_id}");
        let id = format!("app:{}", normalize_id_path(&path));
        items.push(SearchItem::new(&id, "app", title, &path));
    }

    Ok(items)
}

#[cfg(target_os = "windows")]
fn dedupe_apps_by_title(items: Vec<SearchItem>) -> Vec<SearchItem> {
    let mut by_title: HashMap<String, SearchItem> = HashMap::new();
    for item in items {
        let title_key = crate::model::normalize_for_search(item.title.trim());
        let key = if title_key.is_empty() {
            format!("path:{}", normalize_id_path(&item.path))
        } else {
            title_key
        };

        match by_title.get(&key) {
            Some(existing) if app_quality_rank(existing) >= app_quality_rank(&item) => {}
            _ => {
                by_title.insert(key, item);
            }
        }
    }

    let mut out: Vec<SearchItem> = by_title.into_values().collect();
    out.sort_by(|a, b| {
        a.title
            .to_ascii_lowercase()
            .cmp(&b.title.to_ascii_lowercase())
    });
    out
}

#[cfg(target_os = "windows")]
fn app_quality_rank(item: &SearchItem) -> u8 {
    let lowered = item.path.trim().to_ascii_lowercase();
    if lowered.starts_with("shell:appsfolder\\") {
        return 3;
    }
    if lowered.ends_with(".lnk") || lowered.ends_with(".exe") {
        return 2;
    }
    1
}

#[cfg(target_os = "windows")]
fn normalize_id_path(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_lowercase()
}

#[cfg(target_os = "windows")]
fn shortcut_has_launch_target(shortcut_path: &Path) -> bool {
    use windows_sys::Win32::UI::Shell::HlinkResolveShortcutToString;

    let wide_shortcut = to_wide(shortcut_path.to_string_lossy().as_ref());
    let mut target: windows_sys::core::PWSTR = std::ptr::null_mut();
    let mut location: windows_sys::core::PWSTR = std::ptr::null_mut();

    let hr =
        unsafe { HlinkResolveShortcutToString(wide_shortcut.as_ptr(), &mut target, &mut location) };
    if hr < 0 {
        return false;
    }

    let resolved_target = pwstr_to_string_and_free(target);
    let resolved_location = pwstr_to_string_and_free(location);

    if shortcut_resolves_to_web_target(&resolved_target)
        || shortcut_resolves_to_web_target(&resolved_location)
    {
        return false;
    }

    let resolved_target = normalize_shortcut_target_path(resolved_target.as_str());
    if resolved_target.is_empty() {
        return false;
    }

    if looks_like_filesystem_path(resolved_target.as_str()) {
        return Path::new(resolved_target.as_str()).exists();
    }

    true
}

#[cfg(target_os = "windows")]
fn resolve_shortcut_target_for_discovery(shortcut_path: &Path) -> Option<String> {
    use windows_sys::Win32::UI::Shell::HlinkResolveShortcutToString;

    let wide_shortcut = to_wide(shortcut_path.to_string_lossy().as_ref());
    let mut target: windows_sys::core::PWSTR = std::ptr::null_mut();
    let mut location: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr =
        unsafe { HlinkResolveShortcutToString(wide_shortcut.as_ptr(), &mut target, &mut location) };
    if hr < 0 {
        return None;
    }

    let resolved_target = pwstr_to_string_and_free(target);
    let resolved_location = pwstr_to_string_and_free(location);
    let preferred = normalize_shortcut_target_path(resolved_target.as_str());
    if !preferred.is_empty() {
        return Some(preferred);
    }
    let fallback = normalize_shortcut_target_path(resolved_location.as_str());
    if fallback.is_empty() {
        None
    } else {
        Some(fallback)
    }
}

#[cfg(target_os = "windows")]
fn should_exclude_non_app_start_reference(title: &str, reference: &str) -> bool {
    if is_excluded_windows_kits_shortcut_reference(reference) {
        return true;
    }
    if shortcut_resolves_to_web_target(reference) {
        return true;
    }
    if has_non_app_document_extension(reference) {
        return true;
    }

    // Extra guard for label-only docs/help shortcuts that might point to local wrappers.
    if is_documentation_like_start_entry_title(title)
        && !reference_points_to_executable_reference(reference)
    {
        return true;
    }

    false
}

#[cfg(target_os = "windows")]
fn discover_windows_search_items(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    show_files: bool,
    show_folders: bool,
    max_items_total: usize,
    max_items_per_root: usize,
) -> Result<Vec<SearchItem>, ProviderError> {
    use std::collections::HashSet;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let roots_joined = join_windows_paths_for_powershell(roots);
    if roots_joined.is_empty() {
        return Ok(Vec::new());
    }
    let excluded_joined = join_windows_paths_for_powershell(excluded_roots);

    let script = r#"
$ErrorActionPreference = 'Stop'
$separator = [char]0x1f
$roots = @()
$excludes = @()
if ($env:SWIFTFIND_WS_ROOTS) { $roots = $env:SWIFTFIND_WS_ROOTS -split $separator }
if ($env:SWIFTFIND_WS_EXCLUDES) { $excludes = $env:SWIFTFIND_WS_EXCLUDES -split $separator }

$conn = New-Object -ComObject ADODB.Connection
$conn.Open("Provider=Search.CollatorDSO;Extended Properties='Application=Windows'")
$seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)

foreach ($root in $roots) {
  if ([string]::IsNullOrWhiteSpace($root)) { continue }
  $scope = $root.Trim().Replace('\', '/')
  if (-not $scope.EndsWith('/')) { $scope += '/' }
  $scope = $scope.Replace("'", "''")
  $query = "SELECT System.ItemPathDisplay, System.ItemName, System.FileAttributes FROM SYSTEMINDEX WHERE scope='file:$scope'"
  $recordset = $conn.Execute($query)

  while (-not $recordset.EOF) {
    $path = [string]$recordset.Fields.Item("System.ItemPathDisplay").Value
    $name = [string]$recordset.Fields.Item("System.ItemName").Value
    $attrsValue = $recordset.Fields.Item("System.FileAttributes").Value
    $attrs = 0
    if ($null -ne $attrsValue -and "$attrsValue" -ne "") { $attrs = [int64]$attrsValue }

    if (-not [string]::IsNullOrWhiteSpace($path)) {
      $skip = $false
      foreach ($exclude in $excludes) {
        if ([string]::IsNullOrWhiteSpace($exclude)) { continue }
        if ($path.StartsWith($exclude, [System.StringComparison]::OrdinalIgnoreCase)) {
          $skip = $true
          break
        }
      }

      if (-not $skip -and $seen.Add($path)) {
        if ([string]::IsNullOrWhiteSpace($name)) { $name = [System.IO.Path]::GetFileName($path) }
        if ([string]::IsNullOrWhiteSpace($name)) { $name = $path }
        $kind = if (($attrs -band 16) -ne 0) { "folder" } else { "file" }
        "{0}`t{1}`t{2}" -f $kind, $name, $path
      }
    }

    $recordset.MoveNext()
  }

  $recordset.Close()
}

$conn.Close()
"#;

    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .env("SWIFTFIND_WS_ROOTS", roots_joined)
        .env("SWIFTFIND_WS_EXCLUDES", excluded_joined)
        .creation_flags(CREATE_NO_WINDOW);

    let output = command.output().map_err(|error| {
        ProviderError::new(format!(
            "Windows Search provider invocation failed: {error}"
        ))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ProviderError::new(format!(
            "Windows Search provider failed (status={}): {}",
            output.status,
            if stderr.is_empty() {
                "no stderr"
            } else {
                stderr.as_str()
            }
        )));
    }

    let mut seen_ids = HashSet::new();
    let normalized_roots = roots
        .iter()
        .map(|root| normalize_root_for_stamp(root))
        .collect::<Vec<_>>();
    let mut root_counts = vec![0_usize; normalized_roots.len()];
    let total_budget = max_items_total.max(1);
    let per_root_budget = max_items_per_root.max(1).min(total_budget);
    let mut skipped_due_cap = 0_usize;
    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(kind_raw) = parts.next() else {
            continue;
        };
        let Some(title_raw) = parts.next() else {
            continue;
        };
        let Some(path_raw) = parts.next() else {
            continue;
        };
        let kind = kind_raw.trim().to_ascii_lowercase();
        if kind != "file" && kind != "folder" {
            continue;
        }
        if kind == "file" && !show_files {
            continue;
        }
        if kind == "folder" && !show_folders {
            continue;
        }
        let path = path_raw.trim();
        if path.is_empty() {
            continue;
        }
        let normalized_path = normalize_id_path(path);
        let root_index = normalized_roots.iter().position(|root| {
            normalized_path == *root
                || (normalized_path.starts_with(root)
                    && normalized_path[root.len()..].starts_with('\\'))
        });
        let Some(root_index) = root_index else {
            continue;
        };
        if items.len() >= total_budget || root_counts[root_index] >= per_root_budget {
            skipped_due_cap = skipped_due_cap.saturating_add(1);
            continue;
        }
        let title = title_raw.trim();
        let display_title = if title.is_empty() { path } else { title };
        let id = format!("{kind}:{normalized_path}");
        if seen_ids.insert(id.clone()) {
            items.push(SearchItem::new(&id, &kind, display_title, path));
            root_counts[root_index] += 1;
        }
    }

    if skipped_due_cap > 0 {
        crate::logging::info(&format!(
            "[swiftfind-core] discovery_cap provider=windows_search skipped_due_cap={} total_cap={} per_root_cap={}",
            skipped_due_cap, total_budget, per_root_budget
        ));
    }

    Ok(items)
}

#[cfg(target_os = "windows")]
fn join_windows_paths_for_powershell(paths: &[PathBuf]) -> String {
    let mut out = Vec::new();
    for path in paths {
        let mut normalized = path.to_string_lossy().replace('/', "\\");
        while normalized.ends_with('\\') && normalized.len() > 3 {
            normalized.pop();
        }
        let trimmed = normalized.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    out.join("\u{1f}")
}

#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn pwstr_to_string_and_free(ptr: windows_sys::core::PWSTR) -> String {
    use windows_sys::Win32::System::Com::CoTaskMemFree;

    if ptr.is_null() {
        return String::new();
    }

    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let out = String::from_utf16_lossy(slice);
        CoTaskMemFree(ptr as _);
        out
    }
}

#[cfg(target_os = "windows")]
fn shortcut_resolves_to_web_target(raw: &str) -> bool {
    let lowered = raw.trim().trim_matches('"').to_ascii_lowercase();
    if lowered.is_empty() {
        return false;
    }
    lowered.starts_with("http://")
        || lowered.starts_with("https://")
        || lowered.starts_with("microsoft-edge:")
        || lowered.starts_with("msedge:")
        || lowered.starts_with("www.")
        || lowered.contains("://")
}

#[cfg(target_os = "windows")]
fn has_non_app_document_extension(value: &str) -> bool {
    let normalized = normalize_shortcut_target_path(value).to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }

    [
        ".url", ".pdf", ".htm", ".html", ".xhtml", ".mht", ".mhtml", ".chm", ".txt", ".md", ".rtf",
        ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".csv", ".xml", ".json", ".yaml",
        ".yml", ".ini", ".log", ".php",
    ]
    .iter()
    .any(|ext| normalized.ends_with(ext))
}

#[cfg(target_os = "windows")]
fn reference_points_to_executable_reference(reference: &str) -> bool {
    let normalized = normalize_shortcut_target_path(reference).to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }

    if normalized.starts_with("shell:") || normalized.starts_with("ms-") {
        return true;
    }

    [
        ".exe",
        ".com",
        ".bat",
        ".cmd",
        ".msc",
        ".ps1",
        ".vbs",
        ".appref-ms",
    ]
    .iter()
    .any(|ext| normalized.ends_with(ext))
}

#[cfg(target_os = "windows")]
fn normalize_shortcut_target_path(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_start_matches('@')
        .trim()
        .to_string()
}

#[cfg(target_os = "windows")]
fn looks_like_filesystem_path(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

#[cfg(target_os = "windows")]
fn is_documentation_like_start_entry_title(title: &str) -> bool {
    let lower = title.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }

    let has_docs = lower.contains("documentation") || lower.contains(" docs");
    let has_sample = lower.contains("sample");
    let has_tools_for = lower.contains("tools for");
    let has_app_word = lower.contains(" app") || lower.contains("apps");
    let has_platform = lower.contains("uwp")
        || lower.contains("desktop")
        || lower.contains("winui")
        || lower.contains("windows sdk");

    (has_docs && has_app_word)
        || (has_sample && (has_app_word || has_platform))
        || (has_tools_for && has_app_word && has_platform)
}

#[cfg(target_os = "windows")]
fn is_excluded_windows_kits_shortcut_reference(value: &str) -> bool {
    let lower = value.trim().replace('/', "\\").to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    if !lower.contains("\\windows kits\\10\\shortcuts\\") {
        return false;
    }
    if !lower.ends_with(".url") {
        return false;
    }
    lower.contains("sample")
        || lower.contains("documentation")
        || lower.contains("toolsdocumentation")
}
