// What a save compares to know which sheets, and which rows of them, changed since the
// file on disk was read or written. Cells are hashed one by one and summed, so the order
// of IronCalc's hash maps does not matter. The structs are taken apart field by field so
// a field added to IronCalc fails to build here instead of going unnoticed.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::hash::{DefaultHasher, Hash, Hasher};

use ironcalc::base::cell_store::CellStore;
use ironcalc::base::types::{
    Cell, Col, Comment, FormulaValue, Row, SpillValue, Workbook, Worksheet,
};

use crate::error::EngineError;
use crate::row_split::{Formulas, over_formulas, over_rows};
use crate::sheet_settings::SheetSettings;

// Rows are compared in blocks this tall, so a save can tell which rows changed.
pub(crate) const ROWS_PER_BLOCK: u32 = 16;
pub(crate) const MAX_ROW: u32 = 1_048_576;
// A sheet used down to Excel's last row has 1,048,576 / 16 = 65,536 blocks of 8 bytes,
// 512 KiB; the budget keeps the blocks of 64 such sheets. Sheets past it are rewritten
// whole when they change.
pub(crate) const ROW_PRINT_BUDGET_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct SheetPrint {
    hash: u64,
    // Everything a sheet writes outside its rows.
    frame: u64,
    rows: RowBlocks,
    bounds: UsedBounds,
    dynamic_arrays: DynamicArrays,
    // Kept out of the hash: an insert undone before a save leaves the part as it was.
    moves: u64,
}

// What IronCalc's `Worksheet::dimension` reports, found in the pass that hashes the rows
// so a row-level save does not walk every cell again. A sheet without cells is A1:A1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UsedBounds {
    pub(crate) min_row: i32,
    pub(crate) max_row: i32,
    pub(crate) min_column: i32,
    pub(crate) max_column: i32,
}

type Reach = Option<(i32, i32)>;

fn widen(reach: &mut Reach, value: i32) {
    *reach = Some(match *reach {
        Some((low, high)) => (low.min(value), high.max(value)),
        None => (value, value),
    });
}

fn join(reach: &mut Reach, other: Reach) {
    for value in other.into_iter().flat_map(|(low, high)| [low, high]) {
        widen(reach, value);
    }
}

// Rows count even without cells, but a sheet whose rows hold no cell has no columns.
fn used_bounds(rows: Reach, columns: Reach) -> UsedBounds {
    match rows.zip(columns) {
        Some(((min_row, max_row), (min_column, max_column))) => UsedBounds {
            min_row,
            max_row,
            min_column,
            max_column,
        },
        None => UsedBounds {
            min_row: 1,
            max_row: 1,
            min_column: 1,
            max_column: 1,
        },
    }
}

#[derive(Clone, Debug)]
enum RowBlocks {
    // The hash of each block of ROWS_PER_BLOCK rows from row 1; an empty block is zero.
    Hashed(Vec<u64>),
    Unavailable,
}

// A dynamic array formula is written with cm="1", which points into the metadata part
// IronCalc writes and another application's file may lack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DynamicArrays {
    Absent,
    Present,
}

impl SheetPrint {
    pub(crate) fn bounds(&self) -> UsedBounds {
        self.bounds
    }

    pub(crate) fn dynamic_arrays(&self) -> DynamicArrays {
        self.dynamic_arrays
    }

    // Whether the sheet's part must be written again; the row blocks only say where.
    pub(crate) fn changed_since(&self, saved: &SheetPrint) -> bool {
        self.hash != saved.hash
    }

    pub(crate) fn same_frame(&self, saved: &SheetPrint) -> bool {
        self.frame == saved.frame
    }

    // Whether rows or columns may have moved since `saved`, which shifts what the file
    // holds beyond the model (validations, hyperlinks, drawings) and a copy would keep.
    pub(crate) fn moved_since(&self, saved: &SheetPrint) -> bool {
        self.moves != saved.moves
    }

    pub(crate) fn row_blocks(&self) -> Option<&[u64]> {
        match &self.rows {
            RowBlocks::Hashed(blocks) => Some(blocks),
            RowBlocks::Unavailable => None,
        }
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
        FormulaValue::Error(error) => {
            std::mem::discriminant(&error.ei).hash(hasher);
            error.o.hash(hasher);
            error.m.hash(hasher);
        }
    }
}

