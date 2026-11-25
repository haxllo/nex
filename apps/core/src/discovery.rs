#[cfg(target_os = "windows")]
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::model::SearchItem;

const FILESYSTEM_DISCOVERY_SCHEMA_VERSION: &str = "5";
pub(crate) const TOP_LEVEL_EXCLUDED_DIR_NAMES: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "$recycle.bin",
    "system volume information",
    "$extend",
    "appdata",
];
pub(crate) const ANY_DEPTH_EXCLUDED_DIR_NAMES: &[&str] = &[
    "appdata",
    "node_modules",
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".gradle",
    ".m2",
    ".dropbox.cache",
    ".ssh",
    "target",
    ".bun",
    ".cargo",
    ".npm",
    ".yarn",
    ".nuget",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".pnpm-store",
    ".claude",
    ".commandcode",
    ".grok",
    ".pi",
    ".rustup",
    "EBWebView",
    ".next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    ".angular",
    ".turbo",
    ".parcel-cache",
    ".cache",
    ".nyc_output",
    "coverage",
    ".idea",
    ".vs",
    "obj",
    "CMakeFiles",
    "cmake-build-debug",
    "cmake-build-release",
    "cmake-build-relwithdebinfo",
    ".zig-cache",
    ".zig-global-cache",
    "zig-cache",
    "zig-out",
    ".stack-work",
    ".dart_tool",
    ".swiftpm",
    "DerivedData",
    ".pio",
    ".terraform",
    ".serverless",
    ".aws-sam",
    "bower_components",
    "jspm_packages",
];
pub(crate) const EXCLUDED_FILE_NAMES: &[&str] = &[
    "pagefile.sys",
    "hiberfil.sys",
    "thumbs.db",
    "desktop.ini",
];
/// Directory-name prefixes that are always build/cache output even when the
/// concrete name carries a suffix (e.g. Cursor's `.node_modules.staging-9756.1`).
pub(crate) const EXCLUDED_DIR_PREFIXES: &[&str] = &[".node_modules.", "cmake-build-"];

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

pub struct IncrementalUpdate {
    pub upserts: Vec<SearchItem>,
    pub deleted_ids: Vec<String>,
    /// Directory paths whose whole subtree should be removed (deletes and
    /// rename-old records).
    pub deleted_prefixes: Vec<String>,
    /// `(volume letter, journal id, next usn)` to persist after the caller has
    /// applied the update, so a failed apply never advances the journal.
    pub states: Vec<(char, u64, i64)>,
}

pub trait DiscoveryProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError>;
    fn discover_with_progress(
        &self,
        _on_item_discovered: Option<&mut dyn FnMut(usize)>,
    ) -> Result<Vec<SearchItem>, ProviderError> {
        self.discover()
    }
    /// Stream discovered items to `emit` instead of materializing them all.
    /// The default is fine for the small app providers; the filesystem
    /// provider overrides it so large roots never build a full Vec.
    fn discover_into(
        &self,
        on_item_discovered: Option<&mut dyn FnMut(usize)>,
        emit: &mut dyn FnMut(SearchItem) -> Result<(), ProviderError>,
    ) -> Result<usize, ProviderError> {
        let items = self.discover_with_progress(on_item_discovered)?;
        let count = items.len();
        for item in items {
            emit(item)?;
        }
        Ok(count)
    }
    /// Optional incremental pass. `Some` means the caller applies the update
    /// and skips `discover()`/prune for this cycle.
    fn take_incremental(&self) -> Option<Result<IncrementalUpdate, ProviderError>> {
        None
    }
    /// Called after a full pass's writes and prune completed. Providers that
    /// track a change-journal position persist it here, never earlier.
    fn commit_pass_state(&self) {}
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
            roots: application_shortcut_roots(),
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
            let uninstall_publishers = crate::uninstall_registry::publishers_by_display_name()
                .unwrap_or_else(|error| {
                    crate::logging::warn(&format!(
                        "[nex] uninstall publisher map unavailable: {}",
                        error
                    ));
                    HashMap::new()
                });
            let mut items = Vec::new();
            for root in &self.roots {
                items.extend(discover_start_menu_root(root, &uninstall_publishers)?);
            }
            if let Ok(system_apps) = discover_start_apps(&uninstall_publishers) {
                items.extend(system_apps);
            }
            if let Ok(registered_apps) = discover_registered_app_paths() {
                items.extend(registered_apps);
            }
            items.extend(crate::settings_catalog::settings_page_items());
            items.extend(crate::control_panel::control_panel_items());
            Ok(dedupe_apps_by_title(items))
        }
    }

    fn change_stamp(&self) -> Option<String> {
        // Bump when Start menu discovery/filtering behavior changes so incremental
        // rebuilds do not keep stale cached app entries.
        // 13: entries whose AppsFolder target no longer exists are filtered out,
        // so a cached app list from an older build would keep the stale rows.
        const START_MENU_DISCOVERY_SCHEMA_VERSION: &str = "13";
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
    show_files: bool,
    show_folders: bool,
}

impl FileSystemDiscoveryProvider {
    pub fn new(roots: Vec<PathBuf>, max_depth: usize, excluded_roots: Vec<PathBuf>) -> Self {
        Self::with_options(roots, max_depth, excluded_roots, true, true)
    }

    pub fn with_options(
        roots: Vec<PathBuf>,
        max_depth: usize,
        excluded_roots: Vec<PathBuf>,
        show_files: bool,
        show_folders: bool,
    ) -> Self {
        Self {
            roots,
            excluded_roots,
            max_depth,
            show_files,
            show_folders,
        }
    }

    pub fn backend_label(&self) -> &'static str {
        "walkdir"
    }

    pub fn from_config(
        config: &crate::config::Config,
        max_depth: usize,
    ) -> Self {
        #[cfg(target_os = "windows")]
        if config.file_discovery_backend == crate::config::DiscoveryBackend::Auto {
            crate::logging::info(
                "[nex] file_discovery_backend=auto uses native discovery (MFT for NTFS, walkdir otherwise)",
            );
        }
        Self::with_options(
            config.discovery_roots.clone(),
            max_depth,
            config.discovery_exclude_roots.clone(),
            config.show_files,
            config.show_folders,
        )
    }
}

