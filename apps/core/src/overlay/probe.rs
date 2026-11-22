//! Runtime probe for the show path (CIQ-236).
//!
//! The show after a long idle is slow in `painted_wait_ms`, and every guess
//! about why has so far been a guess: the WebView2 renderer being suspended,
//! the process losing priority while hidden, its pages being trimmed, the
//! compositor not waking, or the glass renderer being cold. This module puts a
//! number on each of those at the moment of a show, so the next person argues
//! with measurements instead of plausibility.
//!
//! One line per show, both at the moment `Show` is received and when the page
//! reports its first paint:
//!
//! ```text
//! [nex][probe] show idle_s=1650 pow=0/0 ws_mb=142 pf=8123 cloaked=0 vis=1
//!   wv_procs=4 wv_ws_mb=96 wv_cpu_ms=1204 hb_age_ms=780
//! ```
//!
//! Fields that mean nothing to the reader are deliberately terse: `pow` is
//! control/state for `ProcessPowerThrottling` (1 in the state mask means the
//! process is being run at reduced speed), `pf` is the process page-fault
//! count, `cloaked` is `DWMWA_CLOAKED`, `wv_*` aggregates the WebView2 process
//! tree under this process, and `hb_age_ms` is how long ago the page last
//! reported that it is alive (see `note_heartbeat`).
//!
//! Everything here is a query, never a change: the probe must not alter the
//! timing it is measuring.

#![cfg(target_os = "windows")]

use std::sync::Mutex;
use std::time::Instant;

/// When the panel was last hidden, so a show can say how long it slept.
static LAST_HIDE: Mutex<Option<Instant>> = Mutex::new(None);
/// When the page last said it was alive, and what its own clock said.
static LAST_HEARTBEAT: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
/// Largest heartbeat gap seen since the last show, in milliseconds.
static WORST_GAP_MS: Mutex<u32> = Mutex::new(0);

/// Record a hide, which is the start of the idle period a later show measures.
pub(crate) fn note_hide() {
    if let Ok(mut slot) = LAST_HIDE.lock() {
        *slot = Some(Instant::now());
    }
    if let Ok(mut worst) = WORST_GAP_MS.lock() {
        *worst = 0;
    }
}

/// The page's heartbeat. Logs only a gap that is long enough to mean the
/// renderer stopped running while the panel was hidden: that is the difference
/// between "the page was suspended" and "this machine was just busy".
pub(crate) fn note_heartbeat(page_clock_ms: u32) {
    let now = Instant::now();
    if let Ok(mut slot) = LAST_HEARTBEAT.lock() {
        if let Some((previous, previous_page_clock)) = *slot {
            let gap = now.duration_since(previous).as_millis() as u32;
            let page_gap = page_clock_ms.saturating_sub(previous_page_clock);
            if gap > 3500 {
                crate::runtime::log_info(&format!(
                    "[nex][probe] page heartbeat gap={gap}ms page_clock_gap={page_gap}ms"
                ));
                if let Ok(mut worst) = WORST_GAP_MS.lock() {
                    *worst = (*worst).max(gap);
                }
            }
        }
        *slot = Some((now, page_clock_ms));
    }
}

fn heartbeat_age_ms() -> Option<u128> {
    LAST_HEARTBEAT
        .lock()
        .ok()
        .and_then(|slot| slot.map(|(at, _)| at.elapsed().as_millis()))
}

fn worst_gap_ms() -> u32 {
    WORST_GAP_MS.lock().map(|worst| *worst).unwrap_or(0)
}

fn idle_seconds() -> u128 {
    LAST_HIDE
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .map(|at| at.elapsed().as_secs() as u128)
        .unwrap_or(0)
}

/// `ProcessPowerThrottling` for this process: control mask and state mask.
///
/// Bit 0 of the state mask is `EXECUTION_SPEED`, i.e. Windows has put the
/// process on low power. A hidden overlay is an obvious candidate for it, and
/// nothing in Nex has ever asked.
pub(crate) fn power_throttling() -> (u32, u32) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessInformation, ProcessPowerThrottling,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE,
    };

    let mut state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: 0,
        StateMask: 0,
    };
    let ok = unsafe {
        GetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &mut state as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if ok == 0 {
        return (u32::MAX, u32::MAX);
    }
    (state.ControlMask, state.StateMask)
}

/// Working set in MB and total page faults for this process.
fn memory() -> Option<(u64, u32)> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    Some((
        counters.WorkingSetSize as u64 / (1024 * 1024),
        counters.PageFaultCount,
    ))
}

/// `DWMWA_CLOAKED` for a window: non-zero means Windows is not compositing it,
/// which would explain a cold first frame on reveal.
pub(crate) fn cloaked(hwnd: *mut core::ffi::c_void) -> u32 {
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};

    let mut value: u32 = 0;
    let rc = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut value as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    if rc == 0 {
        value
    } else {
        u32::MAX
    }
}

