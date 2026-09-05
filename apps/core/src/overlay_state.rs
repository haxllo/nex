#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ShowAndFocus,
    Hide,
    FocusExisting,
}

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
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn on_hotkey(&mut self, has_focus: bool) -> HotkeyAction {
        if !self.visible {
            self.visible = true;
            return HotkeyAction::ShowAndFocus;
        }
        if has_focus {
            self.visible = false;
            HotkeyAction::Hide
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

    #[test]
    fn hotkey_shows_hidden_overlay() {
        let mut state = OverlayState::default();
        let action = state.on_hotkey(false);
        assert_eq!(action, HotkeyAction::ShowAndFocus);
        assert!(state.is_visible());
    }

    #[test]
    fn hotkey_hides_visible_overlay_when_focused() {
        let mut state = OverlayState::default();
        state.on_hotkey(false);
        let action = state.on_hotkey(true);
        assert_eq!(action, HotkeyAction::Hide);
        assert!(!state.is_visible());
    }

    #[test]
    fn hotkey_refocuses_visible_overlay_when_not_focused() {
        let mut state = OverlayState::default();
        state.on_hotkey(false);
        let action = state.on_hotkey(false);
        assert_eq!(action, HotkeyAction::FocusExisting);
        assert!(state.is_visible());
    }

    #[test]
    fn escape_hides_only_when_visible() {
        let mut state = OverlayState::default();
        assert!(!state.on_escape());
        state.on_hotkey(false);
        assert!(state.on_escape());
        assert!(!state.is_visible());
    }
}
