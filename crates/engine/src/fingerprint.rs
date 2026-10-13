// What a save compares to know which sheets changed since the file on disk was read or
// written. Cells are hashed one by one and summed, so the order of IronCalc's hash maps
// does not matter. The structs are taken apart field by field so a field added to
// IronCalc fails to build here instead of going unnoticed.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::hash::{DefaultHasher, Hash, Hasher};

use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::types::{
    ArrayKind, Cell, Col, Comment, FormulaValue, Row, SheetData, SpillValue, Workbook, Worksheet,
};

use crate::error::EngineError;
use crate::row_split::{Formulas, Rows, over_formulas, over_rows};
use crate::sheet_settings::SheetSettings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SheetPrint {
    hash: u64,
    dynamic_arrays: DynamicArrays,
}

// A dynamic array formula is written with cm="1", which points into the metadata part
// IronCalc writes and another application's file may lack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DynamicArrays {
    Absent,
    Present,
}

impl SheetPrint {
    pub(crate) fn dynamic_arrays(self) -> DynamicArrays {
        self.dynamic_arrays
    }
}

struct HashWriter<'h>(&'h mut DefaultHasher);

impl fmt::Write for HashWriter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.write(text.as_bytes());
        Ok(())
    }
}

// For the values IronCalc derives no Hash for, without building their text.
fn hash_debug(hasher: &mut DefaultHasher, value: &impl fmt::Debug) -> Result<(), EngineError> {
    write!(HashWriter(hasher), "{value:?}")
        .map_err(|_| EngineError::Rejected("a value could not be fingerprinted".to_string()))
}

fn hash_formula_value(hasher: &mut DefaultHasher, value: &FormulaValue) {
    std::mem::discriminant(value).hash(hasher);
    match value {
        FormulaValue::Unevaluated => {}
        FormulaValue::Boolean(v) => v.hash(hasher),
        FormulaValue::Number(v) => v.to_bits().hash(hasher),
        FormulaValue::Text(v) => v.hash(hasher),
        FormulaValue::Error { ei, o, m } => {
            std::mem::discriminant(ei).hash(hasher);
            o.hash(hasher);
            m.hash(hasher);
        }
    }
}

fn cell_hash(row: i32, column: i32, cell: &Cell) -> u64 {
    let mut hasher = DefaultHasher::new();
    (row, column).hash(&mut hasher);
    std::mem::discriminant(cell).hash(&mut hasher);
    match cell {
        Cell::EmptyCell { s } => s.hash(&mut hasher),
        Cell::BooleanCell { v, s } => (v, s).hash(&mut hasher),
        Cell::NumberCell { v, s } => (v.to_bits(), s).hash(&mut hasher),
        Cell::ErrorCell { ei, s } => {
            std::mem::discriminant(ei).hash(&mut hasher);
            s.hash(&mut hasher);
        }
        Cell::SharedString { si, s } => (si, s).hash(&mut hasher),
        Cell::CellFormula { f, s, v } => {
            (f, s).hash(&mut hasher);
            hash_formula_value(&mut hasher, v);
        }
        Cell::ArrayFormula { f, s, r, kind, v } => {
            (f, s, r).hash(&mut hasher);
            std::mem::discriminant(kind).hash(&mut hasher);
            hash_formula_value(&mut hasher, v);
        }
        Cell::SpillCell { s, a, v } => {
            (s, a).hash(&mut hasher);
            std::mem::discriminant(v).hash(&mut hasher);
            match v {
                SpillValue::Boolean(v) => v.hash(&mut hasher),
                SpillValue::Number(v) => v.to_bits().hash(&mut hasher),
                SpillValue::Text(v) => v.hash(&mut hasher),
                SpillValue::Error(ei) => std::mem::discriminant(ei).hash(&mut hasher),
            }
        }
    }
    hasher.finish()
}

