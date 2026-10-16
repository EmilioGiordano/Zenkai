use std::collections::HashMap;

use bitcode::{Decode, Encode};

use crate::{
    cf_types::CfRule,
    expressions::types::Area,
    types::{Cell, Col, Color, Row, SheetState, Style, StyleIncludes, Theme, Worksheet},
};

#[derive(Clone, Encode, Decode)]
pub(crate) struct RowData {
    pub(crate) row: Option<Row>,
    pub(crate) data: HashMap<i32, Cell>,
}

#[derive(Clone, Encode, Decode)]
pub(crate) struct ColumnData {
    pub(crate) column: Option<Col>,
    pub(crate) data: HashMap<i32, Cell>,
}

#[derive(Clone, Encode, Decode)]
pub(crate) enum Diff {
    // Cell diffs
    SetCellValue {
        sheet: u32,
        row: i32,
        column: i32,
        new_value: String,
        old_value: Box<Option<Cell>>,
    },
    // The formula of the cell at `source_row`, `source_column` written over the area; redo
    // reads it from there again, as the cell is outside the area and unchanged.
    FillFormula {
        sheet: u32,
        source_row: i32,
        source_column: i32,
        row: i32,
        column: i32,
        width: i32,
        height: i32,
        old_values: Vec<Option<Cell>>,
    },
    SetArrayValue {
        sheet: u32,
        row: i32,
        column: i32,
        width: i32,
        height: i32,
        new_value: String,
        old_values: Vec<Vec<Option<Cell>>>,
    },
    RangeClearContents {
        sheet: u32,
        row: i32,
        column: i32,
        width: i32,
        height: i32,
        old_value: Vec<Vec<Option<Cell>>>,
    },
    RangeClearAll {
        sheet: u32,
        row: i32,
        column: i32,
        width: i32,
        height: i32,
        old_value: Vec<Vec<Option<Cell>>>,
        old_style: Vec<Vec<Style>>,
    },
    CellClearFormatting {
        sheet: u32,
        row: i32,
        column: i32,
        old_style: Box<Option<Style>>,
    },
    SetCellStyle {
        sheet: u32,
        row: i32,
        column: i32,
        old_value: Box<Option<Style>>,
        new_value: Box<Style>,
    },
    // Unlike `SetCellStyle`, applying a named style is recorded by name so that
    // redo re-links the cell to the style instead of copying its formatting.
    ApplyNamedStyle {
        sheet: u32,
        row: i32,
        column: i32,
        old_value: Box<Option<Style>>,
        name: String,
    },
    // Column and Row diffs
    SetColumnWidth {
        sheet: u32,
        column: i32,
        new_value: f64,
        old_value: f64,
    },
    SetColumnHidden {
        sheet: u32,
        column: i32,
        new_value: bool,
        old_value: bool,
    },
    SetRowHeight {
        sheet: u32,
        row: i32,
        new_value: f64,
        old_value: f64,
    },
    SetRowHidden {
        sheet: u32,
        row: i32,
        new_value: bool,
        old_value: bool,
    },
    SetColumnStyle {
        sheet: u32,
        column: i32,
        old_value: Box<Option<Style>>,
        new_value: Box<Style>,
    },
    SetRowStyle {
        sheet: u32,
        row: i32,
        old_value: Box<Option<Style>>,
        new_value: Box<Style>,
    },
    DeleteColumnStyle {
        sheet: u32,
        column: i32,
        old_value: Box<Option<Style>>,
    },
    DeleteRowStyle {
        sheet: u32,
        row: i32,
        old_value: Box<Option<Style>>,
    },
    InsertRows {
        sheet: u32,
        row: i32,
        count: i32,
    },
    DeleteRows {
        sheet: u32,
        row: i32,
        count: i32,
        old_data: Vec<RowData>,
    },
    InsertColumns {
        sheet: u32,
        column: i32,
        count: i32,
    },
    DeleteColumns {
        sheet: u32,
        column: i32,
        count: i32,
        old_data: Vec<ColumnData>,
    },
    DeleteSheet {
        sheet: u32,
        old_data: Box<Worksheet>,
    },
    SetFrozenRowsCount {
        sheet: u32,
        new_value: i32,
        old_value: i32,
    },
    SetFrozenColumnsCount {
        sheet: u32,
        new_value: i32,
        old_value: i32,
    },
    NewSheet {
        index: u32,
        name: String,
    },
    DuplicateSheet {
        /// Index of the sheet that was duplicated.
        source_index: u32,
        /// Index of the resulting copy (always `source_index + 1`).
        new_index: u32,
    },
    RenameSheet {
        index: u32,
        old_value: String,
        new_value: String,
    },
    MoveSheet {
        /// Index of the worksheet before the move.
        sheet_index: u32,
        /// Index the worksheet was moved to.
        new_index: u32,
    },
    SetSheetColor {
        index: u32,
        old_value: Color,
        new_value: Color,
    },
    SetSheetState {
        index: u32,
        old_value: SheetState,
        new_value: SheetState,
    },
    SetShowGridLines {
        sheet: u32,
        old_value: bool,
        new_value: bool,
    },
    SetTheme {
        old_value: Box<Theme>,
        new_value: Box<Theme>,
    },
    CreateDefinedName {
        name: String,
        scope: Option<u32>,
        value: String,
    },
    DeleteDefinedName {
        name: String,
        scope: Option<u32>,
        old_value: String,
    },
    UpdateDefinedName {
        name: String,
        scope: Option<u32>,
        old_formula: String,
        new_name: String,
        new_scope: Option<u32>,
        new_formula: String,
    },
    MoveColumns {
        sheet: u32,
        column: i32,
        column_count: i32,
        delta: i32,
    },
    MoveRows {
        sheet: u32,
        row: i32,
        row_count: i32,
        delta: i32,
    },
    SetLocale {
        old_value: String,
        new_value: String,
    },
    SetWorkbookName {
        old_value: String,
        new_value: String,
    },
    SetTimezone {
        old_value: String,
        new_value: String,
    },
    // Named style diffs
    CreateNamedStyle {
        name: String,
        style: Box<Style>,
        includes: StyleIncludes,
    },
    DeleteNamedStyle {
        name: String,
        old_xf_id: i32,
    },
    UpdateNamedStyle {
        name: String,
        new_name: String,
        old_style: Box<Style>,
        new_style: Box<Style>,
        old_includes: StyleIncludes,
        new_includes: StyleIncludes,
    },
    // Conditional formatting diffs
    AddConditionalFormatting {
        sheet: u32,
        range: String,
        rule: Box<CfRule>,
        priority: u32,
    },
    DeleteConditionalFormatting {
        sheet: u32,
        index: u32,
        old_range: String,
        old_rule: Box<CfRule>,
        old_priority: u32,
    },
    UpdateConditionalFormatting {
        sheet: u32,
        index: u32,
        old_range: String,
        old_rule: Box<CfRule>,
        old_priority: u32,
        new_range: String,
        new_rule: Box<CfRule>,
    },
    /// Swaps the priorities of the two CF rules at `index_a` and `index_b`.
    /// `priority_a`/`priority_b` are their priorities *before* the swap.
    SwapConditionalFormattingPriority {
        sheet: u32,
        index_a: u32,
        index_b: u32,
        priority_a: u32,
        priority_b: u32,
    },
    // FIXME: we are missing SetViewDiffs
}

