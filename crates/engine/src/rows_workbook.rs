use zenkai_types::{CellPos, SheetId};

use crate::error::EngineError;
use crate::preflight::run_with_engine_stack;
use crate::workbook::{Engine, Workbook};

pub fn workbook_from_rows(rows: Vec<Vec<String>>) -> Result<Workbook, EngineError> {
    run_with_engine_stack(move || {
        let mut workbook = Workbook::new_empty()?;
        workbook.set_inputs(SheetId(0), CellPos::default(), &rows)?;
        workbook.warm_used_areas();
        workbook.forget_changes();
        Ok(workbook)
    })
}

#[cfg(test)]
mod tests {
    use zenkai_types::{ColIdx, RowIdx};

    use super::*;

    #[test]
    fn rows_land_on_the_first_sheet_without_leaving_changes() {
        let rows = vec![
            vec!["a".to_string(), "1".to_string()],
            vec!["b".to_string(), "=B1+1".to_string()],
        ];
        let workbook = workbook_from_rows(rows).unwrap();
        let at = |row: i64, col: i64| CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col));
        assert_eq!(workbook.cell(SheetId(0), at(0, 0)).text, "a");
        assert_eq!(workbook.cell(SheetId(0), at(1, 1)).text, "2");
        assert_eq!(workbook.sheets().len(), 1);
    }
}
