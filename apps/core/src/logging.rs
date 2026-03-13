use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_FILE_NAME: &str = "nex.log";
const LEGACY_LOG_FILE_NAME: &str = "swiftfind.log";
const MAX_LOG_BYTES: u64 = 1_000_000;
const MAX_ARCHIVES: usize = 5;

static LOGGER: OnceLock<Logger> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

struct Logger {
    file: Mutex<File>,
}

pub fn logs_dir() -> PathBuf {
    crate::config::stable_app_data_dir().join("logs")
}

pub fn primary_log_path() -> PathBuf {
    logs_dir().join(LOG_FILE_NAME)
}

pub fn candidate_log_paths() -> Vec<PathBuf> {
    let mut paths = vec![primary_log_path(), logs_dir().join(LEGACY_LOG_FILE_NAME)];
    paths.sort();
    paths.dedup();
    paths
}

pub fn init() -> Result<(), std::io::Error> {
    let log_dir = logs_dir();
    fs::create_dir_all(&log_dir)?;
    migrate_legacy_log_file(&log_dir)?;
    let log_path = primary_log_path();
    rotate_if_needed(&log_path, &log_dir)?;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let _ = LOGGER.set(Logger {
        file: Mutex::new(file),
    });

    install_panic_hook();
    Ok(())
}

pub fn info(message: &str) {
    write_line("INFO", message);
}

pub fn warn(message: &str) {
    write_line("WARN", message);
}

pub fn error(message: &str) {
    write_line("ERROR", message);
}

pub fn open_logs_folder() -> Result<(), String> {
    let dir = logs_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create logs dir: {e}"))?;

    #[cfg(target_os = "windows")]
    {
        let target = dir.to_string_lossy().into_owned();
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(&target)
            .status()
            .map_err(|e| format!("failed to open logs folder: {e}"))?;
        if !status.success() {
            return Err(format!(
                "failed to open logs folder; cmd/start exit status: {status}"
            ));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Keep tests/platform-agnostic paths stable without requiring desktop integration.
    }

    Ok(())
}

fn write_line(level: &str, message: &str) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let Ok(mut file) = logger.file.lock() else {
        return;
    };

    let ts = now_secs();
    let line = format!("[{ts}] [{level}] {message}\n");
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn rotate_if_needed(log_path: &Path, log_dir: &Path) -> Result<(), std::io::Error> {
    let meta = match fs::metadata(log_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if meta.len() < MAX_LOG_BYTES {
        return Ok(());
    }

    let stamp = now_secs();
    let archived = log_dir.join(format!("nex-{stamp}.log"));
    fs::rename(log_path, archived)?;
    prune_old_archives(log_dir)?;
    Ok(())
}

fn migrate_legacy_log_file(log_dir: &Path) -> Result<(), std::io::Error> {
    let legacy_path = log_dir.join(LEGACY_LOG_FILE_NAME);
    let current_path = log_dir.join(LOG_FILE_NAME);
    if !current_path.exists() && legacy_path.exists() {
        fs::rename(legacy_path, current_path)?;
    }
    Ok(())
}

fn prune_old_archives(log_dir: &Path) -> Result<(), std::io::Error> {
    let mut archives = fs::read_dir(log_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("nex-") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    archives.sort();
    while archives.len() > MAX_ARCHIVES {
        if let Some(oldest) = archives.first() {
            let _ = fs::remove_file(oldest);
        }
        archives.remove(0);
    }
    Ok(())
}

fn install_panic_hook() {
    let _ = PANIC_HOOK_INSTALLED.get_or_init(|| {
        let prior = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let location = panic_info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "unknown".to_string());
            let payload = panic_info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic payload unavailable".to_string());
            error(&format!("panic at {location}: {payload}"));
            prior(panic_info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::logs_dir;

    #[test]
    fn logs_dir_uses_stable_app_data_layout() {
        let dir = logs_dir();
        let lowered = dir.to_string_lossy().to_ascii_lowercase();
        assert!(lowered.contains("nex") || lowered.contains("swiftfind"));
    }
}
