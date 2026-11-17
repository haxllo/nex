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
        let mut command = std::process::Command::new("cmd");
        command.arg("/C").arg("start").arg("").arg(&target);
        {
            use std::os::windows::process::CommandExt;
            // No console flash from the GUI process.
            command.creation_flags(0x0800_0000);
        }
        let status = command
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

    let ts = timestamp();
    let line = format!("[{ts}] [{level}] {message}\n");
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
}

/// `YYYY-MM-DD HH:MM:SS.mmm`, fixed width so lines stay aligned and sort
/// correctly as plain text.
fn format_stamp(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
) -> String {
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}")
}

/// Local wall clock for the start of a log line.
///
/// The line used to begin with a raw epoch second, which is an id you have to
/// convert before it means anything. A log is read while something is wrong,
/// so it should say when it happened. Milliseconds are kept because several
/// startup phases land inside the same second and their order is the point.
fn timestamp() -> String {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut time: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut time) };
        format_stamp(
            i64::from(time.wYear),
            u32::from(time.wMonth),
            u32::from(time.wDay),
            u32::from(time.wHour),
            u32::from(time.wMinute),
            u32::from(time.wSecond),
            u32::from(time.wMilliseconds),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let (year, month, day, hour, minute, second) = civil_from_epoch(now_secs());
        format_stamp(year, month, day, hour, minute, second, 0)
    }
}

/// Epoch seconds as a UTC calendar date and time.
///
/// Off Windows there is no local-time API in `std`, so the fallback reports UTC
/// rather than inventing an offset. Exact rather than approximate so it can be
/// tested against known dates instead of against the clock.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn civil_from_epoch(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = secs as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3_600) as u32;
    let minute = ((rem % 3_600) / 60) as u32;
    let second = (rem % 60) as u32;

    // Days-to-civil conversion (Howard Hinnant's algorithm): shift the epoch to
    // 0000-03-01 so leap days land at the end of the year and need no special
    // case.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day, hour, minute, second)
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
            error(&format_panic(panic_info));
            prior(panic_info);
        }));
    });
}

/// One panic as a log line: where, on which thread, and what it said.
///
/// Split out from [`install_panic_hook`] so a test can drive the exact
/// formatting. The hook itself already captured the panic text, location and
/// backtrace, but nothing asserted that a panic on a worker thread produced a
/// line at all, so a regression there would have been silent.
fn format_panic(panic_info: &std::panic::PanicHookInfo<'_>) -> String {
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
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    let backtrace = std::backtrace::Backtrace::force_capture();
    format!("panic at {location} on thread '{name}': {payload}\nbacktrace:\n{backtrace}")
}

#[cfg(test)]
mod tests {
    use super::{civil_from_epoch, format_panic, format_stamp, logs_dir};

    #[test]
    fn a_line_states_its_date_and_time_without_conversion() {
        // Epoch values taken from `date -u`, not from this code.
        assert_eq!(civil_from_epoch(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_epoch(1_794_913_714), (2026, 11, 17, 11, 8, 34));
        // Leap day is the case a naive day count gets wrong.
        assert_eq!(civil_from_epoch(1_709_251_199), (2024, 2, 29, 23, 59, 59));
    }

    #[test]
    fn timestamps_are_fixed_width_so_lines_stay_aligned() {
        assert_eq!(
            format_stamp(2026, 1, 2, 3, 4, 5, 6),
            "2026-01-02 03:04:05.006"
        );
        assert_eq!(
            format_stamp(2026, 11, 17, 17, 9, 6, 123),
            "2026-11-17 17:09:06.123"
        );
    }

    #[test]
    fn logs_dir_uses_stable_app_data_layout() {
        let dir = logs_dir();
        let lowered = dir.to_string_lossy().to_ascii_lowercase();
        assert!(lowered.contains("nex") || lowered.contains("swiftfind"));
    }

    #[test]
    fn a_worker_thread_panic_is_reported_with_where_and_which_thread() {
        // The UI process owns the overlay, hotkeys and tray, so a panic on a
        // worker is not a degraded app: it is the window disappearing. The hook
        // has to name the thread and the site for that to be diagnosable from a
        // diagnostics bundle afterwards.
        let hook = std::panic::take_hook();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = captured.clone();
        std::panic::set_hook(Box::new(move |info| {
            sink.lock().unwrap().push(format_panic(info));
        }));

        let result = std::thread::Builder::new()
            .name("nex-test-worker".into())
            .spawn(|| panic!("index row was malformed"))
            .expect("spawn worker")
            .join();
        std::panic::set_hook(hook);

        assert!(result.is_err(), "the worker must have panicked");
        let lines = captured.lock().unwrap();
        assert_eq!(lines.len(), 1, "one panic must produce exactly one line");
        let line = &lines[0];
        assert!(
            line.contains("panic at "),
            "the line must say where: {line}"
        );
        assert!(
            line.contains("on thread 'nex-test-worker'"),
            "the line must name the thread: {line}"
        );
        assert!(
            line.contains("index row was malformed"),
            "the line must carry the payload: {line}"
        );
    }
}
