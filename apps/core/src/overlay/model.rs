//! The Elm-style `Model`, `Message`, and `update()` for the Iced
//! overlay. The model mirrors the legacy `OverlayShellState` plus
//! every public setter the runtime calls on `NativeOverlayShell`.
//!
//! `update()` is pure: it mutates the model and returns Iced `Task`s
//! for side effects (search, launch, focus). The legacy
//! `NativeOverlayShell::run_message_loop_with_events` callback fires
//! when an "interesting" `Message` (Hotkey, Submit, Escape, …) is
//! processed — see [`crate::overlay::shim`] for the translation.

use std::time::Instant;

use crate::overlay::theme::Theme;

/// One row in the visible result list. Mirrors the legacy
/// `OverlayRow` so `runtime_overlay_rows` can build it without
/// changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayRow {
    pub role: OverlayRowRole,
    pub result_index: Option<usize>,
    pub kind: String,
    pub title: String,
    pub path: String,
    pub icon_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayRowRole {
    Item,
    Header,
    TopHit,
    Status,
    Calculator,
}

/// Events the Iced runtime can deliver to the legacy `runtime_loop`
/// callback. The shape is the same as the legacy `OverlayEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayEvent {
    Hotkey(i32),
    QueryChanged(String),
    MoveSelection(i32),
    Submit,
    Escape,
    ExternalShow,
    ExternalQuit,
    TrayToggleGameMode,
    TrayCheckForUpdates,
    SearchResultsReady,
}

/// The complete overlay state. Every field the legacy
/// `NativeOverlayShell` setter touched lives here, so the shim
/// (`crate::overlay::shim`) can update fields and post `Message`s
/// without locking more than `Arc<Mutex<Model>>`.
#[derive(Debug)]
pub struct Model {
    pub query: String,
    pub status_text: String,
    pub hotkey_hint: String,
    pub mode_strip_text: String,
    pub help_config_path: String,
    pub placeholder_hint: Option<String>,
    pub hotkey_issue_active: bool,
    pub game_mode_enabled: bool,
    pub idle_cache_trim_ms: u32,
    pub active_memory_target_mb: u16,
    pub visible: bool,
    pub has_focus: bool,
    pub theme: Theme,
    pub rows: Vec<OverlayRow>,
    pub selected: usize,
    pub last_query_change: Option<Instant>,
    pub results_fade_started: Option<Instant>,
    pub loading: bool,
    pub loading_frame: usize,
    pub search_generation: u64,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            query: String::new(),
            status_text: String::new(),
            hotkey_hint: "Ctrl+Space".into(),
            mode_strip_text: "All   Apps   Files   Actions   Clipboard".into(),
            help_config_path: String::new(),
            placeholder_hint: None,
            hotkey_issue_active: false,
            game_mode_enabled: false,
            idle_cache_trim_ms: 90_000,
            active_memory_target_mb: 72,
            visible: false,
            has_focus: false,
            theme: Theme::Dark,
            rows: Vec::new(),
            selected: 0,
            last_query_change: None,
            results_fade_started: None,
            loading: false,
            loading_frame: 0,
            search_generation: 0,
        }
    }
}

/// Every mutation the legacy shim can post, plus Iced's own events.
#[derive(Debug, Clone)]
pub enum Message {
    // Imperative setters forwarded from `NativeOverlayShell`:
    SetQueryText(String),
    SetStatusText(String),
    SetHotkeyHint(String),
    SetModeStripText(String),
    SetHelpConfigPath(String),
    ShowPlaceholderHint(String),
    ClearPlaceholderHint,
    SetHotkeyIssueActive(bool),
    SetGameModeEnabled(bool),
    SetPerformanceTuning {
        idle_cache_trim_ms: u32,
        active_memory_target_mb: u16,
    },
    SetResults {
        rows: Vec<OverlayRow>,
        selected_index: usize,
    },
    SetSelectedIndex(usize),
    Show,
    Hide,
    HideNow,
    FocusInputAndSelectAll,

