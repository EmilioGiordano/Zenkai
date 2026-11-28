use std::collections::{BTreeMap, HashMap, HashSet};

use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{
    ColIdx, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, RowIdx, SheetId, SheetSizes, ValueKind,
};

use crate::line_size::Lines;

pub const MAX_AUTOFIT_CELLS: usize = 100_000;
pub const AUTOFIT_CHAR_WIDTH: f32 = 7.5;
const CELL_PADDING: f32 = 12.0;
const MIN_FITTED_WIDTH: f32 = 24.0;
const MAX_FITTED_WIDTH: f32 = 600.0;
const LINE_HEIGHT: f32 = 19.0;
const ROW_PADDING: f32 = 4.0;
const MAX_FITTED_LINES: f32 = 20.0;

// The line count is estimated from the text length and the column width. Breaking at word
// boundaries wastes part of each line.
pub fn wrapped_lines(text: &str, column_width: f32) -> f32 {
    let per_line = ((column_width - CELL_PADDING) / AUTOFIT_CHAR_WIDTH).max(1.0);
    text.split('\n')
        .map(|part| {
            (part.chars().count() as f32 * 1.3 / per_line)
                .ceil()
                .max(1.0)
        })
        .sum()
}

pub fn height_of_lines(lines: f32) -> f32 {
    lines.min(MAX_FITTED_LINES) * LINE_HEIGHT + ROW_PADDING
}

pub fn column_width_of(characters: usize) -> f32 {
    (characters as f32 * AUTOFIT_CHAR_WIDTH + CELL_PADDING)
        .clamp(MIN_FITTED_WIDTH, MAX_FITTED_WIDTH)
}

// What the sheet's sizes say about its lines, looked up without walking them again for
// every cell.
struct Measures {
    widths: HashMap<ColIdx, f32>,
    default_width: f32,
    row_entries: HashSet<RowIdx>,
    hidden_rows: HashSet<RowIdx>,
    default_height: f32,
}

impl Measures {
    fn of(sizes: &SheetSizes) -> Measures {
        let mut widths = HashMap::new();
        for span in &sizes.columns {
            for col in span.first.get()..=span.last.get() {
                widths.insert(ColIdx::clamped(i64::from(col)), span.width);
            }
        }
        Measures {
            widths,
            default_width: sizes.default_column_width.unwrap_or(DEFAULT_COL_WIDTH),
            row_entries: sizes.rows.iter().map(|(row, _)| *row).collect(),
            hidden_rows: sizes
                .rows
                .iter()
                .filter(|(_, height)| *height <= 0.0)
                .map(|(row, _)| *row)
                .collect(),
            default_height: sizes.default_row_height.unwrap_or(DEFAULT_ROW_HEIGHT),
        }
    }

    fn width(&self, col: ColIdx) -> f32 {
        self.widths.get(&col).copied().unwrap_or(self.default_width)
    }
}

// Each column of the range gets the width of its longest displayed text. A column with no
// cells or hidden stays as it is. Only the first MAX_AUTOFIT_CELLS cells of the range are
// read.
pub fn fit_columns(wb: &mut Workbook, sheet: SheetId, columns: Lines) -> Result<(), EngineError> {
    let measures = Measures::of(&wb.sizes(sheet));
    let mut longest: BTreeMap<ColIdx, usize> = BTreeMap::new();
    for pos in wb.filled_cells_within(sheet, columns.range(), MAX_AUTOFIT_CELLS) {
        let characters = wb.cell(sheet, pos).text.chars().count();
        let entry = longest.entry(pos.col).or_insert(0);
        *entry = (*entry).max(characters);
    }
    let runs: Vec<(ColIdx, ColIdx, f32)> = longest
        .into_iter()
        .filter(|(col, _)| measures.width(*col) > 0.0)
        .map(|(col, characters)| (col, col, column_width_of(characters)))
        .collect();
    wb.set_column_widths(sheet, &runs)
}

