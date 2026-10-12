use zenkai_types::{CellPos, ColIdx, MAX_COLS, MAX_ROWS, Range, RowIdx};

use crate::region;

// A dialog row per column: wider tables are generated in several passes.
pub const MAX_COLUMNS: u16 = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub range: Range,
    pub headers: Vec<String>,
}

impl Layout {
    pub fn from_selection(
        selection: Range,
        used_end: CellPos,
        filled: impl Fn(CellPos) -> bool,
        input: impl Fn(CellPos) -> String,
    ) -> Layout {
        let range = if selection.start == selection.end {
            region::current_region(selection.start, filled)
        } else {
            clip_to_used(selection, used_end)
        };
        Layout::from_range(range, input)
    }

    pub fn from_range(range: Range, input: impl Fn(CellPos) -> String) -> Layout {
        let width = range.cols().min(MAX_COLUMNS);
        let last = range.start.col.offset(i64::from(width) - 1);
        let range = Range::new(range.start, CellPos::new(range.end.row, last));
        let headers = (range.start.col.get()..=range.end.col.get())
            .map(|col| {
                input(CellPos::new(
                    range.start.row,
                    ColIdx::clamped(i64::from(col)),
                ))
            })
            .collect();
        Layout { range, headers }
    }

    pub fn header_row(&self) -> RowIdx {
        self.range.start.row
    }

    pub fn first_col(&self) -> ColIdx {
        self.range.start.col
    }

    pub fn data_rows(&self) -> u32 {
        self.range.rows() - 1
    }
}

// Selecting whole rows or columns means "the data there", not the million empty cells.
fn clip_to_used(selection: Range, used_end: CellPos) -> Range {
    let mut end = selection.end;
    if selection.rows() == MAX_ROWS {
        end.row = used_end.row.max(selection.start.row);
    }
    if selection.cols() == MAX_COLS {
        end.col = used_end.col.max(selection.start.col);
    }
    Range::new(selection.start, end)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn pos(a1: &str) -> CellPos {
        CellPos::parse_a1(a1).unwrap()
    }

    fn range(a1: &str) -> Range {
        Range::parse_a1(a1).unwrap()
    }

    fn sheet(cells: &[(&str, &str)]) -> HashMap<CellPos, String> {
        cells
            .iter()
            .map(|(a1, text)| (pos(a1), text.to_string()))
            .collect()
    }

    fn layout(selection: &str, cells: &HashMap<CellPos, String>, used_end: &str) -> Layout {
        Layout::from_selection(
            range(selection),
            pos(used_end),
            |at| cells.contains_key(&at),
            |at| cells.get(&at).cloned().unwrap_or_default(),
        )
    }

    #[test]
    fn a_header_row_selection_generates_below_it() {
        let cells = sheet(&[("A1", "Name"), ("B1", "Email")]);
        let found = layout("A1:B1", &cells, "B1");
        assert_eq!(found.headers, ["Name", "Email"]);
        assert_eq!(found.data_rows(), 0);
    }

    #[test]
    fn headers_with_rows_fill_the_selected_rows() {
        let cells = sheet(&[("A1", "Name"), ("A2", "x"), ("A3", "y")]);
        let found = layout("A1:A3", &cells, "A3");
        assert_eq!(found.data_rows(), 2);
        assert_eq!(found.header_row(), pos("A1").row);
    }

    #[test]
    fn one_cell_inside_a_table_selects_the_whole_table() {
        let cells = sheet(&[("B2", "Name"), ("C2", "Age"), ("B3", "Ana"), ("C3", "30")]);
        let found = layout("C3", &cells, "C3");
        assert_eq!(found.range, range("B2:C3"));
        assert_eq!(found.headers, ["Name", "Age"]);
    }

    #[test]
    fn one_empty_cell_is_one_unnamed_column() {
        let found = layout("D4", &HashMap::new(), "A1");
        assert_eq!(found.range, range("D4"));
        assert_eq!(found.headers, [""]);
    }

    #[test]
    fn empty_columns_keep_empty_headers() {
        let found = layout("A1:C1", &HashMap::new(), "A1");
        assert_eq!(found.headers, ["", "", ""]);
    }

    #[test]
    fn whole_columns_stop_at_the_used_area() {
        let cells = sheet(&[("A1", "Name"), ("A101", "z"), ("B1", "Age")]);
        let found = layout("A1:B1048576", &cells, "B101");
        assert_eq!(found.range, range("A1:B101"));
        let empty = layout("A1:B1048576", &HashMap::new(), "A1");
        assert_eq!(empty.data_rows(), 0);
    }

    #[test]
    fn whole_rows_stop_at_the_used_area_and_the_dialog_width() {
        let cells = sheet(&[("A1", "Name"), ("C1", "Age")]);
        let found = layout("A1:XFD1", &cells, "C1");
        assert_eq!(found.range, range("A1:C1"));
        let wide = Layout::from_range(range("A1:CZ1"), |_| String::new());
        assert_eq!(wide.headers.len(), usize::from(MAX_COLUMNS));
        assert_eq!(wide.range, range("A1:BL1"));
    }
}