impl DiscoveryProvider for FileSystemDiscoveryProvider {
    fn provider_name(&self) -> &'static str {
        "filesystem"
    }

    fn discover_into(
        &self,
        on_item_discovered: Option<&mut dyn FnMut(usize)>,
        emit: &mut dyn FnMut(SearchItem) -> Result<(), ProviderError>,
    ) -> Result<usize, ProviderError> {
        if !self.show_files && !self.show_folders {
            return Ok(0);
        }

        clear_pending_states();
        discover_filesystem_walk_into(
            &self.roots,
            &self.excluded_roots,
            self.max_depth,
            self.show_files,
            self.show_folders,
            emit,
            on_item_discovered,
        )
    }

    fn commit_pass_state(&self) {
        commit_pending_states();
    }

    fn take_incremental(&self) -> Option<Result<IncrementalUpdate, ProviderError>> {
        #[cfg(target_os = "windows")]
        {
            return mft_incremental(self);
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    fn discover(&self) -> Result<Vec<SearchItem>, ProviderError> {
        if !self.show_files && !self.show_folders {
            return Ok(Vec::new());
        }

        discover_filesystem_walk(
            &self.roots,
            &self.excluded_roots,
            self.max_depth,
            self.show_files,
            self.show_folders,
            None,
        )
    }

    fn discover_with_progress(
        &self,
        on_item_discovered: Option<&mut dyn FnMut(usize)>,
    ) -> Result<Vec<SearchItem>, ProviderError> {
        if !self.show_files && !self.show_folders {
            return Ok(Vec::new());
        }

        discover_filesystem_walk(
            &self.roots,
            &self.excluded_roots,
            self.max_depth,
            self.show_files,
            self.show_folders,
            on_item_discovered,
        )
    }

    fn change_stamp(&self) -> Option<String> {
        let mut stamp = String::new();
        stamp.push_str("schema=");
        stamp.push_str(FILESYSTEM_DISCOVERY_SCHEMA_VERSION);
        stamp.push(';');
        stamp.push_str("roots=");
        stamp.push_str(&roots_change_stamp(&self.roots));
        stamp.push_str(";exclude=");
        stamp.push_str(&roots_change_stamp(&self.excluded_roots));
        stamp.push_str(";depth=");
        stamp.push_str(&self.max_depth.to_string());
        stamp.push_str(";show_files=");
        stamp.push_str(if self.show_files { "true" } else { "false" });
        stamp.push_str(";show_folders=");
        stamp.push_str(if self.show_folders { "true" } else { "false" });
        Some(stamp)
    }
}

/// Streams walk results to the caller; callers check the budgets before pushing.
struct WalkSink<'a, 'b> {
    emit: &'a mut dyn FnMut(SearchItem) -> Result<(), ProviderError>,
    error: Option<ProviderError>,
    total_added: usize,
    root_added: usize,
    on_item_discovered: Option<&'b mut dyn FnMut(usize)>,
}

impl WalkSink<'_, '_> {
    fn push(&mut self, kind: &str, path: &Path) {
        if self.error.is_some() {
            return;
        }
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let id = format!("{kind}:{}", path.to_string_lossy().to_ascii_lowercase());
        let item = SearchItem::new(&id, kind, &title, &path.to_string_lossy());
        self.total_added += 1;
        self.root_added += 1;
        if let Some(callback) = self.on_item_discovered.as_deref_mut() {
            callback(self.total_added);
        }
        if let Err(error) = (self.emit)(item) {
            self.error = Some(error);
        }
    }
}

/// Breadth-first so a budget cap keeps the shallowest entries of every
/// subtree instead of exhausting itself inside the first deep folder.
fn discover_filesystem_walk_into<'a, 'b>(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    max_depth: usize,
    show_files: bool,
    show_folders: bool,
    emit: &'a mut dyn FnMut(SearchItem) -> Result<(), ProviderError>,
    on_item_discovered: Option<&'b mut dyn FnMut(usize)>,
) -> Result<usize, ProviderError> {
    let exclusion_policy = DiscoveryExclusionPolicy::new(excluded_roots);
    let mut skipped_due_exclusion = 0_usize;
    let walk_depth = effective_walk_depth(max_depth);
    let mut sink = WalkSink {
        emit,
        error: None,
        total_added: 0,
        root_added: 0,
        on_item_discovered,
    };

    // One snapshot per NTFS volume serves every root on it.
    #[cfg(target_os = "windows")]
    let mft_handled = if mft_backend_enabled() {
        try_mft_volumes(
            roots,
            &exclusion_policy,
            show_files,
            show_folders,
            max_depth,
            &mut sink,
            &mut skipped_due_exclusion,
        )
    } else {
        vec![false; roots.len()]
    };

    for (index, root) in roots.iter().enumerate() {
        #[cfg(target_os = "windows")]
        if mft_handled[index] {
            continue;
        }
        if !root.exists() {
            continue;
        }
        if exclusion_policy.should_exclude_path_under_root(root, root) {
            skipped_due_exclusion = skipped_due_exclusion.saturating_add(1);
            continue;
        }
        let Ok(root_meta) = std::fs::metadata(root) else {
            continue;
        };

        sink.root_added = 0;
        if !root_meta.is_dir() {
            if root_meta.is_file() && show_files {
                sink.push("file", root);
            }
            continue;
        }

        let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
        queue.push_back((root.clone(), 0));
        while let Some((dir, depth)) = queue.pop_front() {
            // Unreadable directories are skipped silently.
            let Ok(read_dir) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<std::fs::DirEntry> = read_dir.filter_map(Result::ok).collect();
            entries.sort_by_cached_key(|entry| entry.file_name());

            for entry in entries {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                let is_symlink = file_type.is_symlink();
                // Links are classified through their target but never descended.
                let (is_dir, is_file) = if is_symlink {
                    match std::fs::metadata(&path) {
                        Ok(meta) => (meta.is_dir(), meta.is_file()),
                        Err(_) => (false, false),
                    }
                } else {
                    (file_type.is_dir(), file_type.is_file())
                };

                if exclusion_policy.should_exclude_entry_under_root(&path, root, is_dir) {
                    skipped_due_exclusion = skipped_due_exclusion.saturating_add(1);
                    continue;
                }

                if is_dir {
                    if show_folders {
                        sink.push("folder", &path);
                    }
                    if !is_symlink && depth + 1 < walk_depth {
                        queue.push_back((path, depth + 1));
                    }
                } else if is_file && show_files {
                    sink.push("file", &path);
                }
            }
        }
    }

    if skipped_due_exclusion > 0 {
        crate::logging::info(&format!(
            "[nex] discovery_exclusion provider=filesystem skipped={} policy_schema={}",
            skipped_due_exclusion, FILESYSTEM_DISCOVERY_SCHEMA_VERSION
        ));
    }

    if let Some(error) = sink.error {
        return Err(error);
    }
    Ok(sink.total_added)
}

fn discover_filesystem_walk(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    max_depth: usize,
    show_files: bool,
    show_folders: bool,
    on_item_discovered: Option<&mut dyn FnMut(usize)>,
) -> Result<Vec<SearchItem>, ProviderError> {
    let mut out = Vec::new();
    {
        let mut emit = |item: SearchItem| {
            out.push(item);
            Ok(())
        };
        discover_filesystem_walk_into(
            roots,
            excluded_roots,
            max_depth,
            show_files,
            show_folders,
            &mut emit,
            on_item_discovered,
        )?;
    }
    Ok(out)
}

/// Config depth 0 means unlimited.
fn effective_walk_depth(max_depth: usize) -> usize {
    if max_depth == 0 {
        usize::MAX
    } else {
        max_depth
    }
}

/// MFT is the primary backend for NTFS roots; the directory walk is the
/// fallback (non-NTFS, helper unavailable, unlocked/denied volumes).
/// `NEX_DISCOVERY_BACKEND=walkdir` forces the old path.
#[cfg(target_os = "windows")]
fn mft_backend_enabled() -> bool {
    match std::env::var("NEX_DISCOVERY_BACKEND") {
        Ok(value) => !matches!(value.trim().to_ascii_lowercase().as_str(), "walkdir" | "off"),
        Err(_) => true,
    }
}

/// Take one snapshot per NTFS volume for the roots that live on it. The
/// returned flags mark roots that were fully enumerated, so the caller must not
/// walk them; unmarked roots (non-NTFS, helper unavailable) fall back to walking.
#[cfg(target_os = "windows")]
fn try_mft_volumes(
    roots: &[PathBuf],
    exclusion_policy: &DiscoveryExclusionPolicy,
    show_files: bool,
    show_folders: bool,
    max_depth: usize,
    sink: &mut WalkSink<'_, '_>,
    skipped_due_exclusion: &mut usize,
) -> Vec<bool> {
    use std::collections::BTreeMap;

    let mut handled = vec![false; roots.len()];
    let mut by_volume: BTreeMap<char, Vec<usize>> = BTreeMap::new();
    for (index, root) in roots.iter().enumerate() {
        let Some(letter) = crate::mft_discovery::root_volume_letter(root) else {
            continue;
        };
        if !crate::mft_discovery::is_fixed_volume(letter)
            || !crate::mft_discovery::is_ntfs_volume(letter)
        {
            continue;
        }
        by_volume.entry(letter).or_default().push(index);
    }
    for (letter, indexes) in by_volume {
        let volume_roots: Vec<&Path> = indexes.iter().map(|&i| roots[i].as_path()).collect();
        if try_mft_volume(
            letter,
            &volume_roots,
            exclusion_policy,
            show_files,
            show_folders,
            max_depth,
            sink,
            skipped_due_exclusion,
        ) {
            for index in indexes {
                handled[index] = true;
            }
        }
    }
    handled
}

/// Stream one full NTFS volume snapshot and emit every entry that belongs to
/// at least one of `roots`. Returns true when the snapshot was available (even
/// with zero matches).
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn try_mft_volume(
    letter: char,
    roots: &[&Path],
    exclusion_policy: &DiscoveryExclusionPolicy,
    show_files: bool,
    show_folders: bool,
    max_depth: usize,
    sink: &mut WalkSink<'_, '_>,
    skipped_due_exclusion: &mut usize,
) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    // The helper starts asynchronously with the UI; give the first volume time
    // to see its pipe, then keep later attempts short. When the helper is
    // absent (portable run, no scheduled task), skip MFT for a while so the
    // walk fallback is not delayed on every volume.
    static FIRST_ATTEMPT: AtomicBool = AtomicBool::new(true);
    static BACKOFF_UNTIL: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    if let Ok(guard) = BACKOFF_UNTIL.lock() {
        if guard.is_some_and(|until| Instant::now() < until) {
            return false;
        }
    }
    let timeout_ms = if FIRST_ATTEMPT.swap(false, Ordering::Relaxed) {
        6000
    } else {
        1000
    };

    let started = Instant::now();
    let matchers: Vec<MftRootMatcher<'_>> = roots.iter().map(|root| MftRootMatcher::new(root)).collect();
    let mut matched = 0usize;
    let mut kept = 0usize;

    let result = crate::mft_discovery::stream_snapshot_into(
        letter,
        crate::mft_discovery::SnapshotRequest::Full,
        timeout_ms,
        |record| {
            let Some(path) = record.path.as_deref() else {
                return Ok(());
            };
            let is_dir = record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            match classify_mft_entry(path, is_dir, &matchers, max_depth, exclusion_policy) {
                MftEntryVerdict::OutsideRoots => {}
                MftEntryVerdict::TooDeep => matched += 1,
                MftEntryVerdict::Excluded => {
                    matched += 1;
                    *skipped_due_exclusion = skipped_due_exclusion.saturating_add(1);
                }
                MftEntryVerdict::Keep => {
                    matched += 1;
                    if is_dir && show_folders {
                        sink.push("folder", Path::new(path));
                        kept += 1;
                    } else if !is_dir && show_files {
                        sink.push("file", Path::new(path));
                        kept += 1;
                    }
                }
            }
            Ok(())
        },
    );

    let meta = match result {
        Ok(meta) => {
            if let Ok(mut guard) = BACKOFF_UNTIL.lock() {
                *guard = None;
            }
            meta
        }
        Err(error) => {
            if let Ok(mut guard) = BACKOFF_UNTIL.lock() {
                *guard = Some(Instant::now() + std::time::Duration::from_secs(60));
            }
            crate::logging::info(&format!(
                "[nex] discovery_mft volume={letter} roots={} unavailable: {error}",
                roots.len()
            ));
            return false;
        }
    };
    // The journal position is committed by `commit_pass_state` only after the
    // pass's writes and prune landed; advancing it here would let an
    // interrupted pass leave a position the index never caught up to.
    if !meta.delta {
        record_pending_state(letter, meta.journal_id, meta.next_usn);
    }

    crate::logging::info(&format!(
        "[nex] discovery_mft volume={letter} roots={} mode={} records={} matched={} kept={} depth={} elapsed_ms={} journal={} next_usn={}",
        roots.len(),
        if meta.delta { "delta" } else { "full" },
        meta.records,
        matched,
        kept,
        max_depth,
        started.elapsed().as_millis(),
        meta.journal_id,
        meta.next_usn
    ));
    true
}