// `formulas` holds the hash of each of the sheet's parsed formulas, by index.
fn formula_hash(formulas: &[u64], index: i32) -> Result<u64, EngineError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| formulas.get(index))
        .copied()
        .ok_or_else(|| EngineError::Rejected("a cell points at a missing formula".to_string()))
}

// A formula is hashed by its content, not its index: a formula rewritten by an edit
// elsewhere, such as a row inserted on the sheet it reads, changes there and not in the
// cell, and the rows that use it must show the change.
fn cell_hash(row: i32, column: i32, cell: &Cell, formulas: &[u64]) -> Result<u64, EngineError> {
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
            (formula_hash(formulas, *f)?, s).hash(&mut hasher);
            hash_formula_value(&mut hasher, v);
        }
        Cell::ArrayFormula(array) => {
            (formula_hash(formulas, array.f)?, array.s, array.r).hash(&mut hasher);
            std::mem::discriminant(&array.kind).hash(&mut hasher);
            hash_formula_value(&mut hasher, &array.v);
        }
        Cell::SpillCell(spill) => {
            (spill.s, spill.a).hash(&mut hasher);
            std::mem::discriminant(&spill.v).hash(&mut hasher);
            match &spill.v {
                SpillValue::Boolean(v) => v.hash(&mut hasher),
                SpillValue::Number(v) => v.to_bits().hash(&mut hasher),
                SpillValue::Text(v) => v.hash(&mut hasher),
                SpillValue::Error(ei) => std::mem::discriminant(ei).hash(&mut hasher),
            }
        }
    }
    Ok(hasher.finish())
}

fn row_style_hash(row: &Row) -> u64 {
    let Row {
        r,
        height,
        custom_format,
        custom_height,
        s,
        hidden,
    } = row;
    let mut hasher = DefaultHasher::new();
    "row style".hash(&mut hasher);
    (r, height.to_bits(), custom_format, custom_height, s, hidden).hash(&mut hasher);
    hasher.finish()
}

