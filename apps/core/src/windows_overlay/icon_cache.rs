#![allow(dead_code)]

use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, DeleteObject, DrawTextW, GetObjectW, SelectObject, SetBkMode, SetTextColor,
    DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBITMAP, HDC, TRANSPARENT,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, FindExecutableW, HlinkResolveShortcutToString, SHCreateItemFromParsingName,
    SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_ICON, SHGFI_ICONLOCATION,
    SHGFI_LARGEICON, SHGFI_PIDL, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, DrawIconEx, KillTimer, SetTimer, ICONINFO,
};

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::windows_overlay::state::{IconCacheMetrics, IconLoadRequest, OverlayShellState};
use crate::windows_overlay::types::*;

// Constants not in windows-sys 0.59
const DI_NORMAL: u32 = 0x0000_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const SHGFI_SHELLICONSIZE: u32 = 0x0000_0004;
extern "system" {
    fn ImageList_GetIcon(himl: isize, i: i32, flags: u32) -> *mut core::ffi::c_void;
}

// ==================== IShellItemImageFactory COM interface ====================
// Not available in windows-sys 0.59.x; define manually.
// IShellItemImageFactory GUID: {bcc18b79-ba16-442f-80c4-8a59c30c463b}

// SIIGBF flags (from windows-sys, but kept local for clarity)
const SIIGBF_RESIZETOFIT: i32 = 0i32;
const SIIGBF_BIGGERSIZEOK: i32 = 1i32;
const SIIGBF_ICONONLY: i32 = 4i32;

#[allow(non_snake_case, non_upper_case_globals, dead_code)]
mod com_defs {
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::HBITMAP;

    pub(crate) const IID_IShellItem: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);
    pub(crate) const IID_IShellItemImageFactory: GUID =
        GUID::from_u128(0xbcc18b79_ba16_442f_80c4_8a59c30c463b);

    pub(crate) struct IShellItemImageFactoryVtbl {
        #[allow(dead_code)]
        pub(crate) parent: IUnknownVtbl,
        pub(crate) GetImage: unsafe extern "system" fn(
            this: *mut core::ffi::c_void,
            size: SIZE,
            flags: i32,
            phbm: *mut HBITMAP,
        ) -> i32,
    }

    #[repr(C)]
    pub(crate) struct IUnknownVtbl {
        pub(crate) QueryInterface: unsafe extern "system" fn(
            this: *mut core::ffi::c_void,
            riid: *const GUID,
            ppv: *mut *mut core::ffi::c_void,
        ) -> i32,
        pub(crate) AddRef: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
        pub(crate) Release: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
    }
}
use com_defs::*;

/// Load a shell icon via `IShellItemImageFactory::GetImage`.
/// This is the modern, DPI-aware replacement for `SHGetFileInfoW`.
fn shell_icon_via_image_factory(path: &str, desired_size: i32) -> Option<isize> {
    let wide_path = to_wide(path);
    let mut shell_item: *mut core::ffi::c_void = std::ptr::null_mut();

    // 1. Create IShellItem from parsing name
    let hr = unsafe {
        SHCreateItemFromParsingName(
            wide_path.as_ptr(),
            std::ptr::null_mut(),
            &IID_IShellItem,
            &mut shell_item,
        )
    };
    if hr < 0 || shell_item.is_null() {
        if hr < 0 {
            crate::logging::info(&format!(
                "[nex] shell_icon_via_image_factory: SHCreateItemFromParsingName failed for path={} hr={:#x}",
                path, hr
            ));
        }
        return None;
    }

    // 2. QI for IShellItemImageFactory
    let mut factory: *mut core::ffi::c_void = std::ptr::null_mut();
    let hr = unsafe {
        let vtbl = &*(shell_item as *const *const IUnknownVtbl);
        ((*(*vtbl)).QueryInterface)(shell_item, &IID_IShellItemImageFactory, &mut factory)
    };
    if hr < 0 || factory.is_null() {
        crate::logging::info(&format!(
            "[nex] shell_icon_via_image_factory: QI for IShellItemImageFactory failed hr={:#x}",
            hr
        ));
        unsafe {
            let vtbl = &*(shell_item as *const *const IUnknownVtbl);
            ((*(*vtbl)).Release)(shell_item);
        }
        return None;
    }

    // 3. Request the image at the desired size
    let size = SIZE {
        cx: desired_size,
        cy: desired_size,
    };
    let flags = SIIGBF_RESIZETOFIT | SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK;
    let mut hbitmap: HBITMAP = std::ptr::null_mut();
    let hr = unsafe {
        let vtbl = &*(factory as *const *const IShellItemImageFactoryVtbl);
        ((*(*vtbl)).GetImage)(factory, size, flags, &mut hbitmap)
    };

    // Release factory
    unsafe {
        let vtbl = &*(factory as *const *const IUnknownVtbl);
        ((*(*vtbl)).Release)(factory);
    }
    // Release shell item
    unsafe {
        let vtbl = &*(shell_item as *const *const IUnknownVtbl);
        ((*(*vtbl)).Release)(shell_item);
    }

    if hr < 0 || hbitmap.is_null() {
        if hr < 0 {
            crate::logging::info(&format!(
                "[nex] shell_icon_via_image_factory: GetImage failed for size={} hr={:#x}",
                desired_size, hr
            ));
        }
        return None;
    }

    // 4. Convert HBITMAP to HICON
    let hicon = hbitmap_to_icon(hbitmap);
    unsafe {
        DeleteObject(hbitmap as _);
    }
    if hicon.is_none() {
        crate::logging::info("[nex] shell_icon_via_image_factory: hbitmap_to_icon failed");
    }
    hicon
}

