//! Iced 0.14 application boot. The Iced runtime holds a [`State`]
//! that owns the overlay [`Model`]. The runtime's `view` function
//! produces `Element<'a, Message>` that borrows from the state —
//! the Iced 0.14 `ViewFn` trait requires `'a` on the returned
//! `Element` to match the `&'a State` borrow, so the model must be
//! a direct field of the state.
//!
//! The runtime thread (the worker spawned by `runtime_loop.rs`)
//! cannot reach into the Iced state directly. Instead the shim
//! holds an `Arc<Mutex<Model>>` that the Iced `apply` function
//! mirrors into the state's owned model. A polling `Subscription`
//! fires a `Message::SyncFromShim` at ~30 Hz so user input latency
//! stays under one frame.
//!
//! This module's [`run`] function **must be called on the main
//! thread**. winit 0.30 panics if `EventLoop::new` runs on a
//! non-main thread; Iced 0.14's `iced::application().run()` calls
//! `EventLoop::with_user_event().build()` internally with no way
//! to inject a pre-configured `EventLoopBuilder` carrying
//! `with_any_thread(true)`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::Sender;
use iced::Element;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE};

use crate::overlay::geometry::{
    DIVIDER_HEIGHT, FOOTER_HINT_HEIGHT, FOOTER_SEPARATOR_HEIGHT, INPUT_HEIGHT, MAX_VISIBLE_ROWS,
    PANEL_MARGIN_BOTTOM, ROW_HEIGHT, WINDOW_WIDTH,
};
use crate::overlay::icons::IconCache;
use crate::overlay::model::{message_to_event, update, Message, Model, OverlayEvent};
use crate::overlay::view::view as build_view;

/// Build a one-shot task that asks the Iced runtime for the oldest
/// window, then calls Win32 `ShowWindow` with `SW_SHOWNOACTIVATE`
/// (visible) or `SW_HIDE` (hidden). We can't keep a stable
/// `iced::window::Id` because Iced generates one internally and the
/// public API doesn't expose it; `oldest()` always returns the main
/// launcher window since we only ever open one.
fn visibility_task(visible: bool) -> iced::Task<Message> {
    iced::window::oldest().then(move |id_opt| match id_opt {
        Some(id) => iced::window::run(id, move |handle| set_visible(handle, visible)).discard(),
        None => iced::Task::none(),
    })
}

