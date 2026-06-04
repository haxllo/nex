//! `NativeOverlayShell` — the imperative overlay API the runtime uses.
//!
//! Internally the shim owns an `Arc<Mutex<Model>>` plus an
//! `Arc<IconCache>`. All the public setters apply their change
//! directly to the model under the mutex. The Iced event loop is
//! driven by [`boot::run`], which the shim spawns from
//! `run_message_loop_with_events`.
//!
//! This is the *only* public surface of the new `overlay` module.
//! Runtime code outside `runtime_loop.rs` and friends should not need
//! to touch any of the lower-level `iced::` types.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::unbounded;

use crate::overlay::boot::Boot;
use crate::overlay::icons::IconCache;
use crate::overlay::model::{update, Message, Model, OverlayEvent, OverlayRow};
use crate::overlay::platform;

/// A safe-to-clone handle to the Iced overlay. Cheap to clone
/// (`Arc` inside) so callers like `runtime_loop.rs` can hold one
/// across the lifetime of the runtime.
#[derive(Clone)]
pub struct NativeOverlayShell {
    inner: Arc<Inner>,
}

struct Inner {
    model: Arc<Mutex<Model>>,
    icon_cache: Arc<IconCache>,
    is_running: Arc<AtomicBool>,
}

impl NativeOverlayShell {
    /// Construct the shell. Does not create the Iced window or start
    /// the event loop; call [`run_message_loop_with_events`] to do
    /// that.
    pub fn create() -> Result<Self, String> {
        let model = Arc::new(Mutex::new(Model {
            theme: platform::detect_system_theme(),
            ..Model::default()
        }));
        let icon_cache = Arc::new(IconCache::default());
        Ok(Self {
            inner: Arc::new(Inner {
                model,
                icon_cache,
                is_running: Arc::new(AtomicBool::new(false)),
            }),
        })
    }

    /// Apply a message to the model.
    fn apply(&self, message: Message) {
        if let Ok(mut model) = self.inner.model.lock() {
            update(&mut model, message);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.inner
            .model
            .lock()
            .map(|m| m.visible)
            .unwrap_or(false)
    }

    pub fn has_focus(&self) -> bool {
        self.inner
            .model
            .lock()
            .map(|m| m.has_focus)
            .unwrap_or(false)
    }

    /// Placeholder HWND accessor. The Iced shell does not own a
    /// native `HWND` for the SearchWorker to post messages to —
    /// the runtime polls [`SearchWorker::try_recv`] and calls
    /// [`NativeOverlayShell::set_results`] directly. This method
    /// exists only so the legacy `SearchWorker::new(_, _, _, hwnd,
    /// msg)` call signature can stay identical between the legacy
    /// and Iced paths. Returns `0` (a sentinel `HWND` that
    /// `PostMessageW` will safely fail on).
    pub fn hwnd(&self) -> isize {
        0
    }

    pub fn show_and_focus(&self) {
        self.apply(Message::Show);
    }

    pub fn focus_input_and_select_all(&self) {
        self.apply(Message::FocusInputAndSelectAll);
    }

    pub fn hide(&self) {
        self.apply(Message::Hide);
    }

    pub fn hide_now(&self) {
        self.apply(Message::HideNow);
    }

    pub fn query_text(&self) -> String {
        self.inner
            .model
            .lock()
            .map(|m| m.query.clone())
            .unwrap_or_default()
    }

    pub fn set_query_text(&self, query: &str) {
        self.apply(Message::SetQueryText(query.to_string()));
    }

    pub fn set_status_text(&self, message: &str) {
        self.apply(Message::SetStatusText(message.to_string()));
    }

    pub fn set_hotkey_hint(&self, hotkey: &str) {
        self.apply(Message::SetHotkeyHint(hotkey.to_string()));
    }

    pub fn set_performance_tuning(
        &self,
        idle_cache_trim_ms: u32,
        active_memory_target_mb: u16,
    ) {
        self.apply(Message::SetPerformanceTuning {
            idle_cache_trim_ms,
            active_memory_target_mb,
        });
        self.inner.icon_cache.clear();
    }

    pub fn set_game_mode_enabled(&self, enabled: bool) {
        self.apply(Message::SetGameModeEnabled(enabled));
    }

    pub fn set_hotkey_issue_active(&self, active: bool) {
        self.apply(Message::SetHotkeyIssueActive(active));
    }

    pub fn trim_runtime_memory(&self) {
        let _ = self.inner.icon_cache.trim_unused();
    }

    pub fn set_mode_strip_text(&self, text: &str) {
        self.apply(Message::SetModeStripText(text.to_string()));
    }

    pub fn set_help_config_path(&self, path: &str) {
        self.apply(Message::SetHelpConfigPath(path.to_string()));
    }

    pub fn show_placeholder_hint(&self, message: &str) {
        self.apply(Message::ShowPlaceholderHint(message.to_string()));
    }

    pub fn clear_placeholder_hint(&self) {
        self.apply(Message::ClearPlaceholderHint);
    }

    pub fn clear_query_text(&self) {
        self.apply(Message::SetQueryText(String::new()));
    }

    pub fn set_results(&self, rows: &[OverlayRow], selected_index: usize) {
        self.apply(Message::SetResults {
            rows: rows.to_vec(),
            selected_index,
        });
        let cache = self.inner.icon_cache.clone();
        let rows = rows.to_vec();
        std::thread::spawn(move || {
            crate::overlay::icons::prefetch_rows(&cache, &rows);
        });
    }

    pub fn set_selected_index(&self, selected_index: usize) {
        self.apply(Message::SetSelectedIndex(selected_index));
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.inner
            .model
            .lock()
            .ok()
            .and_then(|m| if m.rows.is_empty() { None } else { Some(m.selected) })
    }

    /// Start the Iced event loop. Blocks until the window is closed.
    ///
    /// `on_event` is invoked on the **calling thread** for every
    /// legacy `OverlayEvent` produced by user input — not on the
    /// Iced worker thread. This matches the legacy Win32 semantics
    /// (the callback was driven by the `GetMessageW` loop on the
    /// runtime thread) and lets callers borrow local state without
    /// wrapping it in `Arc<Mutex<>>`.
    ///
    /// The Iced event loop runs on a dedicated worker thread and
    /// posts events over a `crossbeam_channel`; the calling thread
    /// drains the channel and calls `on_event` synchronously.
    ///
    /// Takes `&self` (not `self`) so callers can continue to use
    /// the shell after the loop exits — matches the legacy
    /// `windows_overlay::NativeOverlayShell::run_message_loop` which
    /// also took `&self`.
    pub fn run_message_loop_with_events<F>(&self, mut on_event: F) -> Result<(), String>
    where
        F: FnMut(OverlayEvent),
    {
        let inner = self.inner.clone();
        inner.is_running.store(true, Ordering::SeqCst);

        let (event_tx, event_rx) = unbounded::<OverlayEvent>();
        let model_for_iced = inner.model.clone();
        let is_running = inner.is_running.clone();

        std::thread::Builder::new()
            .name("nex-overlay-iced".into())
            .spawn(move || {
                let result = crate::overlay::boot::run(Boot {
                    model: model_for_iced,
                    event_tx,
                });
                is_running.store(false, Ordering::SeqCst);
                if let Err(e) = result {
                    crate::runtime::log_warn(&format!("[nex] overlay iced loop exited: {e}"));
                }
            })
            .map_err(|e| format!("failed to spawn Iced thread: {e}"))?;

        while inner.is_running.load(Ordering::SeqCst) {
            match event_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(event) => on_event(event),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
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