/// A configured root prepared once per volume pass for matching snapshot paths.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct MftRootMatcher<'a> {
    root: &'a Path,
    lower: String,
    ends_separator: bool,
    depth: usize,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl<'a> MftRootMatcher<'a> {
    fn new(root: &'a Path) -> Self {
        let lower = root.to_string_lossy().to_ascii_lowercase();
        Self {
            root,
            ends_separator: lower.ends_with('\\') || lower.ends_with('/'),
            lower,
            depth: root.components().count(),
        }
    }

    /// Strictly below the root: the root itself and sibling prefixes
    /// (`C:\Users\me2` vs `C:\Users\me`) do not match.
    fn contains(&self, lower_path: &str) -> bool {
        if lower_path == self.lower || !lower_path.starts_with(&self.lower) {
            return false;
        }
        self.ends_separator
            || matches!(
                lower_path.as_bytes().get(self.lower.len()),
                Some(b'\\') | Some(b'/')
            )
    }
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum MftEntryVerdict {
    OutsideRoots,
    TooDeep,
    Excluded,
    Keep,
}

/// Decide one snapshot entry against every root on its volume. An entry is
/// kept once when any containing root accepts it (depth and exclusions are
/// per root), so overlapping roots never duplicate items and the result equals
/// the union of the per-root passes.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn classify_mft_entry(
    path: &str,
    is_dir: bool,
    roots: &[MftRootMatcher<'_>],
    max_depth: usize,
    policy: &DiscoveryExclusionPolicy,
) -> MftEntryVerdict {
    let lower = path.to_ascii_lowercase();
    let path_buf = PathBuf::from(path);
    let path_depth = path_buf.components().count();
    let mut contained = false;
    let mut excluded = false;
    for matcher in roots {
        if !matcher.contains(&lower) {
            continue;
        }
        contained = true;
        if max_depth != 0 && path_depth.saturating_sub(matcher.depth) > max_depth {
            continue;
        }
        if policy.should_exclude_entry_under_root(&path_buf, matcher.root, is_dir) {
            excluded = true;
            continue;
        }
        return MftEntryVerdict::Keep;
    }
    match (contained, excluded) {
        (false, _) => MftEntryVerdict::OutsideRoots,
        (true, true) => MftEntryVerdict::Excluded,
        (true, false) => MftEntryVerdict::TooDeep,
    }
}

const HELPER_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);
const HELPER_WAIT_FIRST_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
const HELPER_WAIT_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(16);

/// Next pause while waiting for the elevated helper to come up: 2 s doubling to
/// a 16 s cap, clipped to the remaining budget. `None` once the budget is spent.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn helper_wait_delay(
    attempt: u32,
    waited: std::time::Duration,
    budget: std::time::Duration,
) -> Option<std::time::Duration> {
    let remaining = budget.checked_sub(waited).filter(|left| !left.is_zero())?;
    let delay = HELPER_WAIT_FIRST_DELAY
        .saturating_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
        .min(HELPER_WAIT_MAX_DELAY);
    Some(delay.min(remaining))
}

#[cfg(target_os = "windows")]
static HELPER_WAIT_EXHAUSTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Warm start: when a saved journal position exists but the elevated helper has
/// not started yet (late at login), wait for it with backoff so the cycle can
/// run the USN delta instead of falling back to a full walk. The existing
/// index keeps serving meanwhile. Call this *before* the cycle takes the
/// service lock. After one expired wait later calls only probe once, so a
/// machine without a helper converges through the usual fallback.
#[cfg(target_os = "windows")]
pub fn defer_until_helper_ready() {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    if !mft_backend_enabled() {
        return;
    }
    // Listen for the helper's command channel before anything waits on it.
    crate::mft_channel::ensure_started();
    let letters: Vec<char> = ('A'..='Z')
        .filter(|&letter| crate::mft_discovery::read_processed_state(letter).is_some())
        .collect();
    if letters.is_empty() {
        return;
    }
    let budget = if HELPER_WAIT_EXHAUSTED.load(Ordering::Relaxed) {
        Duration::ZERO
    } else {
        HELPER_WAIT_BUDGET
    };
    let started = Instant::now();
    let mut attempt = 0u32;
    loop {
        if crate::mft_discovery::helper_is_up(&letters) {
            HELPER_WAIT_EXHAUSTED.store(false, Ordering::Relaxed);
            if attempt > 0 {
                crate::logging::info(&format!(
                    "[nex] discovery_delta_defer helper_ready attempts={attempt} waited_ms={}",
                    started.elapsed().as_millis()
                ));
            }
            return;
        }
        match helper_wait_delay(attempt, started.elapsed(), budget) {
            Some(delay) => {
                if attempt == 0 {
                    crate::logging::info(&format!(
                        "[nex] discovery_delta_defer helper_not_ready volumes={}; retrying for up to {}s",
                        letters.len(),
                        budget.as_secs()
                    ));
                }
                std::thread::sleep(delay);
                attempt += 1;
            }
            None => {
                if !budget.is_zero() {
                    HELPER_WAIT_EXHAUSTED.store(true, Ordering::Relaxed);
                    crate::logging::info(&format!(
                        "[nex] discovery_delta_defer gave_up waited_ms={}; falling back",
                        started.elapsed().as_millis()
                    ));
                }
                return;
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn defer_until_helper_ready() {}

const FILE_RENAME_OLD_NAME: u32 = 0x0000_0001;
const FILE_RENAME_NEW_NAME: u32 = 0x0000_0002;
const FILE_DELETE: u32 = 0x0000_0200;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

fn path_under_root(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    let root = root.to_string_lossy().to_ascii_lowercase();
    if path == root || !path.starts_with(&root) {
        return false;
    }
    if root.ends_with('\\') || root.ends_with('/') {
        return true;
    }
    matches!(path.as_bytes().get(root.len()), Some(b'\\') | Some(b'/'))
}

/// How the delta pass treats one NTFS volume given its saved journal position.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum DeltaVolumePlan {
    Delta,
    FullPass,
    SkipNoJournal,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn plan_delta_volume(has_saved_position: bool, full_pass_tried: bool) -> DeltaVolumePlan {
    match (has_saved_position, full_pass_tried) {
        (true, _) => DeltaVolumePlan::Delta,
        (false, false) => DeltaVolumePlan::FullPass,
        (false, true) => DeltaVolumePlan::SkipNoJournal,
    }
}

/// Keep the earliest position per journal: roots on one volume snapshot at
/// different times, and the oldest snapshot bounds what the pass observed.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn merge_pending_state(
    existing: Option<(u64, i64)>,
    journal_id: u64,
    next_usn: i64,
) -> (u64, i64) {
    match existing {
        Some((id, usn)) if id == journal_id => (id, usn.min(next_usn)),
        _ => (journal_id, next_usn),
    }
}

#[cfg(target_os = "windows")]
static PENDING_STATES: std::sync::Mutex<std::collections::BTreeMap<char, (u64, i64)>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
#[cfg(target_os = "windows")]
static FULL_PASS_TRIED: std::sync::Mutex<std::collections::BTreeSet<char>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[cfg(target_os = "windows")]
fn record_pending_state(letter: char, journal_id: u64, next_usn: i64) {
    if let Ok(mut guard) = PENDING_STATES.lock() {
        let merged = merge_pending_state(guard.get(&letter).copied(), journal_id, next_usn);
        guard.insert(letter, merged);
    }
}

#[cfg(target_os = "windows")]
fn clear_pending_states() {
    if let Ok(mut guard) = PENDING_STATES.lock() {
        guard.clear();
    }
}

#[cfg(not(target_os = "windows"))]
fn clear_pending_states() {}

#[cfg(target_os = "windows")]
fn commit_pending_states() {
    let pending = match PENDING_STATES.lock() {
        Ok(mut guard) => std::mem::take(&mut *guard),
        Err(_) => return,
    };
    for (letter, (journal_id, next_usn)) in pending {
        crate::mft_discovery::write_processed_state(letter, journal_id, next_usn);
    }
}

#[cfg(not(target_os = "windows"))]
fn commit_pending_states() {}

#[cfg(target_os = "windows")]
fn full_pass_tried(letter: char) -> bool {
    FULL_PASS_TRIED
        .lock()
        .map(|guard| guard.contains(&letter))
        .unwrap_or(true)
}

#[cfg(target_os = "windows")]
fn mark_full_pass_tried(letter: char) {
    if let Ok(mut guard) = FULL_PASS_TRIED.lock() {
        guard.insert(letter);
    }
}

/// USN delta pass. `None` means a full pass is required: no saved journal
/// position, helper unavailable, journal invalid/rolled, directory changes,
/// or an unresolvable path. Non-delete records upsert; delete and rename-old
/// records remove the old path.
#[cfg(target_os = "windows")]
fn mft_incremental(
    provider: &FileSystemDiscoveryProvider,
) -> Option<Result<IncrementalUpdate, ProviderError>> {
    use crate::mft_discovery;
    use std::collections::{BTreeMap, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};

    // The helper starts asynchronously with the UI; the first delta attempt
    // waits for its pipe, later volumes reuse the same session.
    static FIRST_ATTEMPT: AtomicBool = AtomicBool::new(true);
    let timeout_ms = if FIRST_ATTEMPT.swap(false, Ordering::Relaxed) {
        6000
    } else {
        1500
    };

    if !mft_backend_enabled() || provider.roots.is_empty() {
        return None;
    }
    let policy = DiscoveryExclusionPolicy::new(&provider.excluded_roots);
    let mut roots_by_volume: BTreeMap<char, Vec<&Path>> = BTreeMap::new();
    for root in &provider.roots {
        // Non-NTFS roots (FAT32/exFAT/network) have no USN journal. They are
        // covered by the full pass after a root-set change and by the file
        // watchers; one of them must never disable the delta for the NTFS
        // roots alongside it.
        let Some(letter) = mft_discovery::root_volume_letter(root) else {
            continue;
        };
        if !mft_discovery::is_fixed_volume(letter) || !mft_discovery::is_ntfs_volume(letter) {
            continue;
        }
        roots_by_volume
            .entry(letter)
            .or_default()
            .push(root.as_path());
    }
    if roots_by_volume.is_empty() {
        return None;
    }

    let mut upserts = Vec::new();
    let mut deleted = Vec::new();
    let mut deleted_prefixes: Vec<String> = Vec::new();
    let mut seen_upserts: HashSet<String> = HashSet::new();
    let mut seen_deletes: HashSet<String> = HashSet::new();
    let mut states = Vec::new();
    let mut skipped_unresolved = 0usize;
    let started = std::time::Instant::now();

    for (letter, roots) in roots_by_volume {
        let saved = mft_discovery::read_processed_state(letter);
        match plan_delta_volume(saved.is_some(), full_pass_tried(letter)) {
            DeltaVolumePlan::Delta => {}
            DeltaVolumePlan::FullPass => {
                // No usable position (state lost, or the earlier pass fell
                // back to a walk): the index may never have seen this volume
                // through the journal, so one full pass must record it.
                mark_full_pass_tried(letter);
                crate::logging::info(&format!(
                    "[nex] discovery_delta volume={letter} skip=no_saved_position; running a full pass"
                ));
                return Some(Err(ProviderError::new(
                    "MFT saved position missing; running a full pass",
                )));
            }
            DeltaVolumePlan::SkipNoJournal => {
                // Already tried this run and still no position (journal
                // disabled / helper cannot open it): the watchers cover it and
                // a full pass per cycle would only repeat the same walk.
                crate::logging::info(&format!(
                    "[nex] discovery_delta volume={letter} skip=no_journal"
                ));
                continue;
            }
        };
        let Some((journal_id, start_usn)) = saved else {
            continue;
        };
        let snapshot = match mft_discovery::fetch_delta(letter, journal_id, start_usn, timeout_ms) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let (reason, message) = match &error {
                    mft_discovery::FetchError::HelperNotReady(_) => (
                        "helper_not_ready",
                        "MFT helper not ready; running a full pass",
                    ),
                    mft_discovery::FetchError::JournalInvalid(_) => (
                        "journal_invalid",
                        "MFT journal invalid; running a full pass",
                    ),
                    mft_discovery::FetchError::Failed(_) => (
                        "stream_failed",
                        "MFT delta stream failed; running a full pass",
                    ),
                };
                crate::logging::info(&format!(
                    "[nex] discovery_delta volume={letter} skip={reason}: {error}"
                ));
                return Some(Err(ProviderError::new(message)));
            }
        };
        if !snapshot.delta {
            crate::logging::info(&format!(
                "[nex] discovery_delta volume={letter} skip=journal_invalid"
            ));
            return Some(Err(ProviderError::new(
                "MFT journal invalid; running a full pass",
            )));
        }
        for record in &snapshot.records {
            let Some(path_text) = record.path.as_deref() else {
                // Protected/orphaned parents cannot be resolved by file id.
                // Ignore them: the stale pruner and periodic full pass cover
                // any rare miss under a root.
                skipped_unresolved += 1;
                continue;
            };
            let path = PathBuf::from(path_text);
            let Some(root) = roots
                .iter()
                .copied()
                .find(|root| path_under_root(&path, *root))
            else {
                continue;
            };
            if provider.max_depth != 0
                && path
                    .components()
                    .count()
                    .saturating_sub(root.components().count())
                    > provider.max_depth
            {
                continue;
            }
            if policy.should_exclude_entry_under_root(
                &path,
                root,
                record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            ) {
                continue;
            }
            if record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                if record.reason & (FILE_DELETE | FILE_RENAME_OLD_NAME) != 0 {
                    let lower = path.to_string_lossy().to_ascii_lowercase();
                    if seen_deletes.insert(format!("folder:{lower}")) {
                        deleted.push(format!("folder:{lower}"));
                    }
                    deleted_prefixes.push(path.to_string_lossy().to_string());
                    continue;
                }
                if record.reason & FILE_RENAME_NEW_NAME != 0 {
                    // The moved subtree's children are not in the delta.
                    // Enumerate the new path locally instead of a full pass;
                    // the rename-old record already removed the old subtree.
                    if !path.exists() {
                        continue;
                    }
                    if provider.show_folders {
                        let lower = path.to_string_lossy().to_ascii_lowercase();
                        let id = format!("folder:{lower}");
                        if seen_upserts.insert(id.clone()) {
                            let title = path
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.to_string_lossy().to_string());
                            upserts.push(SearchItem::new(
                                &id,
                                "folder",
                                &title,
                                &path.to_string_lossy(),
                            ));
                        }
                    }
                    let mut moved = Vec::new();
                    collect_subtree_items(
                        &path,
                        provider.max_depth,
                        provider.show_files,
                        provider.show_folders,
                        &policy,
                        root,
                        &mut moved,
                    );
                    for item in moved {
                        if seen_upserts.insert(item.id.clone()) {
                            upserts.push(item);
                        }
                    }
                    continue;
                }
                if provider.show_folders {
                    let lower = path.to_string_lossy().to_ascii_lowercase();
                    let id = format!("folder:{lower}");
                    if seen_upserts.insert(id.clone()) {
                        let title = path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string());
                        upserts.push(SearchItem::new(
                            &id,
                            "folder",
                            &title,
                            &path.to_string_lossy(),
                        ));
                    }
                }
                continue;
            }
            let lower = path.to_string_lossy().to_ascii_lowercase();
            let id = format!("file:{lower}");
            if record.reason & (FILE_DELETE | FILE_RENAME_OLD_NAME) != 0 {
                if seen_deletes.insert(id.clone()) {
                    deleted.push(id);
                }
                continue;
            }
            if !provider.show_files || !path.exists() {
                continue;
            }
            if seen_upserts.insert(id.clone()) {
                let title = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                upserts.push(SearchItem::new(
                    &id,
                    "file",
                    &title,
                    &path.to_string_lossy(),
                ));
            }
        }
        states.push((letter, snapshot.journal_id, snapshot.next_usn));
    }

    // A path recreated in the same batch must survive its delete.
    deleted.retain(|id| !seen_upserts.contains(id));
    // Unresolved records are transient/inaccessible entries (deleted between
    // the journal write and the read, or protected parents). They are not
    // actionable here; the stale pruner and the periodic full pass reconcile.
    crate::logging::info(&format!(
        "[nex] discovery_delta provider=filesystem upserts={} deletes={} unresolved={} elapsed_ms={}",
        upserts.len(),
        deleted.len(),
        skipped_unresolved,
        started.elapsed().as_millis()
    ));
    Some(Ok(IncrementalUpdate {
        upserts,
        deleted_ids: deleted,
        deleted_prefixes,
        states,
    }))
}

