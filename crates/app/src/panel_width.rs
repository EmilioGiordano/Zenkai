use serde::{Deserialize, Serialize};

const GRID_MINIMUM: f32 = 480.0;
pub const KEY_STEP: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Sidebar,
    Chat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resize {
    Wider,
    Narrower,
}

impl Panel {
    pub fn limits(self) -> (f32, f32) {
        match self {
            Panel::Sidebar => (180.0, 420.0),
            Panel::Chat => (320.0, 720.0),
        }
    }

    fn default_width(self) -> f32 {
        match self {
            Panel::Sidebar => 248.0,
            Panel::Chat => 464.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PanelWidth(f32);

// A value of the wrong type must not make the whole session unreadable; it reads as unusable
// and validation replaces it.
impl<'de> Deserialize<'de> for PanelWidth {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<PanelWidth, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Number(f32),
            Other(serde::de::IgnoredAny),
        }
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Number(value) => PanelWidth(value),
            Stored::Other(_) => PanelWidth(f32::NAN),
        })
    }
}

impl PanelWidth {
    fn stored(panel: Panel, value: f32) -> PanelWidth {
        let (min, max) = panel.limits();
        if (min..=max).contains(&value) {
            PanelWidth(value)
        } else {
            PanelWidth(panel.default_width())
        }
    }

