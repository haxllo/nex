//! Background icon loader thread.
//!
//! Moves `SHGetFileInfoW` / `ExtractIconExW` calls off the UI thread by
//! processing [`IconLoadRequest`] items on a dedicated COM-initialized thread
//! and feeding [`IconLoadResult`] items back through an `mpsc` channel.

use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use crate::windows_overlay::icon_cache::load_shell_icon_for_values;
use crate::windows_overlay::state::{IconLoadRequest, IconLoadResult};

/// Spawn the background icon loader thread.
///
/// Returns the join handle (owner drops when overlay state is cleaned up) and
/// the two channel endpoints: the sender is used by the UI thread to enqueue
/// work, the receiver is polled periodically (via `WM_TIMER`) for completed
/// results.
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
                let handle =
                    load_shell_icon_for_values(&request.kind, &request.icon_path).unwrap_or(0);
                let _ = result_sender.send(IconLoadResult {
                    key: request.key,
                    handle,
                });
            }

            unsafe {
                CoUninitialize();
            }
        })
        .expect("failed to spawn icon loader thread");

    (handle, request_sender, result_receiver)
}
