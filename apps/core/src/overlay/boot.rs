//! Iced 0.14 application boot. The Iced runtime holds a [`State`]
//! that owns the overlay [`Model`]. The runtime's `view` function
//! produces `Element<'a, Message>` that borrows from the state —
//! the Iced 0.14 `ViewFn` trait requires `'a` on the returned
//! `Element` to match the `&'a State` borrow, so the model must be
//! a direct field of the state.
//!
//! The runtime thread (the `shim` and friends) cannot reach into
//! the Iced state directly. Instead the shim holds an
//! `Arc<Mutex<Model>>` that the Iced `apply` function mirrors into
//! the state's owned model. A polling `Subscription` fires a
//! `Message::SyncFromShim` at ~30 Hz so user input latency stays
//! under one frame.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::Sender;
use iced::Element;

use crate::overlay::model::{message_to_event, update, Message, Model, OverlayEvent};
use crate::overlay::view::view as build_view;

/// Bundle passed into the Iced boot function.
pub(crate) struct Boot {
    pub(crate) model: Arc<Mutex<Model>>,
    pub(crate) event_tx: Sender<OverlayEvent>,
}

/// The Iced application state. Owns the [`Model`] directly so the
/// `view` function can return an `Element` borrowing from
/// `&self.model` with the right lifetime.
pub(crate) struct State {
    pub(crate) model: Model,
    pub(crate) shared: Arc<Mutex<Model>>,
    pub(crate) event_tx: Sender<OverlayEvent>,
}

impl State {
    pub(crate) fn boot(
        initial: Model,
        shared: Arc<Mutex<Model>>,
        event_tx: Sender<OverlayEvent>,
    ) -> (Self, iced::Task<Message>) {
        (
            Self {
                model: initial,
                shared,
                event_tx,
            },
            iced::Task::none(),
        )
    }

    pub(crate) fn apply(&mut self, message: Message) -> iced::Task<Message> {
        if matches!(message, Message::SyncFromShim) {
            if let Ok(g) = self.shared.lock() {
                self.model = g.clone();
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
        build_view(&state.model)
    }
}

pub(crate) fn run(boot: Boot) -> Result<(), String> {
    let initial = boot
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default();
    let shared = boot.model.clone();
    let event_tx = boot.event_tx.clone();

    let window_settings = iced::window::Settings {
        size: iced::Size::new(640.0, 480.0),
        resizable: false,
        decorations: false,
        transparent: true,
        level: iced::window::Level::AlwaysOnTop,
        position: iced::window::Position::Centered,
        visible: true,
        exit_on_close_request: true,
        ..iced::window::Settings::default()
    };

    let settings = iced::Settings {
        antialiasing: true,
        ..iced::Settings::default()
    };

    iced::application(
        move || State::boot(initial.clone(), shared.clone(), event_tx.clone()),
        State::apply,
        View,
    )
    .subscription(|_state: &State| {
        iced::time::every(Duration::from_millis(33)).map(|_| Message::SyncFromShim)
    })
    .settings(settings)
    .window(window_settings)
    .run()
    .map_err(|e| format!("iced application failed: {e}"))
}
