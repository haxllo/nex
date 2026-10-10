use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;

use crate::overlay::ipc::{
    MAX_CHAT_ATTACHMENTS, MAX_CHAT_ATTACHMENTS_TOTAL_BYTES, MAX_CHAT_ATTACHMENT_BYTES,
    MAX_CHAT_ATTACHMENT_NAME_CHARS,
};
use serde::Serialize;

const MAX_CHAT_PICKER_ENTRIES: usize = 500;
const MAX_CHAT_PICKER_SCAN_ENTRIES: usize = 10_000;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatPickerEntry {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) is_directory: bool,
    pub(crate) size: Option<u64>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatPickerDirectory {
    pub(crate) location: String,
    pub(crate) can_go_up: bool,
    pub(crate) truncated: bool,
    pub(crate) entries: Vec<ChatPickerEntry>,
    #[serde(skip)]
    pub(crate) current_dir: Option<PathBuf>,
}

pub(crate) fn chat_picker_locations() -> ChatPickerDirectory {
    let mut locations = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        if home.is_dir() {
            locations.push(("Home".to_string(), home));
        }
    }

    let drive_mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    for drive in 0..26 {
        if drive_mask & (1 << drive) == 0 {
            continue;
        }
        let path = PathBuf::from(format!("{}:\\", char::from(b'A' + drive as u8)));
        if path.is_dir() {
            locations.push((path.display().to_string(), path));
        }
    }

    ChatPickerDirectory {
        location: "This PC".into(),
        can_go_up: false,
        truncated: false,
        entries: locations
            .into_iter()
            .enumerate()
            .map(|(id, (name, path))| ChatPickerEntry {
                id: id as u64,
                name,
                is_directory: true,
                size: None,
                path,
            })
            .collect(),
        current_dir: None,
    }
}

pub(crate) fn chat_picker_directory(path: PathBuf) -> Result<ChatPickerDirectory, String> {
    let metadata = fs::metadata(&path).map_err(|e| format!("Couldn't open this folder: {e}"))?;
    if !metadata.is_dir() {
        return Err("This location is no longer a folder.".into());
    }

    let mut entries = Vec::new();
    let mut truncated = false;
    for (scanned, entry) in fs::read_dir(&path)
        .map_err(|e| format!("Couldn't read this folder: {e}"))?
        .enumerate()
    {
        if scanned >= MAX_CHAT_PICKER_SCAN_ENTRIES || entries.len() >= MAX_CHAT_PICKER_ENTRIES {
            truncated = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let is_directory = file_type.is_dir();
        if !is_directory && (!file_type.is_file() || !is_supported_extension(&entry.path())) {
            continue;
        }
        let size = if is_directory {
            None
        } else {
            entry.metadata().ok().map(|metadata| metadata.len())
        };
        entries.push(ChatPickerEntry {
            id: 0,
            name: entry.file_name().to_string_lossy().into_owned(),
            is_directory,
            size,
            path: entry.path(),
        });
    }
    entries.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for (id, entry) in entries.iter_mut().enumerate() {
        entry.id = id as u64;
    }

    // Every directory listing can go back: either to its parent directory
    // or, at a drive root, to the "This PC" locations view.
    let can_go_up = true;
    Ok(ChatPickerDirectory {
        location: path.display().to_string(),
        can_go_up,
        truncated,
        entries,
        current_dir: Some(path),
    })
}

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "json", "json5", "log", "xml", "toml", "ini", "yaml", "yml", "rs", "py",
    "js", "ts", "tsx", "jsx", "html", "css", "ps1", "bat", "cmd", "sh", "sql",
];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelectedChatFile {
    pub(crate) name: String,
    pub(crate) content: String,
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