/// The WebView2 process tree under this process: count, total working set in
/// MB, and total CPU in milliseconds.
///
/// A renderer that has been trimmed or throttled shows up here before it shows
/// up in a stopwatch: its working set collapses and its CPU stops advancing.
fn webview_tree() -> (usize, u64, u64) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let me = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return (0, 0, 0);
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut parents: Vec<(u32, u32, [u16; 260])> = Vec::new();
    let mut found = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while found {
        parents.push((entry.th32ProcessID, entry.th32ParentProcessID, entry.szExeFile));
        found = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };

    // Walk up the parent chain for every candidate and keep those descending
    // from this process; a fixed depth keeps a corrupt snapshot from looping.
    let mut descendants: Vec<u32> = Vec::new();
    for (pid, _, name) in &parents {
        let low: Vec<u16> = name.iter().copied().take_while(|c| *c != 0).collect();
        let name = String::from_utf16_lossy(&low).to_ascii_lowercase();
        if !name.contains("msedgewebview2") {
            continue;
        }
        let mut cursor = *pid;
        for _ in 0..8 {
            let Some((_, parent, _)) = parents.iter().find(|(p, _, _)| *p == cursor) else {
                break;
            };
            if *parent == me {
                descendants.push(*pid);
                break;
            }
            if *parent == 0 || *parent == cursor {
                break;
            }
            cursor = *parent;
        }
    }

    let mut ws_bytes = 0u64;
    let mut cpu_ms = 0u64;
    for pid in &descendants {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid) };
        if handle.is_null() {
            continue;
        }
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe {
            K32GetProcessMemoryInfo(
                handle,
                &mut counters,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        } != 0
        {
            ws_bytes += counters.WorkingSetSize as u64;
        }
        let mut creation = filetime(0);
        let mut exit = filetime(0);
        let mut kernel = filetime(0);
        let mut user = filetime(0);
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } != 0
        {
            cpu_ms += filetime_ms(kernel) + filetime_ms(user);
        }
        unsafe { CloseHandle(handle) };
    }
    (descendants.len(), ws_bytes / (1024 * 1024), cpu_ms)
}

/// Machine-wide memory pressure: available physical, commit total and commit
/// limit, in MB.
///
/// The owner's own read on the idle slowdown is memory pressure from other
/// programs, which is worth measuring rather than agreeing with: if a show is
/// slow only when the commit charge sits near its limit, the answer is "do not
/// allocate on the show path"; if it is slow regardless, the answer is
/// elsewhere.
fn system_memory() -> (u64, u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};

    let mut info: PERFORMANCE_INFORMATION = unsafe { std::mem::zeroed() };
    info.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
    if unsafe { GetPerformanceInfo(&mut info, info.cb) } == 0 {
        return (0, 0, 0);
    }
    let page = info.PageSize.max(1) as u64;
    let to_mb = |pages: usize| (pages as u64 * page) / (1024 * 1024);
    (
        to_mb(info.PhysicalAvailable),
        to_mb(info.CommitTotal),
        to_mb(info.CommitLimit),
    )
}

/// A `FILETIME` built from its two halves, for the `GetProcessTimes` out params.
fn filetime(high: u32) -> windows_sys::Win32::Foundation::FILETIME {
    windows_sys::Win32::Foundation::FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: high,
    }
}

/// `FILETIME` is 100 ns units; return whole milliseconds.
fn filetime_ms(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    let ticks = ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
    ticks / 10_000
}

/// Log one probe line. `tag` names the moment (`show`, `painted`, `hide`).
pub(crate) fn log(tag: &str, hwnd: *mut core::ffi::c_void, extra: &str) {
    let (control, state) = power_throttling();
    let (ws_mb, page_faults) = memory().unwrap_or((0, 0));
    let (procs, procs_ws_mb, procs_cpu_ms) = webview_tree();
    let (avail_mb, commit_mb, commit_limit_mb) = system_memory();
    let visible = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) };
    let heartbeat = heartbeat_age_ms()
        .map(|age| age.to_string())
        .unwrap_or_else(|| "none".to_string());
    let fields = format!(
        "idle_s={} pow={control}/{state} ws_mb={ws_mb} pf={page_faults} cloaked={} vis={visible}          wv_procs={procs} wv_ws_mb={procs_ws_mb} wv_cpu_ms={procs_cpu_ms} hb_age_ms={heartbeat}          gap_ms={} avail_mb={avail_mb} commit_mb={commit_mb}/{commit_limit_mb}{extra}",
        idle_seconds(),
        cloaked(hwnd),
        worst_gap_ms(),
    );
    crate::runtime::log_info(&format!("[nex][probe] {tag} {fields}"));
}
