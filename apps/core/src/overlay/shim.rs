//! `NativeOverlayShell` — the imperative overlay API the runtime uses.
//!
//! Internally the shim owns an `Arc<Mutex<Model>>` plus an
//! `Arc<IconCache>`. All the public setters apply their change
//! directly to the model under the mutex. In Phase 4 the shim will
//! also own the Iced event-loop thread and post every change to it
//! over a channel; until then the in-place mutation keeps the
//! runtime tests green and the legacy API working.
//!
//! This is the *only* public surface of the new `overlay` module.
//! Runtime code outside `runtime_loop.rs` and friends should not need
//! to touch any of the lower-level `iced::` types.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    /// `on_event` is invoked on the **Iced thread** (not the caller
    /// thread) for every legacy `OverlayEvent` produced by user
    /// input. The caller is expected to marshal those events into
    /// the runtime's main loop — historically that loop was driven
    /// by `GetMessageW` and the callback was synchronous.
    ///
    /// In Phase 1 this is a thin shim that drives the runtime's
    /// callback with a periodic heartbeat and lets the calling
    /// thread's setters keep working. Phase 4 replaces the body with
    /// a real `iced::application(...).run()` call.
    pub fn run_message_loop_with_events<F>(self, mut on_event: F) -> Result<(), String>
    where
        F: FnMut(OverlayEvent) + Send + 'static,
    {
        self.inner.is_running.store(true, Ordering::SeqCst);
        let is_running = self.inner.is_running.clone();
        std::thread::Builder::new()
            .name("nex-overlay-pump".into())
            .spawn(move || {
                while is_running.load(Ordering::SeqCst) {
                    // Phase 4: read from a real `Receiver<Message>`
                    // and apply each through `update()`.
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                let _ = on_event;
            })
            .map_err(|e| format!("failed to spawn pump thread: {e}"))?;
        Ok(())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}