fn hash_layout(hasher: &mut DefaultHasher, cols: &[Col], rows: &[Row], comments: &[Comment]) {
    for Col {
        min,
        max,
        width,
        custom_width,
        hidden,
        style,
    } in cols
    {
        (min, max, width.to_bits(), custom_width, hidden, style).hash(hasher);
    }
    for Row {
        r,
        height,
        custom_format,
        custom_height,
        s,
        hidden,
    } in rows
    {
        (r, height.to_bits(), custom_format, custom_height, s, hidden).hash(hasher);
    }
    for Comment {
        text,
        author_name,
        author_id,
        cell_ref,
    } in comments
    {
        (text, author_name, author_id, cell_ref).hash(hasher);
    }
}

fn rows_print(rows: &Rows<'_>) -> (u64, DynamicArrays) {
    let mut cells = 0u64;
    let mut dynamic_arrays = DynamicArrays::Absent;
    for &(row, columns) in rows {
        // An empty row map writes nothing, but it moves the dimension.
        cells = cells.wrapping_add(cell_hash(*row, 0, &Cell::EmptyCell { s: -1 }));
        for (column, cell) in columns {
            cells = cells.wrapping_add(cell_hash(*row, *column, cell));
            if let Cell::ArrayFormula {
                kind: ArrayKind::Dynamic,
                ..
            } = cell
            {
                dynamic_arrays = DynamicArrays::Present;
            }
        }
    }
    (cells, dynamic_arrays)
}

// The sum does not depend on how the rows are split.
fn cells_and_formulas(
    sheet_data: &SheetData,
    formulas_hash: impl FnOnce() -> Result<u64, EngineError>,
) -> Result<(u64, DynamicArrays, u64), EngineError> {
    let (shares, formulas) = over_rows(sheet_data, rows_print, formulas_hash)?;
    let mut cells = 0u64;
    let mut dynamic_arrays = DynamicArrays::Absent;
    for (sum, arrays) in shares {
        cells = cells.wrapping_add(sum);
        if arrays == DynamicArrays::Present {
            dynamic_arrays = DynamicArrays::Present;
        }
    }
    Ok((cells, dynamic_arrays, formulas))
}

// Each formula is hashed with its index, which cells point at, and the hashes are summed,
// so the sum does not depend on how the formulas are split.
fn formulas_print(formulas: &Formulas) -> Result<u64, EngineError> {
    let shares = over_formulas(formulas, |first, share| {
        share
            .iter()
            .enumerate()
            .try_fold(0u64, |sum, (offset, formula)| {
                let mut hasher = DefaultHasher::new();
                (first + offset).hash(&mut hasher);
                hash_debug(&mut hasher, formula)?;
                Ok(sum.wrapping_add(hasher.finish()))
            })
    })?;
    shares
        .into_iter()
        .try_fold(0u64, |sum, share| Ok(sum.wrapping_add(share?)))
}

// `formulas` are the sheet's parsed formulas, which cells point into by index: a
// formula rewritten by an edit elsewhere, such as a row inserted on the sheet it reads,
// changes there and not in the cell.
pub(crate) fn sheet_print(
    sheet: &Worksheet,
    formulas: &[(Node, StaticResult)],
    settings: Option<&SheetSettings>,
) -> Result<SheetPrint, EngineError> {
    let Worksheet {
        // Recomputed on every save.
        dimension: _,
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
        // The selection is not worth rewriting a sheet for.
        views: _,
        show_grid_lines,
        conditional_formatting,
    } = sheet;
    let (cells, dynamic_arrays, formulas) =
        cells_and_formulas(sheet_data, || formulas_print(formulas))?;
    let mut hasher = DefaultHasher::new();
    (cells, formulas).hash(&mut hasher);
    hash_layout(&mut hasher, cols, rows, comments);
    (name, shared_formulas, sheet_id, merge_cells).hash(&mut hasher);
    (frozen_rows, frozen_columns, show_grid_lines).hash(&mut hasher);
    settings.hash(&mut hasher);
    hash_debug(&mut hasher, &(state, color, conditional_formatting))?;
    Ok(SheetPrint {
        hash: hasher.finish(),
        dynamic_arrays,
    })
}