pub(crate) type DiffList = Vec<Diff>;

/// The cells an undo or redo step can change. See [crate::UserModel::undo_cells].
#[derive(Default)]
pub struct StepCells {
    /// Areas whose cells the step writes, clears or restyles. Clearing a dynamic array
    /// anchor also clears the cells it spills into, which are not listed.
    pub areas: Vec<Area>,
    /// Sheets whose rows or columns the step inserts, deletes or moves, which can move
    /// any of their cells. Row and column sizes, visibility and styles, panes, grid
    /// lines, sheet colour and state and conditional formatting add, move or remove no
    /// cell and are not listed.
    pub sheets: Vec<u32>,
}

fn cell_area(sheet: u32, row: i32, column: i32, width: i32, height: i32) -> Area {
    Area {
        sheet,
        row,
        column,
        width,
        height,
    }
}

// `None` when the step adds, removes, renames or reorders sheets, or changes something
// every sheet depends on, such as defined names, styles or the locale.
pub(crate) fn step_cells(diff_list: &[Diff]) -> Option<StepCells> {
    let mut cells = StepCells::default();
    for diff in diff_list {
        match diff {
            Diff::SetCellValue {
                sheet, row, column, ..
            }
            | Diff::CellClearFormatting {
                sheet, row, column, ..
            }
            | Diff::SetCellStyle {
                sheet, row, column, ..
            }
            | Diff::ApplyNamedStyle {
                sheet, row, column, ..
            } => cells.areas.push(cell_area(*sheet, *row, *column, 1, 1)),
            Diff::SetArrayValue {
                sheet,
                row,
                column,
                width,
                height,
                ..
            }
            | Diff::RangeClearContents {
                sheet,
                row,
                column,
                width,
                height,
                ..
            }
            | Diff::RangeClearAll {
                sheet,
                row,
                column,
                width,
                height,
                ..
            } => cells
                .areas
                .push(cell_area(*sheet, *row, *column, *width, *height)),
            Diff::InsertRows { sheet, .. }
            | Diff::DeleteRows { sheet, .. }
            | Diff::InsertColumns { sheet, .. }
            | Diff::DeleteColumns { sheet, .. }
            | Diff::MoveColumns { sheet, .. }
            | Diff::MoveRows { sheet, .. } => cells.sheets.push(*sheet),
            Diff::SetColumnWidth { .. }
            | Diff::SetColumnHidden { .. }
            | Diff::SetRowHeight { .. }
            | Diff::SetRowHidden { .. }
            | Diff::SetColumnStyle { .. }
            | Diff::SetRowStyle { .. }
            | Diff::DeleteColumnStyle { .. }
            | Diff::DeleteRowStyle { .. }
            | Diff::SetFrozenRowsCount { .. }
            | Diff::SetFrozenColumnsCount { .. }
            | Diff::SetShowGridLines { .. }
            | Diff::AddConditionalFormatting { .. }
            | Diff::DeleteConditionalFormatting { .. }
            | Diff::UpdateConditionalFormatting { .. }
            | Diff::SwapConditionalFormattingPriority { .. }
            | Diff::SetSheetColor { .. }
            | Diff::SetSheetState { .. } => {}
            Diff::DeleteSheet { .. }
            | Diff::NewSheet { .. }
            | Diff::DuplicateSheet { .. }
            | Diff::RenameSheet { .. }
            | Diff::MoveSheet { .. }
            | Diff::SetTheme { .. }
            | Diff::CreateDefinedName { .. }
            | Diff::DeleteDefinedName { .. }
            | Diff::UpdateDefinedName { .. }
            | Diff::SetLocale { .. }
            | Diff::SetWorkbookName { .. }
            | Diff::SetTimezone { .. }
            | Diff::CreateNamedStyle { .. }
            | Diff::DeleteNamedStyle { .. }
            | Diff::UpdateNamedStyle { .. } => return None,
        }
    }
    Some(cells)
}

#[derive(Default)]
pub(crate) struct History {
    pub(crate) undo_stack: Vec<DiffList>,
    pub(crate) redo_stack: Vec<DiffList>,
}

impl History {
    pub fn push(&mut self, diff_list: DiffList) {
        self.undo_stack.push(diff_list);
        self.redo_stack = vec![];
    }

    pub fn undo(&mut self) -> Option<Vec<Diff>> {
        match self.undo_stack.pop() {
            Some(diff_list) => {
                self.redo_stack.push(diff_list.clone());
                Some(diff_list)
            }
            None => None,
        }
    }

    pub fn redo(&mut self) -> Option<Vec<Diff>> {
        match self.redo_stack.pop() {
            Some(diff_list) => {
                self.undo_stack.push(diff_list.clone());
                Some(diff_list)
            }
            None => None,
        }
    }
}

#[derive(Clone, Encode, Decode)]
pub enum DiffType {
    Undo,
    Redo,
}

#[derive(Clone, Encode, Decode)]
pub struct QueueDiffs {
    pub r#type: DiffType,
    pub list: DiffList,
}