/// Convert a 32-bit PARGB HBITMAP to an HICON.
/// We create a 1x1 monochrome AND-mask (all zeros = use alpha channel)
/// and call CreateIconIndirect.
fn hbitmap_to_icon(hbitmap: HBITMAP) -> Option<isize> {
    // Get bitmap dimensions
    let mut bm: windows_sys::Win32::Graphics::Gdi::BITMAP = unsafe { std::mem::zeroed() };
    let got_size = unsafe {
        GetObjectW(
            hbitmap as _,
            std::mem::size_of::<windows_sys::Win32::Graphics::Gdi::BITMAP>() as i32,
            &mut bm as *mut _ as *mut core::ffi::c_void,
        )
    };
    if got_size == 0 {
        return None;
    }

    let width = bm.bmWidth;
    let height = bm.bmHeight;
    if width <= 0 || height <= 0 {
        return None;
    }

    // Create monochrome mask bitmap: all zeros = alpha channel controls transparency
    let mask_row_bytes = ((width + 15) / 16 * 2) as usize;
    let mask_data = vec![0u8; mask_row_bytes * height as usize];
    let hbm_mask = unsafe { CreateBitmap(width, height, 1, 1, mask_data.as_ptr() as _) };
    if hbm_mask.is_null() {
        return None;
    }

    let icon_info = ICONINFO {
        fIcon: 1, // TRUE = icon, not cursor
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbitmap,
    };

    let hicon = unsafe { CreateIconIndirect(&icon_info) };
    unsafe {
        DeleteObject(hbm_mask as _);
    }

    if hicon.is_null() {
        None
    } else {
        Some(hicon as isize)
    }
}

pub(crate) enum ActionIconKind {
    WebSearch,
    Uninstall,
    Clipboard,
    Settings,
    Diagnostics,
    Logs,
    Rebuild,
    Generic,
}

fn action_icon_kind_for_title(title: &str) -> ActionIconKind {
    let lower = title.to_ascii_lowercase();
    if lower.contains("web") || lower.contains("search") {
        ActionIconKind::WebSearch
    } else if lower.contains("uninstall") || lower.contains("remove") {
        ActionIconKind::Uninstall
    } else if lower.contains("clipboard") {
        ActionIconKind::Clipboard
    } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer") {
        ActionIconKind::Settings
    } else if lower.contains("diagnostic") || lower.contains("bundle") || lower.contains("support")
    {
        ActionIconKind::Diagnostics
    } else if lower.contains("log") {
        ActionIconKind::Logs
    } else if lower.contains("rebuild") || lower.contains("index") || lower.contains("refresh") {
        ActionIconKind::Rebuild
    } else {
        ActionIconKind::Generic
    }
}

fn action_icon_codepoint(kind: ActionIconKind) -> u32 {
    match kind {
        ActionIconKind::WebSearch => 0xE721,   // Search
        ActionIconKind::Uninstall => 0xE74D,   // Delete
        ActionIconKind::Clipboard => 0xE8C8,   // Clipboard List
        ActionIconKind::Settings => 0xE713,    // Settings
        ActionIconKind::Diagnostics => 0xE8A5, // Page/Report
        ActionIconKind::Logs => 0xE8B7,        // Folder
        ActionIconKind::Rebuild => 0xE895,     // Sync
        ActionIconKind::Generic => 0xE756,     // Command Prompt
    }
}

