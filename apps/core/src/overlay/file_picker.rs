use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use serde::Serialize;
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, PCWSTR, w};

use crate::overlay::ipc::{
    MAX_CHAT_ATTACHMENT_BYTES, MAX_CHAT_ATTACHMENT_NAME_CHARS, MAX_CHAT_ATTACHMENTS,
    MAX_CHAT_ATTACHMENTS_TOTAL_BYTES,
};

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "json", "json5", "log", "xml", "toml", "ini", "yaml", "yml", "rs", "py",
    "js", "ts", "tsx", "jsx", "html", "css", "ps1", "bat", "cmd", "sh", "sql",
];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelectedChatFile {
    pub(crate) name: String,
    pub(crate) content: String,
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(crate) fn show_chat_file_dialog(owner: isize) -> Result<Option<Vec<PathBuf>>, String> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() {
        return Err("Nex couldn't initialize the Windows file picker.".into());
    }
    let _apartment = ComApartment;

    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| "Nex couldn't start the Windows file picker.".to_string())?;

    let patterns = SUPPORTED_EXTENSIONS
        .iter()
        .map(|extension| format!("*.{extension}"))
        .collect::<Vec<_>>()
        .join(";");
    let filter_name = wide("Text and source files");
    let filter_pattern = wide(&patterns);
    let filter = COMDLG_FILTERSPEC {
        pszName: PCWSTR(filter_name.as_ptr()),
        pszSpec: PCWSTR(filter_pattern.as_ptr()),
    };
    unsafe {
        dialog
            .SetFileTypes(&[filter])
            .map_err(|_| "Nex couldn't configure the Windows file picker.".to_string())?;
        let options = dialog
            .GetOptions()
            .map_err(|_| "Nex couldn't configure the Windows file picker.".to_string())?;
        dialog
            .SetOptions(
                options
                    | FOS_ALLOWMULTISELECT
                    | FOS_DONTADDTORECENT
                    | FOS_FILEMUSTEXIST
                    | FOS_FORCEFILESYSTEM,
            )
            .map_err(|_| "Nex couldn't configure the Windows file picker.".to_string())?;
        dialog
            .SetTitle(w!("Choose text files to attach"))
            .map_err(|_| "Nex couldn't configure the Windows file picker.".to_string())?;
    }

    let owner = (owner != 0).then_some(HWND(owner as *mut _));
    if let Err(error) = unsafe { dialog.Show(owner) } {
        if error.code() == HRESULT(ERROR_CANCELLED.0 as i32) {
            return Ok(None);
        }
        return Err("Windows couldn't open the file picker.".into());
    }

    let items = unsafe { dialog.GetResults() }
        .map_err(|_| "Windows couldn't read the selected files.".to_string())?;
    let count = unsafe { items.GetCount() }
        .map_err(|_| "Windows couldn't read the selected files.".to_string())?;
    if count as usize > MAX_CHAT_ATTACHMENTS {
        return Err("Choose no more than three files at a time.".into());
    }

    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let item = unsafe { items.GetItemAt(index) }
            .map_err(|_| "Windows couldn't read the selected files.".to_string())?;
        let display_name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
            .map_err(|_| "Choose files stored on this device.".to_string())?;
        let path = PathBuf::from(unsafe { OsString::from_wide(display_name.as_wide()) });
        unsafe { CoTaskMemFree(Some(display_name.as_ptr().cast())) };
        paths.push(path);
    }
    Ok(Some(paths))
}

pub(crate) fn read_chat_files(paths: Vec<PathBuf>) -> Result<Vec<SelectedChatFile>, String> {
    if paths.len() > MAX_CHAT_ATTACHMENTS {
        return Err("Choose no more than three files at a time.".into());
    }

    let mut total_bytes = 0;
    let mut selected = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .map(|name| safe_name(&name.to_string_lossy()))
            .map(|name| {
                name.chars()
                    .take(MAX_CHAT_ATTACHMENT_NAME_CHARS)
                    .collect::<String>()
            })
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "Nex couldn't use one of those file names.".to_string())?;
        if !is_supported_extension(&path) {
            return Err(format!("{name} isn't a supported text or source file."));
        }

        let file = File::open(&path).map_err(|_| format!("Nex couldn't read {name}."))?;
        let mut bytes = Vec::new();
        file.take((MAX_CHAT_ATTACHMENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| format!("Nex couldn't read {name}."))?;
        if bytes.len() > MAX_CHAT_ATTACHMENT_BYTES {
            return Err(format!("{name} is over the 12 KB per-file limit."));
        }
        total_bytes += bytes.len();
        if total_bytes > MAX_CHAT_ATTACHMENTS_TOTAL_BYTES {
            return Err("Selected files are over the 20 KB total limit.".into());
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| format!("{name} doesn't look like a supported text file."))?;
        if content.contains('\0') {
            return Err(format!("{name} doesn't look like a supported text file."));
        }
        selected.push(SelectedChatFile { name, content });
    }
    Ok(selected)
}

fn is_supported_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\') {
                '_'
            } else {
                character
            }
        })
        .collect()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{is_supported_extension, safe_name};
    use std::path::Path;

    #[test]
    fn accepts_supported_extensions_case_insensitively() {
        assert!(is_supported_extension(Path::new("report.JSON")));
        assert!(is_supported_extension(Path::new("main.rs")));
        assert!(!is_supported_extension(Path::new("image.png")));
        assert!(!is_supported_extension(Path::new("README")));
    }

    #[test]
    fn sanitizes_control_characters_from_display_names() {
        assert_eq!(safe_name("test\nfile.rs"), "test_file.rs");
    }
}