/// Enumerate one moved directory locally (no MFT) so a rename can apply the
/// new subtree as part of the delta instead of a full discovery pass.
#[cfg(target_os = "windows")]
fn collect_subtree_items(
    dir: &Path,
    max_depth: usize,
    show_files: bool,
    show_folders: bool,
    policy: &DiscoveryExclusionPolicy,
    root: &Path,
    out: &mut Vec<SearchItem>,
) {
    let limit = if max_depth == 0 {
        usize::MAX
    } else {
        max_depth
    };
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((dir.to_path_buf(), 0));
    while let Some((current, depth)) = queue.pop_front() {
        let Ok(read_dir) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read_dir.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let is_symlink = file_type.is_symlink();
            let (is_dir, is_file) = if is_symlink {
                match std::fs::metadata(&path) {
                    Ok(meta) => (meta.is_dir(), meta.is_file()),
                    Err(_) => (false, false),
                }
            } else {
                (file_type.is_dir(), file_type.is_file())
            };
            if policy.should_exclude_entry_under_root(&path, root, is_dir) {
                continue;
            }
            let lower = path.to_string_lossy().to_ascii_lowercase();
            let title = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            if is_dir {
                if show_folders {
                    out.push(SearchItem::new(
                        &format!("folder:{lower}"),
                        "folder",
                        &title,
                        &path.to_string_lossy(),
                    ));
                }
                if !is_symlink && depth + 1 < limit {
                    queue.push_back((path, depth + 1));
                }
            } else if is_file && show_files {
                out.push(SearchItem::new(
                    &format!("file:{lower}"),
                    "file",
                    &title,
                    &path.to_string_lossy(),
                ));
            }
        }
    }
}

/// Uncapped raw entry counts for one discovery root, using discovery's
/// exclusion policy and depth limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RootFsCount {
    pub files: u64,
    pub folders: u64,
    pub excluded: u64,
    pub errors: u64,
    /// Some directory at the depth limit has contents that were not walked.
    pub depth_limited: bool,
}

pub fn count_filesystem_entries_for_root(
    root: &Path,
    cfg: &crate::config::Config,
) -> RootFsCount {
    let mut count = RootFsCount::default();
    if !root.exists() {
        return count;
    }
    let policy = DiscoveryExclusionPolicy::new(&cfg.discovery_exclude_roots);
    if policy.should_exclude_path_under_root(root, root) {
        count.excluded += 1;
        return count;
    }
    let max_depth = effective_walk_depth(cfg.discovery_max_depth as usize);
    let mut excluded = 0_u64;
    let walker = walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| {
            let skip = policy.should_exclude_path_under_root(entry.path(), root);
            if skip && entry.path() != root {
                excluded += 1;
            }
            !skip
        });
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                count.errors += 1;
                continue;
            }
        };
        let ft = entry.file_type();
        if ft.is_dir() {
            if entry.depth() > 0 {
                count.folders += 1;
            }
            if entry.depth() >= max_depth
                && std::fs::read_dir(entry.path())
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false)
            {
                count.depth_limited = true;
            }
        } else if ft.is_file() {
            count.files += 1;
        }
    }
    count.excluded += excluded;
    count
}

#[derive(Debug, Clone)]
pub(crate) struct DiscoveryExclusionPolicy {
    excluded_roots: Vec<String>,
    user_excluded_roots: Vec<String>,
    top_level_dir_names: HashSet<&'static str>,
    any_depth_dir_names: HashSet<&'static str>,
    file_names: HashSet<&'static str>,
}

impl DiscoveryExclusionPolicy {
    pub(crate) fn new(user_excluded_roots: &[PathBuf]) -> Self {
        Self {
            excluded_roots: normalized_exclusion_roots(&builtin_exclusion_roots()),
            user_excluded_roots: normalized_exclusion_roots(user_excluded_roots),
            top_level_dir_names: TOP_LEVEL_EXCLUDED_DIR_NAMES.iter().copied().collect(),
            any_depth_dir_names: ANY_DEPTH_EXCLUDED_DIR_NAMES.iter().copied().collect(),
            file_names: EXCLUDED_FILE_NAMES.iter().copied().collect(),
        }
    }

    pub(crate) fn should_exclude_path_under_root(&self, path: &Path, root: &Path) -> bool {
        self.exclude_under_root(path, root, || path.is_dir())
    }

    /// Same decision as `should_exclude_path_under_root` for callers that
    /// already know whether `path` is a directory, so no filesystem stat is
    /// needed per entry.
    pub(crate) fn should_exclude_entry_under_root(
        &self,
        path: &Path,
        root: &Path,
        is_dir: bool,
    ) -> bool {
        self.exclude_under_root(path, root, || is_dir)
    }