// What the parts shared by every sheet depend on: the sheet list, the names, the styles
// and the rest of the workbook a save writes outside the sheets. The shared strings are
// left out; they only grow, and a save appends them.
pub(crate) fn book_print(book: &Workbook) -> Result<u64, EngineError> {
    let Workbook {
        shared_strings: _,
        defined_names,
        worksheets,
        styles,
        name,
        settings,
        metadata,
        tables,
        // The active sheet a save keeps is the file's; see `SavedFile::selected`.
        views: _,
        theme,
    } = book;
    let mut hasher = DefaultHasher::new();
    let sheets: Vec<_> = worksheets
        .iter()
        .map(|sheet| (&sheet.name, sheet.sheet_id, &sheet.state))
        .collect();
    let tables: BTreeMap<_, _> = tables.iter().collect();
    hash_debug(
        &mut hasher,
        &(&sheets, defined_names, styles, &tables, name),
    )?;
    hash_debug(&mut hasher, &(settings, metadata, theme))?;
    Ok(hasher.finish())
}

#[cfg(test)]
mod tests {
    use ironcalc::base::Model;

    use super::{SheetPrint, book_print, sheet_print};

    fn model() -> Model<'static> {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        model.set_user_input(0, 1, 1, "1".to_string()).unwrap();
        model.set_user_input(0, 2, 1, "=A1*2".to_string()).unwrap();
        model.new_sheet();
        model
            .set_user_input(1, 1, 1, "=Sheet1!A1".to_string())
            .unwrap();
        model.evaluate();
        model
    }

    fn print(model: &Model, sheet: usize) -> SheetPrint {
        sheet_print(
            &model.workbook.worksheets[sheet],
            &model.parsed_formulas[sheet],
            None,
        )
        .unwrap()
    }

    #[test]
    fn a_value_style_or_formula_change_shows() {
        let base = model();
        let mut edited = model();
        assert_eq!(print(&base, 0), print(&edited, 0));
        edited.set_user_input(0, 1, 1, "2".to_string()).unwrap();
        edited.evaluate();
        assert_ne!(print(&base, 0), print(&edited, 0));
        let mut styled = model();
        styled
            .set_cell_style(0, 1, 1, &styled.get_style_for_cell(0, 2, 1).unwrap())
            .unwrap();
        styled.workbook.worksheets[0].frozen_rows = 1;
        assert_ne!(print(&base, 0), print(&styled, 0));
        assert_eq!(
            book_print(&base.workbook).unwrap(),
            book_print(&model().workbook).unwrap()
        );
    }

    #[test]
    fn a_sheet_hashed_on_several_threads_matches_one_pass() {
        let mut model = model();
        let rows = i32::try_from(crate::row_split::MIN_ROWS_PER_THREAD * 3).unwrap();
        for row in 1..=rows {
            model.set_user_input(0, row, 3, row.to_string()).unwrap();
        }
        let sheet = &model.workbook.worksheets[0];
        let all: Vec<_> = sheet.sheet_data.iter().collect();
        let (one_pass, _) = super::rows_print(&all);
        let (split, _, _) = super::cells_and_formulas(&sheet.sheet_data, || Ok(0)).unwrap();
        assert_eq!(split, one_pass);
        let before = print(&model, 0);
        model.set_user_input(0, rows, 3, "x".to_string()).unwrap();
        assert_ne!(before, print(&model, 0));
    }

    #[test]
    fn formulas_split_over_threads_still_count_by_index() {
        let mut model = model();
        let count = i32::try_from(crate::row_split::MIN_FORMULAS_PER_THREAD * 2).unwrap();
        for row in 1..=count {
            model
                .set_user_input(0, row, 4, format!("=A1+{row}"))
                .unwrap();
        }
        let before = print(&model, 0);
        let formulas = &mut model.parsed_formulas[0];
        let last = formulas.len() - 1;
        formulas.swap(1, last);
        assert_ne!(before, print(&model, 0));
        model.parsed_formulas[0].swap(1, last);
        assert_eq!(before, print(&model, 0));
    }

    #[test]
    fn a_row_inserted_on_a_read_sheet_changes_the_reader() {
        let base = model();
        let mut moved = model();
        moved.insert_rows(0, 1, 1).unwrap();
        moved.evaluate();
        assert_ne!(print(&base, 1), print(&moved, 1));
    }
}