    fn within(panel: Panel, requested: f32, window: f32, other: f32) -> PanelWidth {
        let (min, max) = panel.limits();
        let room = window - GRID_MINIMUM - other;
        let upper = max.min(room).max(min);
        PanelWidth(requested.clamp(min, upper))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelWidths {
    sidebar: PanelWidth,
    chat: PanelWidth,
}

impl Default for PanelWidths {
    fn default() -> PanelWidths {
        PanelWidths {
            sidebar: PanelWidth(Panel::Sidebar.default_width()),
            chat: PanelWidth(Panel::Chat.default_width()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub sidebar: bool,
    pub chat: bool,
}

impl PanelWidths {
    // Values a file edit or an older version left out of range go back to the defaults.
    pub fn validated(self) -> PanelWidths {
        PanelWidths {
            sidebar: PanelWidth::stored(Panel::Sidebar, self.sidebar.0),
            chat: PanelWidth::stored(Panel::Chat, self.chat.0),
        }
    }

    fn of(self, panel: Panel) -> PanelWidth {
        match panel {
            Panel::Sidebar => self.sidebar,
            Panel::Chat => self.chat,
        }
    }

    fn with(self, panel: Panel, width: PanelWidth) -> PanelWidths {
        match panel {
            Panel::Sidebar => PanelWidths {
                sidebar: width,
                ..self
            },
            Panel::Chat => PanelWidths {
                chat: width,
                ..self
            },
        }
    }

    // The widths to draw: the stored ones, shrunk when the window leaves the grid less room
    // than its minimum.
    pub fn fitted(self, window: f32, shown: Shown) -> PanelWidths {
        let sidebar_stored = if shown.sidebar { self.sidebar.0 } else { 0.0 };
        let chat = PanelWidth::within(Panel::Chat, self.chat.0, window, sidebar_stored);
        let chat_taken = if shown.chat { chat.0 } else { 0.0 };
        let sidebar = PanelWidth::within(Panel::Sidebar, self.sidebar.0, window, chat_taken);
        PanelWidths { sidebar, chat }
    }

    pub fn resized(self, panel: Panel, requested: f32, window: f32, shown: Shown) -> PanelWidths {
        let fitted = self.fitted(window, shown);
        let other = match panel {
            Panel::Sidebar if shown.chat => fitted.chat.0,
            Panel::Chat if shown.sidebar => fitted.sidebar.0,
            _ => 0.0,
        };
        let width = PanelWidth::within(panel, requested, window, other);
        fitted.with(panel, width)
    }

    pub fn stepped(self, panel: Panel, resize: Resize, window: f32, shown: Shown) -> PanelWidths {
        let current = self.fitted(window, shown).of(panel).0;
        let requested = match resize {
            Resize::Wider => current + KEY_STEP,
            Resize::Narrower => current - KEY_STEP,
        };
        self.resized(panel, requested, window, shown)
    }

    pub fn restored(self, panel: Panel, window: f32, shown: Shown) -> PanelWidths {
        self.resized(panel, panel.default_width(), window, shown)
    }

    pub fn width_of(self, panel: Panel) -> f32 {
        self.of(panel).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: Shown = Shown {
        sidebar: true,
        chat: true,
    };
    const BIG: f32 = 3000.0;

    #[test]
    fn a_drag_stays_inside_the_panel_limits() {
        let widths = PanelWidths::default();
        let low = widths.resized(Panel::Sidebar, 10.0, BIG, BOTH);
        let high = widths.resized(Panel::Sidebar, 5000.0, BIG, BOTH);
        assert_eq!(low.width_of(Panel::Sidebar), 180.0);
        assert_eq!(high.width_of(Panel::Sidebar), 420.0);
        let low = widths.resized(Panel::Chat, 10.0, BIG, BOTH);
        let high = widths.resized(Panel::Chat, 5000.0, BIG, BOTH);
        assert_eq!(low.width_of(Panel::Chat), 320.0);
        assert_eq!(high.width_of(Panel::Chat), 720.0);
    }

    #[test]
    fn the_grid_keeps_its_minimum_on_a_small_window() {
        let widths = PanelWidths::default();
        let window = 1000.0;
        let sidebar = widths.resized(Panel::Sidebar, 420.0, window, BOTH);
        let chat_width = sidebar.width_of(Panel::Chat);
        assert_eq!(
            sidebar.width_of(Panel::Sidebar),
            window - 480.0 - chat_width
        );
        let chat = widths.resized(Panel::Chat, 720.0, window, BOTH);
        let sidebar_width = chat.width_of(Panel::Sidebar);
        assert_eq!(chat.width_of(Panel::Chat), window - 480.0 - sidebar_width);
    }

    #[test]
    fn a_hidden_panel_takes_no_room_from_the_grid() {
        let only_chat = Shown {
            sidebar: false,
            chat: true,
        };
        let widths = PanelWidths::default().resized(Panel::Chat, 720.0, 1200.0, only_chat);
        assert_eq!(widths.width_of(Panel::Chat), 720.0);
    }

    #[test]
    fn a_window_too_small_for_the_grid_still_gets_the_minimum_widths() {
        let widths = PanelWidths::default().fitted(500.0, BOTH);
        assert_eq!(widths.width_of(Panel::Sidebar), 180.0);
        assert_eq!(widths.width_of(Panel::Chat), 320.0);
    }

    #[test]
    fn drawn_widths_shrink_with_the_window_without_forgetting_the_stored_ones() {
        let stored = PanelWidths::default();
        let small = stored.fitted(1100.0, BOTH);
        assert!(small.width_of(Panel::Sidebar) + small.width_of(Panel::Chat) <= 1100.0 - 480.0);
        assert_eq!(stored.fitted(BIG, BOTH), stored);
    }

    #[test]
    fn restoring_gives_the_default_width() {
        let widths = PanelWidths::default().resized(Panel::Chat, 600.0, BIG, BOTH);
        let restored = widths.restored(Panel::Chat, BIG, BOTH);
        assert_eq!(restored, PanelWidths::default());
    }

    #[test]
    fn a_key_step_moves_sixteen_pixels_and_stops_at_the_limits() {
        let widths = PanelWidths::default();
        let wider = widths.stepped(Panel::Sidebar, Resize::Wider, BIG, BOTH);
        assert_eq!(wider.width_of(Panel::Sidebar), 248.0 + KEY_STEP);
        let mut narrow = widths;
        for _ in 0..100 {
            narrow = narrow.stepped(Panel::Chat, Resize::Narrower, BIG, BOTH);
        }
        assert_eq!(narrow.width_of(Panel::Chat), 320.0);
    }

    #[test]
    fn stored_values_out_of_range_fall_back_to_the_defaults() {
        let from_file: PanelWidths =
            serde_json::from_str(r#"{"sidebar": 50.0, "chat": 99999.0}"#).unwrap();
        assert_eq!(from_file.validated(), PanelWidths::default());
        let unusable = PanelWidths {
            sidebar: PanelWidth(f32::NAN),
            chat: PanelWidth(f32::INFINITY),
        };
        assert_eq!(unusable.validated(), PanelWidths::default());
    }

    #[test]
    fn a_value_of_the_wrong_type_reads_as_the_default() {
        let from_file: PanelWidths =
            serde_json::from_str(r#"{"sidebar": "wide", "chat": null}"#).unwrap();
        assert_eq!(from_file.validated(), PanelWidths::default());
    }

    #[test]
    fn valid_stored_values_are_kept_and_a_missing_one_gets_its_default() {
        let from_file: PanelWidths = serde_json::from_str(r#"{"sidebar": 300.0}"#).unwrap();
        let validated = from_file.validated();
        assert_eq!(validated.width_of(Panel::Sidebar), 300.0);
        assert_eq!(validated.width_of(Panel::Chat), 464.0);
    }
}
