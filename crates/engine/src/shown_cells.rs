use zenkai_types::{CellPos, Range, SheetId};

use crate::error::EngineError;
use crate::model::content_cells_within;
use crate::workbook::{Engine, Workbook, col_i32, row_i32};

pub(crate) const UNREADABLE_CELL: &str = "#ERROR";

pub struct ShownCells {
    pub end: CellPos,
    // Row-major; only cells whose text is not empty.
    pub cells: Vec<(CellPos, String)>,
}

impl Workbook {
    // What a sheet shows, spilled array cells included, for writing it out cell by cell.
    // The cost follows the cells the sheet holds, never the area up to its end.
    pub fn shown_cells(&self, sheet: SheetId) -> Result<ShownCells, EngineError> {
        let end = self.used_end(sheet);
        let ws = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map_err(|_| EngineError::UnknownSheet(sheet))?;
        let mut positions =
            content_cells_within(&ws.sheet_data, Range::new(CellPos::default(), end));
        positions.sort_unstable_by_key(|pos| (pos.row, pos.col));
        let cells = positions
            .into_iter()
            .map(|pos| {
                let text = self
                    .formatted_text(sheet, pos)
                    .unwrap_or_else(|| UNREADABLE_CELL.to_string());
                (pos, text)
            })
            .filter(|(_, text)| !text.is_empty())
            .collect();
        Ok(ShownCells { end, cells })
    }

    pub(crate) fn formatted_text(&self, sheet: SheetId, pos: CellPos) -> Option<String> {
        match self
            .model
            .get_formatted_cell_value(sheet.0, row_i32(pos.row), col_i32(pos.col))
        {
            Ok(text) => Some(text),
            Err(error) => {
                tracing::warn!(%error, %pos, "could not read cell");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use zenkai_types::{ColIdx, MAX_COLS, MAX_ROWS, NumberFormat, RowIdx, StyleChange};

    use super::*;

    fn pos(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    fn dense_scan(workbook: &Workbook, sheet: SheetId) -> Vec<(CellPos, String)> {
        let end = workbook.used_end(sheet);
        let mut cells = Vec::new();
        for row in 0..=i64::from(end.row.get()) {
            for col in 0..=i64::from(end.col.get()) {
                let text = workbook.cell(sheet, pos(row, col)).text;
                if !text.is_empty() {
                    cells.push((pos(row, col), text));
                }
            }
        }
        cells
    }

    #[test]
    fn spilled_cells_are_shown_in_row_major_order() {
        let mut workbook = Workbook::new_empty().unwrap();
        workbook
            .set_input(SheetId(0), pos(0, 2), "=SEQUENCE(3)")
            .unwrap();
        workbook.set_input(SheetId(0), pos(1, 0), "a,b").unwrap();
        workbook.set_input(SheetId(0), pos(0, 0), "1.5").unwrap();
        let shown = workbook.shown_cells(SheetId(0)).unwrap();
        assert_eq!(shown.end, pos(2, 2));
        let texts: Vec<(CellPos, &str)> = shown
            .cells
            .iter()
            .map(|(pos, text)| (*pos, text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (pos(0, 0), "1.5"),
                (pos(0, 2), "1"),
                (pos(1, 0), "a,b"),
                (pos(1, 2), "2"),
                (pos(2, 2), "3"),
            ]
        );
    }

    #[test]
    fn a_sheet_reaching_the_last_cell_gathers_only_its_cells() {
        let mut workbook = Workbook::new_empty().unwrap();
        let last = pos(i64::from(MAX_ROWS - 1), i64::from(MAX_COLS - 1));
        workbook.set_input(SheetId(0), pos(0, 0), "first").unwrap();
        workbook.set_input(SheetId(0), last, "last").unwrap();
        let shown = workbook.shown_cells(SheetId(0)).unwrap();
        assert_eq!(shown.end, last);
        assert_eq!(
            shown.cells,
            vec![(pos(0, 0), "first".to_string()), (last, "last".to_string())]
        );
    }

    #[test]
    fn an_unknown_sheet_is_an_error() {
        let workbook = Workbook::new_empty().unwrap();
        assert!(matches!(
            workbook.shown_cells(SheetId(7)),
            Err(EngineError::UnknownSheet(_))
        ));
    }

    #[derive(Clone, Debug)]
    enum Step {
        Input(i64, i64, &'static str),
        Currency(i64, i64),
        Clear(i64, i64),
    }

    fn step() -> impl Strategy<Value = Step> {
        let inputs = vec![
            "1",
            "-2.5",
            "text",
            "=\"\"",
            "=1/0",
            "=SEQUENCE(3)",
            "=SEQUENCE(2,2)",
            "=A1*2",
            "TRUE",
            "",
        ];
        prop_oneof![
            4 => (0..6i64, 0..6i64, prop::sample::select(inputs))
                .prop_map(|(row, col, text)| Step::Input(row, col, text)),
            1 => (0..8i64, 0..8i64).prop_map(|(row, col)| Step::Currency(row, col)),
            1 => (0..6i64, 0..6i64).prop_map(|(row, col)| Step::Clear(row, col)),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

        #[test]
        fn the_gather_matches_a_dense_scan_of_the_used_area(
            steps in prop::collection::vec(step(), 0..12),
        ) {
            let mut workbook = Workbook::new_empty().unwrap();
            let sheet = SheetId(0);
            for step in steps {
                match step {
                    Step::Input(row, col, text) => {
                        workbook.set_input(sheet, pos(row, col), text).unwrap();
                    }
                    Step::Currency(row, col) => {
                        let at = pos(row, col);
                        workbook
                            .apply_style(
                                sheet,
                                Range::new(at, at),
                                StyleChange::NumberFormat(NumberFormat::Currency),
                            )
                            .unwrap();
                    }
                    Step::Clear(row, col) => {
                        let at = pos(row, col);
                        workbook.clear(sheet, Range::new(at, at)).unwrap();
                    }
                }
            }
            let shown = workbook.shown_cells(sheet).unwrap();
            prop_assert_eq!(shown.end, workbook.used_end(sheet));
            prop_assert_eq!(shown.cells, dense_scan(&workbook, sheet));
        }
    }
}
