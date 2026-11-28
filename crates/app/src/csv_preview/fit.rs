use super::columns::SAMPLE_ROWS;

pub const DIALOG_HEIGHT: f32 = 696.0;
pub const ROW_HEIGHT: f32 = 22.0;
pub const MIN_ROWS: usize = 4;
const TALL_WINDOW: f32 = 768.0;
const TOP_TALL: f32 = 48.0;
const TOP_SHORT: f32 = 24.0;
const BOTTOM: f32 = 24.0;

// How the dialog sits in the window: the distance from the top, its height, and how many
// sample rows it shows. Sizes are in pixels; `scale` is the interface scale (1.0 at 100%).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub top: f32,
    pub height: f32,
    pub rows: usize,
    // The part between the title and the footer is taller than what is left for it.
    pub scrolls: bool,
}

fn height_with(rows: usize, scale: f32) -> f32 {
    (DIALOG_HEIGHT - (SAMPLE_ROWS - rows) as f32 * ROW_HEIGHT) * scale
}

// Sample rows shrink first, down to MIN_ROWS; when that is not enough the dialog takes all the
// window leaves and the part between the title and the footer scrolls.
pub fn fit(window_height: f32, scale: f32) -> Fit {
    let top = if window_height >= TALL_WINDOW {
        TOP_TALL
    } else {
        TOP_SHORT
    };
    let room = (window_height - top - BOTTOM).max(0.0);
    let missing = height_with(SAMPLE_ROWS, scale) - room;
    let removed = if missing > 0.0 {
        ((missing / (ROW_HEIGHT * scale)).ceil() as usize).min(SAMPLE_ROWS - MIN_ROWS)
    } else {
        0
    };
    let rows = SAMPLE_ROWS - removed;
    let natural = height_with(rows, scale);
    Fit {
        top,
        height: natural.min(room),
        rows,
        scrolls: natural > room,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tall_window_shows_every_row_48_pixels_from_the_top() {
        let tall = fit(800.0, 1.0);
        assert_eq!(tall.top, 48.0);
        assert_eq!(tall.height, 696.0);
        assert_eq!(tall.rows, 12);
        assert!(!tall.scrolls);
        assert_eq!(fit(768.0, 1.0).rows, 12);
    }

    #[test]
    fn a_window_a_little_short_drops_sample_rows_first() {
        let fitted = fit(700.0, 1.0);
        assert_eq!(fitted.top, 24.0);
        assert_eq!(fitted.height, 652.0);
        assert_eq!(fitted.rows, 10);
        assert!(!fitted.scrolls);
        assert_eq!(fit(767.0, 1.0).top, 24.0);
    }

    #[test]
    fn a_window_too_short_for_four_rows_scrolls_the_middle() {
        let fitted = fit(520.0, 1.0);
        assert_eq!(fitted.top, 24.0);
        assert_eq!(fitted.height, 472.0);
        assert_eq!(fitted.rows, 4);
        assert!(fitted.scrolls);
    }

    #[test]
    fn a_larger_interface_scale_takes_more_room() {
        let fitted = fit(1000.0, 1.25);
        assert_eq!(fitted.rows, 12);
        assert_eq!(fitted.height, 870.0);
        let squeezed = fit(800.0, 1.5);
        assert!(squeezed.rows < 12 && squeezed.rows >= MIN_ROWS);
        assert!(squeezed.height <= 800.0 - 48.0 - 24.0);
    }
}