// Each row of the range gets the height of its tallest wrapped cell, or the sheet's own
// row height when nothing wraps. A row with no cells or hidden stays as it is, and a cell
// in a hidden column does not count.
pub fn fit_rows(wb: &mut Workbook, sheet: SheetId, rows: Lines) -> Result<(), EngineError> {
    let measures = Measures::of(&wb.sizes(sheet));
    let mut tallest: BTreeMap<RowIdx, f32> = BTreeMap::new();
    for pos in wb.filled_cells_within(sheet, rows.range(), MAX_AUTOFIT_CELLS) {
        let width = measures.width(pos.col);
        if width <= 0.0 {
            continue;
        }
        let view = wb.cell(sheet, pos);
        let lines = if view.style.wrap && view.kind == ValueKind::Text {
            wrapped_lines(&view.text, width)
        } else {
            1.0
        };
        let entry = tallest.entry(pos.row).or_insert(1.0);
        *entry = entry.max(lines);
    }
    let heights: Vec<(RowIdx, f32)> = tallest
        .into_iter()
        .filter(|(row, _)| !measures.hidden_rows.contains(row))
        .map(|(row, lines)| {
            let height = if lines > 1.0 {
                height_of_lines(lines)
            } else {
                measures.default_height
            };
            (row, height)
        })
        .filter(|(row, height)| {
            measures.row_entries.contains(row) || *height != measures.default_height
        })
        .collect();
    wb.set_row_heights(sheet, &heights)
}

#[cfg(test)]
mod tests {
    use zenkai_engine::{Engine, Workbook};
    use zenkai_types::{
        CellPos, ColIdx, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, LineAxis, Range, RowIdx, SheetId,
        StyleChange,
    };

    use super::{
        AUTOFIT_CHAR_WIDTH, column_width_of, fit_columns, fit_rows, height_of_lines, wrapped_lines,
    };
    use crate::line_size::Lines;

    const SHEET: SheetId = SheetId(0);

