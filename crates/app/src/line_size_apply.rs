use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{ColIdx, LineAxis, RowIdx, SheetId, SheetSizes};

use crate::line_size::{Lines, Size};

fn row_at(index: u32) -> RowIdx {
    RowIdx::clamped(i64::from(index))
}

fn column_at(index: u32) -> ColIdx {
    ColIdx::clamped(i64::from(index))
}

fn set_hidden(
    wb: &mut Workbook,
    sheet: SheetId,
    lines: Lines,
    hidden: bool,
) -> Result<(), EngineError> {
    let range = lines.range();
    match lines.axis {
        LineAxis::Row => wb.set_rows_hidden(sheet, range, hidden),
        LineAxis::Column => wb.set_columns_hidden(sheet, range, hidden),
    }
}

// Rows are shown in one pass over the sheet's entries. Columns are shown run by run: a sheet
// has at most 16,384 of them.
fn show_hidden(
    wb: &mut Workbook,
    sheet: SheetId,
    lines: Lines,
    sizes: &SheetSizes,
) -> Result<(), EngineError> {
    if lines.axis == LineAxis::Row {
        return set_hidden(wb, sheet, lines, false);
    }
    for span in sizes.columns.iter().filter(|span| span.width <= 0.0) {
        let first = u32::from(span.first.get()).max(lines.first);
        let last = u32::from(span.last.get()).min(lines.last);
        if first <= last {
            set_hidden(
                wb,
                sheet,
                Lines {
                    first,
                    last,
                    ..lines
                },
                false,
            )?;
        }
    }
    Ok(())
}

