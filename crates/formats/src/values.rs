use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};

pub const MAX_CELLS: usize = 20_000_000;

#[derive(Debug, thiserror::Error)]
pub enum ValuesError {
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("the workbook has more than {MAX_CELLS} cells")]
    TooLarge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetValues {
    pub name: String,
    pub first_row: u32,
    pub first_col: u32,
    pub rows: Vec<Vec<String>>,
}

// Values typed back into the engine: text that would parse as a formula keeps a
// quote prefix so the read-only copy can never evaluate anything from the file.
fn as_input(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::Int(n) => n.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(dt) => dt.as_f64().to_string(),
        Data::Error(e) => e.to_string(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => {
            if s.starts_with(['=', '+', '-', '@', '\'']) {
                format!("'{s}")
            } else {
                s.clone()
            }
        }
    }
}

pub fn read_values(path: &Path) -> Result<Vec<SheetValues>, ValuesError> {
    let unreadable = |e: calamine::Error| ValuesError::Unreadable(e.to_string());
    let mut workbook = open_workbook_auto(path).map_err(unreadable)?;
    let mut sheets = Vec::new();
    let mut cells = 0usize;
    for name in workbook.sheet_names() {
        let range = workbook.worksheet_range(&name).map_err(unreadable)?;
        let (height, width) = range.get_size();
        cells = cells.saturating_add(height.saturating_mul(width));
        if cells > MAX_CELLS {
            return Err(ValuesError::TooLarge);
        }
        let (first_row, first_col) = range.start().unwrap_or((0, 0));
        let rows = range
            .rows()
            .map(|row| row.iter().map(as_input).collect())
            .collect();
        sheets.push(SheetValues {
            name,
            first_row,
            first_col,
            rows,
        });
    }
    Ok(sheets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_looks_like_a_formula_stays_text() {
        assert_eq!(as_input(&Data::String("=1+1".to_string())), "'=1+1");
        assert_eq!(as_input(&Data::String("plain".to_string())), "plain");
        assert_eq!(as_input(&Data::Float(1.5)), "1.5");
        assert_eq!(as_input(&Data::Bool(true)), "TRUE");
        assert_eq!(as_input(&Data::Empty), "");
    }
}