fn kind_icon_codepoint(kind: &str) -> Option<u32> {
    match kind.to_ascii_lowercase().as_str() {
        "app" => Some(0xE714),    // Program
        "folder" => Some(0xE8B7), // Folder
        "file" => Some(0xE8A5),   // Document
        _ => None,
    }
}

/// Render any single codepoint glyph using the icon fonts.
/// Shared by action icons and kind-based file-type icons.
fn draw_glyph_with_icon_font(
    hdc: HDC,
    icon_rect: &RECT,
    state: &OverlayShellState,
    codepoint: u32,
    color: u32,
) -> bool {
    let Some(glyph) = char::from_u32(codepoint) else {
        return false;
    };
    let glyph_text = glyph.to_string();
    let glyph_wide = to_wide(&glyph_text);
    let mut glyph_rect = *icon_rect;
    glyph_rect.top += 1;

    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);

        if state.command_icon_font != 0 {
            let old_font = SelectObject(hdc, state.command_icon_font as _);
            let drawn = DrawTextW(
                hdc,
                glyph_wide.as_ptr(),
                -1,
                &mut glyph_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            ) != 0;
            SelectObject(hdc, old_font);
            if drawn {
                return true;
            }
        }

        if state.command_icon_fallback_font != 0 {
            let old_font = SelectObject(hdc, state.command_icon_fallback_font as _);
            let drawn = DrawTextW(
                hdc,
                glyph_wide.as_ptr(),
                -1,
                &mut glyph_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            ) != 0;
            SelectObject(hdc, old_font);
            if drawn {
                return true;
            }
        }
    }
    false
}

pub(crate) fn draw_action_icon(
    hdc: HDC,
    icon_rect: &RECT,
    row: &OverlayRow,
    state: &OverlayShellState,
    color: u32,
) -> bool {
    if !row.kind.eq_ignore_ascii_case("action") {
        return false;
    }
    let codepoint = action_icon_codepoint(action_icon_kind_for_title(&row.title));
    draw_glyph_with_icon_font(hdc, icon_rect, state, codepoint, color)
}

/// Draw an icon font glyph for non-action rows based on their kind.
/// Falls between shell icons (background-loaded) and the plain-text letter fallback.
pub(crate) fn draw_kind_icon(
    hdc: HDC,
    icon_rect: &RECT,
    row: &OverlayRow,
    state: &OverlayShellState,
    color: u32,
) -> bool {
    if row.kind.eq_ignore_ascii_case("action") || row.kind.eq_ignore_ascii_case("clipboard") {
        return false;
    }
    let Some(codepoint) = kind_icon_codepoint(&row.kind) else {
        return false;
    };
    draw_glyph_with_icon_font(hdc, icon_rect, state, codepoint, color)
}

pub(crate) fn draw_row_icon(
    hdc: HDC,
    icon_rect: &RECT,
    row: &OverlayRow,
    state: &mut OverlayShellState,
) -> bool {
    let Some(icon_handle) = icon_handle_for_row(state, row) else {
        return false;
    };
    let icon_size = state.icon_draw_size;
    let x = icon_rect.left + (state.icon_container_size - icon_size) / 2;
    let y = icon_rect.top + (state.icon_container_size - icon_size) / 2;
    unsafe {
        DrawIconEx(
            hdc,
            x,
            y,
            icon_handle as _,
            icon_size,
            icon_size,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        ) != 0
    }
}

fn icon_handle_for_row(state: &mut OverlayShellState, row: &OverlayRow) -> Option<isize> {
    let key = icon_cache_key(row);
    if let Some(cached) = state.icon_cache.get(&key).copied() {
        state.icon_cache_metrics.hits = state.icon_cache_metrics.hits.saturating_add(1);
        touch_icon_cache_key(state, &key);
        return if cached == 0 { None } else { Some(cached) };
    }
    state.icon_cache_metrics.misses = state.icon_cache_metrics.misses.saturating_add(1);

    if row.kind.eq_ignore_ascii_case("action") {
        insert_icon_cache_entry(state, key, 0);
        return None;
    }

    if should_queue_specific_icon_load(row) {
        if state.pending_icon_loads.insert(key.clone()) {
            if let Some(ref sender) = state.icon_load_sender {
                let request = IconLoadRequest {
                    key: key.clone(),
                    kind: row.kind.clone(),
                    icon_path: row.icon_path.clone(),
                    hwnd: state.overlay_hwnd as isize,
                };
                if sender.send(request).is_err() {
                    state.pending_icon_loads.remove(&key);
                    state.icon_cache_metrics.load_failures =
                        state.icon_cache_metrics.load_failures.saturating_add(1);
                }
            } else {
                state.icon_cache_metrics.load_failures =
                    state.icon_cache_metrics.load_failures.saturating_add(1);
            }
        }
    }

    None
}

