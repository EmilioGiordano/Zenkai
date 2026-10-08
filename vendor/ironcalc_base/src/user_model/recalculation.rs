use crate::{
    expressions::types::Area,
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
            for row in area.row..area.row + area.height {
                for column in area.column..area.column + area.width {
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

    // The same cells change whether `diff_list` is applied or undone.
    pub(crate) fn add(&mut self, diff_list: &DiffList) {
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
                Diff::CellClearFormatting { .. }
                | Diff::SetCellStyle { .. }
                | Diff::ApplyNamedStyle { .. }
                | Diff::SetColumnWidth { .. }
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
                // SUBTOTAL skips hidden rows, so hiding is not only presentation.
                Diff::SetColumnHidden { .. }
                | Diff::SetRowHidden { .. }
                | Diff::SetArrayValue { .. }
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
