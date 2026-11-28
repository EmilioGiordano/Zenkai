// How tall the list is and where it hangs, in pixels of the interface scale. The height is
// fixed in every state, so nothing moves when values arrive.
pub const ROW_HEIGHT: f32 = 24.0;
pub const FULL_ROWS: usize = 9;
pub const SHORT_ROWS: usize = 4;
pub const WIDTH: f32 = 264.0;
// Everything but the list rows: three menu items, rules, search, the note line and the
// buttons, with the popover's padding and border.
const CHROME: f32 = 4.0 + 56.0 + 9.0 + 28.0 + 9.0 + 40.0 + 2.0 + 20.0 + 36.0 + 4.0 + 2.0;
// From the popover's top edge to the first row of the list.
pub const LIST_TOP: f32 = 4.0 + 56.0 + 9.0 + 28.0 + 9.0 + 40.0 + 1.0;
const GAP: f32 = 2.0;
const WINDOW_MARGIN: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub side: Side,
    pub rows: usize,
}

impl Placement {
    pub fn height(&self, scale: f32) -> f32 {
        (CHROME + self.rows as f32 * ROW_HEIGHT) * scale
    }
}

// Below the button when the full list fits there, else above it, else the room that is
// larger with the short list. `scale` is the interface scale (the list does not follow the
// grid zoom).
pub fn place(button_top: f32, button_bottom: f32, window_height: f32, scale: f32) -> Placement {
    let below = window_height - button_bottom - GAP - WINDOW_MARGIN;
    let above = button_top - GAP - WINDOW_MARGIN;
    let full = Placement {
        side: Side::Below,
        rows: FULL_ROWS,
    };
    let fits = |room: f32, rows: usize| {
        Placement {
            side: Side::Below,
            rows,
        }
        .height(scale)
            <= room
    };
    if fits(below, FULL_ROWS) {
        full
    } else if fits(above, FULL_ROWS) {
        Placement {
            side: Side::Above,
            ..full
        }
    } else if below >= above {
        Placement {
            side: Side::Below,
            rows: SHORT_ROWS,
        }
    } else {
        Placement {
            side: Side::Above,
            rows: SHORT_ROWS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Side, place};

    #[test]
    fn the_full_list_hangs_below_the_button_when_it_fits() {
        let placement = place(100.0, 120.0, 900.0, 1.0);
        assert_eq!((placement.side, placement.rows), (Side::Below, 9));
    }

    #[test]
    fn it_flips_above_the_button_when_only_there_it_fits() {
        let placement = place(700.0, 720.0, 800.0, 1.0);
        assert_eq!((placement.side, placement.rows), (Side::Above, 9));
    }

    #[test]
    fn a_short_window_gets_four_rows_on_the_roomier_side() {
        let placement = place(150.0, 170.0, 330.0, 1.0);
        assert_eq!(placement.rows, 4);
        assert_eq!(placement.side, Side::Below);
        let below = place(20.0, 40.0, 400.0, 1.0);
        assert_eq!((below.side, below.rows), (Side::Below, 4));
    }

    #[test]
    fn the_interface_scale_makes_the_list_taller() {
        assert!(place(100.0, 120.0, 600.0, 1.0).rows == 9);
        assert!(place(100.0, 120.0, 600.0, 1.5).rows == 4);
    }

    #[test]
    fn the_height_is_fixed_by_the_rows_and_the_scale() {
        let full = place(100.0, 120.0, 900.0, 1.0).height(1.0);
        let short = place(20.0, 40.0, 400.0, 1.0).height(1.0);
        assert_eq!(full - short, 5.0 * 24.0);
    }
}