    fn exclude_under_root(
        &self,
        path: &Path,
        root: &Path,
        is_dir: impl FnOnce() -> bool,
    ) -> bool {
        // The explicitly configured root is never excluded.
        if path == root {
            return false;
        }

        let Ok(relative) = path.strip_prefix(root) else {
            // Path outside the explicit root: check absolute exclusion (built-in + user).
            return is_path_under_any_excluded_root(path, &self.excluded_roots)
                || is_path_under_any_excluded_root(path, &self.user_excluded_roots);
        };
        // Path under the explicit root: check user-provided absolute exclusions
        // and component-based exclusions, but NOT built-in exclusions.
        if is_path_under_any_excluded_root(path, &self.user_excluded_roots) {
            return true;
        }
        let components = relative
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(value) => {
                    Some(value.to_string_lossy().to_ascii_lowercase())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if components.is_empty() {
            return false;
        }

        if self.top_level_dir_names.contains(components[0].as_str()) {
            return true;
        }

        for (index, component) in components.iter().enumerate() {
            let is_last = index + 1 == components.len();
            if EXCLUDED_DIR_PREFIXES
                .iter()
                .any(|prefix| component.starts_with(prefix))
            {
                return true;
            }
            if self.file_names.contains(component.as_str()) {
                return true;
            }
            if self.any_depth_dir_names.contains(component.as_str()) {
                // A trailing match only excludes directories, so files that
                // merely share the name stay discoverable.
                return !is_last || is_dir();
            }
        }

        false
    }
}

fn builtin_exclusion_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();

        // The app's own data holds the SQLite store and Tantivy segments, which
        // change on every write. Indexing them feeds the watcher and the prune.
        roots.push(crate::config::stable_app_data_dir());

        if let Ok(system_drive) = std::env::var("SystemDrive") {
            let trimmed = system_drive.trim();
            if !trimmed.is_empty() {
                let drive_root = PathBuf::from(format!("{trimmed}\\"));
                roots.push(drive_root.join("Windows"));
                roots.push(drive_root.join("Program Files"));
                roots.push(drive_root.join("Program Files (x86)"));
                roots.push(drive_root.join("$Recycle.Bin"));
                roots.push(drive_root.join("System Volume Information"));
                roots.push(drive_root.join("pagefile.sys"));
                roots.push(drive_root.join("hiberfil.sys"));
            }
        }

        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let local = PathBuf::from(local_app_data.trim());
            if !local.as_os_str().is_empty() {
                roots.push(local.join("Temp"));
                roots.push(local.join("Microsoft").join("Windows").join("INetCache"));
            }
        }

        if let Ok(app_data) = std::env::var("APPDATA") {
            let roaming = PathBuf::from(app_data.trim());
            if !roaming.as_os_str().is_empty() {
                if let Some(parent) = roaming.parent().and_then(|path| path.parent()) {
                    roots.push(parent.join("AppData"));
                    roots.push(parent.join(".ssh"));
                }
            }
        }

        roots
    }

    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

fn roots_change_stamp(roots: &[PathBuf]) -> String {
    // Identity + existence only. Live per-root metadata (child counts,
    // latest mtimes) changed constantly on busy roots, so the stamp
    // mismatched on every startup and forced a full ~13k-file walk.
    // Content freshness is handled by the reconcile interval and the
    // directory watcher instead.
    let mut parts = Vec::with_capacity(roots.len());
    for root in roots {
        let normalized = normalize_root_for_stamp(root);
        let (exists, _, _, _) = quick_path_fingerprint(root);
        parts.push(format!("{normalized}:{exists}"));
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
pub(crate) fn application_shortcut_roots() -> Vec<PathBuf> {
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

    if let Ok(profile) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(profile).join("Desktop"));
    }
    if let Ok(public) = std::env::var("PUBLIC") {
        roots.push(PathBuf::from(public).join("Desktop"));
    }

    roots.sort();
    roots.dedup();
    roots
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn application_shortcut_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn discover_start_menu_root(
    root: &Path,
    uninstall_publishers: &HashMap<String, String>,
) -> Result<Vec<SearchItem>, ProviderError> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    #[derive(Debug, Clone)]
    struct StartMenuCandidate {
        path: PathBuf,
        title: String,
        ext: String,
        shortcut_target: Option<String>,
    }

    let mut candidates: Vec<StartMenuCandidate> = Vec::new();
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
        let mut resolved_shortcut_target = None;
        if ext == "lnk" {
            resolved_shortcut_target = resolve_shortcut_target_for_discovery(path);
            if let Some(shortcut_target) = resolved_shortcut_target.as_deref() {
                if should_exclude_non_app_start_reference(title.as_str(), shortcut_target) {
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

        candidates.push(StartMenuCandidate {
            path: path.to_path_buf(),
            title,
            ext,
            shortcut_target: resolved_shortcut_target,
        });
    }

    // Prune start-menu variant shortcuts: when several shortcuts target the
    // same executable and exactly one of them maps to a registered uninstall
    // DisplayName, keep only that canonical entry (e.g. VLC's "… skinned" /
    // "… - reset preferences" variants vs "VLC media player").
    let mut target_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, candidate) in candidates.iter().enumerate() {
        if candidate.ext != "lnk" {
            continue;
        }
        let Some(target) = candidate.shortcut_target.as_deref() else {
            continue;
        };
        let key = normalize_shortcut_target_path(target).to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        target_groups.entry(key).or_default().push(idx);
    }
    let mut drop_indices: HashSet<usize> = HashSet::new();
    for (_, group) in target_groups {
        if group.len() < 2 {
            continue;
        }
        let canonical: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&idx| {
                publisher_from_uninstall_map(candidates[idx].title.as_str(), uninstall_publishers)
                    .is_some()
            })
            .collect();
        if canonical.len() == 1 {
            let keep = canonical[0];
            for &idx in &group {
                if idx != keep {
                    drop_indices.insert(idx);
                }
            }
        }
    }
    if !drop_indices.is_empty() {
        let mut kept = Vec::with_capacity(
            candidates.len().saturating_sub(drop_indices.len()),
        );
        for (idx, candidate) in candidates.into_iter().enumerate() {
            if !drop_indices.contains(&idx) {
                kept.push(candidate);
            }
        }
        candidates = kept;
    }

    let mut exe_paths = HashSet::new();
    for candidate in &candidates {
        if candidate.ext == "exe" {
            let exe = normalize_shortcut_target_path(candidate.path.to_string_lossy().as_ref());
            if !exe.is_empty() {
                exe_paths.insert(exe);
            }
            continue;
        }
        if let Some(target) = candidate.shortcut_target.as_deref() {
            let normalized_target = normalize_shortcut_target_path(target);
            if looks_like_filesystem_path(normalized_target.as_str())
                && normalized_target.to_ascii_lowercase().ends_with(".exe")
            {
                exe_paths.insert(normalized_target);
            }
        }
    }
    let mut exe_paths_vec: Vec<String> = exe_paths.into_iter().collect();
    exe_paths_vec.sort();
    let exe_publishers = load_exe_company_names(&exe_paths_vec).unwrap_or_default();

    let mut items = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let path_text = candidate.path.to_string_lossy().to_string();
        let id = format!("app:{path_text}");
        let mut subtitle = String::new();

        if let Some(publisher) =
            publisher_from_uninstall_map(candidate.title.as_str(), uninstall_publishers)
        {
            subtitle = publisher;
        }

        if subtitle.trim().is_empty() {
            let exe_target = if candidate.ext == "exe" {
                normalize_shortcut_target_path(path_text.as_str())
            } else {
                candidate
                    .shortcut_target
                    .as_deref()
                    .map(normalize_shortcut_target_path)
                    .unwrap_or_default()
            };
            if !exe_target.trim().is_empty() {
                let exe_key = normalize_id_path(exe_target.as_str());
                if let Some(exe_subtitle) = exe_publishers.get(&exe_key) {
                    subtitle = exe_subtitle.clone();
                }
            }
        }

        if subtitle.trim().is_empty() {
            if let Some(fallback) = start_menu_entry_subtitle(
                root,
                candidate.path.as_path(),
                candidate.shortcut_target.as_deref(),
            ) {
                subtitle = fallback;
            }
        }

        let mut item = SearchItem::new(&id, "app", &candidate.title, &path_text);
        if let Some(clean_subtitle) = sanitize_publisher_label(subtitle.as_str()) {
            item = item.with_subtitle(clean_subtitle.as_str());
        }
        items.push(item);
    }

    Ok(items)
}

#[cfg(target_os = "windows")]
fn discover_registered_app_paths() -> Result<Vec<SearchItem>, ProviderError> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY};

    /// `(label, hive, subkey)`.
    ///
    /// The 32-bit view is opened through its explicit `WOW6432Node` path rather
    /// than with `KEY_WOW64_32KEY`, so all three roots go through one code path
    /// and only the path differs.
    const ROOTS: [(&str, HKEY, &str); 3] = [
        (
            "hkcu",
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\App Paths",
        ),
        (
            "hklm",
            HKEY_LOCAL_MACHINE,
            r"Software\Microsoft\Windows\CurrentVersion\App Paths",
        ),
        (
            "hklm32",
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths",
        ),
    ];

    let mut items = Vec::new();
    for (label, hive, subkey) in ROOTS {
        for (name, command) in enumerate_app_path_key(hive, subkey) {
            let Some(path) = registered_command_target(&command) else {
                continue;
            };
            if !Path::new(&path).is_file() {
                continue;
            }
            let trimmed = name.trim();
            let title = trimmed.strip_suffix(".exe").unwrap_or(trimmed).trim();
            if title.is_empty() {
                continue;
            }
            let id = format!("app-registry:{label}:{}", normalize_id_path(trimmed));
            items.push(SearchItem::new(&id, "app", title, &path));
        }
    }
    Ok(items)
}

