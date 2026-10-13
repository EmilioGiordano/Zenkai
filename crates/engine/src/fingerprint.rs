// What a save compares to know which sheets changed since the file on disk was read or
// written. Cells are hashed one by one and summed, so the order of IronCalc's hash maps
// does not matter. The structs are taken apart field by field so a field added to
// IronCalc fails to build here instead of going unnoticed.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::types::{
    ArrayKind, Cell, Col, Comment, FormulaValue, Row, SpillValue, Workbook, Worksheet,
};

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

// For the small values IronCalc derives no Hash for.
fn hash_debug(hasher: &mut DefaultHasher, value: &impl std::fmt::Debug) {
    format!("{value:?}").hash(hasher);
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

// `formulas` are the sheet's parsed formulas, which cells point into by index: a
// formula rewritten by an edit elsewhere, such as a row inserted on the sheet it reads,
// changes there and not in the cell.
pub(crate) fn sheet_print(
    sheet: &Worksheet,
    formulas: &[(Node, StaticResult)],
    settings: Option<&SheetSettings>,
) -> SheetPrint {
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
    let mut cells = 0u64;
    let mut dynamic_arrays = DynamicArrays::Absent;
    for (row, columns) in sheet_data {
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
    let mut hasher = DefaultHasher::new();
    cells.hash(&mut hasher);
    hash_layout(&mut hasher, cols, rows, comments);
    for formula in formulas {
        hash_debug(&mut hasher, formula);
    }
    (name, shared_formulas, sheet_id, merge_cells).hash(&mut hasher);
    (frozen_rows, frozen_columns, show_grid_lines).hash(&mut hasher);
    settings.hash(&mut hasher);
    hash_debug(&mut hasher, &(state, color, conditional_formatting));
    SheetPrint {
        hash: hasher.finish(),
        dynamic_arrays,
    }
}

// What the parts shared by every sheet depend on: the sheet list, the names, the styles
// and the rest of the workbook a save writes outside the sheets. The shared strings are
// left out; they only grow, and a save appends them.
pub(crate) fn book_print(book: &Workbook) -> u64 {
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
    );
    hash_debug(&mut hasher, &(settings, metadata, theme));
    hasher.finish()
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
        assert_eq!(book_print(&base.workbook), book_print(&model().workbook));
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
