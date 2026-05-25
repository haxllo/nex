//! Background icon loader thread.
//!
//! Moves `SHGetFileInfoW` / `ExtractIconExW` calls off the UI thread by
//! processing [`IconLoadRequest`] items on a dedicated COM-initialized thread
//! and feeding [`IconLoadResult`] items back through an `mpsc` channel.
//! After each result is sent, the thread posts `NEX_WM_ICON_LOADED` to the
//! overlay window so the UI thread wakes immediately instead of polling.

use std::panic;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::windows_overlay::icon_cache::load_shell_icon_for_values;
use crate::windows_overlay::state::{IconLoadRequest, IconLoadResult};
use crate::windows_overlay::types::NEX_WM_ICON_LOADED;

/// Spawn the background icon loader thread.
///
/// Returns the join handle (owner drops when overlay state is cleaned up) and
/// the two channel endpoints: the sender is used by the UI thread to enqueue
/// work, the receiver receives completed results. The background thread
/// notifies the UI thread via `PostMessageW` so no polling timer is needed.
pub(crate) fn spawn_icon_loader_thread() -> (
    JoinHandle<()>,
    mpsc::Sender<IconLoadRequest>,
    mpsc::Receiver<IconLoadResult>,
) {
    let (request_sender, request_receiver) = mpsc::channel::<IconLoadRequest>();
    let (result_sender, result_receiver) = mpsc::channel::<IconLoadResult>();

    let handle = thread::Builder::new()
        .name("nex-icon-loader".into())
        .spawn(move || {
            unsafe {
                CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as u32);
            }

            for request in request_receiver {
                let hwnd = request.hwnd as HWND;
                let handle = match panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    load_shell_icon_for_values(&request.kind, &request.icon_path)
                })) {
                    Ok(Some(h)) => h,
                    Ok(None) => 0,
                    Err(e) => {
                        let msg = if let Some(s) = e.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = e.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        crate::logging::error(&format!(
                            "[nex] icon_loader panic for kind={} path={}: {}",
                            request.kind, request.icon_path, msg
                        ));
                        0
                    }
                };
                let _ = result_sender.send(IconLoadResult {
                    key: request.key,
                    handle,
                });
                if !hwnd.is_null() {
                    unsafe {
                        PostMessageW(hwnd, NEX_WM_ICON_LOADED, 0, 0);
                    }
                }
            }

            unsafe {
                CoUninitialize();
            }
        })
        .expect("failed to spawn icon loader thread");

    (handle, request_sender, result_receiver)
}