fn action_icon_category(title: &str) -> &str {
    let lower = title.to_ascii_lowercase();
    if lower.contains("web") || lower.contains("search") {
        "web"
    } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer") {
        "settings"
    } else if lower.contains("restart") || lower.contains("quit") {
        "restart"
    } else if lower.contains("rebuild") || lower.contains("index") || lower.contains("refresh")
        || lower.contains("sync")
    {
        "sync"
    } else if lower.contains("diagnostic") || lower.contains("bundle") || lower.contains("support")
    {
        "diagnostics"
    } else if lower.contains("log") {
        "logs"
    } else if lower.contains("uninstall") || lower.contains("remove") {
        "uninstall"
    } else if lower.contains("clipboard") {
        "clipboard"
    } else {
        "generic"
    }
}

pub(crate) fn icon_cache_key(row: &OverlayRow) -> String {
    let kind = row.kind.to_ascii_lowercase();
    let source = row.icon_path.trim().to_ascii_lowercase();
    if !source.is_empty() {
        return format!("kind:{kind}|{source}");
    }
    if kind == "action" {
        return format!("kind:action:{}", action_icon_category(&row.title));
    }
    format!("kind:{kind}")
}

fn should_queue_specific_icon_load(row: &OverlayRow) -> bool {
    !(row.kind.eq_ignore_ascii_case("action") || row.kind.eq_ignore_ascii_case("clipboard"))
}

pub(crate) fn load_shell_icon_for_values(kind: &str, icon_path: &str) -> Option<isize> {
    let row = OverlayRow {
        role: OverlayRowRole::Item,
        result_index: 0,
        kind: kind.to_string(),
        title: String::new(),
        path: icon_path.to_string(),
        icon_path: icon_path.to_string(),
    };
    load_shell_icon_for_row(&row)
}

fn load_shell_icon_for_row(row: &OverlayRow) -> Option<isize> {
    let kind = row.kind.to_ascii_lowercase();
    let source = row.icon_path.trim();
    let is_app_shortcut = kind == "app" && source.to_ascii_lowercase().ends_with(".lnk");

    // Action/command rows are semantic operations, not filesystem targets.
    // Force deterministic in-app iconography instead of generic shell-file icons.
    if kind == "action" {
        return None;
    }

    if kind == "folder" {
        return shell_icon_with_attrs("folder", FILE_ATTRIBUTE_DIRECTORY);
    }

    if !source.is_empty() {
        if let Some(icon) = shell_icon_from_appsfolder_target(source) {
            return Some(icon);
        }
        if is_app_shortcut {
            if let Some(icon) = executable_icon_from_shortcut_hlink(source) {
                return Some(icon);
            }
            if let Some(icon) = shortcut_target_icon(source) {
                return Some(icon);
            }
            if let Some(icon) = executable_icon_from_shortcut(source) {
                return Some(icon);
            }
            if let Some(icon) = shortcut_system_icon_without_overlay(source) {
                return Some(icon);
            }
            // Do not extract icon directly from `.lnk` for app entries:
            // this is the primary source of shortcut-arrow overlays.
            if let Some(icon) = shell_icon_with_attrs("nex.exe", FILE_ATTRIBUTE_NORMAL) {
                return Some(icon);
            }
            return None;
        }
        if let Some(icon) = shell_icon_for_existing_path(source) {
            return Some(icon);
        }
        if let Some(icon) = shell_icon_with_attrs(source, FILE_ATTRIBUTE_NORMAL) {
            return Some(icon);
        }
        if let Some(icon) = shell_icon_via_image_factory(source, ROW_ICON_DRAW_SIZE) {
            return Some(icon);
        }
    }

    if kind == "app" {
        if let Some(icon) = shell_icon_with_attrs("nex.exe", FILE_ATTRIBUTE_NORMAL) {
            return Some(icon);
        }
    }

    shell_icon_with_attrs("file.txt", FILE_ATTRIBUTE_NORMAL)
}

fn shortcut_target_icon(shortcut_path: &str) -> Option<isize> {
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let wide_shortcut = to_wide(shortcut_path);
    let result = unsafe {
        SHGetFileInfoW(
            wide_shortcut.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICONLOCATION,
        )
    };
    if result == 0 {
        return None;
    }

    let icon_source = wide_buf_to_string(&info.szDisplayName);
    if icon_source.trim().is_empty() {
        return None;
    }
    if let Some(icon) = shell_icon_from_appsfolder_target(icon_source.trim()) {
        return Some(icon);
    }
    let (icon_path, parsed_index) = split_icon_resource_spec(icon_source.trim());
    let icon_index = if info.iIcon == 0 {
        parsed_index.unwrap_or(0)
    } else {
        info.iIcon
    };
    extract_icon_from_path(icon_path, icon_index)
}