/// Every `App Paths` entry under one key, as `(subkey name, default command)`.
///
/// This was three cold PowerShell starts, and the script that did the work had
/// lost its backslashes (`HKCU:SoftwareMicrosoftWindowsCurrentVersionApp
/// Paths`), so `Get-ChildItem` resolved nothing under
/// `$ErrorActionPreference = 'SilentlyContinue'` and the provider indexed **no
/// registry apps at all**. Reading the keys directly is both the correct
/// enumeration and a removed interpreter start.
#[cfg(target_os = "windows")]
fn enumerate_app_path_key(hive: windows_sys::Win32::System::Registry::HKEY, subkey: &str) -> Vec<(String, String)> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, KEY_READ,
    };

    let mut entries = Vec::new();
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `subkey` is NUL terminated through `to_wide` and `key` is a valid
    // out pointer for the call.
    let opened = unsafe { RegOpenKeyExW(hive, to_wide(subkey).as_ptr(), 0, KEY_READ, &mut key) };
    if opened != 0 {
        // A machine without the 32-bit view, or without this key at all, simply
        // has nothing to report.
        return entries;
    }

    let mut index = 0u32;
    loop {
        let mut name = vec![0u16; 512];
        let mut name_len = name.len() as u32;
        // SAFETY: the name buffer is writable for `name_len` code units, and
        // every optional out pointer is null.
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            // ERROR_NO_MORE_ITEMS ends the enumeration; any other failure ends
            // it too, with whatever was read so far.
            break;
        }
        index += 1;
        let name = String::from_utf16_lossy(&name[..name_len as usize]);
        if let Some(command) = app_path_default_command(key, &name) {
            entries.push((name, command));
        }
    }

    // SAFETY: `key` was opened above and is not used afterwards.
    unsafe { RegCloseKey(key) };
    entries
}

/// The default value of one `App Paths` subkey, with environment strings
/// expanded so `%ProgramFiles%` style registrations resolve to a real path.
#[cfg(target_os = "windows")]
fn app_path_default_command(parent: windows_sys::Win32::System::Registry::HKEY, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    };

    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `name` is NUL terminated through `to_wide`.
    if unsafe { RegOpenKeyExW(parent, to_wide(name).as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }

    let empty = to_wide("");
    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: a null data pointer with a zero size is the documented size query.
    let sized = unsafe {
        RegQueryValueExW(
            key,
            empty.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };

    let mut command = None;
    if sized == 0 && size >= 2 && (kind == REG_SZ || kind == REG_EXPAND_SZ) {
        let mut buffer = vec![0u8; size as usize];
        // SAFETY: `buffer` is writable for `size` bytes, the length the API
        // itself reported.
        let read = unsafe {
            RegQueryValueExW(
                key,
                empty.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buffer.as_mut_ptr(),
                &mut size,
            )
        };
        if read == 0 {
            let units: Vec<u16> = buffer
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let text = String::from_utf16_lossy(&units);
            let text = text.trim_end_matches('\0').trim();
            if !text.is_empty() {
                command = Some(if kind == REG_EXPAND_SZ {
                    expand_environment(text)
                } else {
                    text.to_string()
                });
            }
        }
    }

    // SAFETY: `key` was opened above and is not used afterwards.
    unsafe { RegCloseKey(key) };
    command
}

/// Expand `%VAR%` references, falling back to the input when the call fails.
#[cfg(target_os = "windows")]
fn expand_environment(value: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;

    let source = to_wide(value);
    // SAFETY: a null destination with a zero length is the documented size
    // query.
    let needed = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return value.to_string();
    }
    let mut buffer = vec![0u16; needed as usize];
    // SAFETY: `buffer` is writable for `needed` code units.
    let written = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), needed) };
    if written == 0 || written as usize > buffer.len() {
        return value.to_string();
    }
    let text = String::from_utf16_lossy(&buffer[..written as usize]);
    text.trim_end_matches('\0').to_string()
}

#[cfg(target_os = "windows")]
fn registered_command_target(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let target = if let Some(rest) = command.strip_prefix('"') {
        rest.split_once('"').map(|(path, _)| path).unwrap_or(rest)
    } else if let Some(end) = command.to_ascii_lowercase().find(".exe") {
        &command[..end + 4]
    } else {
        command.split_whitespace().next().unwrap_or_default()
    };
    let target = target.trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// Every entry the Start menu shows, as `(display name, App ID)`.
///
/// This is the same list `Get-StartApps` reports: the shell's `AppsFolder`
/// namespace, enumerated as shell items. `SIGDN_PARENTRELATIVEPARSING` on a
/// child is the App ID (`Anysphere.Cursor`, `Microsoft.Paint_8wekyb3d8bbwe!App`),
/// and `SIGDN_NORMALDISPLAY` is the localised name the Start menu shows.
///
/// Replacing `Get-StartApps` removes a cold `powershell.exe` start and 430 ms of
/// interpreter work from every app pass.
#[cfg(target_os = "windows")]
fn enumerate_shell_apps() -> Vec<(String, String)> {
    use windows::Win32::Foundation::{S_FALSE, S_OK};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
        COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{
        IEnumShellItems, IShellItem, BHID_EnumItems, FOLDERID_AppsFolder, KF_FLAG_DEFAULT,
        SHGetKnownFolderItem, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    };

    /// Read one display name out of a shell item and free the shell's copy.
    ///
    /// # Safety
    /// `item` must be a live shell item interface.
    unsafe fn shell_item_name(item: &IShellItem, sigdn: SIGDN) -> Option<String> {
        // SAFETY: the caller guarantees `item` is live; the returned string is
        // CoTaskMem-allocated and freed exactly once below.
        let raw = unsafe { item.GetDisplayName(sigdn) }.ok()?;
        if raw.is_null() {
            return None;
        }
        // SAFETY: `raw` is NUL terminated and owned by this function.
        let text = unsafe { raw.to_string() }.ok();
        // SAFETY: `raw` came from `GetDisplayName` and is not used afterwards.
        unsafe { CoTaskMemFree(Some(raw.0 as *const core::ffi::c_void)) };
        let text = text?;
        let trimmed = text.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    }

    // A thread already in the multi-threaded apartment can still enumerate the
    // shell namespace, so a changed-mode result is not fatal: only a successful
    // initialisation is undone, and `S_FALSE` belongs to someone else.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    let owned = init == S_OK;
    if init.is_err() && init != S_FALSE {
        crate::logging::warn(&format!(
            "[nex] shell AppsFolder enumeration could not initialise COM: {init:?}"
        ));
        return Vec::new();
    }

    let mut apps = Vec::new();
    // SAFETY: COM is initialised above (or was already), and every interface is
    // released by the time its binding leaves scope.
    unsafe {
        let folder = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None);
        let Ok(folder) = folder else {
            if owned {
                CoUninitialize();
            }
            return apps;
        };
        let items = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems);

        if let Ok(items) = items {
            loop {
                let mut batch: [Option<IShellItem>; 16] = Default::default();
                let mut fetched = 0u32;
                // `Next` returns S_FALSE when the enumerator is exhausted, which
                // the typed wrapper reports as `Ok(())` with a short batch.
                if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                for item in batch.iter().take(fetched as usize).flatten() {
                    let name = shell_item_name(item, SIGDN_NORMALDISPLAY);
                    let app_id = shell_item_name(item, SIGDN_PARENTRELATIVEPARSING);
                    if let (Some(name), Some(app_id)) = (name, app_id) {
                        apps.push((name, app_id));
                    }
                }
            }
        }
    }

    if owned {
        // SAFETY: mirrors the successful `CoInitializeEx` above.
        unsafe { CoUninitialize() };
    }
    apps
}

#[cfg(target_os = "windows")]
fn discover_start_apps_via_powershell(
    uninstall_publishers: &HashMap<String, String>,
) -> Result<Vec<SearchItem>, ProviderError> {
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
        let mut item = SearchItem::new(&id, "app", title, &path);
        if let Some(subtitle) = publisher_from_uninstall_map(title, uninstall_publishers)
            .or_else(|| start_app_subtitle_from_app_id(app_id))
            .and_then(|raw| sanitize_publisher_label(raw.as_str()))
        {
            item = item.with_subtitle(subtitle.as_str());
        }
        items.push(item);
    }

    Ok(items)
}

