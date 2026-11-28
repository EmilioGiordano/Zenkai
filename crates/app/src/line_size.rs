use zenkai_types::{
    ColIdx, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, Language, LineAxis, MAX_COLS, MAX_ROWS, Range,
    SheetSizes,
};

const POINTS_PER_PIXEL: f64 = 0.75;
const PIXELS_PER_CHARACTER: f64 = 7.0;
const COLUMN_PADDING_PIXELS: f64 = 5.0;
pub const MAX_POINTS: f64 = 409.5;
pub const MAX_CHARACTERS: f64 = 255.0;
// A size above zero never rounds down to a line that is not there: one pixel of height,
// one pixel of room inside a column.
const MIN_ROW_PIXELS: f64 = 1.0;
const MIN_COLUMN_PIXELS: f64 = COLUMN_PADDING_PIXELS + 1.0;

pub fn limit(axis: LineAxis) -> f64 {
    match axis {
        LineAxis::Row => MAX_POINTS,
        LineAxis::Column => MAX_CHARACTERS,
    }
}

// Excel's units: points for a row, characters of the default font for a column.
pub fn display_value(axis: LineAxis, pixels: f32) -> f64 {
    let pixels = f64::from(pixels);
    match axis {
        LineAxis::Row => pixels * POINTS_PER_PIXEL,
        LineAxis::Column => (pixels - COLUMN_PADDING_PIXELS) / PIXELS_PER_CHARACTER,
    }
}

// Excel stores whole pixels: a row is a multiple of 0.75 points, a column a whole number
// of pixels.
fn snapped_pixels(axis: LineAxis, value: f64) -> f32 {
    let pixels = match axis {
        LineAxis::Row => (value / POINTS_PER_PIXEL).round().max(MIN_ROW_PIXELS),
        LineAxis::Column => (value * PIXELS_PER_CHARACTER + COLUMN_PADDING_PIXELS)
            .round()
            .max(MIN_COLUMN_PIXELS),
    };
    pixels as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Separator {
    Dot,
    Comma,
}

impl Separator {
    pub fn of(language: Language) -> Separator {
        match language {
            Language::English => Separator::Dot,
            Language::Spanish => Separator::Comma,
        }
    }

    fn symbol(self) -> char {
        match self {
            Separator::Dot => '.',
            Separator::Comma => ',',
        }
    }
}

// Up to two decimals, with the zeros after the last digit dropped: 15, 20.25, 8.43.
pub fn format_number(value: f64, separator: Separator) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0')
        .trim_end_matches('.')
        .replace('.', &separator.symbol().to_string())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Hidden,
    Pixels(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry {
    Unchanged,
    Size(Size),
}

#[derive(Debug, PartialEq, Eq)]
pub struct OutOfRange;

// "1,000" typed in English is a thousand to anyone who writes it that way, and "1.000" in
// Spanish likewise; reading either as one would silently give a line a size it was not
// meant to have, so it is refused like any other number out of range.
fn number_of(text: &str, native: Separator) -> Option<f64> {
    let mut parts = text.split(['.', ',']);
    let whole = parts.next()?;
    let fraction = parts.next();
    if parts.next().is_some() {
        return None;
    }
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || !fraction.is_none_or(digits) {
        return None;
    }
    let fraction_digits = fraction.map(str::len);
    if whole.is_empty() && fraction_digits.is_none_or(|len| len == 0) {
        return None;
    }
    let used = text
        .find(['.', ','])
        .and_then(|at| text[at..].chars().next());
    if used.is_some_and(|used| used != native.symbol()) && fraction_digits == Some(3) {
        return None;
    }
    let whole = if whole.is_empty() { "0" } else { whole };
    format!(
        "{whole}.{}",
        fraction.filter(|part| !part.is_empty()).unwrap_or("0")
    )
    .parse()
    .ok()
}

pub fn parse(axis: LineAxis, text: &str, separator: Separator) -> Result<Entry, OutOfRange> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Entry::Unchanged);
    }
    let value = number_of(text, separator).ok_or(OutOfRange)?;
    if !(0.0..=limit(axis)).contains(&value) {
        return Err(OutOfRange);
    }
    Ok(Entry::Size(if value == 0.0 {
        Size::Hidden
    } else {
        Size::Pixels(snapped_pixels(axis, value))
    }))
}

