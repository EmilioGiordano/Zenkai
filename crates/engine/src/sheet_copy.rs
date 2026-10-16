use ironcalc::base::cell_store::CellStore;
use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::types::Worksheet;

use crate::error::EngineError;
use crate::row_split::{Formulas, over_formulas, over_rows};

// The copy is taken under the workbook's read lock, so a large sheet's rows and formulas
// are copied on several threads.
pub(crate) fn copied_sheet(
    sheet: &Worksheet,
    formulas: &Formulas,
) -> Result<(Worksheet, Vec<(Node, StaticResult)>), EngineError> {
    let Worksheet {
        dimension,
        cols,
        rows,
        name,
        sheet_data,
        shared_formulas,
        sheet_id,
        state,
        color,
        merge_cells,
        comments,
        frozen_rows,
        frozen_columns,
        views,
        show_grid_lines,
        conditional_formatting,
    } = sheet;
    let copy_rows = |sheet_data: &CellStore, rows: &[i32]| {
        rows.iter()
            .map(|&row| {
                let cells: Vec<_> = sheet_data
                    .row_cells(row)
                    .map(|(column, cell)| (column, cell.clone()))
                    .collect();
                (row, cells)
            })
            .collect::<Vec<_>>()
    };
    let (shares, formulas) = over_rows(sheet_data, copy_rows, || {
        let shares = over_formulas(formulas, |_, share| share.to_vec())?;
        Ok(shares.into_iter().flatten().collect())
    })?;
    let mut copied_data = CellStore::new();
    for (row, cells) in shares.into_iter().flatten() {
        copied_data.insert_row(row, cells);
    }
    let copied = Worksheet {
        dimension: dimension.clone(),
        cols: cols.clone(),
        rows: rows.clone(),
        name: name.clone(),
        sheet_data: copied_data,
        shared_formulas: shared_formulas.clone(),
        sheet_id: *sheet_id,
        state: state.clone(),
        color: color.clone(),
        merge_cells: merge_cells.clone(),
        comments: comments.clone(),
        frozen_rows: *frozen_rows,
        frozen_columns: *frozen_columns,
        views: views.clone(),
        show_grid_lines: *show_grid_lines,
        conditional_formatting: conditional_formatting.clone(),
    };
    Ok((copied, formulas))
}

#[cfg(test)]
mod tests {
    use ironcalc::base::Model;

    use super::copied_sheet;
    use crate::row_split::{MIN_FORMULAS_PER_THREAD, MIN_ROWS_PER_THREAD};

    #[test]
    fn a_sheet_copied_on_several_threads_equals_its_clone() {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        let rows = i32::try_from(MIN_ROWS_PER_THREAD * 3).unwrap();
        for row in 1..=rows {
            model.set_user_input(0, row, 1, row.to_string()).unwrap();
            model
                .set_user_input(0, row, 2, format!("=A{row}+{row}"))
                .unwrap();
        }
        assert!(model.parsed_formulas[0].len() > MIN_FORMULAS_PER_THREAD * 2);
        model.workbook.worksheets[0].frozen_rows = 2;
        let (sheet, formulas) =
            copied_sheet(&model.workbook.worksheets[0], &model.parsed_formulas[0]).unwrap();
        assert_eq!(sheet, model.workbook.worksheets[0]);
        assert_eq!(formulas, model.parsed_formulas[0]);
    }
}
