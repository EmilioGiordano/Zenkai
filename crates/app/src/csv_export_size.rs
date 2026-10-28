use zenkai_engine::ShownCells;

const MIN_CONFIRMED_BYTES: u64 = 100 * 1024 * 1024;
const OUTSIZED_FACTOR: u64 = 50;
const BYTE_ORDER_MARK_BYTES: u64 = 3;
const LINE_BREAK_BYTES: u64 = 2;

// What an export of the sheet would write: the delimiters of the whole used rectangle
// plus the text of its cells (quoting is not counted).
pub struct ExportSize {
    pub bytes: u64,
    pub rows: u64,
    pub columns: u64,
    pub cells: usize,
    text_bytes: u64,
}

impl ExportSize {
    pub fn of(shown: &ShownCells) -> ExportSize {
        let rows = u64::from(shown.end.row.get()) + 1;
        let columns = u64::from(shown.end.col.get()) + 1;
        let text_bytes: u64 = shown.cells.iter().map(|(_, text)| text.len() as u64).sum();
        let bytes = BYTE_ORDER_MARK_BYTES + rows * (columns - 1 + LINE_BREAK_BYTES) + text_bytes;
        ExportSize {
            bytes,
            rows,
            columns,
            cells: shown.cells.len(),
            text_bytes,
        }
    }

    // Large and mostly delimiters: a dense big sheet is exported without asking.
    pub fn needs_confirmation(&self) -> bool {
        self.bytes > MIN_CONFIRMED_BYTES && self.bytes > OUTSIZED_FACTOR * self.text_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_types::{CellPos, ColIdx, MAX_COLS, MAX_ROWS, RowIdx};

    fn at(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    fn shown(cells: &[(CellPos, &str)]) -> ShownCells {
        let end = CellPos::new(
            RowIdx::clamped(
                cells
                    .iter()
                    .map(|(p, _)| i64::from(p.row.get()))
                    .max()
                    .unwrap(),
            ),
            ColIdx::clamped(
                cells
                    .iter()
                    .map(|(p, _)| i64::from(p.col.get()))
                    .max()
                    .unwrap(),
            ),
        );
        ShownCells {
            end,
            cells: cells.iter().map(|(p, t)| (*p, t.to_string())).collect(),
        }
    }

    #[test]
    fn counts_every_delimiter_of_the_used_rectangle() {
        let size = ExportSize::of(&shown(&[(at(0, 0), "ab"), (at(1, 2), "c")]));
        assert_eq!(size.bytes, 3 + 2 * (2 + 2) + 3);
        assert_eq!(size.cells, 2);
    }

    #[test]
    fn a_sheet_from_a_to_the_last_cell_is_outsized() {
        let last = shown(&[
            (at(0, 0), "x"),
            (at(i64::from(MAX_ROWS) - 1, i64::from(MAX_COLS) - 1), "y"),
        ]);
        let size = ExportSize::of(&last);
        assert!(size.bytes > 17_000_000_000);
        assert!(size.needs_confirmation());
    }

    #[test]
    fn a_small_sparse_sheet_is_not_asked_about() {
        let size = ExportSize::of(&shown(&[(at(0, 0), "x"), (at(500, 200), "y")]));
        assert!(!size.needs_confirmation());
    }

    #[test]
    fn a_dense_big_sheet_is_not_asked_about() {
        let size = ExportSize {
            bytes: 400 * 1024 * 1024,
            rows: 1,
            columns: 1,
            cells: 40_000_000,
            text_bytes: 300 * 1024 * 1024,
        };
        assert!(!size.needs_confirmation());
    }

    #[test]
    fn a_big_sheet_with_little_text_is_asked_about() {
        let size = ExportSize {
            bytes: 400 * 1024 * 1024,
            rows: 1,
            columns: 1,
            cells: 1000,
            text_bytes: 2 * 1024 * 1024,
        };
        assert!(size.needs_confirmation());
        let small = ExportSize {
            bytes: 90 * 1024 * 1024,
            rows: 1,
            columns: 1,
            cells: 1,
            text_bytes: 1,
        };
        assert!(!small.needs_confirmation());
    }
}