fn extract_icon_from_path(path: &str, icon_index: i32) -> Option<isize> {
    let normalized = normalize_icon_source_path(path);
    if normalized.is_empty() {
        return None;
    }
    let wide_source = to_wide(&normalized);
    let mut large_icon = std::ptr::null_mut();
    let mut small_icon = std::ptr::null_mut();
    let extracted = unsafe {
        ExtractIconExW(
            wide_source.as_ptr(),
            icon_index,
            &mut large_icon,
            &mut small_icon,
            1,
        )
    };

    if !small_icon.is_null() {
        unsafe {
            DestroyIcon(small_icon);
        }
    }

    if extracted == 0 || large_icon.is_null() {
        return shell_icon_from_display_name(&normalized);
    }
    Some(large_icon as isize)
}

fn executable_icon_from_shortcut(shortcut_path: &str) -> Option<isize> {
    let wide_shortcut = to_wide(shortcut_path);
    let mut exe_out = vec![0u16; 260];
    let result = unsafe {
        FindExecutableW(
            wide_shortcut.as_ptr(),
            std::ptr::null(),
            exe_out.as_mut_ptr(),
        )
    };
    if (result as isize) <= 32 {
        return None;
    }
    let exe = wide_buf_to_string(&exe_out);
    let normalized = normalize_icon_source_path(exe.trim());
    if normalized.is_empty() {
        return None;
    }
    extract_icon_from_path(&normalized, 0)
}

fn executable_icon_from_shortcut_hlink(shortcut_path: &str) -> Option<isize> {
    let wide_shortcut = to_wide(shortcut_path);
    let mut target: windows_sys::core::PWSTR = std::ptr::null_mut();
    let mut location: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr =
        unsafe { HlinkResolveShortcutToString(wide_shortcut.as_ptr(), &mut target, &mut location) };

    let resolved_target = pwstr_to_string_and_free(target);
    let resolved_location = pwstr_to_string_and_free(location);

    if hr < 0 {
        return None;
    }
    let resolved_location_trimmed = resolved_location.trim();
    if !resolved_location_trimmed.is_empty() {
        if let Some(icon) = shell_icon_from_appsfolder_target(resolved_location_trimmed) {
            return Some(icon);
        }
        let (icon_path, parsed_index) = split_icon_resource_spec(resolved_location_trimmed);
        let normalized_icon_path = normalize_icon_source_path(icon_path);
        if is_icon_module_path(&normalized_icon_path) {
            if let Some(icon) =
                extract_icon_from_path(&normalized_icon_path, parsed_index.unwrap_or(0))
            {
                return Some(icon);
            }
        }
    }
    if let Some(icon) = shell_icon_from_appsfolder_target(resolved_target.trim()) {
        return Some(icon);
    }
    let normalized = normalize_icon_source_path(resolved_target.trim());
    if !normalized.is_empty() {
        if let Some(icon) = extract_icon_from_path(&normalized, 0) {
            return Some(icon);
        }
    }
    None
}

fn shell_icon_for_existing_path(path: &str) -> Option<isize> {
    let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let wide = to_wide(path);
    // Prefer direct shell icon extraction for concrete files/apps.
    // This tends to pick a better source icon than generic image-list lookup.
    let flags = SHGFI_ICON | SHGFI_LARGEICON | SHGFI_SHELLICONSIZE;
    let result = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            0,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if result == 0 || sfi.hIcon.is_null() {
        None
    } else {
        Some(sfi.hIcon as isize)
    }
}

fn shell_icon_with_attrs(path_hint: &str, attrs: u32) -> Option<isize> {
    let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let wide = to_wide(path_hint);
    let flags =
        SHGFI_SYSICONINDEX | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES | SHGFI_SHELLICONSIZE;
    let result = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if result == 0 || sfi.iIcon < 0 {
        None
    } else {
        let icon = unsafe { ImageList_GetIcon(result as _, sfi.iIcon, 0) };
        if icon.is_null() {
            None
        } else {
            Some(icon as isize)
        }
    }
}

