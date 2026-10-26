use crate::{
    constants::{LAST_COLUMN, LAST_ROW},
    expressions::types::Area,
    types::{Cell, Style},
    user_model::history::Diff,
    UserModel,
};

impl UserModel<'_> {
    /// Gives every cell of `target` the whole cell format (number format, font, fill,
    /// borders, alignment) of the cell at `source`, as one undo step. The source's format
    /// is the one it shows, so a cell with no style of its own passes its row or column
    /// style on. Cell contents are left alone.
    pub fn fill_style(&mut self, source: (i32, i32), target: &Area) -> Result<(), String> {
        let style_index = self
            .model
            .get_cell_style_index(target.sheet, source.0, source.1)?;
        let old_styles = self.write_filled_style(style_index, target)?;
        self.push_diff_list(vec![Diff::FillStyle {
            sheet: target.sheet,
            row: target.row,
            column: target.column,
            width: target.width,
            height: target.height,
            style_index,
            old_styles,
        }]);
        Ok(())
    }

    // The own style of each cell of `target` as it was, row by row, None where it had no cell.
    pub(super) fn write_filled_style(
        &mut self,
        style_index: i32,
        target: &Area,
    ) -> Result<Vec<Option<i32>>, String> {
        let fits = target.width > 0
            && target.height > 0
            && target.row >= 1
            && target.column >= 1
            && target.row.saturating_add(target.height - 1) <= LAST_ROW
            && target.column.saturating_add(target.width - 1) <= LAST_COLUMN;
        if !fits {
            return Err("Invalid area".to_string());
        }
        let worksheet = self.model.workbook.worksheet_mut(target.sheet)?;
        let mut old_styles = Vec::with_capacity(target.width as usize * target.height as usize);
        for row in target.row..target.row + target.height {
            for column in target.column..target.column + target.width {
                old_styles.push(worksheet.cell(row, column).map(Cell::get_style));
                worksheet.set_cell_style(row, column, style_index)?;
            }
        }
        Ok(old_styles)
    }

    // Puts back the own style of each cell of `area`, given row by row. A cell that had none
    // is removed unless something was written in it since, which then keeps no style.
    pub(super) fn restore_styles(
        &mut self,
        area: &Area,
        old_styles: &[Option<i32>],
    ) -> Result<(), String> {
        if old_styles.len() as i64 != i64::from(area.width) * i64::from(area.height) {
            return Err("The saved styles do not match the area".to_string());
        }
        let default_style = self
            .model
            .workbook
            .styles
            .get_style_index_or_create(&Style::default());
        let worksheet = self.model.workbook.worksheet_mut(area.sheet)?;
        let cells = (area.row..area.row + area.height).flat_map(|row| {
            (area.column..area.column + area.width).map(move |column| (row, column))
        });
        for ((row, column), old) in cells.zip(old_styles) {
            match old {
                Some(style_index) => worksheet.set_cell_style(row, column, *style_index)?,
                None => match worksheet.cell(row, column) {
                    Some(Cell::EmptyCell { .. }) => worksheet.remove_cell(row, column)?,
                    Some(_) => worksheet.set_cell_style(row, column, default_style)?,
                    None => {}
                },
            }
        }
        Ok(())
    }
}
