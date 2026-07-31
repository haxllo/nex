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

/// Launch the standard Windows shutdown dialog (with 60s countdown).
/// More familiar to users than instant ExitWindowsEx. Used by tray menu.
pub fn shell_shutdown_dialog() -> Result<(), String> {
    let output = std::process::Command::new("shutdown.exe")
        .args(&["/s", "/t", "60"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("shutdown.exe spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}

/// Launch the standard Windows restart dialog (with 60s countdown).
pub fn shell_restart_dialog() -> Result<(), String> {
    let output = std::process::Command::new("shutdown.exe")
        .args(&["/r", "/t", "60"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("shutdown.exe spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}