// The rows or the columns a selection spans, by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lines {
    pub axis: LineAxis,
    pub first: u32,
    pub last: u32,
}

impl Lines {
    pub fn single(axis: LineAxis, index: u32) -> Lines {
        Lines {
            axis,
            first: index,
            last: index,
        }
    }

    pub fn of(axis: LineAxis, selection: Range) -> Lines {
        let (first, last) = match axis {
            LineAxis::Row => (selection.start.row.get(), selection.end.row.get()),
            LineAxis::Column => (
                u32::from(selection.start.col.get()),
                u32::from(selection.end.col.get()),
            ),
        };
        Lines { axis, first, last }
    }

    pub fn count(self) -> u32 {
        self.last - self.first + 1
    }

    // Every line of the sheet: the size becomes the sheet's own instead of one entry each.
    pub fn is_all(self) -> bool {
        match self.axis {
            LineAxis::Row => self.count() == MAX_ROWS,
            LineAxis::Column => self.count() == u32::from(MAX_COLS),
        }
    }

    pub fn target(self) -> String {
        let name = |index: u32| match self.axis {
            LineAxis::Row => (index + 1).to_string(),
            LineAxis::Column => ColIdx::clamped(i64::from(index)).letters(),
        };
        if self.first == self.last {
            name(self.first)
        } else {
            format!("{}:{}", name(self.first), name(self.last))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prefill {
    Uniform(f32),
    Mixed,
    Hidden,
}

// The sizes of the lines with an entry, so a selection of a million lines is judged by
// the few that differ from the sheet's own.
#[derive(Default)]
struct Seen {
    first: Option<f32>,
    mixed: bool,
    covered: u64,
}

impl Seen {
    fn add(&mut self, pixels: f32, lines: u64) {
        self.covered += lines;
        match self.first {
            None => self.first = Some(pixels),
            Some(first) if first != pixels => self.mixed = true,
            Some(_) => {}
        }
    }
}

impl Prefill {
    pub fn of(lines: Lines, sizes: &SheetSizes) -> Prefill {
        let default = match lines.axis {
            LineAxis::Row => sizes.default_row_height.unwrap_or(DEFAULT_ROW_HEIGHT),
            LineAxis::Column => sizes.default_column_width.unwrap_or(DEFAULT_COL_WIDTH),
        };
        let mut seen = Seen::default();
        match lines.axis {
            LineAxis::Row => {
                for (row, height) in &sizes.rows {
                    if (lines.first..=lines.last).contains(&row.get()) {
                        seen.add(*height, 1);
                    }
                }
            }
            LineAxis::Column => {
                for span in &sizes.columns {
                    let first = u32::from(span.first.get()).max(lines.first);
                    let last = u32::from(span.last.get()).min(lines.last);
                    if first <= last {
                        seen.add(span.width, u64::from(last - first) + 1);
                    }
                }
            }
        }
        if seen.covered < u64::from(lines.count()) {
            seen.add(default, 0);
        }
        match (seen.mixed, seen.first) {
            (true, _) => Prefill::Mixed,
            (false, Some(pixels)) if pixels <= 0.0 => Prefill::Hidden,
            (false, Some(pixels)) => Prefill::Uniform(pixels),
            (false, None) => Prefill::Uniform(default),
        }
    }

    pub fn text(self, axis: LineAxis, separator: Separator) -> String {
        match self {
            Prefill::Uniform(pixels) => format_number(display_value(axis, pixels), separator),
            Prefill::Hidden => "0".to_string(),
            Prefill::Mixed => String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    OutOfRange,
    EveryLineHidden,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Close,
    Apply(Size),
    Refuse(Refusal),
}

pub fn outcome(lines: Lines, text: &str, separator: Separator) -> Outcome {
    match parse(lines.axis, text, separator) {
        Err(OutOfRange) => Outcome::Refuse(Refusal::OutOfRange),
        Ok(Entry::Unchanged) => Outcome::Close,
        Ok(Entry::Size(Size::Hidden)) if lines.is_all() => {
            Outcome::Refuse(Refusal::EveryLineHidden)
        }
        Ok(Entry::Size(size)) => Outcome::Apply(size),
    }
}

// Only zeros, with a separator and more zeros after it at most: "0." and "0,5" are on
// their way to another number and say nothing yet.
fn is_zero_text(text: &str) -> bool {
    let (whole, fraction) = match text.trim().split_once(['.', ',']) {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text.trim(), None),
    };
    let zeros = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte == b'0');
    zeros(whole) && fraction.is_none_or(zeros)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Hint,
    ZeroNote,
    HiddenNote,
    Refusal(Refusal),
    EveryLineHiddenNote,
}

impl Slot {
    // After a refused OK the slot shows what was wrong until the next keystroke, with the
    // alert cues; before it, the same refusal of every line is plain text.
    pub fn of(
        lines: Lines,
        prefill: Prefill,
        prefill_text: &str,
        text: &str,
        refused: Option<Refusal>,
    ) -> Slot {
        if let Some(refusal) = refused {
            return Slot::Refusal(refusal);
        }
        if prefill == Prefill::Hidden && text == prefill_text {
            Slot::HiddenNote
        } else if is_zero_text(text) {
            if lines.is_all() {
                Slot::EveryLineHiddenNote
            } else {
                Slot::ZeroNote
            }
        } else {
            Slot::Hint
        }
    }

    pub fn is_alert(self) -> bool {
        matches!(self, Slot::Refusal(_))
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::{CellPos, ColumnSpan, RowIdx};

    use super::{
        Entry, Lines, MAX_CHARACTERS, MAX_POINTS, OutOfRange, Outcome, Prefill, Refusal, Separator,
        Size, Slot, display_value, format_number, outcome, parse,
    };
    use zenkai_types::{ColIdx, Language, LineAxis, MAX_COLS, MAX_ROWS, Range, SheetSizes};

    const EN: Separator = Separator::Dot;
    const ES: Separator = Separator::Comma;

    fn rows(text: &str, separator: Separator) -> Result<Entry, OutOfRange> {
        parse(LineAxis::Row, text, separator)
    }

    fn columns(text: &str, separator: Separator) -> Result<Entry, OutOfRange> {
        parse(LineAxis::Column, text, separator)
    }

    fn pixels(entry: Result<Entry, OutOfRange>) -> f32 {
        match entry {
            Ok(Entry::Size(Size::Pixels(pixels))) => pixels,
            other => panic!("not a size: {other:?}"),
        }
    }

    #[test]
    fn rows_show_points_and_columns_show_characters() {
        assert_eq!(display_value(LineAxis::Row, 20.0), 15.0);
        assert_eq!(display_value(LineAxis::Row, 546.0), MAX_POINTS);
        assert_eq!(
            format_number(display_value(LineAxis::Column, 64.0), EN),
            "8.43"
        );
        assert_eq!(
            format_number(display_value(LineAxis::Column, 145.0), EN),
            "20"
        );
        assert_eq!(
            format_number(display_value(LineAxis::Row, 27.0), EN),
            "20.25"
        );
        assert_eq!(format_number(display_value(LineAxis::Row, 20.0), EN), "15");
    }

    #[test]
    fn numbers_are_shown_with_the_interface_decimal_separator() {
        assert_eq!(format_number(8.428_571, ES), "8,43");
        assert_eq!(format_number(409.5, ES), "409,5");
        assert_eq!(format_number(409.5, EN), "409.5");
        assert_eq!(format_number(0.0, EN), "0");
        assert_eq!(format_number(100.0, ES), "100");
    }

    #[test]
    fn a_row_snaps_to_whole_pixels_which_are_multiples_of_three_quarters_of_a_point() {
        assert_eq!(pixels(rows("20", EN)), 27.0);
        assert_eq!(display_value(LineAxis::Row, 27.0), 20.25);
        assert_eq!(pixels(rows("15", EN)), 20.0);
        assert_eq!(pixels(rows("409.5", EN)), 546.0);
    }

    #[test]
    fn a_row_above_zero_never_rounds_down_to_hidden() {
        for text in ["0.01", "0.3", "0.74", "0.75"] {
            assert_eq!(pixels(rows(text, EN)), 1.0, "{text}");
        }
        assert_eq!(display_value(LineAxis::Row, 1.0), 0.75);
    }

    #[test]
    fn a_column_snaps_to_whole_pixels() {
        assert_eq!(pixels(columns("10.5", EN)), 79.0);
        assert_eq!(
            format_number(display_value(LineAxis::Column, 79.0), EN),
            "10.57"
        );
        assert_eq!(pixels(columns("8.43", EN)), 64.0);
        assert_eq!(pixels(columns("255", EN)), 1790.0);
        assert_eq!(pixels(columns("0.01", EN)), 6.0);
    }

    #[test]
    fn both_separators_are_read_whatever_the_language() {
        assert_eq!(pixels(rows("20.25", EN)), pixels(rows("20,25", EN)));
        assert_eq!(pixels(rows("20.25", ES)), pixels(rows("20,25", ES)));
        assert_eq!(pixels(rows(".75", EN)), 1.0);
        assert_eq!(pixels(rows("15.", ES)), 20.0);
        assert_eq!(rows("0,5", ES), rows("0.5", ES));
    }

    #[test]
    fn exactly_three_digits_after_the_other_separator_are_refused() {
        assert_eq!(rows("1,000", EN), Err(OutOfRange));
        assert_eq!(rows("1.000", ES), Err(OutOfRange));
        assert_eq!(columns("2,500", EN), Err(OutOfRange));
        assert_eq!(pixels(rows("1.500", EN)), pixels(rows("1.5", EN)));
        assert_eq!(pixels(columns("1,500", ES)), pixels(columns("1.5", EN)));
        assert_eq!(pixels(rows("1,50", EN)), pixels(rows("1.5", EN)));
        assert_eq!(pixels(rows("1,5", EN)), pixels(rows("1.5", EN)));
        assert_eq!(pixels(rows("1,5000", EN)), pixels(rows("1.5", EN)));
    }

    #[test]
    fn zero_hides_and_empty_changes_nothing() {
        assert_eq!(rows("0", EN), Ok(Entry::Size(Size::Hidden)));
        assert_eq!(rows("0.0", EN), Ok(Entry::Size(Size::Hidden)));
        assert_eq!(rows("  ", EN), Ok(Entry::Unchanged));
        assert_eq!(rows("", ES), Ok(Entry::Unchanged));
    }

    #[test]
    fn a_size_outside_zero_to_the_limit_is_out_of_range() {
        assert!(rows("409.5", EN).is_ok());
        assert_eq!(rows("409.6", EN), Err(OutOfRange));
        assert_eq!(rows("500", EN), Err(OutOfRange));
        assert_eq!(rows("-1", EN), Err(OutOfRange));
        assert!(columns(&MAX_CHARACTERS.to_string(), EN).is_ok());
        assert_eq!(columns("255.01", EN), Err(OutOfRange));
        assert_eq!(columns("300", ES), Err(OutOfRange));
    }

    #[test]
    fn text_that_is_not_a_plain_number_is_out_of_range() {
        for text in [
            "wide", "15pt", "1e2", "--1", "+5", "1 5", ".", ",", "1.2.3", "1,2,3", "1.2,3", "0x10",
            "٣",
        ] {
            assert_eq!(rows(text, EN), Err(OutOfRange), "{text}");
        }
    }

    fn span(first: u16, last: u16, width: f32) -> ColumnSpan {
        ColumnSpan {
            first: ColIdx::clamped(i64::from(first)),
            last: ColIdx::clamped(i64::from(last)),
            width,
        }
    }

    fn row_lines(first: u32, last: u32) -> Lines {
        Lines {
            axis: LineAxis::Row,
            first,
            last,
        }
    }

    fn column_lines(first: u32, last: u32) -> Lines {
        Lines {
            axis: LineAxis::Column,
            first,
            last,
        }
    }

    fn row_sizes(entries: &[(u32, f32)]) -> SheetSizes {
        SheetSizes {
            rows: entries
                .iter()
                .map(|(row, height)| (RowIdx::clamped(i64::from(*row)), *height))
                .collect(),
            ..SheetSizes::default()
        }
    }

    #[test]
    fn lines_with_the_same_size_prefill_it_and_the_sheet_default_counts() {
        let sizes = row_sizes(&[(3, 40.0), (4, 40.0)]);
        assert_eq!(Prefill::of(row_lines(3, 4), &sizes), Prefill::Uniform(40.0));
        assert_eq!(Prefill::of(row_lines(7, 9), &sizes), Prefill::Uniform(20.0));
        assert_eq!(Prefill::of(row_lines(2, 4), &sizes), Prefill::Mixed);
        let wide = SheetSizes {
            default_row_height: Some(30.0),
            ..sizes
        };
        assert_eq!(Prefill::of(row_lines(7, 9), &wide), Prefill::Uniform(30.0));
    }

    #[test]
    fn different_sizes_prefill_nothing() {
        let sizes = row_sizes(&[(3, 40.0), (4, 25.0)]);
        assert_eq!(Prefill::of(row_lines(3, 4), &sizes), Prefill::Mixed);
        assert_eq!(Prefill::Mixed.text(LineAxis::Row, EN), "");
    }

    #[test]
    fn hidden_lines_prefill_zero_only_when_every_one_is_hidden() {
        let sizes = row_sizes(&[(3, 0.0), (4, 0.0)]);
        assert_eq!(Prefill::of(row_lines(3, 4), &sizes), Prefill::Hidden);
        assert_eq!(Prefill::Hidden.text(LineAxis::Row, EN), "0");
        assert_eq!(Prefill::of(row_lines(3, 5), &sizes), Prefill::Mixed);
    }

    #[test]
    fn column_spans_count_for_each_column_they_cover() {
        let sizes = SheetSizes {
            columns: vec![span(1, 3, 100.0)],
            ..SheetSizes::default()
        };
        assert_eq!(
            Prefill::of(column_lines(1, 3), &sizes),
            Prefill::Uniform(100.0)
        );
        assert_eq!(Prefill::of(column_lines(0, 3), &sizes), Prefill::Mixed);
        assert_eq!(
            Prefill::of(column_lines(2, 2), &sizes).text(LineAxis::Column, ES),
            "13,57"
        );
        let hidden = SheetSizes {
            columns: vec![span(2, 4, 0.0)],
            ..SheetSizes::default()
        };
        assert_eq!(Prefill::of(column_lines(2, 4), &hidden), Prefill::Hidden);
    }

    #[test]
    fn a_selection_of_every_line_is_judged_without_walking_them() {
        let sizes = row_sizes(&[(3, 40.0)]);
        let all = Lines::of(
            LineAxis::Row,
            Range::new(
                CellPos::default(),
                CellPos::new(RowIdx::LAST, ColIdx::default()),
            ),
        );
        assert!(all.is_all());
        assert_eq!(all.count(), MAX_ROWS);
        assert_eq!(Prefill::of(all, &sizes), Prefill::Mixed);
        assert_eq!(
            Prefill::of(all, &SheetSizes::default()),
            Prefill::Uniform(20.0)
        );
        let columns = Lines::of(
            LineAxis::Column,
            Range::new(
                CellPos::default(),
                CellPos::new(RowIdx::default(), ColIdx::LAST),
            ),
        );
        assert!(columns.is_all());
        assert_eq!(columns.count(), u32::from(MAX_COLS));
        assert!(!row_lines(0, MAX_ROWS - 2).is_all());
    }

    #[test]
    fn the_scope_names_the_lines() {
        assert_eq!(row_lines(6, 6).target(), "7");
        assert_eq!(row_lines(3, 5).target(), "4:6");
        assert_eq!(column_lines(2, 2).target(), "C");
        assert_eq!(column_lines(1, 3).target(), "B:D");
    }

    #[test]
    fn ok_decides_from_the_text_and_the_lines() {
        let some = row_lines(3, 5);
        let all = row_lines(0, MAX_ROWS - 1);
        assert_eq!(outcome(some, "", EN), Outcome::Close);
        assert_eq!(outcome(some, "0", EN), Outcome::Apply(Size::Hidden));
        assert_eq!(outcome(some, "15", EN), Outcome::Apply(Size::Pixels(20.0)));
        assert_eq!(
            outcome(some, "500", EN),
            Outcome::Refuse(Refusal::OutOfRange)
        );
        assert_eq!(outcome(all, "20", EN), Outcome::Apply(Size::Pixels(27.0)));
        assert_eq!(
            outcome(all, "0", EN),
            Outcome::Refuse(Refusal::EveryLineHidden)
        );
        assert_eq!(
            outcome(column_lines(0, u32::from(MAX_COLS) - 1), "0,0", ES),
            Outcome::Refuse(Refusal::EveryLineHidden)
        );
    }

    fn slot(lines: Lines, prefill: Prefill, text: &str, refused: Option<Refusal>) -> Slot {
        let prefill_text = prefill.text(lines.axis, EN);
        Slot::of(lines, prefill, &prefill_text, text, refused)
    }

    #[test]
    fn the_slot_explains_a_zero_only_when_the_field_is_exactly_zero() {
        let lines = row_lines(3, 5);
        let uniform = Prefill::Uniform(20.0);
        for text in ["0", "00", "0.0", "0,00"] {
            assert_eq!(slot(lines, uniform, text, None), Slot::ZeroNote, "{text}");
        }
        for text in ["", "15", "0.", "0,", "0,5", "0.05", ".0", "0.0.0", "0x"] {
            assert_eq!(slot(lines, uniform, text, None), Slot::Hint, "{text}");
        }
    }

    #[test]
    fn hidden_lines_explain_how_to_show_them_until_the_field_is_edited() {
        let lines = row_lines(3, 4);
        assert_eq!(slot(lines, Prefill::Hidden, "0", None), Slot::HiddenNote);
        assert_eq!(slot(lines, Prefill::Hidden, "00", None), Slot::ZeroNote);
        assert_eq!(slot(lines, Prefill::Hidden, "15", None), Slot::Hint);
        assert_eq!(slot(lines, Prefill::Hidden, "", None), Slot::Hint);
    }

    #[test]
    fn every_line_with_zero_shows_the_refusal_plainly_and_without_the_zero_note() {
        let all = row_lines(0, MAX_ROWS - 1);
        let plain = slot(all, Prefill::Uniform(20.0), "0", None);
        assert_eq!(plain, Slot::EveryLineHiddenNote);
        assert!(!plain.is_alert());
        let refused = slot(
            all,
            Prefill::Uniform(20.0),
            "0",
            Some(Refusal::EveryLineHidden),
        );
        assert!(refused.is_alert());
    }

    #[test]
    fn a_refused_ok_shows_its_reason_as_an_alert() {
        let lines = row_lines(3, 5);
        let refused = slot(
            lines,
            Prefill::Uniform(20.0),
            "500",
            Some(Refusal::OutOfRange),
        );
        assert_eq!(refused, Slot::Refusal(Refusal::OutOfRange));
        assert!(refused.is_alert());
        assert!(!Slot::Hint.is_alert());
    }

    #[test]
    fn the_interface_language_picks_the_separator() {
        assert_eq!(Separator::of(Language::English), Separator::Dot);
        assert_eq!(Separator::of(Language::Spanish), Separator::Comma);
    }
}
