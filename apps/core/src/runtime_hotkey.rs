use crate::config::{self, Config};
use crate::runtime::log_info;
use crate::runtime::log_warn;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ForegroundWindowSnapshot {
    pub(crate) class_name: String,
    pub(crate) process_name: String,
    pub(crate) process_path: String,
    pub(crate) covers_monitor: bool,
    pub(crate) has_standard_frame: bool,
    pub(crate) maximized: bool,
}

#[cfg(target_os = "windows")]
use crate::overlay::NativeOverlayShell;

#[cfg(target_os = "windows")]
pub(crate) fn toggle_game_mode_from_tray(
    overlay: &NativeOverlayShell,
    runtime_config: &mut Config,
) {
    let previous = runtime_config.game_mode_enabled;
    let next = !previous;
    runtime_config.game_mode_enabled = next;
    overlay.set_game_mode_enabled(next);

    if let Err(error) = config::write_user_template(runtime_config, &runtime_config.config_path) {
        runtime_config.game_mode_enabled = previous;
        overlay.set_game_mode_enabled(previous);
        log_warn(&format!(
            "[nex] failed to persist game mode toggle: {error}"
        ));
        overlay.set_status_text("Could not update Game Mode setting");
        return;
    }

    log_info(&format!(
        "[nex] game mode updated from tray: enabled={next}"
    ));
    overlay.set_status_text(if next {
        "Game Mode enabled"
    } else {
        "Game Mode disabled"
    });
}

#[cfg(target_os = "windows")]
pub(crate) fn should_suppress_hotkey_for_game_mode(cfg: &Config) -> bool {
    if !cfg.game_mode_enabled {
        return false;
    }

    collect_foreground_window_snapshot()
        .is_some_and(|snapshot| should_block_hotkey_for_foreground_window(&snapshot))
}

#[cfg(target_os = "windows")]
fn collect_foreground_window_snapshot() -> Option<ForegroundWindowSnapshot> {
    use windows_sys::Win32::Foundation::{CloseHandle, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_STYLE, SW_SHOWMAXIMIZED,
        WINDOWPLACEMENT, WS_CAPTION, WS_MAXIMIZE, WS_SYSMENU, WS_THICKFRAME,
    };
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }
    if unsafe { IsWindowVisible(foreground) } == 0 || unsafe { IsIconic(foreground) } != 0 {
        return None;
    }

    let mut class_buf = [0u16; 128];
    let class_len =
        unsafe { GetClassNameW(foreground, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    let class_name = if class_len > 0 {
        String::from_utf16_lossy(&class_buf[..class_len as usize])
    } else {
        String::new()
    };
    if is_shell_surface_class_name(&class_name) {
        return None;
    }

    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }

    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rcWork: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        dwFlags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut monitor_info as *mut MONITORINFO) } == 0 {
        return None;
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(foreground, &mut rect as *mut RECT) } == 0 {
        return None;
    }

    let fuzz = 2;
    let covers_monitor = rect.left <= monitor_info.rcMonitor.left + fuzz
        && rect.top <= monitor_info.rcMonitor.top + fuzz
        && rect.right >= monitor_info.rcMonitor.right - fuzz
        && rect.bottom >= monitor_info.rcMonitor.bottom - fuzz;

    let style = unsafe { GetWindowLongPtrW(foreground, GWL_STYLE) as u32 };
    let has_standard_frame = style & ((WS_CAPTION | WS_THICKFRAME | WS_SYSMENU) as u32) != 0;

    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        flags: 0,
        showCmd: 0,
        ptMinPosition: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        ptMaxPosition: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        rcNormalPosition: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    };
    let placement_reports_maximized =
        unsafe { GetWindowPlacement(foreground, &mut placement as *mut WINDOWPLACEMENT) } != 0
            && placement.showCmd == SW_SHOWMAXIMIZED as u32;
    let maximized = placement_reports_maximized || (style & (WS_MAXIMIZE as u32) != 0);

    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(foreground, &mut pid as *mut u32);
    }

    let mut process_path = String::new();
    let mut process_name = String::new();
    if pid != 0 {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if !process.is_null() {
            let mut buffer = vec![0u16; 1024];
            let mut length = buffer.len() as u32;
            let ok = unsafe {
                QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length as *mut u32)
            };
            if ok != 0 && length > 0 {
                process_path = String::from_utf16_lossy(&buffer[..length as usize]);
                process_name = std::path::Path::new(&process_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
            }
            unsafe {
                CloseHandle(process);
            }
        }
    }

    Some(ForegroundWindowSnapshot {
        class_name,
        process_name,
        process_path,
        covers_monitor,
        has_standard_frame,
        maximized,
    })
}

pub(crate) fn should_block_hotkey_for_foreground_window(
    snapshot: &ForegroundWindowSnapshot,
) -> bool {
    if !snapshot.covers_monitor {
        return false;
    }
    if snapshot.maximized && snapshot.has_standard_frame {
        return false;
    }

    let process_name = snapshot.process_name.trim().to_ascii_lowercase();
    let process_path = snapshot.process_path.trim().to_ascii_lowercase();
    if is_known_non_game_process(&process_name) || is_known_non_game_path(&process_path) {
        return false;
    }
    if is_known_game_process(&process_name) || is_likely_game_path(&process_path) {
        return true;
    }

    !snapshot.has_standard_frame
}

fn is_shell_surface_class_name(class_name: &str) -> bool {
    matches!(
        class_name.trim().to_ascii_lowercase().as_str(),
        "progman" | "workerw" | "shell_traywnd"
    )
}

fn is_known_non_game_process(process_name: &str) -> bool {
    matches!(
        process_name,
        "" | "explorer.exe"
            | "taskmgr.exe"
            | "chrome.exe"
            | "msedge.exe"
            | "firefox.exe"
            | "waterfox.exe"
            | "code.exe"
            | "devenv.exe"
            | "wezterm-gui.exe"
            | "windowsterminal.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "cmd.exe"
            | "notepad.exe"
            | "notepad++.exe"
            | "vlc.exe"
            | "mpv.exe"
            | "obs64.exe"
            | "applicationframehost.exe"
            | "searchhost.exe"
            | "startmenuexperiencehost.exe"
            | "lockapp.exe"
    )
}

fn is_known_non_game_path(process_path: &str) -> bool {
    process_path.contains("\\microsoft\\edge\\application\\")
        || process_path.contains("\\google\\chrome\\application\\")
        || process_path.contains("\\mozilla firefox\\")
        || process_path.contains("\\microsoft\\windowsapps\\")
}

fn is_known_game_process(process_name: &str) -> bool {
    process_name.contains("valorant")
        || process_name == "cs2.exe"
        || process_name == "csgo.exe"
        || process_name == "cod.exe"
        || process_name == "modernwarfare.exe"
        || process_name.ends_with("-shipping.exe")
}

fn is_likely_game_path(process_path: &str) -> bool {
    process_path.contains("\\steamapps\\common\\")
        || process_path.contains("\\riot games\\")
        || process_path.contains("\\epic games\\")
        || process_path.contains("\\battle.net\\")
        || process_path.contains("\\blizzard entertainment\\")
        || process_path.contains("\\ubisoft\\")
        || process_path.contains("\\rockstar games\\")
        || process_path.contains("\\gog galaxy\\games\\")
        || process_path.contains("\\ea games\\")
        || process_path.contains("\\electronic arts\\")
}
