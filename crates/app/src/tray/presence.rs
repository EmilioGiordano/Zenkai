#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Shown,
    HiddenToTray,
}

impl Presence {
    // A minimized window also reports itself hidden, but it still sits in the taskbar, so
    // only a hide the tray asked for counts.
    pub fn after_visibility(self, visible: bool, hide_requested: bool) -> Presence {
        match (visible, hide_requested) {
            (true, _) => Presence::Shown,
            (false, true) => Presence::HiddenToTray,
            (false, false) => self,
        }
    }

    pub fn dot_rgb(self) -> [u8; 3] {
        match self {
            Presence::Shown => [0x3f, 0xb9, 0x50],
            Presence::HiddenToTray => [0xd2, 0x99, 0x22],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visible_window_is_always_shown() {
        assert_eq!(
            Presence::HiddenToTray.after_visibility(true, true),
            Presence::Shown
        );
        assert_eq!(
            Presence::HiddenToTray.after_visibility(true, false),
            Presence::Shown
        );
    }

    #[test]
    fn only_a_requested_hide_turns_the_state_to_hidden() {
        assert_eq!(
            Presence::Shown.after_visibility(false, true),
            Presence::HiddenToTray
        );
        assert_eq!(
            Presence::Shown.after_visibility(false, false),
            Presence::Shown
        );
        assert_eq!(
            Presence::HiddenToTray.after_visibility(false, false),
            Presence::HiddenToTray
        );
    }

    #[test]
    fn the_dot_is_green_when_shown_and_yellow_when_hidden() {
        assert_eq!(Presence::Shown.dot_rgb(), [0x3f, 0xb9, 0x50]);
        assert_eq!(Presence::HiddenToTray.dot_rgb(), [0xd2, 0x99, 0x22]);
    }
}