fn shortcut_system_icon_without_overlay(shortcut_path: &str) -> Option<isize> {
    let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let wide = to_wide(shortcut_path);
    let flags = SHGFI_SYSICONINDEX | SHGFI_LARGEICON | SHGFI_SHELLICONSIZE;
    let result = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            0,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if result == 0 || sfi.iIcon < 0 {
        return None;
    }
    let icon = unsafe { ImageList_GetIcon(result as _, sfi.iIcon, 0) };
    if icon.is_null() {
        None
    } else {
        Some(icon as isize)
    }
}

fn shell_icon_from_appsfolder_target(target: &str) -> Option<isize> {
    for candidate in appsfolder_display_name_candidates(target) {
        if let Some(icon) = shell_icon_from_display_name(&candidate) {
            return Some(icon);
        }
    }
    None
}

fn appsfolder_display_name_candidates(target: &str) -> Vec<String> {
    let trimmed = target.trim().trim_matches('"');
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::with_capacity(6);
    push_unique_candidate(&mut candidates, trimmed);

    if let Some(appsfolder_token) = extract_appsfolder_token(trimmed) {
        push_unique_candidate(&mut candidates, &appsfolder_token);
        if appsfolder_token
            .to_ascii_lowercase()
            .starts_with("shell:appsfolder\\")
        {
            push_unique_candidate(&mut candidates, &appsfolder_token[6..]);
        } else if appsfolder_token
            .to_ascii_lowercase()
            .starts_with("appsfolder\\")
        {
            push_unique_candidate(&mut candidates, &format!("shell:{appsfolder_token}"));
        }
    }

    let lowered = trimmed.to_ascii_lowercase();
    if lowered.starts_with("appsfolder\\") {
        push_unique_candidate(&mut candidates, &format!("shell:{trimmed}"));
    } else if lowered.starts_with("shell:appsfolder\\") {
        push_unique_candidate(&mut candidates, &trimmed[6..]);
    } else if let Some(index) = lowered.find("appsfolder\\") {
        push_unique_candidate(&mut candidates, &format!("shell:{}", &trimmed[index..]));
    }

    candidates
}

fn push_unique_candidate(candidates: &mut Vec<String>, value: &str) {
    let normalized = value.trim();
    if normalized.is_empty() {
        return;
    }
    if candidates
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(normalized))
    {
        return;
    }
    candidates.push(normalized.to_string());
}

fn extract_appsfolder_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }

    let lowered = trimmed.to_ascii_lowercase();
    let start = lowered
        .find("shell:appsfolder\\")
        .or_else(|| lowered.find("appsfolder\\"))?;
    let tail = &trimmed[start..];
    if tail.is_empty() {
        return None;
    }

    let mut end = tail.len();
    for (index, ch) in tail.char_indices() {
        if index == 0 {
            continue;
        }
        if ch.is_whitespace() || ch == '"' || ch == '\'' {
            end = index;
            break;
        }
    }

    let token = tail[..end]
        .trim()
        .trim_end_matches(',')
        .trim_end_matches(';')
        .trim_matches('"')
        .trim_matches('\'');
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn is_icon_module_path(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    lowered.ends_with(".exe") || lowered.ends_with(".dll") || lowered.ends_with(".ico")
}

