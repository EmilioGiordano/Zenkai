// Excel stores a column width, `<col width>` and `defaultColWidth`, with the padding of the
// cell already in it, in units of the widest digit of the default font (7 pixels): 20
// characters are stored as 20.7109375 and drawn 145 pixels wide.
use std::collections::BTreeMap;

const DIGIT_PIXELS: f64 = 7.0;
const PADDING_PIXELS: f64 = 5.0;
// The width Excel's dialog shows is in characters: the pixels without the padding.
pub(crate) const MAX_PIXELS: f64 = 255.0 * DIGIT_PIXELS + PADDING_PIXELS;
// Excel gives a column with no width of its own a whole number of 8 pixel steps.
const DEFAULT_STEP_PIXELS: f64 = 8.0;

// What the widest column, 255 characters, is stored as; it is the most `<col width>` and
// `defaultColWidth` can validly say.
pub(crate) const MAX_STORED_WIDTH: f64 = 255.710_937_5;

// Where entries overlap, the first one decides, as IronCalc's lookup does. The pieces come
// out disjoint, so a file that repeats a full-width entry thousands of times costs one
// piece, not one visit per column of each.
pub(crate) fn first_wins<T: Copy>(
    entries: impl IntoIterator<Item = (i32, i32, T)>,
) -> Vec<(i32, i32, T)> {
    let mut covered: BTreeMap<i32, i32> = BTreeMap::new();
    let mut pieces = Vec::new();
    for (first, last, value) in entries {
        if first > last {
            continue;
        }
        let mut touching: Vec<(i32, i32)> = covered
            .range(..=last + 1)
            .rev()
            .take_while(|(_, end)| **end >= first - 1)
            .map(|(start, end)| (*start, *end))
            .collect();
        touching.reverse();
        let (mut cursor, mut union_first, mut union_last) = (first, first, last);
        for (start, end) in touching {
            covered.remove(&start);
            union_first = union_first.min(start);
            union_last = union_last.max(end);
            if start > cursor {
                pieces.push((cursor, start - 1, value));
            }
            cursor = cursor.max(end + 1);
        }
        if cursor <= last {
            pieces.push((cursor, last, value));
        }
        covered.insert(union_first, union_last);
    }
    pieces
}

// The stored width of a column `pixels` wide: whole pixels, then truncated to 1/256 as Excel
// does. A drag or an AutoFit gives fractions of a pixel.
pub(crate) fn stored_width(pixels: f64) -> f64 {
    if !pixels.is_finite() {
        return 0.0;
    }
    (pixels.round().clamp(0.0, MAX_PIXELS) / DIGIT_PIXELS * 256.0).floor() / 256.0
}

// The pixels Excel draws a stored width at. A file can say anything: what is not a width
// is hidden, and what is too wide is the widest column.
pub(crate) fn pixels_of(width: f64) -> f32 {
    if !width.is_finite() || width <= 0.0 {
        return 0.0;
    }
    let half_digit = (128.0 / DIGIT_PIXELS).floor();
    let pixels =
        (((256.0 * width.min(MAX_STORED_WIDTH) + half_digit) / 256.0) * DIGIT_PIXELS).floor();
    pixels.min(MAX_PIXELS) as f32
}

// What a column with no `<col>` is stored as when the sheet gives only baseColWidth.
pub(crate) fn default_stored_width(base_characters: f64) -> f64 {
    let pixels = ((base_characters * DIGIT_PIXELS + PADDING_PIXELS) / DEFAULT_STEP_PIXELS).ceil()
        * DEFAULT_STEP_PIXELS;
    stored_width(pixels)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PIXELS, MAX_STORED_WIDTH, default_stored_width, first_wins, pixels_of, stored_width,
    };

    #[test]
    fn twenty_characters_are_stored_as_excel_stores_them_and_drawn_145_pixels_wide() {
        assert_eq!(stored_width(145.0), 20.710_937_5);
        assert_eq!(pixels_of(20.710_937_5), 145.0);
    }

    #[test]
    fn the_default_column_is_64_pixels_and_stored_9_140625() {
        assert_eq!(stored_width(64.0), 9.140_625);
        assert_eq!(pixels_of(9.140_625), 64.0);
        assert_eq!(default_stored_width(8.0), 9.140_625);
    }

    #[test]
    fn a_width_survives_the_trip_through_its_pixels() {
        for pixels in 0..=1790 {
            let pixels = f64::from(pixels);
            assert_eq!(f64::from(pixels_of(stored_width(pixels))), pixels);
        }
    }

    #[test]
    fn the_widest_column_is_255_characters_and_is_stored_255_7109375() {
        assert_eq!(pixels_of(stored_width(5000.0)), 1790.0);
        assert_eq!(stored_width(-3.0), 0.0);
        assert_eq!(stored_width(MAX_PIXELS), MAX_STORED_WIDTH);
    }

    #[test]
    fn a_fraction_of_a_pixel_snaps_to_the_whole_pixel_first() {
        assert_eq!(stored_width(64.4), stored_width(64.0));
        assert_eq!(stored_width(64.5), stored_width(65.0));
        assert_eq!(pixels_of(stored_width(64.5)), 65.0);
    }

    #[test]
    fn what_a_file_says_that_is_no_width_never_reaches_the_pixels() {
        assert_eq!(pixels_of(f64::NAN), 0.0);
        assert_eq!(pixels_of(f64::INFINITY), 0.0);
        assert_eq!(pixels_of(-4.0), 0.0);
        assert_eq!(pixels_of(1.0e-300), 0.0);
        assert_eq!(pixels_of(1.0e308), 1790.0);
        assert_eq!(stored_width(f64::NAN), 0.0);
    }

    #[test]
    fn widths_written_by_other_programs_draw_at_excels_pixels() {
        for (width, pixels) in [(13.710_937_5, 96.0), (8.43, 59.0), (38.269_531_25, 268.0)] {
            assert_eq!(pixels_of(width), pixels, "{width}");
        }
    }

    #[test]
    fn overlapping_entries_give_the_columns_to_the_first_that_names_them() {
        let pieces = first_wins([(3, 5, 'a'), (1, 8, 'b'), (7, 9, 'c'), (4, 4, 'd')]);
        assert_eq!(pieces, [(3, 5, 'a'), (1, 2, 'b'), (6, 8, 'b'), (9, 9, 'c')]);
    }

    #[test]
    fn a_full_width_entry_repeated_many_times_is_one_piece() {
        let pieces = first_wins((0..20_000).map(|n| (1, 16_384, n)));
        assert_eq!(pieces, [(1, 16_384, 0)]);
    }

    #[test]
    fn entries_that_touch_or_miss_each_other_stay_apart() {
        let pieces = first_wins([(1, 2, 'a'), (3, 4, 'b'), (10, 12, 'c'), (11, 11, 'd')]);
        assert_eq!(pieces, [(1, 2, 'a'), (3, 4, 'b'), (10, 12, 'c')]);
    }
}
