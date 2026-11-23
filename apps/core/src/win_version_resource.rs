//! Windows version-resource reads, in process.
//!
//! Both callers of this module previously asked PowerShell for the same data:
//! `[System.Diagnostics.FileVersionInfo]::GetVersionInfo(...)` behind
//! `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass
//! -WindowStyle Hidden -Command`. That argument set is the shape malware
//! loaders use to hide a payload, and a user's antivirus flagged it (CIQ-201).
//! The work was a file read either way, so it happens here instead: no child
//! process, no hidden window, no flag that exists only to look suspicious.

/// Read one string field out of a PE version resource.
///
/// `field` is a `StringFileInfo` name (`CompanyName`, `FileVersion`,
/// `ProductVersion`, ...). `None` covers everything the old script swallowed in
/// a bare `catch {}`: no resource, an unreadable file, a missing field.
#[cfg(target_os = "windows")]
pub(crate) fn version_string(path: &str, field: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW};

    let mut wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().collect();
    if wide.contains(&0) {
        return None;
    }
    wide.push(0);

    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        // A resource that reports a size but cannot be read is a real case on
        // truncated or still-being-written binaries during discovery.
        let mut buffer = vec![0u8; size as usize];
        let read = GetFileVersionInfoW(wide.as_ptr(), 0, size, buffer.as_mut_ptr().cast());
        if read == 0 {
            return None;
        }

        version_field_from_block(&buffer, field)
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn version_string(_path: &str, _field: &str) -> Option<String> {
    None
}

/// The installed build's version, as the file resource reports it.
///
/// A `VS_FIXEDFILEINFO` FileVersion is four parts (`3.3.0.0`), while every
/// other version in Nex is three. A trailing `.0` fourth part is dropped so
/// the value compares and displays like `CARGO_PKG_VERSION`.
pub(crate) fn file_version(path: &str) -> Option<String> {
    let raw = version_string(path, "FileVersion")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() == 4 && parts[3] == "0" {
        return Some(parts[..3].join("."));
    }
    Some(trimmed.to_string())
}

/// Pull one field out of a `VS_VERSIONINFO` tree.
///
/// The field lives in `\StringFileInfo\<lang><codepage>\<field>`, and the
/// language table can hold several entries, so every string table is visited
/// and the first non-empty value wins — the same fallback .NET applied.
#[cfg(target_os = "windows")]
unsafe fn version_field_from_block(block: &[u8], field: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::VerQueryValueW;

    let mut translations_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut translations_len = 0u32;
    let translations_key: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
    if unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            translations_key.as_ptr(),
            &mut translations_ptr,
            &mut translations_len,
        )
    } == 0
    {
        return None;
    }
    if translations_ptr.is_null() || translations_len < 4 {
        return None;
    }

    let count = (translations_len as usize) / 4;
    let pairs = unsafe { std::slice::from_raw_parts(translations_ptr.cast::<u16>(), count * 2) };
    for pair in pairs.chunks_exact(2) {
        let (language, codepage) = (pair[0], pair[1]);
        let key = format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\{field}\0");
        let key: Vec<u16> = key.encode_utf16().collect();
        let mut value_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut value_len = 0u32;
        if unsafe {
            VerQueryValueW(
                block.as_ptr().cast(),
                key.as_ptr(),
                &mut value_ptr,
                &mut value_len,
            )
        } == 0
            || value_ptr.is_null()
            || value_len == 0
        {
            continue;
        }
        let units = unsafe { std::slice::from_raw_parts(value_ptr.cast::<u16>(), value_len as usize) };
        let text = String::from_utf16_lossy(units);
        let text = text.trim_matches('\0').trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}
