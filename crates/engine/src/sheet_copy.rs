use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::types::{SheetData, Worksheet};

use crate::error::EngineError;
use crate::row_split::{Rows, over_rows};

type Formulas = Vec<(Node, StaticResult)>;

// The copy is taken under the workbook's read lock, so a large sheet's rows are copied on
// several threads while the calling thread, the engine's deep stack, copies the formulas.
pub(crate) fn copied_sheet(
    sheet: &Worksheet,
    formulas: &[(Node, StaticResult)],
) -> Result<(Worksheet, Formulas), EngineError> {
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
    let copy_rows = |rows: &Rows<'_>| {
        rows.iter()
            .map(|&(row, cells)| (*row, cells.clone()))
            .collect::<Vec<_>>()
    };
    let (shares, formulas) = over_rows(sheet_data, copy_rows, || Ok(formulas.to_vec()))?;
    let mut copied_data = SheetData::with_capacity(sheet_data.len());
    for share in shares {
        copied_data.extend(share);
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
    use crate::row_split::MIN_ROWS_PER_THREAD;

    #[test]
    fn a_sheet_copied_on_several_threads_equals_its_clone() {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        let rows = i32::try_from(MIN_ROWS_PER_THREAD * 3).unwrap();
        for row in 1..=rows {
            model.set_user_input(0, row, 1, row.to_string()).unwrap();
        }
        model.set_user_input(0, 1, 2, "=A1*2".to_string()).unwrap();
        model.workbook.worksheets[0].frozen_rows = 2;
        let (sheet, formulas) =
            copied_sheet(&model.workbook.worksheets[0], &model.parsed_formulas[0]).unwrap();
        assert_eq!(sheet, model.workbook.worksheets[0]);
        assert_eq!(formulas, model.parsed_formulas[0]);
    }
}