    fn at(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    fn lines(axis: LineAxis, first: u32, last: u32) -> Lines {
        Lines { axis, first, last }
    }

    fn widths(book: &Workbook) -> Vec<(u16, f32)> {
        let mut widths: Vec<_> = book
            .sizes(SHEET)
            .columns
            .iter()
            .map(|span| (span.first.get(), span.width))
            .collect();
        widths.sort_by_key(|(col, _)| *col);
        widths
    }

    fn heights(book: &Workbook) -> Vec<(u32, f32)> {
        let mut heights: Vec<_> = book
            .sizes(SHEET)
            .rows
            .iter()
            .map(|(row, height)| (row.get(), *height))
            .collect();
        heights.sort_by_key(|(row, _)| *row);
        heights
    }

    fn book_with(cells: &[(&str, &str)]) -> Workbook {
        let mut book = Workbook::new_empty().unwrap();
        for (cell, text) in cells {
            book.set_input(SHEET, at(cell), text).unwrap();
        }
        book
    }

    #[test]
    fn each_column_fits_its_own_longest_text() {
        let mut book = book_with(&[("A1", "abcd"), ("A2", "ab"), ("B1", "abcdefghij")]);
        fit_columns(&mut book, SHEET, lines(LineAxis::Column, 0, 1)).unwrap();
        assert_eq!(
            widths(&book),
            [
                (0, column_width_of(4).round()),
                (1, column_width_of(10).round())
            ]
        );
        assert!(column_width_of(10) > column_width_of(4));
    }

    #[test]
    fn a_column_without_cells_keeps_its_width() {
        let mut book = book_with(&[("A1", "abcdefghij")]);
        book.set_column_width(SHEET, ColIdx::clamped(2), 200.0)
            .unwrap();
        fit_columns(&mut book, SHEET, lines(LineAxis::Column, 0, 3)).unwrap();
        let widths = widths(&book);
        assert!(widths.contains(&(2, 200.0)), "{widths:?}");
        assert_eq!(widths.len(), 2);
    }

    #[test]
    fn a_hidden_column_stays_hidden() {
        let mut book = book_with(&[("B1", "abcdefghij")]);
        let range = Range::parse_a1("B1:B1048576").unwrap();
        book.set_columns_hidden(SHEET, range, true).unwrap();
        fit_columns(&mut book, SHEET, lines(LineAxis::Column, 0, 3)).unwrap();
        assert_eq!(widths(&book), [(1, 0.0)]);
    }

    #[test]
    fn only_the_columns_of_the_range_are_fitted() {
        let mut book = book_with(&[("A1", "abcdefghij"), ("C1", "abcdefghij")]);
        fit_columns(&mut book, SHEET, lines(LineAxis::Column, 2, 2)).unwrap();
        let fitted: Vec<u16> = widths(&book).iter().map(|(col, _)| *col).collect();
        assert_eq!(fitted, [2]);
    }

    #[test]
    fn a_column_of_numbers_fits_the_text_shown() {
        let mut book = book_with(&[("A1", "1234567")]);
        fit_columns(&mut book, SHEET, lines(LineAxis::Column, 0, 0)).unwrap();
        assert_eq!(widths(&book), [(0, column_width_of(7).round())]);
    }

    fn wrapped(book: &mut Workbook, cell: &str) {
        let pos = at(cell);
        book.apply_style(SHEET, Range::single(pos), StyleChange::Wrap(true))
            .unwrap();
    }

    #[test]
    fn a_wrapped_cell_makes_its_row_as_tall_as_its_lines() {
        let mut book = book_with(&[("A1", "one\ntwo\nthree")]);
        wrapped(&mut book, "A1");
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 0)).unwrap();
        assert_eq!(heights(&book), [(0, height_of_lines(3.0).round())]);
    }

    #[test]
    fn a_taller_row_than_its_content_shrinks_to_the_row_height_of_the_sheet() {
        let mut book = book_with(&[("A1", "one")]);
        book.set_row_height(SHEET, RowIdx::clamped(0), 90.0)
            .unwrap();
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 0)).unwrap();
        assert_eq!(heights(&book), [(0, DEFAULT_ROW_HEIGHT)]);
        book.set_default_row_height(SHEET, 30.0).unwrap();
        book.set_row_height(SHEET, RowIdx::clamped(0), 90.0)
            .unwrap();
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 0)).unwrap();
        assert_eq!(heights(&book), [(0, 30.0)]);
    }

    #[test]
    fn a_row_that_already_fits_gets_no_entry() {
        let mut book = book_with(&[("A1", "one"), ("A2", "two")]);
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 1)).unwrap();
        assert!(heights(&book).is_empty());
    }

    #[test]
    fn rows_without_cells_and_hidden_rows_stay_as_they_are() {
        let mut book = book_with(&[("A1", "one\ntwo"), ("A3", "one\ntwo")]);
        wrapped(&mut book, "A1");
        wrapped(&mut book, "A3");
        book.set_row_height(SHEET, RowIdx::clamped(3), 80.0)
            .unwrap();
        let hidden = Range::parse_a1("A3:A3").unwrap();
        book.set_rows_hidden(SHEET, hidden, true).unwrap();
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 5)).unwrap();
        assert_eq!(
            heights(&book),
            [(0, height_of_lines(2.0).round()), (2, 0.0), (3, 80.0)]
        );
    }

    #[test]
    fn a_cell_in_a_hidden_column_does_not_make_its_row_taller() {
        let mut book = book_with(&[("A1", "one\ntwo\nthree"), ("B1", "x")]);
        wrapped(&mut book, "A1");
        let column = Range::parse_a1("A1:A1048576").unwrap();
        book.set_columns_hidden(SHEET, column, true).unwrap();
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 0)).unwrap();
        assert!(heights(&book).is_empty());
    }

    #[test]
    fn text_that_does_not_wrap_never_makes_a_row_taller() {
        let mut book = book_with(&[("A1", "one\ntwo")]);
        fit_rows(&mut book, SHEET, lines(LineAxis::Row, 0, 0)).unwrap();
        assert!(heights(&book).is_empty());
    }

    #[test]
    fn a_fit_is_one_undo_step() {
        let mut book = book_with(&[("A1", "one\ntwo"), ("A2", "one\ntwo\nthree")]);
        wrapped(&mut book, "A1");
        wrapped(&mut book, "A2");
        let before = book.sizes(SHEET);
        book.as_one_undo_step(|book| fit_rows(book, SHEET, lines(LineAxis::Row, 0, 1)))
            .unwrap();
        assert_eq!(heights(&book).len(), 2);
        book.undo().unwrap();
        assert_eq!(book.sizes(SHEET), before);
    }

    #[test]
    fn a_selection_of_every_row_fits_only_the_rows_with_cells() {
        let mut book = book_with(&[("A1", "one\ntwo")]);
        wrapped(&mut book, "A1");
        let all = lines(LineAxis::Row, 0, zenkai_types::MAX_ROWS - 1);
        fit_rows(&mut book, SHEET, all).unwrap();
        assert_eq!(heights(&book).len(), 1);
    }

    #[test]
    fn the_estimates_grow_with_the_text_and_shrink_with_the_column() {
        assert_eq!(wrapped_lines("short", DEFAULT_COL_WIDTH), 1.0);
        assert_eq!(wrapped_lines("a\nb\nc", DEFAULT_COL_WIDTH), 3.0);
        let long = "word ".repeat(30);
        assert!(wrapped_lines(&long, DEFAULT_COL_WIDTH) > wrapped_lines(&long, 300.0));
        assert_eq!(column_width_of(0), 24.0);
        assert_eq!(column_width_of(10_000), 600.0);
        assert_eq!(column_width_of(10), 10.0 * AUTOFIT_CHAR_WIDTH + 12.0);
        assert_eq!(height_of_lines(100.0), height_of_lines(20.0));
    }
}