fn hash_frame(hasher: &mut DefaultHasher, cols: &[Col], comments: &[Comment]) {
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

struct RowsShare {
    rows: Vec<(i32, u64)>,
    row_reach: Reach,
    column_reach: Reach,
    dynamic_arrays: DynamicArrays,
}

fn rows_print(
    sheet_data: &CellStore,
    rows: &[i32],
    formulas: &[u64],
) -> Result<RowsShare, EngineError> {
    let mut hashed = Vec::with_capacity(rows.len());
    let mut dynamic_arrays = DynamicArrays::Absent;
    let (mut row_reach, mut column_reach) = (None, None);
    for &row in rows {
        widen(&mut row_reach, row);
        // An empty row map writes nothing, but it moves the dimension.
        let mut sum = cell_hash(row, 0, &Cell::EmptyCell { s: -1 }, formulas)?;
        for (column, cell) in sheet_data.row_cells(row) {
            sum = sum.wrapping_add(cell_hash(row, column, cell, formulas)?);
            widen(&mut column_reach, column);
            if cell.dynamic_range().is_some() {
                dynamic_arrays = DynamicArrays::Present;
            }
        }
        hashed.push((row, sum));
    }
    Ok(RowsShare {
        rows: hashed,
        row_reach,
        column_reach,
        dynamic_arrays,
    })
}

fn formula_hashes(formulas: &Formulas) -> Result<Vec<u64>, EngineError> {
    let shares = over_formulas(formulas, |_, share| {
        share
            .iter()
            .map(|formula| {
                let mut hasher = DefaultHasher::new();
                hash_debug(&mut hasher, formula)?;
                Ok(hasher.finish())
            })
            .collect::<Result<Vec<u64>, EngineError>>()
    })?;
    let mut hashes = Vec::with_capacity(formulas.len());
    for share in shares {
        hashes.extend(share?);
    }
    Ok(hashes)
}

fn block_of(row: i32) -> Option<usize> {
    let row = u32::try_from(row)
        .ok()
        .filter(|row| (1..=MAX_ROW).contains(row))?;
    usize::try_from((row - 1) / ROWS_PER_BLOCK).ok()
}

// A whole rewrite drops rows outside Excel's grid, so a sheet holding one has no blocks.
fn row_blocks(rows: &[(i32, u64)], budget: &mut usize) -> RowBlocks {
    let blocks: Option<Vec<usize>> = rows.iter().map(|(row, _)| block_of(*row)).collect();
    let Some(blocks) = blocks else {
        return RowBlocks::Unavailable;
    };
    let count = blocks.iter().max().map_or(0, |last| last + 1);
    let bytes = count.saturating_mul(size_of::<u64>());
    if bytes > *budget {
        return RowBlocks::Unavailable;
    }
    *budget -= bytes;
    let mut hashes = vec![0u64; count];
    for (block, (_, hash)) in blocks.into_iter().zip(rows) {
        hashes[block] = hashes[block].wrapping_add(*hash);
    }
    RowBlocks::Hashed(hashes)
}

// `budget` is what is left of ROW_PRINT_BUDGET_BYTES for the workbook's row blocks;
// `moves` counts the row and column moves the sheet may have had, see `Carried::moves`.
pub(crate) fn sheet_print(
    sheet: &Worksheet,
    formulas: &Formulas,
    settings: Option<&SheetSettings>,
    moves: u64,
    budget: &mut usize,
) -> Result<SheetPrint, EngineError> {
    let Worksheet {
        // Recomputed on every save.
        dimension: _,
        cols,
        rows,
        name,
        sheet_data,
        // Which formulas share a text, which the parsed formulas tell too; it grows with
        // every formula entered, so it would make every edit change the frame.
        shared_formulas: _,
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
    let formulas = formula_hashes(formulas)?;
    let (shares, ()) = over_rows(
        sheet_data,
        |sheet_data, rows| rows_print(sheet_data, rows, &formulas),
        || Ok(()),
    )?;
    let mut hashed = Vec::with_capacity(sheet_data.row_count() + rows.len());
    let mut dynamic_arrays = DynamicArrays::Absent;
    let (mut row_reach, mut column_reach) = (None, None);
    for share in shares {
        let share = share?;
        join(&mut row_reach, share.row_reach);
        join(&mut column_reach, share.column_reach);
        hashed.extend(share.rows);
        if share.dynamic_arrays == DynamicArrays::Present {
            dynamic_arrays = DynamicArrays::Present;
        }
    }
    hashed.extend(rows.iter().map(|row| (row.r, row_style_hash(row))));
    let cells = hashed
        .iter()
        .fold(0u64, |sum, (_, hash)| sum.wrapping_add(*hash));
    let mut frame = DefaultHasher::new();
    hash_frame(&mut frame, cols, comments);
    (name, sheet_id, merge_cells).hash(&mut frame);
    (frozen_rows, frozen_columns, show_grid_lines).hash(&mut frame);
    settings.hash(&mut frame);
    hash_debug(&mut frame, &(state, color, conditional_formatting))?;
    let frame = frame.finish();
    let mut hasher = DefaultHasher::new();
    (frame, cells).hash(&mut hasher);
    Ok(SheetPrint {
        hash: hasher.finish(),
        frame,
        rows: row_blocks(&hashed, budget),
        bounds: used_bounds(row_reach, column_reach),
        dynamic_arrays,
        moves,
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
    use ironcalc::base::types::Row;

    use super::{
        MAX_ROW, ROW_PRINT_BUDGET_BYTES, ROWS_PER_BLOCK, RowBlocks, SheetPrint, UsedBounds,
        book_print, formula_hashes, row_blocks, rows_print, sheet_print,
    };

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

    fn print_within(model: &Model, sheet: usize, budget: usize) -> SheetPrint {
        let mut budget = budget;
        sheet_print(
            &model.workbook.worksheets[sheet],
            &model.parsed_formulas[sheet],
            None,
            0,
            &mut budget,
        )
        .unwrap()
    }

    fn print(model: &Model, sheet: usize) -> SheetPrint {
        print_within(model, sheet, ROW_PRINT_BUDGET_BYTES)
    }

    fn changed(before: &SheetPrint, after: &SheetPrint) -> bool {
        after.changed_since(before)
    }

    fn dirty_blocks(before: &SheetPrint, after: &SheetPrint) -> Vec<usize> {
        let (before, after) = (before.row_blocks().unwrap(), after.row_blocks().unwrap());
        (0..before.len().max(after.len()))
            .filter(|&block| before.get(block) != after.get(block))
            .collect()
    }

    fn block(row: u32) -> usize {
        ((row - 1) / ROWS_PER_BLOCK) as usize
    }

    #[test]
    fn a_value_style_or_formula_change_shows() {
        let base = model();
        let mut edited = model();
        assert!(!changed(&print(&base, 0), &print(&edited, 0)));
        edited.set_user_input(0, 1, 1, "2".to_string()).unwrap();
        edited.evaluate();
        assert!(changed(&print(&base, 0), &print(&edited, 0)));
        let mut styled = model();
        styled
            .set_cell_style(0, 1, 1, &styled.get_style_for_cell(0, 2, 1).unwrap())
            .unwrap();
        styled.workbook.worksheets[0].frozen_rows = 1;
        assert!(changed(&print(&base, 0), &print(&styled, 0)));
        assert_eq!(
            book_print(&base.workbook).unwrap(),
            book_print(&model().workbook).unwrap()
        );
    }

    #[test]
    fn an_edit_dirties_only_the_block_of_its_row() {
        let mut model = model();
        for row in 1..=100 {
            model.set_user_input(0, row, 3, row.to_string()).unwrap();
        }
        model.evaluate();
        let before = print(&model, 0);
        model.set_user_input(0, 40, 3, "x".to_string()).unwrap();
        model.evaluate();
        let after = print(&model, 0);
        assert!(changed(&before, &after));
        assert!(after.same_frame(&before));
        assert_eq!(dirty_blocks(&before, &after), [block(40)]);
    }

    #[test]
    fn a_recalculated_value_dirties_the_block_of_its_formula() {
        let mut model = model();
        model.set_user_input(0, 70, 2, "=A1+1".to_string()).unwrap();
        model.evaluate();
        let before = print(&model, 0);
        model.set_user_input(0, 1, 1, "5".to_string()).unwrap();
        model.evaluate();
        let after = print(&model, 0);
        assert_eq!(dirty_blocks(&before, &after), [block(1), block(70)]);
    }

    #[test]
    fn a_row_height_dirties_its_block_and_a_merge_the_frame() {
        let base = model();
        let mut tall = model();
        tall.workbook.worksheets[0].rows.push(Row {
            r: 20,
            height: 30.0,
            custom_format: false,
            custom_height: true,
            s: 0,
            hidden: false,
        });
        let (before, after) = (print(&base, 0), print(&tall, 0));
        assert!(changed(&before, &after));
        assert!(after.same_frame(&before));
        assert_eq!(dirty_blocks(&before, &after), [block(20)]);
        let mut merged = model();
        merged.workbook.worksheets[0]
            .merge_cells
            .push("A1:B1".to_string());
        assert!(!print(&merged, 0).same_frame(&before));
    }

    #[test]
    fn a_sheet_hashed_on_several_threads_matches_one_pass() {
        let mut model = model();
        let rows = i32::try_from(crate::row_split::MIN_ROWS_PER_THREAD * 3).unwrap();
        for row in 1..=rows {
            model.set_user_input(0, row, 3, row.to_string()).unwrap();
        }
        let sheet = &model.workbook.worksheets[0];
        let all: Vec<i32> = sheet.sheet_data.rows().collect();
        let formulas = formula_hashes(&model.parsed_formulas[0]).unwrap();
        let one_pass = rows_print(&sheet.sheet_data, &all, &formulas).unwrap();
        let mut budget = usize::MAX;
        let RowBlocks::Hashed(expected) = row_blocks(&one_pass.rows, &mut budget) else {
            panic!("the rows fit");
        };
        let before = print(&model, 0);
        assert_eq!(before.row_blocks().unwrap(), expected.as_slice());
        model.set_user_input(0, rows, 3, "x".to_string()).unwrap();
        assert!(changed(&before, &print(&model, 0)));
    }

    #[test]
    fn a_formula_counts_by_its_content() {
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
        assert!(changed(&before, &print(&model, 0)));
        model.parsed_formulas[0].swap(1, last);
        assert!(!changed(&before, &print(&model, 0)));
    }

    #[test]
    fn a_row_inserted_on_a_read_sheet_changes_the_row_that_reads_it() {
        let base = model();
        let mut moved = model();
        moved.insert_rows(0, 1, 1).unwrap();
        moved.evaluate();
        let (before, after) = (print(&base, 1), print(&moved, 1));
        assert!(changed(&before, &after));
        assert_eq!(dirty_blocks(&before, &after), [block(1)]);
    }

    #[test]
    fn a_row_outside_the_grid_leaves_the_sheet_without_blocks() {
        let mut model = model();
        model.workbook.worksheets[0]
            .sheet_data
            .insert_row(2_000_000, []);
        assert!(print(&model, 0).row_blocks().is_none());
    }

    #[test]
    fn the_blocks_of_a_sheet_fit_the_budget_exactly_or_not_at_all() {
        let mut model = model();
        let last = i32::try_from(MAX_ROW).unwrap();
        model.set_user_input(0, last, 1, "1".to_string()).unwrap();
        let full_height = (MAX_ROW / ROWS_PER_BLOCK) as usize * size_of::<u64>();
        assert_eq!(full_height, 512 * 1024);
        assert_eq!(ROW_PRINT_BUDGET_BYTES / full_height, 64);
        let fits = print_within(&model, 0, full_height);
        assert_eq!(fits.row_blocks().unwrap().len(), 65_536);
        let short = print_within(&model, 0, full_height - 1);
        assert!(short.row_blocks().is_none());
        assert!(!changed(&fits, &short), "the budget never changes the hash");
    }

    #[test]
    fn the_workbook_budget_holds_sixty_four_full_height_sheets() {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        let last = i32::try_from(MAX_ROW).unwrap();
        for sheet in 0..65 {
            if sheet > 0 {
                model.new_sheet();
            }
            model
                .set_user_input(sheet, last, 1, "1".to_string())
                .unwrap();
        }
        let carried = crate::sheet_settings::Carried::default();
        let prints =
            crate::saved::sheet_prints(crate::save::ModelData::of(&model), &carried).unwrap();
        assert!(
            prints[..64]
                .iter()
                .all(|print| print.row_blocks().is_some())
        );
        assert!(prints[64].row_blocks().is_none());
    }

    fn walked(model: &Model, sheet: usize) -> UsedBounds {
        let dimension = model.workbook.worksheets[sheet].dimension();
        UsedBounds {
            min_row: dimension.min_row,
            max_row: dimension.max_row,
            min_column: dimension.min_column,
            max_column: dimension.max_column,
        }
    }

    #[test]
    fn the_bounds_found_while_hashing_equal_the_walked_dimension() {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        assert_eq!(print(&model, 0).bounds(), walked(&model, 0));
        model.workbook.worksheets[0].sheet_data.insert_row(40, []);
        assert_eq!(print(&model, 0).bounds(), walked(&model, 0));
        model.set_user_input(0, 7, 5, "1".to_string()).unwrap();
        assert_eq!(print(&model, 0).bounds(), walked(&model, 0));
        assert_eq!(
            print(&model, 0).bounds(),
            UsedBounds {
                min_row: 7,
                max_row: 40,
                min_column: 5,
                max_column: 5
            }
        );
        for row in (3..4000).step_by(7) {
            model
                .set_user_input(0, row, 2 + row % 9, row.to_string())
                .unwrap();
        }
        model.set_user_input(0, 3000, 1, "x".to_string()).unwrap();
        assert_eq!(print(&model, 0).bounds(), walked(&model, 0));
        let fresh = Model::new_empty("b", "en", "UTC", "en").unwrap();
        assert_eq!(print(&fresh, 0).bounds(), walked(&fresh, 0));
    }
}
