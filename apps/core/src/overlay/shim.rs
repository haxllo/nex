//! `NativeOverlayShell` — the imperative overlay API the runtime uses.
//!
//! Internally the shim owns an `Arc<Mutex<ShimState>>` (the shared,
//! framework-agnostic snapshot of the overlay), an `Arc<IconCache>`,
//! and a slot holding the WebView host's `EventLoopProxy<UiCommand>`.
//! Every public setter mutates the shared state and, when the change
//! is user-visible, posts a [`UiCommand`] to the host so the WebView
//! re-renders. The host event loop runs on the **main thread** (via
//! [`crate::overlay::host::run`]); the runtime's message pump runs on a
//! **worker thread** and drains a `crossbeam_channel` that the WebView
//! IPC handler, the hotkey listener, and the tray all write to.
//!
//! This is the *only* public surface of the new `overlay` module.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use tao::event_loop::EventLoopProxy;

use crate::overlay::host::UiCommand;
use crate::overlay::icons::IconCache;
use crate::overlay::model::{OverlayEvent, OverlayRow, ShimState};

/// A safe-to-clone handle to the WebView overlay. Cheap to clone
/// (`Arc` inside) so callers like `runtime_loop.rs` can hold one
/// across the lifetime of the runtime.
#[derive(Clone)]
pub struct NativeOverlayShell {
    inner: Arc<Inner>,
}

struct Inner {
    state: Arc<Mutex<ShimState>>,
    icon_cache: Arc<IconCache>,
    is_running: Arc<AtomicBool>,
    /// Filled in by [`crate::overlay::host::run`] once the event loop
    /// is built. `None` until then — setters degrade to state-only
    /// updates (the host pushes the full snapshot on `WebviewReady`).
    proxy: Arc<Mutex<Option<EventLoopProxy<UiCommand>>>>,
    /// Send a stop signal to the message pump. When the host event loop
    /// exits, the runtime calls `stop()` so the worker thread unblocks
    /// immediately instead of waiting for the next recv_timeout tick.
    stop_tx: Sender<()>,
    stop_rx: Receiver<()>,
}