fn set_visible(handle: &dyn iced::window::Window, visible: bool) {
    let Ok(window_handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = window_handle.as_raw() else {
        return;
    };
    let hwnd: HWND = win32.hwnd.get() as *mut core::ffi::c_void;
    let cmd = if visible { SW_SHOWNOACTIVATE } else { SW_HIDE };
    unsafe {
        ShowWindow(hwnd, cmd);
    }
}

/// Total launcher height: input + divider + `MAX_VISIBLE_ROWS` rows
/// + footer (separator + hint) + bottom margin. The single source of
/// truth is [`crate::overlay::geometry`].
fn panel_total_height() -> f32 {
    let rows = (MAX_VISIBLE_ROWS as f32) * ROW_HEIGHT;
    let footer = FOOTER_SEPARATOR_HEIGHT + FOOTER_HINT_HEIGHT;
    INPUT_HEIGHT + DIVIDER_HEIGHT + rows + footer + PANEL_MARGIN_BOTTOM
}

/// Bundle passed into the Iced boot function. The runtime creates
/// the shared `Arc<Mutex<Model>>` and the `event_tx` channel
/// endpoint before calling [`run`], so the Iced event loop and the
/// runtime worker thread share them.
pub(crate) struct Boot {
    pub(crate) model: Arc<Mutex<Model>>,
    pub(crate) icon_cache: Arc<IconCache>,
    pub(crate) event_tx: Sender<OverlayEvent>,
    /// Set to `true` by the runtime before starting the Iced event
    /// loop. Set to `false` by [`run`] when the Iced event loop
    /// exits (so the runtime worker thread knows to stop draining
    /// the event channel).
    pub(crate) is_running: Arc<AtomicBool>,
}

/// The Iced application state. Owns the [`Model`] directly so the
/// `view` function can return an `Element` borrowing from
/// `&self.model` with the right lifetime.
pub(crate) struct State {
    pub(crate) model: Model,
    pub(crate) shared: Arc<Mutex<Model>>,
    pub(crate) icon_cache: Arc<IconCache>,
    pub(crate) event_tx: Sender<OverlayEvent>,
    /// Mirrors the last-synced value of `model.visible` so we can
    /// fire a one-shot Win32 `ShowWindow` task when the runtime
    /// toggles visibility (the polling subscription only knows the
    /// transition by comparing last vs. current).
    pub(crate) last_visible: bool,
}

impl State {
    pub(crate) fn boot(
        initial: Model,
        shared: Arc<Mutex<Model>>,
        icon_cache: Arc<IconCache>,
        event_tx: Sender<OverlayEvent>,
    ) -> (Self, iced::Task<Message>) {
        let last_visible = initial.visible;
        (
            Self {
                model: initial,
                shared,
                icon_cache,
                event_tx,
                last_visible,
            },
            iced::Task::none(),
        )
    }

    pub(crate) fn apply(&mut self, message: Message) -> iced::Task<Message> {
        if matches!(message, Message::SyncFromShim) {
            if let Ok(g) = self.shared.lock() {
                self.model = g.clone();
            }
            // Detect the visibility edge and fire a one-shot Win32
            // `ShowWindow` task. Iced 0.14 has no public show/hide
            // window action, so we use `iced::window::run` to get the
            // raw window handle and call `ShowWindow` ourselves.
            let now_visible = self.model.visible;
            if now_visible != self.last_visible {
                self.last_visible = now_visible;
                crate::logging::info(&format!(
                    "[nex] visibility transition: {} -> {} (firing ShowWindow)",
                    !now_visible, now_visible
                ));
                return visibility_task(now_visible);
            }
            return iced::Task::none();
        }

        let task = update(&mut self.model, message.clone());
        if let Ok(mut g) = self.shared.lock() {
            *g = self.model.clone();
        }
        if let Some(event) = message_to_event(&self.model, &message) {
            let _ = self.event_tx.send(event);
        }
        task
    }
}

/// Manual `ViewFn` impl. The blanket impl for closures cannot
/// express the `for<'a>` HRTB we need — the closure return type
/// loses the input lifetime. As a struct method we keep full
/// control.
pub(crate) struct View;

impl<'a> iced::application::ViewFn<'a, State, Message, iced::Theme, iced::Renderer>
    for View
{
    fn view(
        &self,
        state: &'a State,
    ) -> Element<'a, Message, iced::Theme, iced::Renderer> {
        build_view(&state.model, &state.icon_cache)
    }
}

pub(crate) fn run(boot: Boot) -> Result<(), String> {
    let initial = boot
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default();
    let shared = boot.model.clone();
    let icon_cache = boot.icon_cache.clone();
    let event_tx = boot.event_tx.clone();
    let is_running = boot.is_running.clone();

    let window_settings = iced::window::Settings {
        size: iced::Size::new(WINDOW_WIDTH, panel_total_height()),
        resizable: false,
        decorations: false,
        transparent: true,
        level: iced::window::Level::AlwaysOnTop,
        position: iced::window::Position::Centered,
        visible: false,
        exit_on_close_request: true,
        ..iced::window::Settings::default()
    };

    let settings = iced::Settings {
        antialiasing: true,
        ..iced::Settings::default()
    };

    let result = iced::application(
        move || State::boot(initial.clone(), shared.clone(), icon_cache.clone(), event_tx.clone()),
        State::apply,
        View,
    )
    .subscription(|_state: &State| {
        iced::time::every(Duration::from_millis(33)).map(|_| Message::SyncFromShim)
    })
    .settings(settings)
    .window(window_settings)
    .run();

    // Signal the worker thread to stop draining the event channel.
    is_running.store(false, Ordering::SeqCst);

    result.map_err(|e| format!("iced application failed: {e}"))
}
