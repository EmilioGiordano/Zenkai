use gpui_kit::{FocusHandle, ScrollHandle};
use zenkai_types::{Rgb, SheetId};

use crate::tab_colors::{COLUMNS, Swatch, THEME_ROWS};

pub(crate) const SWATCH_SIZE: f32 = 20.0;
pub(crate) const SWATCH_GAP: f32 = 6.0;
// The first theme row sits apart from its tints.
pub(crate) const TINT_GAP: f32 = 4.0;
pub(crate) const LABEL_HEIGHT: f32 = 20.0;
pub(crate) const LABEL_GAP: f32 = 4.0;
pub(crate) const SECTION_GAP: f32 = 12.0;
// Room for a focus ring at the edge of the region or of what scrolls.
pub(crate) const REGION_PADDING: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Swatch(Swatch),
    NoColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caption {
    Swatch(Swatch),
    Outside(Rgb),
    Blank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Move {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

fn with_col(swatch: Swatch, col: usize) -> Swatch {
    match swatch {
        Swatch::Theme { row, .. } => Swatch::Theme { row, col },
        Swatch::Standard { .. } => Swatch::Standard { col },
    }
}

fn col_of(swatch: Swatch) -> usize {
    match swatch {
        Swatch::Theme { col, .. } | Swatch::Standard { col } => col,
    }
}

// Arrows move in two dimensions and never wrap sideways. Down from the standard row reaches
// No color and Up from there returns to the swatch that was left, kept in `back`.
pub(crate) fn moved(from: Focus, back: Swatch, key: Move) -> (Focus, Swatch) {
    let swatch = match from {
        Focus::NoColor => {
            return match key {
                Move::Up => (Focus::Swatch(back), back),
                _ => (from, back),
            };
        }
        Focus::Swatch(swatch) => swatch,
    };
    let col = col_of(swatch);
    let next = match key {
        Move::Left => with_col(swatch, col.saturating_sub(1)),
        Move::Right => with_col(swatch, (col + 1).min(COLUMNS - 1)),
        Move::Home => with_col(swatch, 0),
        Move::End => with_col(swatch, COLUMNS - 1),
        Move::Up => match swatch {
            Swatch::Theme { row, col } => Swatch::Theme {
                row: row.saturating_sub(1),
                col,
            },
            Swatch::Standard { col } => Swatch::Theme {
                row: THEME_ROWS - 1,
                col,
            },
        },
        Move::Down => match swatch {
            Swatch::Theme { row, col } if row + 1 < THEME_ROWS => {
                Swatch::Theme { row: row + 1, col }
            }
            Swatch::Theme { col, .. } => Swatch::Standard { col },
            Swatch::Standard { .. } => return (Focus::NoColor, swatch),
        },
    };
    (Focus::Swatch(next), back)
}

pub(crate) fn theme_top() -> f32 {
    REGION_PADDING + LABEL_HEIGHT + LABEL_GAP
}

fn step() -> f32 {
    SWATCH_SIZE + SWATCH_GAP
}

// Where the swatch starts inside what scrolls, so focusing one can bring it into view.
pub(crate) fn top_of(swatch: Swatch) -> f32 {
    match swatch {
        Swatch::Theme { row, .. } => {
            let apart = if row > 0 { TINT_GAP } else { 0.0 };
            theme_top() + row as f32 * step() + apart
        }
        Swatch::Standard { .. } => {
            theme_top()
                + (THEME_ROWS - 1) as f32 * step()
                + TINT_GAP
                + SWATCH_SIZE
                + SECTION_GAP
                + LABEL_HEIGHT
                + LABEL_GAP
        }
    }
}

// The offset that shows `top..bottom` with the ring around it, moving as little as it can.
pub(crate) fn scrolled_to(offset: f32, view: f32, top: f32, bottom: f32) -> f32 {
    let (top, bottom) = (top - REGION_PADDING, bottom + REGION_PADDING);
    if top < offset {
        top.max(0.0)
    } else if bottom > offset + view {
        (bottom - view).max(0.0)
    } else {
        offset
    }
}

pub(crate) struct TabColorCard {
    // The card acts on the sheet it opened on and the document that held it.
    pub(crate) sheet: SheetId,
    pub(crate) generation: u64,
    // What the file says; a color outside the palette is only named, never marked.
    pub(crate) file_color: Option<Rgb>,
    pub(crate) focus: Focus,
    pub(crate) back: Swatch,
    pub(crate) hovered: Option<Swatch>,
    // Whether the arrows moved focus yet, which decides what the caption names.
    pub(crate) moved: bool,
    pub(crate) card_focus: FocusHandle,
    pub(crate) swatch_focus: Vec<FocusHandle>,
    pub(crate) none_focus: FocusHandle,
    pub(crate) scroll: ScrollHandle,
}

impl TabColorCard {
    pub(crate) fn new(
        sheet: SheetId,
        generation: u64,
        file_color: Option<Rgb>,
        card_focus: FocusHandle,
        swatch_focus: Vec<FocusHandle>,
        none_focus: FocusHandle,
    ) -> TabColorCard {
        let start = file_color.and_then(Swatch::of).unwrap_or(Swatch::FIRST);
        TabColorCard {
            sheet,
            generation,
            file_color,
            focus: Focus::Swatch(start),
            back: start,
            hovered: None,
            moved: false,
            card_focus,
            swatch_focus,
            none_focus,
            scroll: ScrollHandle::new(),
        }
    }

    // The swatch that carries the check; none for a sheet without color (No color carries it)
    // or with one the palette does not hold.
    pub(crate) fn marked(&self) -> Option<Swatch> {
        self.file_color.and_then(Swatch::of)
    }

    pub(crate) fn no_color_marked(&self) -> bool {
        self.file_color.is_none()
    }

    // A color outside the palette is named in the caption.
    pub(crate) fn outside_palette(&self) -> Option<Rgb> {
        self.file_color.filter(|color| Swatch::of(*color).is_none())
    }

    // The hovered swatch, else a color the palette lacks until the arrows move, else the
    // swatch with focus.
    pub(crate) fn caption(&self) -> Caption {
        if let Some(swatch) = self.hovered {
            return Caption::Swatch(swatch);
        }
        if !self.moved
            && let Some(color) = self.outside_palette()
        {
            return Caption::Outside(color);
        }
        match self.focus {
            Focus::Swatch(swatch) => Caption::Swatch(swatch),
            Focus::NoColor => Caption::Blank,
        }
    }

    pub(crate) fn handle_of(&self, swatch: Swatch) -> Option<&FocusHandle> {
        self.swatch_focus.get(swatch.index())
    }

    pub(crate) fn grid_handle(&self) -> Option<&FocusHandle> {
        match self.focus {
            Focus::Swatch(swatch) => self.handle_of(swatch),
            Focus::NoColor => self.handle_of(self.back),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COLUMNS, Focus, LABEL_GAP, LABEL_HEIGHT, Move, REGION_PADDING, SWATCH_SIZE, moved,
        scrolled_to, top_of,
    };
    use crate::tab_colors::Swatch;

    const FIRST: Swatch = Swatch::FIRST;

    fn theme(row: usize, col: usize) -> Swatch {
        Swatch::Theme { row, col }
    }

    fn go(from: Focus, back: Swatch, keys: &[Move]) -> (Focus, Swatch) {
        keys.iter()
            .fold((from, back), |(focus, back), key| moved(focus, back, *key))
    }

    #[test]
    fn arrows_move_in_two_dimensions_and_do_not_wrap_sideways() {
        let (focus, _) = go(Focus::Swatch(FIRST), FIRST, &[Move::Left, Move::Up]);
        assert_eq!(focus, Focus::Swatch(FIRST));
        let (focus, _) = go(
            Focus::Swatch(FIRST),
            FIRST,
            &[Move::Right, Move::Right, Move::Down],
        );
        assert_eq!(focus, Focus::Swatch(theme(1, 2)));
        let (focus, _) = go(Focus::Swatch(theme(3, COLUMNS - 1)), FIRST, &[Move::Right]);
        assert_eq!(focus, Focus::Swatch(theme(3, COLUMNS - 1)));
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_the_row() {
        let (focus, _) = go(Focus::Swatch(theme(2, 4)), FIRST, &[Move::End]);
        assert_eq!(focus, Focus::Swatch(theme(2, COLUMNS - 1)));
        let (focus, _) = go(Focus::Swatch(theme(2, 4)), FIRST, &[Move::Home]);
        assert_eq!(focus, Focus::Swatch(theme(2, 0)));
        let (focus, _) = go(
            Focus::Swatch(Swatch::Standard { col: 3 }),
            FIRST,
            &[Move::End],
        );
        assert_eq!(focus, Focus::Swatch(Swatch::Standard { col: COLUMNS - 1 }));
    }

    #[test]
    fn down_from_the_last_theme_row_enters_the_standard_row_in_the_same_column() {
        let (focus, _) = go(Focus::Swatch(theme(5, 7)), FIRST, &[Move::Down]);
        assert_eq!(focus, Focus::Swatch(Swatch::Standard { col: 7 }));
        let (focus, _) = go(
            Focus::Swatch(Swatch::Standard { col: 7 }),
            FIRST,
            &[Move::Up],
        );
        assert_eq!(focus, Focus::Swatch(theme(5, 7)));
    }

    #[test]
    fn down_from_the_standard_row_reaches_no_color_and_up_returns_to_the_swatch_left() {
        let standard = Swatch::Standard { col: 4 };
        let (focus, back) = go(Focus::Swatch(standard), FIRST, &[Move::Down]);
        assert_eq!((focus, back), (Focus::NoColor, standard));
        let (focus, back) = go(focus, back, &[Move::Down, Move::Left, Move::Home]);
        assert_eq!((focus, back), (Focus::NoColor, standard));
        let (focus, _) = go(focus, back, &[Move::Up]);
        assert_eq!(focus, Focus::Swatch(standard));
    }

    #[test]
    fn the_scroll_moves_the_least_that_shows_the_swatch_with_its_ring() {
        let view = 100.0;
        assert_eq!(scrolled_to(0.0, view, 10.0, 30.0), 0.0);
        assert_eq!(scrolled_to(0.0, view, 150.0, 170.0), 175.0 - view);
        assert_eq!(scrolled_to(200.0, view, 150.0, 170.0), 145.0);
        assert_eq!(scrolled_to(0.0, view, 2.0, 22.0), 0.0);
    }

    #[test]
    fn the_rows_sit_where_the_layout_puts_them() {
        let first = REGION_PADDING + LABEL_HEIGHT + LABEL_GAP;
        assert_eq!(top_of(theme(0, 3)), first);
        assert_eq!(top_of(theme(1, 0)), first + 26.0 + 4.0);
        assert_eq!(top_of(theme(5, 9)), first + 5.0 * 26.0 + 4.0);
        let standard = top_of(Swatch::Standard { col: 0 });
        assert_eq!(
            standard,
            top_of(theme(5, 0)) + SWATCH_SIZE + 12.0 + LABEL_HEIGHT + LABEL_GAP
        );
    }
}
