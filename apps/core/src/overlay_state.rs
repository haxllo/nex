use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ShowAndFocus,
    Hide,
    FocusExisting,
}

/// A show command is in flight (queued Show -> posted snapshot -> Painted).
/// Static rather than a field: the host sets it on the Painted path while
/// the hotkey decider lives on the runtime loop, and plumbing it through a
/// UiCommand would erase the "pending" window this exists to cover.
static SHOW_PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayState {
    visible: bool,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self { visible: false }
    }
}

impl OverlayState {
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            SHOW_PENDING.store(false, Ordering::SeqCst);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The host calls this when the window actually painted and became
    /// visible (UiCommand::Painted), so the flag covers only the
    /// command-to-first-frame gap.
    pub fn notify_show_painted() {
        SHOW_PENDING.store(false, Ordering::SeqCst);
    }

    pub fn on_hotkey(&mut self, has_focus: bool) -> HotkeyAction {
        if !self.visible {
            self.visible = true;
            SHOW_PENDING.store(true, Ordering::SeqCst);
            return HotkeyAction::ShowAndFocus;
        }
        if has_focus {
            self.visible = false;
            SHOW_PENDING.store(false, Ordering::SeqCst);
            HotkeyAction::Hide
        } else if SHOW_PENDING.load(Ordering::SeqCst) {
            // Show still in flight — the previous press never reached the
            // screen (first paint after an idle can take 95-140ms while
            // WebView2 pages fault back in). Re-kick the show instead of
            // refocusing a window that does not exist yet.
            HotkeyAction::ShowAndFocus
        } else {
            HotkeyAction::FocusExisting
        }
    }

    pub fn on_escape(&mut self) -> bool {
        if self.visible {
            self.visible = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{HotkeyAction, OverlayState};
    // The hotkey tests share the module's SHOW_PENDING atomic; without a
    // gate, parallel runs interleave stores/loads between the two and flake.
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn normalize() {
        OverlayState::notify_show_painted();
    }

    #[test]
    fn hotkey_shows_hidden_overlay() {
        let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = OverlayState::default();
        let action = state.on_hotkey(false);
        assert_eq!(action, HotkeyAction::ShowAndFocus);
        assert!(state.is_visible());
        OverlayState::notify_show_painted();
    }

    #[test]
    fn hotkey_hides_visible_overlay_when_focused() {
        let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = OverlayState::default();
        state.on_hotkey(false);
        let action = state.on_hotkey(true);
        assert_eq!(action, HotkeyAction::Hide);
        assert!(!state.is_visible());
    }

    #[test]
    fn hotkey_refocuses_visible_overlay_when_not_focused() {
        let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
        normalize();
        let mut state = OverlayState::default();
        assert_eq!(state.on_hotkey(false), HotkeyAction::ShowAndFocus);
        // painted -> later unfocused press refocuses
        normalize();
        let action = state.on_hotkey(false);
        assert_eq!(action, HotkeyAction::FocusExisting);
        assert!(state.is_visible());
        OverlayState::notify_show_painted();
    }

    #[test]
    fn hotkey_rekicks_show_while_first_paint_is_pending() {
        // Cold show after an idle: press 1 shows, press 2 arrives before the
        // panel painted. It must re-kick the show, not swallow itself into
        // FocusExisting (the "presses do nothing" report).
        let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
        normalize();
        let mut state = OverlayState::default();
        assert_eq!(state.on_hotkey(false), HotkeyAction::ShowAndFocus);
        assert_eq!(state.on_hotkey(false), HotkeyAction::ShowAndFocus);
        // Panel painted; a later unfocused press refocuses again.
        normalize();
        assert_eq!(state.on_hotkey(false), HotkeyAction::FocusExisting);
        // And a focused press still hides after everything.
        assert_eq!(state.on_hotkey(true), HotkeyAction::Hide);
        assert!(!state.is_visible());
        normalize();
    }

    #[test]
    fn escape_hides_only_when_visible() {
        let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = OverlayState::default();
        assert!(!state.on_escape());
        state.on_hotkey(false);
        assert!(state.on_escape());
        assert!(!state.is_visible());
        OverlayState::notify_show_painted();
    }
}
