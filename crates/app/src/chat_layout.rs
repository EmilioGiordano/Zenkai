use crate::panel_width::{GRID_MINIMUM, Panel};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatLayout {
    #[default]
    Docked,
    SideBySide,
    FullWindow,
}

impl ChatLayout {
    fn cycled(self) -> ChatLayout {
        match self {
            ChatLayout::Docked => ChatLayout::SideBySide,
            ChatLayout::SideBySide => ChatLayout::FullWindow,
            ChatLayout::FullWindow => ChatLayout::Docked,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatLayoutState {
    current: ChatLayout,
    before_full_window: ChatLayout,
}

impl Default for ChatLayoutState {
    fn default() -> ChatLayoutState {
        ChatLayoutState {
            current: ChatLayout::Docked,
            before_full_window: ChatLayout::Docked,
        }
    }
}

impl ChatLayoutState {
    pub fn current(self) -> ChatLayout {
        self.current
    }

    pub fn set(self, layout: ChatLayout) -> ChatLayoutState {
        let before_full_window =
            if layout == ChatLayout::FullWindow && self.current != ChatLayout::FullWindow {
                self.current
            } else {
                self.before_full_window
            };
        ChatLayoutState {
            current: layout,
            before_full_window,
        }
    }

    pub fn cycled(self) -> ChatLayoutState {
        self.set(self.current.cycled())
    }

    pub fn toggled_full_window(self) -> ChatLayoutState {
        match self.current {
            ChatLayout::FullWindow => self.set(self.before_full_window),
            _ => self.set(ChatLayout::FullWindow),
        }
    }
}

// The share of the width beside the sidebar that the chat takes when it sits next to the sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SideSplit(f32);

impl Default for SideSplit {
    fn default() -> SideSplit {
        SideSplit(0.5)
    }
}

impl SideSplit {
    pub fn limits(available: f32) -> (f32, f32) {
        let (min, _) = Panel::Chat.limits();
        (min, (available - GRID_MINIMUM).max(min))
    }

    pub fn chat_width(self, available: f32) -> f32 {
        let (min, max) = SideSplit::limits(available);
        (self.0 * available).clamp(min, max)
    }

    pub fn with_chat_width(self, width: f32, available: f32) -> SideSplit {
        if available <= 0.0 {
            return self;
        }
        let (min, max) = SideSplit::limits(available);
        SideSplit(width.clamp(min, max) / available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_walks_the_three_layouts_and_wraps() {
        let docked = ChatLayoutState::default();
        let side = docked.cycled();
        let full = side.cycled();
        assert_eq!(side.current(), ChatLayout::SideBySide);
        assert_eq!(full.current(), ChatLayout::FullWindow);
        assert_eq!(full.cycled().current(), ChatLayout::Docked);
    }

    #[test]
    fn the_toggle_returns_to_the_layout_before_full_window() {
        let docked = ChatLayoutState::default();
        let full = docked.toggled_full_window();
        assert_eq!(full.current(), ChatLayout::FullWindow);
        assert_eq!(full.toggled_full_window().current(), ChatLayout::Docked);

        let side = docked.set(ChatLayout::SideBySide);
        assert_eq!(
            side.toggled_full_window().toggled_full_window().current(),
            ChatLayout::SideBySide
        );
    }

    #[test]
    fn choosing_full_window_twice_keeps_the_layout_to_return_to() {
        let side = ChatLayoutState::default().set(ChatLayout::SideBySide);
        let full = side.set(ChatLayout::FullWindow).set(ChatLayout::FullWindow);
        assert_eq!(full.toggled_full_window().current(), ChatLayout::SideBySide);
    }

    #[test]
    fn the_default_split_is_half_and_respects_the_limits() {
        let split = SideSplit::default();
        assert_eq!(split.chat_width(1200.0), 600.0);
        assert_eq!(split.chat_width(700.0), 320.0);
        let wide = split.with_chat_width(5000.0, 1200.0);
        assert!((wide.chat_width(1200.0) - 720.0).abs() < 0.01);
        let narrow = split.with_chat_width(10.0, 1200.0);
        assert!((narrow.chat_width(1200.0) - 320.0).abs() < 0.01);
    }

    #[test]
    fn the_split_follows_the_window_when_it_resizes() {
        let split = SideSplit::default().with_chat_width(400.0, 1000.0);
        assert!((split.chat_width(2000.0) - 800.0).abs() < 0.01);
    }
}
