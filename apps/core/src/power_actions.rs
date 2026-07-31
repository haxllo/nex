//! Power management actions (Lock, Sleep, Shutdown, Restart, Sign Out).
//!
//! Windows-only. All functions return `Result<(), String>` for uniform
//! error handling at the call site.

use std::os::windows::process::CommandExt;

/// Lock the workstation immediately.
pub fn lock() -> Result<(), String> {
    let output = std::process::Command::new("rundll32.exe")
        .args(&["user32.dll,LockWorkStation"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("rundll32 lock failed: {e}"))?;
    if !output.status.success() {
        return Err(format!("LockWorkStation failed with status {}", output.status));
    }
    Ok(())
}

/// Put the computer to sleep.
pub fn sleep() -> Result<(), String> {
    let output = std::process::Command::new("rundll32.exe")
        .args(&["powrprof.dll,SetSuspendState", "0,1,0"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("rundll32 sleep failed: {e}"))?;
    if !output.status.success() {
        return Err(format!("SetSuspendState failed with status {}", output.status));
    }
    Ok(())
}

/// Shut down immediately (with force-if-hung).
pub fn shutdown() -> Result<(), String> {
    let output = std::process::Command::new("shutdown.exe")
        .args(&["/s", "/t", "0", "/f"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("shutdown.exe spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}

/// Restart immediately (with force-if-hung).
pub fn restart() -> Result<(), String> {
    let output = std::process::Command::new("shutdown.exe")
        .args(&["/r", "/t", "0", "/f"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("shutdown.exe spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}

/// Sign out immediately (with force-if-hung).
pub fn sign_out() -> Result<(), String> {
    let output = std::process::Command::new("shutdown.exe")
        .args(&["/l"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("shutdown.exe spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}

/// Ask the user to confirm an immediate power action via a native dialog.
/// Returns `true` when the user picks "Yes".
fn confirm_now(title: &str, message: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
    };

    let title_wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let msg_wide: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            msg_wide.as_ptr(),
            title_wide.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    result == 6 // IDYES
}

/// Confirm with the user, then shut down immediately (no countdown).
pub fn confirm_and_shutdown() -> Result<(), String> {
    if confirm_now("Nex — Shut down", "Shut down now?\n\nUnsaved work in other apps may be lost.") {
        shutdown()?;
    }
    Ok(())
}

/// Confirm with the user, then restart immediately (no countdown).
pub fn confirm_and_restart() -> Result<(), String> {
    if confirm_now("Nex — Restart", "Restart now?\n\nUnsaved work in other apps may be lost.") {
        restart()?;
    }
    Ok(())
}