fn shell_icon_from_display_name(display_name: &str) -> Option<isize> {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return None;
    }

    let wide = to_wide(trimmed);
    let mut pidl: *mut windows_sys::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
    let hr = unsafe {
        SHParseDisplayName(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            std::ptr::null_mut(),
        )
    };
    if hr < 0 || pidl.is_null() {
        return None;
    }

    let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let flags = SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON | SHGFI_SHELLICONSIZE;
    let result = unsafe {
        SHGetFileInfoW(
            pidl as *const u16,
            0,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    unsafe {
        CoTaskMemFree(pidl as _);
    }
    if result == 0 || sfi.hIcon.is_null() {
        None
    } else {
        Some(sfi.hIcon as isize)
    }
}

fn pwstr_to_string_and_free(ptr: windows_sys::core::PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let len = unsafe { (0..).take_while(|&i| *ptr.add(i) != 0).count() };
    let s = if len > 0 {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        String::from_utf16_lossy(slice)
    } else {
        String::new()
    };
    unsafe {
        CoTaskMemFree(ptr as _);
    }
    s
}

fn wide_buf_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn split_icon_resource_spec(spec: &str) -> (&str, Option<i32>) {
    let trimmed = spec.trim();
    if let Some(comma_pos) = trimmed.rfind(',') {
        let after = trimmed[comma_pos + 1..].trim();
        if let Ok(index) = after.parse::<i32>() {
            (trimmed[..comma_pos].trim(), Some(index))
        } else {
            (trimmed, None)
        }
    } else {
        (trimmed, None)
    }
}

fn normalize_icon_source_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('"');
    if trimmed.is_empty() {
        return String::new();
    }
    // Expand environment variables (e.g. %SYSTEMROOT%)
    let mut result = String::with_capacity(trimmed.len());
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let mut var = String::new();
            for c in chars.by_ref() {
                if c == '%' {
                    break;
                }
                var.push(c);
            }
            if let Ok(value) = std::env::var(&var) {
                result.push_str(&value);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

pub(crate) fn clear_icon_cache(state: &mut OverlayShellState) {
    let cleared_entries = state.icon_cache.len();
    for handle in state.icon_cache.values() {
        if *handle != 0 {
            unsafe {
                DestroyIcon(*handle as _);
            }
        }
    }
    state.icon_cache.clear();
    state.icon_cache_lru.clear();
    log_icon_cache_metrics(state, "cache_clear", cleared_entries);
}

fn touch_icon_cache_key(state: &mut OverlayShellState, key: &str) {
    if let Some(index) = state.icon_cache_lru.iter().position(|k| k == key) {
        state.icon_cache_lru.remove(index);
    }
    state.icon_cache_lru.push_back(key.to_string());
}

pub(crate) fn insert_icon_cache_entry(state: &mut OverlayShellState, key: String, handle: isize) {
    if let Some(previous) = state.icon_cache.insert(key.clone(), handle) {
        if previous != 0 {
            unsafe {
                DestroyIcon(previous as _);
            }
        }
    }
    touch_icon_cache_key(state, &key);
    while state.icon_cache.len() > runtime_icon_cache_max_entries() {
        let Some(oldest_key) = state.icon_cache_lru.pop_front() else {
            break;
        };
        if oldest_key == key {
            continue;
        }
        if let Some(oldest_handle) = state.icon_cache.remove(&oldest_key) {
            state.icon_cache_metrics.evictions =
                state.icon_cache_metrics.evictions.saturating_add(1);
            if oldest_handle != 0 {
                unsafe {
                    DestroyIcon(oldest_handle as _);
                }
            }
        }
    }
}

fn log_icon_cache_metrics(state: &mut OverlayShellState, reason: &str, cleared_entries: usize) {
    let metrics = state.icon_cache_metrics;
    let live_entries = state.icon_cache.len();
    let max_entries = runtime_icon_cache_max_entries();
    if metrics.hits == 0
        && metrics.misses == 0
        && metrics.load_failures == 0
        && metrics.evictions == 0
        && cleared_entries == 0
    {
        return;
    }
    crate::logging::info(&format!(
        "[nex] overlay_icon_cache reason={} hits={} misses={} load_failures={} evictions={} cleared_entries={} live_entries={} max_entries={}",
        reason,
        metrics.hits,
        metrics.misses,
        metrics.load_failures,
        metrics.evictions,
        cleared_entries,
        live_entries,
        max_entries
    ));
    state.icon_cache_metrics = IconCacheMetrics::default();
}

pub(crate) fn log_memory_snapshot(reason: &str) {
    let process = unsafe { GetCurrentProcess() };
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetProcessMemoryInfo(
            process,
            &mut counters as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok == 0 {
        return;
    }

    let mb_divisor = 1024.0_f64 * 1024.0_f64;
    let working_set_mb = (counters.WorkingSetSize as f64) / mb_divisor;
    let private_mb = (counters.PagefileUsage as f64) / mb_divisor;
    crate::logging::info(&format!(
        "[nex] memory_snapshot reason={} working_set_mb={:.1} private_mb={:.1}",
        reason, working_set_mb, private_mb
    ));
}

pub(crate) fn configure_runtime_performance_tuning(
    idle_cache_trim_ms: u32,
    active_memory_target_mb: u16,
) {
    let idle_ms = idle_cache_trim_ms.clamp(250, 120_000);
    ICON_CACHE_IDLE_MS_RUNTIME.store(idle_ms, Ordering::Relaxed);

    // Keep icon-cache size proportional to active-memory target with a tighter cap so
    // active working set stays stable on large result sets.
    let max_entries = ((active_memory_target_mb as usize).saturating_mul(5) / 4).clamp(32, 256);
    ICON_CACHE_MAX_ENTRIES_RUNTIME.store(max_entries, Ordering::Relaxed);
    crate::logging::info(&format!(
        "[nex] overlay_tuning idle_cache_trim_ms={} active_memory_target_mb={} icon_cache_max_entries={}",
        idle_ms, active_memory_target_mb, max_entries
    ));
}

fn runtime_icon_cache_idle_ms() -> u32 {
    ICON_CACHE_IDLE_MS_RUNTIME
        .load(Ordering::Relaxed)
        .clamp(250, 120_000)
}

fn runtime_icon_cache_max_entries() -> usize {
    ICON_CACHE_MAX_ENTRIES_RUNTIME
        .load(Ordering::Relaxed)
        .clamp(32, 256)
}

pub(crate) fn schedule_icon_cache_idle_cleanup(hwnd: HWND) {
    unsafe {
        KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
        SetTimer(
            hwnd,
            TIMER_ICON_CACHE_IDLE,
            runtime_icon_cache_idle_ms(),
            None,
        );
    }
}

pub(crate) fn cancel_icon_cache_idle_cleanup(hwnd: HWND) {
    unsafe {
        KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_icon_kind_web_search() {
        assert!(matches!(
            action_icon_kind_for_title("Web Search"),
            ActionIconKind::WebSearch
        ));
        assert!(matches!(
            action_icon_kind_for_title("Search Everything"),
            ActionIconKind::WebSearch
        ));
        assert!(matches!(
            action_icon_kind_for_title("web"),
            ActionIconKind::WebSearch
        ));
    }

    #[test]
    fn action_icon_kind_uninstall() {
        assert!(matches!(
            action_icon_kind_for_title("Uninstall App"),
            ActionIconKind::Uninstall
        ));
        assert!(matches!(
            action_icon_kind_for_title("Remove Program"),
            ActionIconKind::Uninstall
        ));
    }

    #[test]
    fn action_icon_kind_clipboard() {
        assert!(matches!(
            action_icon_kind_for_title("Clipboard History"),
            ActionIconKind::Clipboard
        ));
    }

    #[test]
    fn action_icon_kind_settings() {
        assert!(matches!(
            action_icon_kind_for_title("Config Editor"),
            ActionIconKind::Settings
        ));
        assert!(matches!(
            action_icon_kind_for_title("Settings"),
            ActionIconKind::Settings
        ));
        assert!(matches!(
            action_icon_kind_for_title("Preferences"),
            ActionIconKind::Settings
        ));
    }

    #[test]
    fn action_icon_kind_diagnostics() {
        assert!(matches!(
            action_icon_kind_for_title("Diagnostics"),
            ActionIconKind::Diagnostics
        ));
        assert!(matches!(
            action_icon_kind_for_title("Support Bundle"),
            ActionIconKind::Diagnostics
        ));
    }

    #[test]
    fn action_icon_kind_logs() {
        assert!(matches!(
            action_icon_kind_for_title("View Logs"),
            ActionIconKind::Logs
        ));
    }

    #[test]
    fn action_icon_kind_rebuild() {
        assert!(matches!(
            action_icon_kind_for_title("Rebuild Index"),
            ActionIconKind::Rebuild
        ));
        assert!(matches!(
            action_icon_kind_for_title("Refresh"),
            ActionIconKind::Rebuild
        ));
    }

    #[test]
    fn action_icon_kind_generic_fallback() {
        assert!(matches!(
            action_icon_kind_for_title("Custom Action"),
            ActionIconKind::Generic
        ));
        assert!(matches!(
            action_icon_kind_for_title("Run Script"),
            ActionIconKind::Generic
        ));
    }

    #[test]
    fn action_icon_codepoint_returns_expected_values() {
        assert_eq!(action_icon_codepoint(ActionIconKind::WebSearch), 0xE721);
        assert_eq!(action_icon_codepoint(ActionIconKind::Uninstall), 0xE74D);
        assert_eq!(action_icon_codepoint(ActionIconKind::Clipboard), 0xE8C8);
        assert_eq!(action_icon_codepoint(ActionIconKind::Settings), 0xE713);
        assert_eq!(action_icon_codepoint(ActionIconKind::Diagnostics), 0xE8A5);
        assert_eq!(action_icon_codepoint(ActionIconKind::Logs), 0xE8B7);
        assert_eq!(action_icon_codepoint(ActionIconKind::Rebuild), 0xE895);
        assert_eq!(action_icon_codepoint(ActionIconKind::Generic), 0xE756);
    }

    #[test]
    fn configure_runtime_performance_tuning_clamps_values() {
        configure_runtime_performance_tuning(50, 10);
        assert_eq!(runtime_icon_cache_idle_ms(), 250); // clamped to min
        configure_runtime_performance_tuning(200_000, 10);
        assert_eq!(runtime_icon_cache_idle_ms(), 120_000); // clamped to max
    }
}
