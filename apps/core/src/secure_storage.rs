#![cfg(target_os = "windows")]

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

pub(crate) fn protect(input: &[u8], marker: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::CryptProtectData;

    let cb_data = u32::try_from(input.len()).ok()?;
    let blob = DataBlob {
        cb_data,
        pb_data: input.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &blob as *const _ as *const _,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0x1,
            &mut output as *mut _ as *mut _,
        )
    };
    if ok == 0 {
        return None;
    }
    let encrypted = if output.cb_data == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() }
    };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pb_data as _) };

    let mut protected = marker.to_vec();
    protected.extend(encrypted);
    Some(protected)
}

pub(crate) fn unprotect(input: &[u8], marker: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::CryptUnprotectData;

    let payload = input.strip_prefix(marker)?;
    let cb_data = u32::try_from(payload.len()).ok()?;
    let blob = DataBlob {
        cb_data,
        pb_data: payload.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &blob as *const _ as *const _,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0x1,
            &mut output as *mut _ as *mut _,
        )
    };
    if ok == 0 {
        return None;
    }
    let plain = if output.cb_data == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() }
    };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pb_data as _) };
    Some(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_data_is_scoped_and_round_trips() {
        let protected = protect(b"private Nex data", b"NEXTEST1").unwrap();
        assert_eq!(
            unprotect(&protected, b"NEXTEST1").as_deref(),
            Some(b"private Nex data".as_slice())
        );
        assert!(unprotect(&protected, b"OTHER001").is_none());
    }
}