/// Start-menu entries, preferring the shell namespace and falling back to
/// `Get-StartApps` only when the namespace enumerates nothing.
///
/// The fallback exists so a machine where COM enumeration fails loses speed
/// rather than 144 app records. It is expected never to run.
#[cfg(target_os = "windows")]
fn discover_start_apps(
    uninstall_publishers: &HashMap<String, String>,
) -> Result<Vec<SearchItem>, ProviderError> {
    let apps = enumerate_shell_apps();
    if apps.is_empty() {
        crate::logging::warn(
            "[nex] shell AppsFolder enumeration returned nothing; using the PowerShell fallback",
        );
        return discover_start_apps_via_powershell(uninstall_publishers);
    }

    let folder_roots = crate::app_target::known_folder_roots();
    let mut dropped_missing_targets = 0usize;
    let mut items = Vec::new();
    for (name, app_id) in apps {
        let title = name.trim();
        let app_id = app_id.trim();
        if title.is_empty() || app_id.is_empty() {
            continue;
        }
        if !crate::app_target::shell_app_target_exists(app_id, &folder_roots) {
            dropped_missing_targets += 1;
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
        let mut item = SearchItem::new(&id, "app", title, &path);
        if let Some(subtitle) = publisher_from_uninstall_map(title, uninstall_publishers)
            .or_else(|| start_app_subtitle_from_app_id(app_id))
            .and_then(|raw| sanitize_publisher_label(raw.as_str()))
        {
            item = item.with_subtitle(subtitle.as_str());
        }
        items.push(item);
    }
    if dropped_missing_targets > 0 {
        crate::logging::info(&format!(
            "[nex] start-menu apps: dropped {dropped_missing_targets} whose target no longer exists"
        ));
    }
    Ok(items)
}

#[cfg(target_os = "windows")]
fn load_exe_company_names(exe_paths: &[String]) -> Result<HashMap<String, String>, ProviderError> {
    let mut out = HashMap::new();
    for path in exe_paths {
        let Some(publisher) = exe_company_name(path) else {
            continue;
        };
        if let Some(clean_publisher) = sanitize_publisher_label(&publisher) {
            out.insert(normalize_id_path(path), clean_publisher);
        }
    }
    Ok(out)
}

/// Read the `CompanyName` string from an executable's version resource.
///
/// This replaced a `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy
/// Bypass -WindowStyle Hidden -Command` call that ran the same lookup via
/// `[System.Diagnostics.FileVersionInfo]::GetVersionInfo(...)`. That argument
/// set is the shape malware loaders use to hide an executable payload, and a
/// user's antivirus flagged the process as a result (CIQ-201). The work was a
/// file read either way, so it is now done in-process: no child process, no
/// hidden window, and no flag that exists only to look suspicious.
///
/// `None` covers everything the old script swallowed in a bare `catch {}` -
/// no resource, an unreadable file, a missing `CompanyName` - so a file that
/// never showed a publisher still shows none.
#[cfg(target_os = "windows")]
fn exe_company_name(path: &str) -> Option<String> {
    // The reader lives in `win_version_resource` because the updater needs the
    // same version-resource walk for the installed build's version.
    crate::win_version_resource::version_string(path, "CompanyName")
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
    let subtitle_bonus = subtitle_quality_score(item.subtitle.as_str());
    let lowered = item.path.trim().to_ascii_lowercase();
    if lowered.starts_with("shell:appsfolder\\") {
        return 3 + subtitle_bonus;
    }
    if lowered.ends_with(".lnk") || lowered.ends_with(".exe") {
        return 2 + subtitle_bonus;
    }
    1 + subtitle_bonus
}

#[cfg(target_os = "windows")]
fn subtitle_quality_score(subtitle: &str) -> u8 {
    let trimmed = subtitle.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let word_count = trimmed.split_whitespace().count();
    if word_count >= 3 {
        3
    } else if word_count == 2 {
        2
    } else {
        1
    }
}

#[cfg(target_os = "windows")]
fn start_menu_entry_subtitle(
    _root: &Path,
    _entry_path: &Path,
    shortcut_target: Option<&str>,
) -> Option<String> {
    let shortcut_target = shortcut_target?;
    let normalized_target = normalize_shortcut_target_path(shortcut_target);
    if normalized_target.is_empty() || !looks_like_filesystem_path(normalized_target.as_str()) {
        return None;
    }
    program_files_vendor_label(normalized_target.as_str())
}

#[cfg(target_os = "windows")]
fn program_files_vendor_label(target_path: &str) -> Option<String> {
    let normalized = target_path.replace('/', "\\");
    let lower = normalized.to_ascii_lowercase();
    let markers = ["\\program files\\", "\\program files (x86)\\"];
    for marker in markers {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let tail = &normalized[start + marker.len()..];
        let vendor = tail.split('\\').next().unwrap_or("").trim();
        if vendor.is_empty() {
            continue;
        }
        let vendor_lower = vendor.to_ascii_lowercase();
        if matches!(
            vendor_lower.as_str(),
            "windowsapps" | "common files" | "windows nt"
        ) {
            continue;
        }
        return Some(vendor.to_string());
    }
    None
}

#[cfg(target_os = "windows")]
fn start_app_subtitle_from_app_id(app_id: &str) -> Option<String> {
    let trimmed = app_id.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("microsoft.autogenerated.") {
        return None;
    }

    if let Some((package_name, _app_entry)) = trimmed.split_once('!') {
        if let Some((publisher_hint, _package_tail)) = package_name.split_once('_') {
            if let Some(publisher) = normalize_publisher_hint(publisher_hint) {
                return Some(publisher);
            }
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn publisher_from_uninstall_map(
    title: &str,
    uninstall_publishers: &HashMap<String, String>,
) -> Option<String> {
    let key = crate::model::normalize_for_search(title);
    if key.is_empty() {
        return None;
    }
    uninstall_publishers
        .get(&key)
        .and_then(|publisher| sanitize_publisher_label(publisher.as_str()))
}

#[cfg(target_os = "windows")]
fn normalize_publisher_hint(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let head = trimmed.split('.').find(|part| !part.trim().is_empty())?;
    let head = head.trim();
    if head.is_empty() {
        return None;
    }

    let lower = head.to_ascii_lowercase();
    if lower.starts_with("microsoft") {
        return Some("Microsoft".to_string());
    }

    sanitize_publisher_label(head)
}

#[cfg(target_os = "windows")]
fn sanitize_publisher_label(raw: &str) -> Option<String> {
    let mut trimmed = raw.trim().trim_matches('"').trim().to_string();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.to_ascii_lowercase().starts_with("cn=") {
        trimmed = trimmed
            .get(3..)
            .unwrap_or_default()
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if trimmed.is_empty() {
            return None;
        }
    }

    let collapsed = trimmed
        .split_whitespace()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return None;
    }

    if collapsed.contains('\\')
        || collapsed.contains('/')
        || collapsed.contains('{')
        || collapsed.contains('}')
        || collapsed.contains("://")
    {
        return None;
    }

    if looks_like_guid_token(collapsed.as_str())
        || looks_like_noisy_publisher_token(collapsed.as_str())
    {
        return None;
    }

    let lowered = collapsed.to_ascii_lowercase();
    if lowered == "microsoftwindows" {
        return Some("Microsoft".to_string());
    }

    Some(collapsed)
}

#[cfg(target_os = "windows")]
fn looks_like_guid_token(value: &str) -> bool {
    let token = value
        .trim()
        .trim_matches('{')
        .trim_matches('}')
        .trim()
        .to_ascii_lowercase();
    let parts: Vec<&str> = token.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected = [8, 4, 4, 4, 12];
    for (part, expected_len) in parts.iter().zip(expected.iter()) {
        if part.len() != *expected_len || !part.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn looks_like_noisy_publisher_token(value: &str) -> bool {
    let compact = value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        return true;
    }

    let lower = compact.to_ascii_lowercase();
    if lower.starts_with("microsoft.autogenerated") || lower.contains("autogenerated") {
        return true;
    }

    if compact.contains('_') {
        return true;
    }

    if !value.contains(' ') && compact.contains('.') {
        return true;
    }

    let letters = compact
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .count();
    let digits = compact.chars().filter(|ch| ch.is_ascii_digit()).count();
    if letters == 0 {
        return true;
    }

    if !value.contains(' ') {
        if digits > 0 && digits * 2 >= letters {
            return true;
        }
        if compact.len() >= 14 && digits >= 3 {
            return true;
        }
    }

    false
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The `App Paths` provider used to run a PowerShell script whose registry
    /// paths had lost their backslashes, so every `Get-ChildItem` resolved
    /// nothing under `$ErrorActionPreference = 'SilentlyContinue'` and the
    /// provider silently indexed no registry apps at all. Reading the keys
    /// directly is the fix; this pins the enumeration itself.
    #[cfg(target_os = "windows")]
    #[test]
    fn app_paths_are_read_from_the_registry_and_look_like_commands() {
        use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

        let entries = enumerate_app_path_key(
            HKEY_LOCAL_MACHINE,
            r"Software\Microsoft\Windows\CurrentVersion\App Paths",
        );
        if entries.is_empty() {
            // A minimal image can carry no registrations; the point of the test
            // is that when they exist they are read, not that they must exist.
            eprintln!("skipping: this machine registers no App Paths entries under HKLM");
            return;
        }
        for (name, command) in entries {
            assert!(!name.trim().is_empty(), "an App Paths subkey had no name");
            assert!(
                !command.trim().is_empty(),
                "{name} produced an empty command"
            );
            assert!(
                !command.contains('\0'),
                "{name} kept its NUL terminator: {command:?}"
            );
        }
    }

    /// The Start menu is read from the shell's `AppsFolder` namespace instead of
    /// `Get-StartApps`. Parity against the PowerShell list was checked by hand
    /// (180 App IDs, 180 names, zero differences); this pins what can be checked
    /// without starting a second shell: a populated, duplicate-free list whose
    /// entries carry a name and an App ID.
    #[cfg(target_os = "windows")]
    #[test]
    fn shell_apps_are_enumerated_with_names_and_app_ids() {
        let apps = enumerate_shell_apps();
        if apps.is_empty() {
            eprintln!("skipping: this shell exposed no AppsFolder entries");
            return;
        }
        assert!(
            apps.len() > 10,
            "expected a populated Start menu, got {}",
            apps.len()
        );
        for (name, app_id) in &apps {
            assert!(!name.trim().is_empty(), "an entry had no display name");
            assert!(!app_id.trim().is_empty(), "{name} had no App ID");
        }
        let mut ids: Vec<String> = apps
            .iter()
            .map(|(_, app_id)| app_id.to_ascii_lowercase())
            .collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(total, ids.len(), "the namespace returned duplicate App IDs");
    }

    /// A subkey that does not exist must read as absent, not as an empty value.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_missing_app_paths_key_reports_nothing() {
        use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

        assert!(enumerate_app_path_key(
            HKEY_LOCAL_MACHINE,
            r"Software\Nex\There Is No Such Key",
        )
        .is_empty());
        assert!(app_path_default_command(
            HKEY_LOCAL_MACHINE,
            r"Software\Nex\There Is No Such Key",
        )
        .is_none());
    }

    /// Registry commands are quoted or bare with arguments; both must resolve to
    /// the executable the registry names.
    #[cfg(target_os = "windows")]
    #[test]
    fn app_paths_commands_resolve_to_their_executable() {
        assert_eq!(
            registered_command_target(r#""C:\Program Files\App\app.exe" --flag"#),
            Some(r"C:\Program Files\App\app.exe".to_string())
        );
        assert_eq!(
            registered_command_target(r"C:\Windows\notepad.exe %1"),
            Some(r"C:\Windows\notepad.exe".to_string())
        );
        assert_eq!(registered_command_target("   "), None);
    }

    #[test]
    fn missing_position_forces_one_full_pass_then_skips() {
        assert_eq!(plan_delta_volume(true, false), DeltaVolumePlan::Delta);
        assert_eq!(plan_delta_volume(true, true), DeltaVolumePlan::Delta);
        assert_eq!(plan_delta_volume(false, false), DeltaVolumePlan::FullPass);
        assert_eq!(plan_delta_volume(false, true), DeltaVolumePlan::SkipNoJournal);
    }

    #[test]
    fn pending_state_keeps_earliest_position_per_journal() {
        assert_eq!(merge_pending_state(None, 7, 500), (7, 500));
        assert_eq!(merge_pending_state(Some((7, 500)), 7, 900), (7, 500));
        assert_eq!(merge_pending_state(Some((7, 900)), 7, 500), (7, 500));
        // A recreated journal invalidates the older position.
        assert_eq!(merge_pending_state(Some((7, 500)), 8, 100), (8, 100));
    }

    fn matchers<'a>(roots: &'a [PathBuf]) -> Vec<MftRootMatcher<'a>> {
        roots.iter().map(|root| MftRootMatcher::new(root)).collect()
    }

    /// One snapshot filtered against all roots must equal the union of
    /// filtering it once per root, with overlapping roots emitting each entry once.
    #[test]
    fn volume_snapshot_filtering_matches_per_root_union() {
        let entries: Vec<(&str, bool)> = vec![
            ("/v/users/me/doc.txt", false),
            ("/v/users/me/projects", true),
            ("/v/users/me/projects/app", true),
            ("/v/users/me/projects/app/main.rs", false),
            ("/v/users/me/projects/app/build", true),
            ("/v/users/me/projects/app/build/out.o", false),
            ("/v/users/me/projects/app/a/b/c/deep.txt", false),
            ("/v/users/me2/other.txt", false),
            ("/v/users/me", true),
            ("/v/games/save.dat", false),
            ("/v/data/notes.md", false),
            ("/v/data/windows", true),
        ];
        let roots = vec![
            PathBuf::from("/v/users/me"),
            PathBuf::from("/v/users/me/projects"),
            PathBuf::from("/v/data"),
        ];
        let policy = DiscoveryExclusionPolicy::new(&[PathBuf::from("/v/users/me/doc.txt")]);

        for max_depth in [0usize, 2, 3] {
            let mut per_root: std::collections::BTreeSet<&str> = Default::default();
            for root in &roots {
                let single = [root.clone()];
                let one = matchers(&single);
                for (path, is_dir) in &entries {
                    if classify_mft_entry(path, *is_dir, &one, max_depth, &policy)
                        == MftEntryVerdict::Keep
                    {
                        per_root.insert(path);
                    }
                }
            }

            let all = matchers(&roots);
            let mut single_pass = Vec::new();
            for (path, is_dir) in &entries {
                if classify_mft_entry(path, *is_dir, &all, max_depth, &policy)
                    == MftEntryVerdict::Keep
                {
                    single_pass.push(*path);
                }
            }
            let unique: std::collections::BTreeSet<&str> = single_pass.iter().copied().collect();
            assert_eq!(single_pass.len(), unique.len(), "depth {max_depth}: no duplicates");
            assert_eq!(unique, per_root, "depth {max_depth}");
        }
    }

    #[test]
    fn nested_roots_keep_entries_each_root_would_drop() {
        let roots = vec![
            PathBuf::from("/v/users/me"),
            PathBuf::from("/v/users/me/projects"),
        ];
        let all = matchers(&roots);
        let policy = DiscoveryExclusionPolicy::new(&[]);
        // Depth 1 from `me` excludes the grandchild, but from `projects` it is depth 1.
        assert_eq!(
            classify_mft_entry("/v/users/me/projects/app", true, &all, 1, &policy),
            MftEntryVerdict::Keep
        );
        assert_eq!(
            classify_mft_entry("/v/users/me/other/deep/file.txt", false, &all, 1, &policy),
            MftEntryVerdict::TooDeep
        );
        assert_eq!(
            classify_mft_entry("/v/users/me2/file.txt", false, &all, 0, &policy),
            MftEntryVerdict::OutsideRoots
        );
        assert_eq!(
            classify_mft_entry("/v/users/me/projects", true, &all, 0, &policy),
            MftEntryVerdict::Keep,
            "a nested root is itself an entry of the outer root"
        );
        assert_eq!(
            classify_mft_entry("/v/users/me/projects/app/node_modules", true, &all, 0, &policy),
            MftEntryVerdict::Excluded
        );
    }

    #[test]
    fn volume_root_matches_without_extra_separator() {
        let roots = vec![PathBuf::from("/")];
        let all = matchers(&roots);
        let policy = DiscoveryExclusionPolicy::new(&[]);
        assert_eq!(
            classify_mft_entry("/pictures", true, &all, 0, &policy),
            MftEntryVerdict::Keep
        );
    }

    #[test]
    fn helper_wait_backs_off_then_stops_at_budget() {
        use std::time::Duration;
        let budget = Duration::from_secs(90);
        let mut waited = Duration::ZERO;
        let mut delays = Vec::new();
        let mut attempt = 0;
        while let Some(delay) = helper_wait_delay(attempt, waited, budget) {
            delays.push(delay.as_secs());
            waited += delay;
            attempt += 1;
        }
        assert_eq!(&delays[..5], &[2, 4, 8, 16, 16]);
        assert_eq!(waited, budget, "waits exactly the budget, never past it");
        assert_eq!(*delays.last().unwrap(), 90 - (2 + 4 + 8 + 16 * 4));
        assert_eq!(helper_wait_delay(0, Duration::ZERO, Duration::ZERO), None);
        assert_eq!(helper_wait_delay(3, budget, budget), None);
        assert_eq!(
            helper_wait_delay(200, Duration::ZERO, budget),
            Some(Duration::from_secs(16)),
            "huge attempt counts must not overflow"
        );
    }

    #[test]
    fn entry_exclusion_matches_stat_based_exclusion() {
        let root = std::env::temp_dir().join(format!("nex-excl-entry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub").join("build")).unwrap();
        std::fs::write(root.join("sub").join("target"), b"x").unwrap();
        std::fs::write(root.join("sub").join("desktop.ini"), b"x").unwrap();

        let policy = DiscoveryExclusionPolicy::new(&[]);
        for (relative, is_dir, expected) in [
            ("sub", true, false),
            ("sub/build", true, true),
            ("sub/build/inner.txt", false, true),
            ("sub/target", false, false),
            ("sub/desktop.ini", false, true),
            ("windows", true, true),
        ] {
            let path = root.join(relative);
            assert_eq!(
                policy.should_exclude_entry_under_root(&path, &root, is_dir),
                expected,
                "entry {relative}"
            );
            if path.exists() {
                assert_eq!(
                    policy.should_exclude_path_under_root(&path, &root),
                    expected,
                    "path {relative}"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The lookup that replaced the PowerShell call must read the company name
    /// from a real signed binary. `notepad.exe` always carries one, and the
    /// expected value is Microsoft's, not ours, so this fails if the string
    /// table walk regresses rather than silently returning None.
    #[cfg(target_os = "windows")]
    #[test]
    fn exe_company_name_reads_a_real_version_resource() {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let notepad = format!("{system_root}\\System32\\notepad.exe");
        if !Path::new(&notepad).is_file() {
            return;
        }
        let publisher = exe_company_name(&notepad)
            .expect("notepad.exe carries a CompanyName resource");
        assert!(
            publisher.to_ascii_lowercase().contains("microsoft"),
            "expected a Microsoft publisher, got {publisher:?}"
        );
    }

    /// A file with no version resource is the case the old script swallowed in
    /// a bare `catch {}`. It must be `None`, not an error and not a panic.
    #[cfg(target_os = "windows")]
    #[test]
    fn exe_company_name_returns_none_for_a_file_without_a_resource() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("plain.txt");
        std::fs::write(&path, b"no version resource here").expect("write");
        let text = path.to_string_lossy().to_string();
        assert_eq!(exe_company_name(&text), None);
    }

    /// Nothing is returned for a path that does not exist, which is normal
    /// during discovery when an entry disappears between scan and read.
    #[cfg(target_os = "windows")]
    #[test]
    fn exe_company_name_returns_none_for_a_missing_path() {
        assert_eq!(exe_company_name("C:\\definitely\\not\\here.exe"), None);
    }

    /// The map's keys stay normalised the same way the callers expect, so a
    /// lookup by the shortcut target still resolves.
    #[cfg(target_os = "windows")]
    #[test]
    fn load_exe_company_names_keys_match_normalize_id_path() {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let notepad = format!("{system_root}\\System32\\notepad.exe");
        if !Path::new(&notepad).is_file() {
            return;
        }
        let map = load_exe_company_names(&[notepad.clone()]).expect("lookup runs");
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&normalize_id_path(&notepad)));
    }

    /// One-off equivalence check against the mechanism this replaced. Both
    /// paths read the same resource, so any file whose label differs is a
    /// parse bug. Not a permanent test: it shells out to PowerShell, which is
    /// the thing being removed, so it is ignored unless asked for by name.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore]
    fn native_lookup_matches_powershell_for_real_binaries() {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let candidates = [
            format!("{system_root}\\System32\\notepad.exe"),
            format!("{system_root}\\System32\\calc.exe"),
            format!("{system_root}\\System32\\mspaint.exe"),
            format!("{system_root}\\System32\\cmd.exe"),
            format!("{system_root}\\explorer.exe"),
            format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        ];
        let existing: Vec<String> = candidates
            .into_iter()
            .filter(|p| Path::new(p).is_file())
            .collect();
        if existing.is_empty() {
            return;
        }

        let script = existing
            .iter()
            .map(|p| format!("[System.Diagnostics.FileVersionInfo]::GetVersionInfo('{p}').CompanyName"))
            .collect::<Vec<_>>()
            .join(";");
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("powershell");
        let expected = String::from_utf8_lossy(&output.stdout);
        let expected: Vec<&str> = expected.lines().map(|l| l.trim()).collect();

        for (index, path) in existing.iter().enumerate() {
            let native = exe_company_name(path);
            let sanitized = native
                .as_deref()
                .and_then(sanitize_publisher_label);
            let old = expected
                .get(index)
                .copied()
                .filter(|v| !v.is_empty())
                .and_then(sanitize_publisher_label);
            assert_eq!(
                sanitized, old,
                "label mismatch for {path}: native={native:?} powershell={:?}",
                expected.get(index)
            );
        }
    }
}