// One undo step, however many lines. A size above zero shows a hidden line at that size,
// as Excel does. A selection of every row or column sets the size of the sheet instead of
// one entry for each of a million lines; the lines that already had a size of their own
// are set too, so every line ends up the same.
pub fn apply(
    wb: &mut Workbook,
    sheet: SheetId,
    lines: Lines,
    size: Size,
) -> Result<(), EngineError> {
    wb.as_one_undo_step(|wb| match size {
        Size::Hidden => set_hidden(wb, sheet, lines, true),
        Size::Pixels(pixels) => {
            let sizes = wb.sizes(sheet);
            show_hidden(wb, sheet, lines, &sizes)?;
            match (lines.axis, lines.is_all()) {
                (LineAxis::Row, false) => {
                    let heights: Vec<(RowIdx, f32)> = (lines.first..=lines.last)
                        .map(|row| (row_at(row), pixels))
                        .collect();
                    wb.set_row_heights(sheet, &heights)
                }
                (LineAxis::Row, true) => {
                    let heights: Vec<(RowIdx, f32)> =
                        sizes.rows.iter().map(|(row, _)| (*row, pixels)).collect();
                    wb.set_row_heights(sheet, &heights)?;
                    wb.set_default_row_height(sheet, pixels)
                }
                (LineAxis::Column, false) => wb.set_column_widths(
                    sheet,
                    &[(column_at(lines.first), column_at(lines.last), pixels)],
                ),
                (LineAxis::Column, true) => {
                    let runs: Vec<(ColIdx, ColIdx, f32)> = sizes
                        .columns
                        .iter()
                        .map(|span| (span.first, span.last, pixels))
                        .collect();
                    wb.set_column_widths(sheet, &runs)?;
                    wb.set_default_column_width(sheet, pixels)
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
    use zenkai_types::{ColIdx, LineAxis, MAX_COLS, MAX_ROWS, Range, RowIdx, SheetId};

    use super::apply;
    use crate::line_size::{Entry, Lines, Separator, Size, display_value, format_number, parse};

    const SHEET: SheetId = SheetId(0);

    fn lines(axis: LineAxis, first: u32, last: u32) -> Lines {
        Lines { axis, first, last }
    }

    fn all_rows() -> Lines {
        lines(LineAxis::Row, 0, MAX_ROWS - 1)
    }

    fn all_columns() -> Lines {
        lines(LineAxis::Column, 0, u32::from(MAX_COLS) - 1)
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

    #[test]
    fn a_size_goes_to_every_line_of_the_selection_in_one_undo_step() {
        let mut book = Workbook::new_empty().unwrap();
        let before = book.sizes(SHEET);
        apply(
            &mut book,
            SHEET,
            lines(LineAxis::Row, 2, 4),
            Size::Pixels(30.0),
        )
        .unwrap();
        assert_eq!(heights(&book), [(2, 30.0), (3, 30.0), (4, 30.0)]);
        book.undo().unwrap();
        assert_eq!(book.sizes(SHEET), before);
        book.redo().unwrap();
        assert_eq!(heights(&book).len(), 3);
    }

    #[test]
    fn columns_take_the_size_too() {
        let mut book = Workbook::new_empty().unwrap();
        apply(
            &mut book,
            SHEET,
            lines(LineAxis::Column, 1, 3),
            Size::Pixels(145.0),
        )
        .unwrap();
        let sizes = book.sizes(SHEET);
        let widths: Vec<f32> = sizes.columns.iter().map(|span| span.width).collect();
        assert!(!widths.is_empty() && widths.iter().all(|width| *width == 145.0));
        book.undo().unwrap();
        assert!(book.sizes(SHEET).columns.is_empty());
    }

    #[test]
    fn a_size_above_zero_shows_a_hidden_line_at_that_size() {
        let mut book = Workbook::new_empty().unwrap();
        let hidden = Range::parse_a1("A3:A4").unwrap();
        book.set_rows_hidden(SHEET, hidden, true).unwrap();
        apply(
            &mut book,
            SHEET,
            lines(LineAxis::Row, 1, 4),
            Size::Pixels(30.0),
        )
        .unwrap();
        assert_eq!(heights(&book), [(1, 30.0), (2, 30.0), (3, 30.0), (4, 30.0)]);
        book.undo().unwrap();
        assert_eq!(heights(&book), [(2, 0.0), (3, 0.0)]);
    }

    #[test]
    fn zero_hides_the_lines_and_undoes() {
        let mut book = Workbook::new_empty().unwrap();
        apply(
            &mut book,
            SHEET,
            lines(LineAxis::Column, 1, 2),
            Size::Hidden,
        )
        .unwrap();
        let widths: Vec<f32> = book.sizes(SHEET).columns.iter().map(|s| s.width).collect();
        assert!(!widths.is_empty() && widths.iter().all(|width| *width == 0.0));
        book.undo().unwrap();
        assert!(book.sizes(SHEET).columns.is_empty());
    }

    #[test]
    fn every_row_sets_the_height_of_the_sheet_and_the_rows_that_had_their_own() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_row_height(SHEET, RowIdx::clamped(2), 60.0)
            .unwrap();
        let hidden = Range::parse_a1("A6:A6").unwrap();
        book.set_rows_hidden(SHEET, hidden, true).unwrap();
        let before = book.sizes(SHEET);
        apply(&mut book, SHEET, all_rows(), Size::Pixels(27.0)).unwrap();
        let sizes = book.sizes(SHEET);
        assert_eq!(sizes.default_row_height, Some(27.0));
        assert_eq!(heights(&book), [(2, 27.0), (5, 27.0)]);
        book.undo().unwrap();
        assert_eq!(book.sizes(SHEET), before);
    }

    #[test]
    fn every_column_sets_the_width_of_the_sheet_and_the_columns_that_had_their_own() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_column_width(SHEET, ColIdx::clamped(1), 200.0)
            .unwrap();
        let before = book.sizes(SHEET);
        apply(&mut book, SHEET, all_columns(), Size::Pixels(145.0)).unwrap();
        let sizes = book.sizes(SHEET);
        assert_eq!(sizes.default_column_width, Some(145.0));
        assert!(sizes.columns.iter().all(|span| span.width == 145.0));
        assert_eq!(sizes.columns.len(), 1);
        book.undo().unwrap();
        assert_eq!(book.sizes(SHEET), before);
    }

    #[test]
    fn the_size_of_the_sheet_is_saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sized.xlsx");
        let mut book = Workbook::new_empty().unwrap();
        apply(&mut book, SHEET, all_rows(), Size::Pixels(27.0)).unwrap();
        apply(&mut book, SHEET, all_columns(), Size::Pixels(145.0)).unwrap();
        save_xlsx(book.save_snapshot(&path, SaveTarget::Document).unwrap()).unwrap();
        let reopened = open_xlsx(&path).unwrap().workbook;
        let sizes = reopened.sizes(SHEET);
        assert_eq!(sizes.default_row_height, Some(27.0));
        assert_eq!(sizes.default_column_width, Some(145.0));
        assert!(sizes.rows.is_empty());
        assert!(sizes.columns.is_empty());
    }

    #[test]
    fn a_selection_of_hundreds_of_thousands_of_rows_is_one_pass_and_one_undo_step() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = lines(LineAxis::Row, 0, 299_999);
        apply(&mut book, SHEET, rows, Size::Pixels(30.0)).unwrap();
        assert_eq!(heights(&book).len(), 300_000);
        apply(&mut book, SHEET, rows, Size::Hidden).unwrap();
        assert!(heights(&book).iter().all(|(_, height)| *height == 0.0));
        book.undo().unwrap();
        book.undo().unwrap();
        assert!(heights(&book).is_empty());
    }

    #[test]
    fn every_column_width_the_dialog_can_show_is_the_pixels_the_engine_draws() {
        let mut book = Workbook::new_empty().unwrap();
        let column = ColIdx::clamped(2);
        for pixels in 6..=1790_u16 {
            let shown = display_value(LineAxis::Column, f32::from(pixels));
            let text = format_number(shown, Separator::Dot);
            let Ok(Entry::Size(Size::Pixels(typed))) =
                parse(LineAxis::Column, &text, Separator::Dot)
            else {
                panic!("{pixels} px shown as {text} is not read back");
            };
            assert_eq!(typed, f32::from(pixels), "{text}");
            book.set_column_width(SHEET, column, typed).unwrap();
            let drawn = book
                .sizes(SHEET)
                .columns
                .iter()
                .map(|span| span.width)
                .next();
            assert_eq!(drawn, Some(typed), "{pixels}");
        }
    }

    #[test]
    fn every_row_height_the_dialog_can_show_is_the_pixels_the_engine_draws() {
        let mut book = Workbook::new_empty().unwrap();
        let row = RowIdx::clamped(2);
        for pixels in 1..=546_u16 {
            let shown = display_value(LineAxis::Row, f32::from(pixels));
            let text = format_number(shown, Separator::Dot);
            let Ok(Entry::Size(Size::Pixels(typed))) = parse(LineAxis::Row, &text, Separator::Dot)
            else {
                panic!("{pixels} px shown as {text} is not read back");
            };
            assert_eq!(typed, f32::from(pixels), "{text}");
            book.set_row_height(SHEET, row, typed).unwrap();
            let drawn = book
                .sizes(SHEET)
                .rows
                .iter()
                .map(|(_, height)| *height)
                .next();
            assert_eq!(drawn, Some(typed), "{pixels}");
        }
    }
}