    // Events produced by the Iced view + winit window:
    QueryInputChanged(String),
    MoveSelection(i32),
    SubmitRequested,
    EscapePressed,
    HotkeyTriggered(i32),
    TrayMenu(TrayMenuAction),
    ExternalShowRequested,
    ExternalQuitRequested,
    SearchResultsReady,
    LoadingTick,
    ResultsFadeTick,
    ThemeDetected(Theme),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMenuAction {
    Show,
    OpenConfig,
    CheckForUpdates,
    ToggleGameMode,
    Quit,
}

/// Update the model in response to a message. Side effects (search
/// worker dispatch, launching selections, focus changes) are returned
/// as Iced `Task`s.
pub fn update(model: &mut Model, message: Message) -> iced::Task<Message> {
    use iced::Task;
    match message {
        Message::SetQueryText(text) => {
            model.query = text;
            Task::none()
        }
        Message::SetStatusText(text) => {
            model.status_text = text;
            Task::none()
        }
        Message::SetHotkeyHint(text) => {
            model.hotkey_hint = text;
            Task::none()
        }
        Message::SetModeStripText(text) => {
            model.mode_strip_text = text;
            Task::none()
        }
        Message::SetHelpConfigPath(text) => {
            model.help_config_path = text;
            Task::none()
        }
        Message::ShowPlaceholderHint(text) => {
            model.placeholder_hint = Some(text);
            Task::none()
        }
        Message::ClearPlaceholderHint => {
            model.placeholder_hint = None;
            Task::none()
        }
        Message::SetHotkeyIssueActive(active) => {
            model.hotkey_issue_active = active;
            Task::none()
        }
        Message::SetGameModeEnabled(enabled) => {
            model.game_mode_enabled = enabled;
            Task::none()
        }
        Message::SetPerformanceTuning {
            idle_cache_trim_ms,
            active_memory_target_mb,
        } => {
            model.idle_cache_trim_ms = idle_cache_trim_ms;
            model.active_memory_target_mb = active_memory_target_mb;
            Task::none()
        }
        Message::SetResults {
            rows,
            selected_index,
        } => {
            model.rows = rows;
            model.selected = selected_index.min(model.rows.len().saturating_sub(1));
            model.results_fade_started = Some(Instant::now());
            model.loading = false;
            model.search_generation = model.search_generation.wrapping_add(1);
            Task::none()
        }
        Message::SetSelectedIndex(idx) => {
            model.selected = idx.min(model.rows.len().saturating_sub(1));
            Task::none()
        }
        Message::Show => {
            model.visible = true;
            Task::none()
        }
        Message::Hide => {
            model.visible = false;
            model.query.clear();
            model.rows.clear();
            model.selected = 0;
            Task::none()
        }
        Message::HideNow => {
            model.visible = false;
            Task::none()
        }
        Message::FocusInputAndSelectAll => {
            model.has_focus = true;
            Task::none()
        }
        Message::QueryInputChanged(text) => {
            if model.query != text {
                model.query = text;
                model.last_query_change = Some(Instant::now());
            }
            Task::none()
        }
        Message::MoveSelection(delta) => {
            let max = model.rows.len();
            if max == 0 {
                return Task::none();
            }
            let current = model.selected as i64;
            let next = (current + delta as i64).clamp(0, max as i64 - 1) as usize;
            model.selected = next;
            Task::none()
        }
        Message::SubmitRequested => Task::none(),
        Message::EscapePressed => {
            model.visible = false;
            Task::none()
        }
        Message::HotkeyTriggered(_) => Task::none(),
        Message::TrayMenu(_) => Task::none(),
        Message::ExternalShowRequested => Task::none(),
        Message::ExternalQuitRequested => Task::none(),
        Message::SearchResultsReady => Task::none(),
        Message::LoadingTick => {
            if model.loading {
                model.loading_frame = model.loading_frame.wrapping_add(1);
            }
            Task::none()
        }
        Message::ResultsFadeTick => Task::none(),
        Message::ThemeDetected(theme) => {
            model.theme = theme;
            Task::none()
        }
        Message::Closed => {
            model.visible = false;
            Task::none()
        }
    }
}

/// Translate a `Message` into a legacy `OverlayEvent`, if any. Returns
/// `Some(_)` only for messages the runtime callback cares about.
pub fn message_to_event(model: &Model, message: &Message) -> Option<OverlayEvent> {
    match message {
        Message::HotkeyTriggered(id) => Some(OverlayEvent::Hotkey(*id)),
        Message::QueryInputChanged(text) => Some(OverlayEvent::QueryChanged(text.clone())),
        Message::MoveSelection(delta) => Some(OverlayEvent::MoveSelection(*delta)),
        Message::SubmitRequested => Some(OverlayEvent::Submit),
        Message::EscapePressed => Some(OverlayEvent::Escape),
        Message::TrayMenu(crate::overlay::model::TrayMenuAction::ToggleGameMode) => {
            Some(OverlayEvent::TrayToggleGameMode)
        }
        Message::TrayMenu(crate::overlay::model::TrayMenuAction::CheckForUpdates) => {
            Some(OverlayEvent::TrayCheckForUpdates)
        }
        Message::ExternalShowRequested => Some(OverlayEvent::ExternalShow),
        Message::ExternalQuitRequested => Some(OverlayEvent::ExternalQuit),
        Message::SearchResultsReady => Some(OverlayEvent::SearchResultsReady),
        _ => None,
    }
    .map(|mut event| {
        // Refresh the query snapshot so QueryChanged always carries the
        // model up-to-date at translation time.
        if let OverlayEvent::QueryChanged(ref mut text) = event {
            *text = model.query.clone();
        }
        event
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_n_rows(n: usize) -> Model {
        let mut m = Model::default();
        m.rows = (0..n)
            .map(|i| OverlayRow {
                role: OverlayRowRole::Item,
                result_index: Some(i),
                kind: "file".into(),
                title: format!("row {i}"),
                path: String::new(),
                icon_path: String::new(),
            })
            .collect();
        m
    }

    #[test]
    fn move_selection_clamps_to_top_and_bottom() {
        let mut m = model_with_n_rows(5);
        update(&mut m, Message::MoveSelection(1));
        assert_eq!(m.selected, 1);
        update(&mut m, Message::MoveSelection(100));
        assert_eq!(m.selected, 4);
        update(&mut m, Message::MoveSelection(-100));
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn move_selection_is_noop_when_no_rows() {
        let mut m = Model::default();
        update(&mut m, Message::MoveSelection(1));
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn set_results_clamps_selected_index() {
        let mut m = Model::default();
        let rows = vec![OverlayRow {
            role: OverlayRowRole::TopHit,
            result_index: Some(0),
            kind: "app".into(),
            title: "x".into(),
            path: String::new(),
            icon_path: String::new(),
        }];
        update(
            &mut m,
            Message::SetResults {
                rows,
                selected_index: 999,
            },
        );
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn set_selected_index_clamps() {
        let mut m = model_with_n_rows(2);
        update(&mut m, Message::SetSelectedIndex(99));
        assert_eq!(m.selected, 1);
    }

    #[test]
    fn show_and_hide_toggle_visibility() {
        let mut m = Model::default();
        update(&mut m, Message::Show);
        assert!(m.visible);
        update(&mut m, Message::Hide);
        assert!(!m.visible);
        assert!(m.query.is_empty());
        assert!(m.rows.is_empty());
    }

    #[test]
    fn hide_now_keeps_query_and_rows() {
        let mut m = model_with_n_rows(3);
        m.visible = true;
        update(&mut m, Message::HideNow);
        assert!(!m.visible);
        assert_eq!(m.rows.len(), 3);
    }

    #[test]
    fn query_input_changed_marks_timestamp() {
        let mut m = Model::default();
        update(&mut m, Message::QueryInputChanged("hello".into()));
        assert_eq!(m.query, "hello");
        assert!(m.last_query_change.is_some());
    }

    #[test]
    fn identical_query_input_change_is_a_noop() {
        let mut m = Model::default();
        m.query = "x".into();
        m.last_query_change = None;
        update(&mut m, Message::QueryInputChanged("x".into()));
        assert!(m.last_query_change.is_none());
    }

    #[test]
    fn message_to_event_emits_query_changed() {
        let mut m = Model::default();
        update(&mut m, Message::QueryInputChanged("hello".into()));
        let event = message_to_event(&m, &Message::QueryInputChanged("hello".into()))
            .expect("event");
        match event {
            OverlayEvent::QueryChanged(text) => assert_eq!(text, "hello"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn message_to_event_ignores_setters() {
        let m = Model::default();
        assert!(message_to_event(&m, &Message::Show).is_none());
        assert!(message_to_event(&m, &Message::Hide).is_none());
        assert!(message_to_event(&m, &Message::SetStatusText("x".into())).is_none());
    }
}
