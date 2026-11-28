use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{CellPos, ColIdx, LineAxis, Range, RowIdx, SheetId, SheetSizes};

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
    match lines.axis {
        LineAxis::Row => {
            let range = Range::new(
                CellPos::new(row_at(lines.first), ColIdx::default()),
                CellPos::new(row_at(lines.last), ColIdx::LAST),
            );
            wb.set_rows_hidden(sheet, range, hidden)
        }
        LineAxis::Column => {
            let range = Range::new(
                CellPos::new(RowIdx::default(), column_at(lines.first)),
                CellPos::new(RowIdx::LAST, column_at(lines.last)),
            );
            wb.set_columns_hidden(sheet, range, hidden)
        }
    }
}

fn set_size(
    wb: &mut Workbook,
    sheet: SheetId,
    axis: LineAxis,
    index: u32,
    pixels: f32,
) -> Result<(), EngineError> {
    match axis {
        LineAxis::Row => wb.set_row_height(sheet, row_at(index), pixels),
        LineAxis::Column => wb.set_column_width(sheet, column_at(index), pixels),
    }
}

// The lines with an entry of their own among `first..=last`, and which of them are hidden.
fn entries_between(sizes: &SheetSizes, axis: LineAxis, first: u32, last: u32) -> Vec<(u32, bool)> {
    let within = |index: u32| (first..=last).contains(&index);
    match axis {
        LineAxis::Row => sizes
            .rows
            .iter()
            .filter(|(row, _)| within(row.get()))
            .map(|(row, height)| (row.get(), *height <= 0.0))
            .collect(),
        LineAxis::Column => {
            let mut entries = Vec::new();
            for span in &sizes.columns {
                let start = u32::from(span.first.get()).max(first);
                let end = u32::from(span.last.get()).min(last);
                entries.extend((start..=end).map(|index| (index, span.width <= 0.0)));
            }
            entries
        }
    }
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
            let entries = entries_between(&wb.sizes(sheet), lines.axis, lines.first, lines.last);
            for (index, hidden) in &entries {
                if *hidden {
                    let line = Lines {
                        first: *index,
                        last: *index,
                        ..lines
                    };
                    set_hidden(wb, sheet, line, false)?;
                }
            }
            if lines.is_all() {
                for (index, _) in &entries {
                    set_size(wb, sheet, lines.axis, *index, pixels)?;
                }
                return match lines.axis {
                    LineAxis::Row => wb.set_default_row_height(sheet, pixels),
                    LineAxis::Column => wb.set_default_column_width(sheet, pixels),
                };
            }
            for index in lines.first..=lines.last {
                set_size(wb, sheet, lines.axis, index, pixels)?;
            }
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
    use zenkai_types::{ColIdx, LineAxis, MAX_COLS, MAX_ROWS, Range, RowIdx, SheetId};

    use super::apply;
    use crate::line_size::{Lines, Size};

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
}
