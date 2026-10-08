use std::path::Path;

use calamine::{Data, DataRef, Reader, Sheets, open_workbook_auto};

pub const MAX_CELLS: usize = 20_000_000;
// A block is written to the engine as one dense rectangle; sparse sheets become
// several blocks so two far-apart cells never allocate the space between them.
const MAX_BLOCK_CELLS: u64 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum ValuesError {
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("the workbook has more than {MAX_CELLS} cells")]
    TooLarge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub first_row: u32,
    pub first_col: u32,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetValues {
    pub name: String,
    pub blocks: Vec<Block>,
}

// Every text keeps a quote prefix, so it stays text ("007", "TRUE", "2024-01-01")
// and nothing from the file is ever evaluated.
fn text(s: &str) -> String {
    format!("'{s}")
}

fn data_input(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::Int(n) => n.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(dt) => dt.as_f64().to_string(),
        Data::Error(e) => e.to_string(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => text(s),
    }
}

fn data_ref_input(cell: &DataRef<'_>) -> String {
    match cell {
        DataRef::Empty => String::new(),
        DataRef::Int(n) => n.to_string(),
        DataRef::Float(f) => f.to_string(),
        DataRef::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        DataRef::DateTime(dt) => dt.as_f64().to_string(),
        DataRef::Error(e) => e.to_string(),
        DataRef::SharedString(s) => text(s),
        DataRef::String(s) | DataRef::DateTimeIso(s) | DataRef::DurationIso(s) => text(s),
    }
}

struct Cells {
    cells: Vec<(u32, u32, String)>,
    total: usize,
}

impl Cells {
    fn push(&mut self, row: u32, col: u32, input: String) -> Result<(), ValuesError> {
        if input.is_empty() {
            return Ok(());
        }
        self.total += 1;
        if self.total > MAX_CELLS {
            return Err(ValuesError::TooLarge);
        }
        self.cells.push((row, col, input));
        Ok(())
    }
}

fn unreadable(e: impl std::fmt::Display) -> ValuesError {
    ValuesError::Unreadable(e.to_string())
}

fn sheet_cells<RS: std::io::Read + std::io::Seek>(
    workbook: &mut Sheets<RS>,
    name: &str,
    sink: &mut Cells,
) -> Result<(), ValuesError> {
    match workbook {
        Sheets::Xlsx(xlsx) => {
            let mut reader = xlsx.worksheet_cells_reader(name).map_err(unreadable)?;
            while let Some(cell) = reader.next_cell().map_err(unreadable)? {
                let (row, col) = cell.get_position();
                sink.push(row, col, data_ref_input(cell.get_value()))?;
            }
        }
        Sheets::Xlsb(xlsb) => {
            let mut reader = xlsb.worksheet_cells_reader(name).map_err(unreadable)?;
            while let Some(cell) = reader.next_cell().map_err(unreadable)? {
                let (row, col) = cell.get_position();
                sink.push(row, col, data_ref_input(cell.get_value()))?;
            }
        }
        // .xls sheets are capped at 65,536 x 256 by the format, and calamine caps .ods.
        Sheets::Xls(_) | Sheets::Ods(_) => {
            let range = workbook.worksheet_range(name).map_err(unreadable)?;
            let (first_row, first_col) = range.start().unwrap_or((0, 0));
            for (r, row) in (0u32..).zip(range.rows()) {
                for (c, cell) in (0u32..).zip(row) {
                    sink.push(first_row + r, first_col + c, data_input(cell))?;
                }
            }
        }
    }
    Ok(())
}

fn into_blocks(mut cells: Vec<(u32, u32, String)>) -> Vec<Block> {
    cells.sort_by_key(|(row, col, _)| (*row, *col));
    let mut blocks = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        let first_row = cells[start].0;
        let (mut min_col, mut max_col) = (cells[start].1, cells[start].1);
        let mut end = start;
        while end < cells.len() {
            let (row, col, _) = cells[end];
            let (lo, hi) = (min_col.min(col), max_col.max(col));
            let area = u64::from(row - first_row + 1) * u64::from(hi - lo + 1);
            if area > MAX_BLOCK_CELLS && end > start {
                break;
            }
            min_col = lo;
            max_col = hi;
            end += 1;
        }
        let last_row = cells[end - 1].0;
        let width = (max_col - min_col + 1) as usize;
        let mut rows = vec![Vec::new(); (last_row - first_row + 1) as usize];
        for (row, col, input) in cells[start..end].iter_mut() {
            let line = &mut rows[(*row - first_row) as usize];
            if line.is_empty() {
                line.resize(width, String::new());
            }
            line[(*col - min_col) as usize] = std::mem::take(input);
        }
        blocks.push(Block {
            first_row,
            first_col: min_col,
            rows,
        });
        start = end;
    }
    blocks
}

pub fn read_values(path: &Path) -> Result<Vec<SheetValues>, ValuesError> {
    let mut workbook = open_workbook_auto(path).map_err(unreadable)?;
    let mut sheets = Vec::new();
    let mut sink = Cells {
        cells: Vec::new(),
        total: 0,
    };
    for name in workbook.sheet_names() {
        sheet_cells(&mut workbook, &name, &mut sink)?;
        let blocks = into_blocks(std::mem::take(&mut sink.cells));
        sheets.push(SheetValues { name, blocks });
    }
    Ok(sheets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_stays_text() {
        assert_eq!(data_input(&Data::String("=1+1".to_string())), "'=1+1");
        assert_eq!(data_input(&Data::String("007".to_string())), "'007");
        assert_eq!(data_input(&Data::Float(1.5)), "1.5");
        assert_eq!(data_input(&Data::Bool(true)), "TRUE");
        assert_eq!(data_input(&Data::Empty), "");
    }

    #[test]
    fn far_apart_cells_become_separate_small_blocks() {
        let cells = vec![
            (0, 0, "a".to_string()),
            (1_048_575, 16_383, "z".to_string()),
            (1, 1, "b".to_string()),
        ];
        let blocks = into_blocks(cells);
        let total: usize = blocks
            .iter()
            .map(|b| b.rows.iter().map(Vec::len).sum::<usize>())
            .sum();
        assert!(total < 10, "no dense block spans the gap: {total}");
        assert_eq!(
            blocks[0].rows,
            vec![
                vec!["a".to_string(), String::new()],
                vec![String::new(), "b".to_string()]
            ]
        );
        assert_eq!(
            (blocks[1].first_row, blocks[1].first_col),
            (1_048_575, 16_383)
        );
    }
}
