use crate::{
    expressions::types::Area,
    model::Model,
    types::Cell,
    user_model::history::{Diff, DiffList},
};

pub(crate) enum PendingRecalculation {
    Cells(Vec<(u32, i32, i32)>),
    Workbook,
}

impl PendingRecalculation {
    pub(crate) fn nothing() -> PendingRecalculation {
        PendingRecalculation::Cells(Vec::new())
    }

    pub(crate) fn add_area(&mut self, area: &Area) {
        if let PendingRecalculation::Cells(cells) = self {
            for row in area.row..area.row.saturating_add(area.height) {
                for column in area.column..area.column.saturating_add(area.width) {
                    cells.push((area.sheet, row, column));
                }
            }
        }
    }

    fn add_cleared(&mut self, sheet: u32, row: i32, column: i32, old: &[Vec<Option<Cell>>]) {
        if let PendingRecalculation::Cells(cells) = self {
            for (row, old_row) in (row..).zip(old) {
                for (column, old_cell) in (column..).zip(old_row) {
                    if old_cell.is_some() {
                        cells.push((sheet, row, column));
                    }
                }
            }
        }
    }

    // SUBTOTAL skips hidden rows, so hiding or showing one changes what reads its cells.
    fn add_row(&mut self, model: &Model, sheet: u32, row: i32) {
        let Ok(worksheet) = model.workbook.worksheet(sheet) else {
            *self = PendingRecalculation::Workbook;
            return;
        };
        if let (PendingRecalculation::Cells(cells), Some(row_cells)) =
            (&mut *self, worksheet.sheet_data.get(&row))
        {
            cells.extend(row_cells.keys().map(|column| (sheet, row, *column)));
        }
    }

    // The same cells change whether `diff_list` is applied or undone.
    pub(crate) fn add(&mut self, diff_list: &DiffList, model: &Model) {
        for diff in diff_list {
            match diff {
                Diff::SetCellValue {
                    sheet, row, column, ..
                } => {
                    if let PendingRecalculation::Cells(cells) = self {
                        cells.push((*sheet, *row, *column));
                    }
                }
                Diff::RangeClearContents {
                    sheet,
                    row,
                    column,
                    old_value,
                    ..
                }
                | Diff::RangeClearAll {
                    sheet,
                    row,
                    column,
                    old_value,
                    ..
                } => self.add_cleared(*sheet, *row, *column, old_value),
                Diff::SetRowHidden { sheet, row, .. } => self.add_row(model, *sheet, *row),
                Diff::CellClearFormatting { .. }
                | Diff::SetCellStyle { .. }
                | Diff::ApplyNamedStyle { .. }
                | Diff::SetColumnWidth { .. }
                | Diff::SetColumnHidden { .. }
                | Diff::SetRowHeight { .. }
                | Diff::SetColumnStyle { .. }
                | Diff::SetRowStyle { .. }
                | Diff::DeleteColumnStyle { .. }
                | Diff::DeleteRowStyle { .. }
                | Diff::SetFrozenRowsCount { .. }
                | Diff::SetFrozenColumnsCount { .. }
                | Diff::SetSheetColor { .. }
                | Diff::SetShowGridLines { .. }
                | Diff::SetTheme { .. }
                | Diff::SetWorkbookName { .. }
                | Diff::CreateNamedStyle { .. }
                | Diff::DeleteNamedStyle { .. }
                | Diff::UpdateNamedStyle { .. } => {}
                Diff::SetArrayValue { .. }
                | Diff::InsertRows { .. }
                | Diff::DeleteRows { .. }
                | Diff::InsertColumns { .. }
                | Diff::DeleteColumns { .. }
                | Diff::MoveColumns { .. }
                | Diff::MoveRows { .. }
                | Diff::NewSheet { .. }
                | Diff::DuplicateSheet { .. }
                | Diff::DeleteSheet { .. }
                | Diff::RenameSheet { .. }
                | Diff::MoveSheet { .. }
                | Diff::SetSheetState { .. }
                | Diff::CreateDefinedName { .. }
                | Diff::DeleteDefinedName { .. }
                | Diff::UpdateDefinedName { .. }
                | Diff::SetLocale { .. }
                | Diff::SetTimezone { .. }
                | Diff::AddConditionalFormatting { .. }
                | Diff::DeleteConditionalFormatting { .. }
                | Diff::UpdateConditionalFormatting { .. }
                | Diff::SwapConditionalFormattingPriority { .. } => {
                    *self = PendingRecalculation::Workbook;
                }
            }
        }
    }
}