impl NativeOverlayShell {
    /// Construct the shell. Does not create the window or WebView; the
    /// runtime calls [`crate::overlay::host::run`] on the main thread
    /// for that, and [`run_message_pump`] on a worker thread to drive
    /// the runtime callback.
    pub fn create() -> Result<Self, String> {
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);
        Ok(Self {
            inner: Arc::new(Inner {
                state: Arc::new(Mutex::new(ShimState::default())),
                icon_cache: Arc::new(IconCache::default()),
                is_running: Arc::new(AtomicBool::new(false)),
                proxy: Arc::new(Mutex::new(None)),
                stop_tx,
                stop_rx,
            }),
        })
    }

    /// Post a command to the WebView host, if the event loop is up.
    fn post(&self, cmd: UiCommand) {
        if let Ok(slot) = self.inner.proxy.lock() {
            if let Some(proxy) = slot.as_ref() {
                let _ = proxy.send_event(cmd);
            }
        }
    }

    /// Mutate the shared state under the lock.
    fn with_state<F: FnOnce(&mut ShimState)>(&self, f: F) {
        if let Ok(mut s) = self.inner.state.lock() {
            f(&mut s);
        }
    }

    /// The shared overlay state. The host reads this to build the JSON
    /// snapshot it pushes to the WebView; the IPC handler writes the
    /// live `query`/`selected` values back here.
    pub fn shared_state(&self) -> Arc<Mutex<ShimState>> {
        self.inner.state.clone()
    }

    /// The slot the host fills with its event-loop proxy. Cloned into
    /// the `Boot`/`Host` bundle before the event loop starts.
    pub(crate) fn proxy_slot(&self) -> Arc<Mutex<Option<EventLoopProxy<UiCommand>>>> {
        self.inner.proxy.clone()
    }

    /// Shared `is_running` flag. Set to `true` by the runtime before
    /// starting the host; set to `false` by the host when its event
    /// loop exits; checked by the worker's [`run_message_pump`] loop.
    /// Signal the message pump to stop. The worker thread unblocks
    /// from its `recv` call immediately rather than waiting up to
    /// 50 ms for the next `is_running` poll.
    pub fn stop(&self) {
        let _ = self.inner.stop_tx.send(());
    }

    pub fn is_running(&self) -> Arc<AtomicBool> {
        self.inner.is_running.clone()
    }

    /// The shared icon cache. The WebView's `nexasset://` protocol
    /// resolves result-row icons through it.
    pub fn icon_cache(&self) -> Arc<IconCache> {
        self.inner.icon_cache.clone()
    }

    pub fn is_visible(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|s| s.visible)
            .unwrap_or(false)
    }

    pub fn has_focus(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|s| s.has_focus)
            .unwrap_or(false)
    }

    /// Placeholder HWND accessor. The WebView shell does not expose a
    /// native `HWND` for the SearchWorker to post messages to — the
    /// runtime polls [`SearchWorker::try_recv`] and calls
    /// [`NativeOverlayShell::set_results`] directly. Kept only so the
    /// `SearchWorker::new(_, _, _, hwnd, msg)` signature is unchanged.
    pub fn hwnd(&self) -> isize {
        0
    }

    pub fn show_and_focus(&self) {
        self.with_state(|s| {
            s.visible = true;
            // Don't claim has_focus here — the window hasn't been shown
            // yet (it's queued via UiCommand::Show and handled async on
            // the main thread).  If we set has_focus=true now, a rapid
            // second hotkey press (before WebView rebuild completes or
            // the page paints) will see "visible + focused" and toggle
            // the overlay closed before the user ever saw it.
            // WindowEvent::Focused(true) sets has_focus after the window
            // is actually shown and gains focus.
        });
        self.post(UiCommand::Show);
    }

    pub fn focus_input_and_select_all(&self) {
        self.with_state(|s| s.has_focus = true);
        self.post(UiCommand::Show);
    }

    pub fn hide(&self) {
        self.with_state(|s| {
            s.visible = false;
            s.has_focus = false;
            s.query.clear();
            s.rows.clear();
            s.selected = 0;
        });
        self.post(UiCommand::Hide);
    }

    pub fn hide_now(&self) {
        self.with_state(|s| {
            s.visible = false;
            s.has_focus = false;
        });
        self.post(UiCommand::Hide);
    }

    pub fn query_text(&self) -> String {
        self.inner
            .state
            .lock()
            .map(|s| s.query.clone())
            .unwrap_or_default()
    }

    pub fn set_query_text(&self, query: &str) {
        self.with_state(|s| s.query = query.to_string());
        self.post(UiCommand::Apply);
    }

    pub fn set_status_text(&self, message: &str) {
        self.with_state(|s| s.status_text = message.to_string());
        self.post(UiCommand::Apply);
    }

    pub fn set_hotkey_hint(&self, hotkey: &str) {
        self.with_state(|s| s.hotkey_hint = hotkey.to_string());
        self.post(UiCommand::Apply);
    }

    pub fn set_performance_tuning(
        &self,
        idle_cache_trim_ms: u32,
        active_memory_target_mb: u16,
        ui_warm_release_ms: u32,
    ) {
        self.with_state(|s| {
            s.idle_cache_trim_ms = idle_cache_trim_ms;
            s.active_memory_target_mb = active_memory_target_mb;
            s.ui_warm_release_ms = ui_warm_release_ms;
        });
        let max_entries =
            IconCache::icon_cache_capacity_from_memory_target(active_memory_target_mb);
        self.inner
            .icon_cache
            .reconfigure(max_entries, idle_cache_trim_ms);
    }

    pub fn set_game_mode_enabled(&self, enabled: bool) {
        self.with_state(|s| s.game_mode_enabled = enabled);
    }

    pub fn set_hotkey_issue_active(&self, active: bool) {
        self.with_state(|s| s.hotkey_issue_active = active);
        self.post(UiCommand::Apply);
    }

    pub fn trim_runtime_memory(&self) {
        let _ = self.inner.icon_cache.trim_unused();
    }

    pub fn set_help_config_path(&self, path: &str) {
        self.with_state(|s| s.help_config_path = path.to_string());
    }

    pub fn show_placeholder_hint(&self, message: &str) {
        self.with_state(|s| s.placeholder_hint = Some(message.to_string()));
        self.post(UiCommand::Apply);
    }

    pub fn clear_placeholder_hint(&self) {
        self.with_state(|s| s.placeholder_hint = None);
        self.post(UiCommand::Apply);
    }

    pub fn clear_query_text(&self) {
        self.with_state(|s| s.query.clear());
        self.post(UiCommand::Apply);
    }

    /// Signal the host event loop to exit cleanly (tray Quit).
    pub fn quit_if_running(&self) {
        self.post(UiCommand::Quit);
    }

    pub fn set_results(&self, rows: &[OverlayRow], selected_index: usize) {
        self.with_state(|s| {
            s.rows = rows.to_vec();
            s.selected = selected_index.min(s.rows.len().saturating_sub(1));
            s.placeholder_hint = None;
        });
        self.post(UiCommand::Apply);

        // Decode first 8 icons synchronously so they appear on this
        // render. Defer the rest to a background thread — icons beyond
        // the first visible batch appear on the next search.
        let cache = self.inner.icon_cache.clone();
        let fast_count = 8.min(rows.len());
        if fast_count > 0 {
            crate::overlay::icons::prefetch_rows(&cache, &rows[..fast_count]);
        }
        let slow_rows: Vec<OverlayRow> = rows.iter().skip(fast_count).cloned().collect();
        if !slow_rows.is_empty() {
            std::thread::Builder::new()
                .name("nex-icon-prefetch".into())
                .spawn(move || {
                    crate::overlay::icons::prefetch_rows(&cache, &slow_rows);
                })
                .ok();
        }
    }

    pub fn set_selected_index(&self, selected_index: usize) {
        self.with_state(|s| {
            s.selected = selected_index.min(s.rows.len().saturating_sub(1));
        });
        self.post(UiCommand::Apply);
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.inner
            .state
            .lock()
            .ok()
            .and_then(|s| if s.rows.is_empty() { None } else { Some(s.selected) })
    }

    /// Drain `event_rx` and call `on_event` for each event. Loops
    /// while `is_running` is `true`; the host flips it to `false` when
    /// the event loop exits.
    ///
    /// Designed to be called from a **worker thread** so the host's
    /// event loop on the main thread is unblocked.
    pub fn run_message_pump<F>(
        &self,
        event_rx: &Receiver<OverlayEvent>,
        is_running: &Arc<AtomicBool>,
        mut on_event: F,
    ) -> Result<(), String>
    where
        F: FnMut(OverlayEvent),
    {
        let stop_rx = self.inner.stop_rx.clone();
        // Periodic tick so the loop rechecks `is_running` even when
        // `stop_rx` fails to wake from `select!` (observed on shutdown
        // after overlay stop + hotkey listener drop).
        let tick = crossbeam_channel::tick(Duration::from_millis(250));
        while is_running.load(Ordering::SeqCst) {
            crossbeam_channel::select! {
                recv(event_rx) -> event => {
                    match event {
                        Ok(ev) => on_event(ev),
                        Err(_) => break,
                    }
                }
                recv(stop_rx) -> _ => break,
                recv(tick) -> _ => {},
            }
        }
        Ok(())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}
