use std::collections::HashMap;

use ironcalc::base::types::{Cell, FormulaValue, SpillValue};
use zenkai_types::{CellPos, LoggedChange, Range, SheetId};

use crate::error::EngineError;
use crate::model::{FormatEdits, content_cells_within, position};
use crate::workbook::{Engine, Workbook, col_i32, is_date_format, row_i32};

pub(crate) const UNREADABLE_CELL: &str = "#ERROR";

pub struct ShownCells {
    pub end: CellPos,
    // Row-major; only cells whose text is not empty.
    pub cells: Vec<(CellPos, String)>,
}

impl ShownCells {
    // The rows from the first to the last cell that shows something.
    pub fn content_rows(&self) -> u32 {
        if self.cells.is_empty() {
            0
        } else {
            self.end.row.get() + 1
        }
    }
}

// One cell of a sheet as a save over its CSV needs it.
pub struct FileCell {
    pub pos: CellPos,
    pub text: String,
    // What the engine holds for the cell, when that is not what it shows.
    pub input: Option<String>,
    pub number: Option<f64>,
    // The serial number of a number the cell shows as a date.
    pub date: Option<f64>,
    // What it shows could not be read, so `text` is not the cell's.
    pub unreadable: bool,
}

impl FileCell {
    pub fn input(&self) -> &str {
        self.input.as_deref().unwrap_or(self.text.as_str())
    }
}

// What a save over the CSV a sheet came from reads of it, in one pass, so that the lock on the
// workbook is held for no longer than that.
pub struct FileCells {
    // Of the cells that are not spilled.
    pub end: CellPos,
    // Cells that show something, row-major, spilled ones included.
    pub cells: Vec<FileCell>,
    // Cells that hold something and show nothing (`=""`), with what they hold.
    pub silent: Vec<(CellPos, String)>,
    // Each cell a dynamic array formula spilled into, with the cell of the formula; row-major.
    pub spilled: Vec<(CellPos, CellPos)>,
    pub edits: FormatEdits,
    // The identity of the sheet, which its changes are logged under.
    pub sheet_key: u32,
    // The changes logged after the one numbered `seen_change`; `None` when the engine dropped
    // some.
    pub changes: Option<Vec<LoggedChange>>,
}

fn stored_number(cell: &Cell) -> Option<f64> {
    match cell {
        Cell::NumberCell { v, .. } => Some(*v),
        Cell::SpillCell(spill) => match spill.v {
            SpillValue::Number(v) => Some(v),
            _ => None,
        },
        cell => match cell.formula_value() {
            Some(FormulaValue::Number(v)) => Some(*v),
            _ => None,
        },
    }
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

    pub fn file_cells(&self, sheet: SheetId, seen_change: u64) -> Result<FileCells, EngineError> {
        let sheet_key = self
            .sheet_key(sheet)
            .ok_or(EngineError::UnknownSheet(sheet))?;
        let used = self.used_end(sheet);
        let model = self.model.get_model();
        let ws = model
            .workbook
            .worksheet(sheet.0)
            .map_err(|_| EngineError::UnknownSheet(sheet))?;
        let mut positions =
            content_cells_within(&ws.sheet_data, Range::new(CellPos::default(), used));
        positions.sort_unstable_by_key(|pos| (pos.row, pos.col));
        let mut found = FileCells {
            end: CellPos::default(),
            cells: Vec::new(),
            silent: Vec::new(),
            spilled: Vec::new(),
            edits: self.model.format_edits(),
            sheet_key,
            changes: self.model.changes_since(seen_change),
        };
        let mut date_styles: HashMap<i32, bool> = HashMap::new();
        for pos in positions {
            let Some(cell) = ws.sheet_data.get(row_i32(pos.row), col_i32(pos.col)) else {
                continue;
            };
            let spilled = match cell {
                Cell::SpillCell(spill) => {
                    found.spilled.push((pos, position(spill.a.0, spill.a.1)));
                    true
                }
                _ => {
                    found.end =
                        CellPos::new(found.end.row.max(pos.row), found.end.col.max(pos.col));
                    false
                }
            };
            let shown = self.formatted_text(sheet, pos);
            let unreadable = shown.is_none();
            let text = shown.unwrap_or_else(|| UNREADABLE_CELL.to_string());
            let input = self.input(sheet, pos);
            if text.is_empty() {
                if !spilled {
                    found.silent.push((pos, input));
                }
                continue;
            }
            let number = stored_number(cell);
            let date = number.filter(|_| {
                *date_styles.entry(cell.get_style()).or_insert_with(|| {
                    model
                        .get_style_for_cell(sheet.0, row_i32(pos.row), col_i32(pos.col))
                        .is_ok_and(|style| is_date_format(&style.num_fmt))
                })
            });
            found.cells.push(FileCell {
                pos,
                input: (input != text).then_some(input),
                text,
                number,
                date,
                unreadable,
            });
        }
        Ok(found)
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
    fn a_file_names_what_a_formula_spilled_and_finds_formulas_that_show_nothing() {
        let mut workbook = Workbook::new_empty().unwrap();
        workbook
            .set_input(SheetId(0), pos(0, 0), "=SEQUENCE(3)")
            .unwrap();
        workbook.set_input(SheetId(0), pos(0, 2), "=\"\"").unwrap();
        workbook.set_input(SheetId(0), pos(0, 3), "4").unwrap();
        let file = workbook.file_cells(SheetId(0), 0).unwrap();
        let shown: Vec<(CellPos, &str)> = file
            .cells
            .iter()
            .map(|cell| (cell.pos, cell.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            vec![
                (pos(0, 0), "1"),
                (pos(0, 3), "4"),
                (pos(1, 0), "2"),
                (pos(2, 0), "3"),
            ]
        );
        assert_eq!(
            file.spilled,
            vec![(pos(1, 0), pos(0, 0)), (pos(2, 0), pos(0, 0))]
        );
        assert_eq!(file.end, pos(0, 3));
        assert_eq!(file.silent, vec![(pos(0, 2), "=\"\"".to_string())]);
        assert_eq!(file.cells[0].input(), "=SEQUENCE(3)");
        assert_eq!(file.cells[1].number, Some(4.0));
        assert!(file.changes.is_some_and(|changes| changes.len() == 3));
    }

    #[test]
    fn a_number_shown_as_a_date_carries_its_serial() {
        let mut workbook = Workbook::new_empty().unwrap();
        workbook
            .set_input(SheetId(0), pos(0, 0), "2026-10-08")
            .unwrap();
        workbook.set_input(SheetId(0), pos(0, 1), "46303").unwrap();
        let file = workbook.file_cells(SheetId(0), 0).unwrap();
        assert_eq!(file.cells[0].date, Some(46_303.0));
        assert_eq!(file.cells[1].date, None);
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
