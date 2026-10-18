// A changed sheet whose frame did not change is written by copying the rows of its file
// part that did not change and writing only the blocks of rows that did. The result
// means what a whole rewrite would; a part the copy cannot vouch for fails the save with
// `BaseUnusable` and is written whole by the next save, which still copies the other parts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, BufWriter, Seek, Write};
use std::ops::RangeInclusive;

use ironcalc::base::cell_store::CellStore;
use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::expressions::parser::stringify::to_excel_string;
use ironcalc::base::expressions::types::CellReferenceRC;
use ironcalc::base::types::{Cell, Row, Worksheet};
use ironcalc::export::{SharedFormula, row_styles, write_rows_xml};
use zenkai_xlsx_reader::{AnchorKind, BasePackage, ReachingAnchor, RowSpan, cannot_splice};
use zip::ZipWriter;

use crate::error::EngineError;
use crate::fingerprint::{MAX_ROW, ROWS_PER_BLOCK, SheetPrint, UsedBounds};
use crate::save::{ModelData, SHEET_BUFFER_BYTES, rejected};
use crate::save_sheet::{bounds_reference, with_row_outline, written_height};
use crate::sheet_settings::{Carried, Outline};

// The blocks of rows that changed, when the sheet can be written row by row.
pub(crate) fn dirty_blocks(saved: &SheetPrint, now: &SheetPrint) -> Option<BTreeSet<usize>> {
    if !now.same_frame(saved) || now.moved_since(saved) {
        return None;
    }
    let (before, after) = (saved.row_blocks()?, now.row_blocks()?);
    let block = |blocks: &[u64], index: usize| blocks.get(index).copied().unwrap_or(0);
    let mut dirty = BTreeSet::new();
    let mut used = 0usize;
    for index in 0..before.len().max(after.len()) {
        let (old, new) = (block(before, index), block(after, index));
        if old != 0 || new != 0 {
            used += 1;
        }
        if old != new {
            dirty.insert(index);
        }
    }
    // Past half of the blocks in use the copy saves little.
    let few = !dirty.is_empty() && dirty.len().saturating_mul(2) <= used;
    few.then_some(dirty)
}

pub(crate) struct RowJob {
    pub(crate) part: String,
    // Only the rows in `spans`, with their formulas renumbered into `formulas`.
    sheet: Worksheet,
    formulas: Vec<(Node, StaticResult)>,
    // Every row written in `spans`, in order.
    rows: Vec<i32>,
    spans: Vec<RowSpan>,
    dimension: String,
    outline: BTreeMap<u32, Outline>,
}

// Blocks index the row prints, so their rows lie in Excel's grid and fit any integer type.
fn block_rows(block: usize) -> RangeInclusive<u32> {
    let first = block as u32 * ROWS_PER_BLOCK + 1;
    first..=first + ROWS_PER_BLOCK - 1
}

fn grid_rows(rows: RangeInclusive<u32>) -> RangeInclusive<i32> {
    *rows.start() as i32..=*rows.end() as i32
}

// An array formula is written over its whole range, so the blocks it covers are written
// with it; a range reaching rows copied from the file would leave them pointing at it.
fn with_arrays(sheet: &Worksheet, mut blocks: BTreeSet<usize>) -> BTreeSet<usize> {
    let mut pending: Vec<usize> = blocks.iter().copied().collect();
    while let Some(block) = pending.pop() {
        for row in grid_rows(block_rows(block)) {
            for (_, cell) in sheet.sheet_data.row_cells(row) {
                let Cell::ArrayFormula(array) = cell else {
                    continue;
                };
                let height = i64::from(array.r.1).max(1);
                let last = (i64::from(row) + height - 1).min(i64::from(MAX_ROW));
                let last = ((last - 1) / i64::from(ROWS_PER_BLOCK)) as usize;
                for covered in block..=last {
                    if blocks.insert(covered) {
                        pending.push(covered);
                    }
                }
            }
        }
    }
    blocks
}

fn spans_of(blocks: &BTreeSet<usize>) -> Vec<RowSpan> {
    let mut spans: Vec<RowSpan> = Vec::new();
    for &block in blocks {
        let (first, last) = block_rows(block).into_inner();
        match spans.last_mut() {
            Some(span) if span.last + 1 == first => span.last = last,
            _ => spans.push(RowSpan { first, last }),
        }
    }
    spans
}

fn span_rows(span: &RowSpan) -> RangeInclusive<i32> {
    grid_rows(span.first..=span.last)
}

// The cell with its formula renumbered into `formulas`.
fn renumbered(
    cell: &Cell,
    parsed: &[(Node, StaticResult)],
    formulas: &mut Vec<(Node, StaticResult)>,
    numbers: &mut HashMap<i32, i32>,
) -> Result<Cell, EngineError> {
    let mut number = |f: i32| -> Result<i32, EngineError> {
        if let Some(&new) = numbers.get(&f) {
            return Ok(new);
        }
        let formula = usize::try_from(f)
            .ok()
            .and_then(|f| parsed.get(f))
            .ok_or_else(|| rejected("a cell points at a missing formula"))?;
        let new = i32::try_from(formulas.len()).map_err(rejected)?;
        formulas.push(formula.clone());
        numbers.insert(f, new);
        Ok(new)
    };
    Ok(match cell {
        Cell::CellFormula { f, s, v } => Cell::CellFormula {
            f: number(*f)?,
            s: *s,
            v: v.clone(),
        },
        Cell::ArrayFormula(array) => {
            let mut array = array.clone();
            array.f = number(array.f)?;
            Cell::ArrayFormula(array)
        }
        other => other.clone(),
    })
}

// Runs under the workbook's read lock: the work is proportional to the rows in `blocks`.
// `bounds` are the sheet's used extent from the fingerprint pass of the same save.
pub(crate) fn row_job(
    data: ModelData<'_>,
    carried: &Carried,
    index: usize,
    part: String,
    blocks: BTreeSet<usize>,
    bounds: &UsedBounds,
) -> Result<RowJob, EngineError> {
    let sheet = data
        .book
        .worksheets
        .get(index)
        .ok_or_else(|| rejected("a sheet is missing"))?;
    let parsed = data
        .formulas
        .get(index)
        .ok_or_else(|| rejected("a sheet has no formulas"))?;
    let spans = spans_of(&with_arrays(sheet, blocks));
    let outline: BTreeMap<u32, Outline> = carried
        .written(sheet)
        .map(|settings| {
            spans
                .iter()
                .flat_map(|span| settings.rows.outline.range(span.first..=span.last))
                .map(|(row, outline)| (*row, *outline))
                .collect()
        })
        .unwrap_or_default();
    let in_spans = |row: i32| {
        u32::try_from(row).is_ok_and(|row| {
            let at = spans.partition_point(|span| span.last < row);
            spans.get(at).is_some_and(|span| span.first <= row)
        })
    };
    let styles: Vec<Row> = sheet
        .rows
        .iter()
        .filter(|row| in_spans(row.r))
        .map(|row| Row {
            height: written_height(row.height),
            ..row.clone()
        })
        .collect();
    let mut rows: BTreeSet<i32> = styles.iter().map(|row| row.r).collect();
    rows.extend(outline.keys().filter_map(|row| i32::try_from(*row).ok()));
    let mut formulas = Vec::new();
    let mut numbers = HashMap::new();
    let mut small = Worksheet {
        dimension: String::new(),
        cols: Vec::new(),
        rows: styles,
        name: sheet.name.clone(),
        sheet_data: CellStore::new(),
        shared_formulas: Vec::new(),
        sheet_id: sheet.sheet_id,
        state: sheet.state.clone(),
        color: sheet.color.clone(),
        merge_cells: Vec::new(),
        comments: Vec::new(),
        frozen_rows: 0,
        frozen_columns: 0,
        views: HashMap::new(),
        show_grid_lines: sheet.show_grid_lines,
        conditional_formatting: Vec::new(),
    };
    for span in &spans {
        for row in span_rows(span) {
            if !sheet.sheet_data.has_row(row) {
                continue;
            }
            let cells = sheet
                .sheet_data
                .row_cells(row)
                .map(|(column, cell)| {
                    Ok((
                        column,
                        renumbered(cell, parsed, &mut formulas, &mut numbers)?,
                    ))
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            small.sheet_data.insert_row(row, cells);
            rows.insert(row);
        }
    }
    Ok(RowJob {
        part,
        dimension: bounds_reference(bounds),
        sheet: small,
        formulas,
        rows: rows.into_iter().collect(),
        spans,
        outline,
    })
}

impl RowJob {
    // A shared formula from a dropped row whose range reaches copied rows is written again
    // with its `si` and range, so the copied cells that take it keep their formula; it
    // must still read the same at its cell.
    fn kept_anchors(
        &self,
        reaching: &[ReachingAnchor],
    ) -> io::Result<HashMap<(i32, i32), SharedFormula>> {
        let mut anchors = HashMap::with_capacity(reaching.len());
        for anchor in reaching {
            let AnchorKind::Shared { si } = anchor.kind else {
                return Err(cannot_splice(
                    "an array formula reaches rows copied from the file",
                ));
            };
            let number = |value: u32| i32::try_from(value).map_err(io::Error::other);
            let (row, column) = (number(anchor.row)?, number(anchor.column)?);
            let formula = match self.sheet.sheet_data.get(row, column) {
                Some(Cell::CellFormula { f, .. }) => {
                    usize::try_from(*f).ok().and_then(|f| self.formulas.get(f))
                }
                _ => None,
            };
            let context = CellReferenceRC {
                sheet: self.sheet.name.clone(),
                row,
                column,
            };
            let same =
                formula.is_some_and(|(node, _)| to_excel_string(node, &context) == anchor.text);
            if !same {
                return Err(cannot_splice(
                    "a shared formula that rows copied from the file take has changed",
                ));
            }
            anchors.insert(
                (row, column),
                SharedFormula::Anchor {
                    si,
                    last_row: number(anchor.last_row)?,
                    last_column: number(anchor.last_column)?,
                },
            );
        }
        Ok(anchors)
    }

    fn write_span<W: Write>(
        &self,
        span: &RowSpan,
        reaching: &[ReachingAnchor],
        styles: &HashMap<i32, &Row>,
        out: &mut W,
    ) -> io::Result<()> {
        let anchors = self.kept_anchors(reaching)?;
        let range = span_rows(span);
        let first = self.rows.partition_point(|row| row < range.start());
        let last = self.rows.partition_point(|row| row <= range.end());
        let rows = &self.rows[first..last];
        let shared = |row: i32, column: i32| {
            anchors
                .get(&(row, column))
                .copied()
                .unwrap_or(SharedFormula::No)
        };
        if self.outline.is_empty() {
            return write_rows_xml(out, &self.sheet, &self.formulas, styles, rows, shared);
        }
        let mut written = Vec::new();
        write_rows_xml(
            &mut written,
            &self.sheet,
            &self.formulas,
            styles,
            rows,
            shared,
        )?;
        out.write_all(&with_row_outline(&written, &self.outline)?)
    }
}

// The caller started the zip entry. An error means the part could not be spliced.
pub(crate) fn write_rows<W: Write + Seek>(
    base: &mut BasePackage,
    zip: &mut ZipWriter<W>,
    job: &RowJob,
) -> io::Result<()> {
    let styles = row_styles(&job.sheet);
    let mut buffered = BufWriter::with_capacity(SHEET_BUFFER_BYTES, zip);
    base.splice_rows(
        &job.part,
        &job.dimension,
        &job.spans,
        &mut buffered,
        |span, reaching, out| job.write_span(span, reaching, &styles, out),
    )?;
    buffered.flush()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ironcalc::base::Model;
    use ironcalc::base::types::{ArrayKind, Cell, FormulaValue};
    use zenkai_xlsx_reader::RowSpan;

    use ironcalc::export::row_styles;

    use super::{dirty_blocks, row_job, spans_of, with_arrays};
    use crate::fingerprint::{ROW_PRINT_BUDGET_BYTES, SheetPrint, sheet_print};
    use crate::save::ModelData;
    use crate::sheet_settings::Carried;

    fn print(model: &Model) -> SheetPrint {
        let mut budget = ROW_PRINT_BUDGET_BYTES;
        sheet_print(
            &model.workbook.worksheets[0],
            &model.parsed_formulas[0],
            None,
            0,
            &mut budget,
        )
        .unwrap()
    }

    fn filled(rows: i32) -> Model<'static> {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        for row in 1..=rows {
            model.set_user_input(0, row, 1, row.to_string()).unwrap();
            model
                .set_user_input(0, row, 2, format!("=A{row}*2"))
                .unwrap();
        }
        model.evaluate();
        model
    }

    #[test]
    fn a_few_edits_are_written_by_block() {
        let mut model = filled(320);
        let before = print(&model);
        model.set_user_input(0, 5, 1, "x".to_string()).unwrap();
        model.set_user_input(0, 300, 1, "7".to_string()).unwrap();
        model.evaluate();
        let dirty = dirty_blocks(&before, &print(&model)).unwrap();
        assert_eq!(dirty, BTreeSet::from([0, 18]));
        assert_eq!(
            spans_of(&dirty),
            [
                RowSpan { first: 1, last: 16 },
                RowSpan {
                    first: 289,
                    last: 304
                }
            ]
        );
    }

    #[test]
    fn rows_inserted_at_the_top_rewrite_the_sheet() {
        let mut model = filled(320);
        let before = print(&model);
        model.insert_rows(0, 1, 1).unwrap();
        model.evaluate();
        assert!(dirty_blocks(&before, &print(&model)).is_none());
    }

    #[test]
    fn a_frame_change_rewrites_the_sheet() {
        let mut model = filled(320);
        let before = print(&model);
        model.set_user_input(0, 5, 1, "x".to_string()).unwrap();
        model.workbook.worksheets[0].frozen_rows = 1;
        assert!(dirty_blocks(&before, &print(&model)).is_none());
    }

    #[test]
    fn rows_moved_since_the_save_rewrite_the_sheet() {
        let mut model = filled(320);
        let before = print(&model);
        model.set_user_input(0, 300, 1, "7".to_string()).unwrap();
        model.evaluate();
        let mut budget = ROW_PRINT_BUDGET_BYTES;
        let moved = sheet_print(
            &model.workbook.worksheets[0],
            &model.parsed_formulas[0],
            None,
            1,
            &mut budget,
        )
        .unwrap();
        assert!(dirty_blocks(&before, &print(&model)).is_some());
        assert!(dirty_blocks(&before, &moved).is_none());
    }

    #[test]
    fn a_sheet_without_blocks_is_rewritten() {
        let model = filled(20);
        let mut none = 0;
        let unavailable = sheet_print(
            &model.workbook.worksheets[0],
            &model.parsed_formulas[0],
            None,
            0,
            &mut none,
        )
        .unwrap();
        assert!(dirty_blocks(&unavailable, &print(&model)).is_none());
        assert!(dirty_blocks(&print(&model), &unavailable).is_none());
    }

    #[test]
    fn an_array_formula_brings_the_blocks_it_covers() {
        let mut model = filled(100);
        let array =
            Cell::new_array_formula(0, 0, (2, 40), ArrayKind::Cse, FormulaValue::Unevaluated);
        let sheet = &mut model.workbook.worksheets[0];
        sheet.sheet_data.insert(10, 3, array);
        assert_eq!(
            with_arrays(sheet, BTreeSet::from([0])),
            BTreeSet::from([0, 1, 2, 3])
        );
        assert_eq!(with_arrays(sheet, BTreeSet::from([5])), BTreeSet::from([5]));
    }

    #[test]
    fn the_job_holds_only_the_rows_of_its_spans() {
        let model = filled(100);
        let carried = Carried::default();
        let job = row_job(
            ModelData::of(&model),
            &carried,
            0,
            "xl/worksheets/sheet1.xml".to_string(),
            BTreeSet::from([2]),
            &print(&model).bounds(),
        )
        .unwrap();
        assert_eq!(job.rows, (33..=48).collect::<Vec<_>>());
        assert_eq!(job.sheet.sheet_data.row_count(), 16);
        assert_eq!(job.formulas.len(), 1, "the column shares one formula");
        assert_eq!(job.dimension, "A1:B100");
        let mut written = Vec::new();
        let styles = row_styles(&job.sheet);
        job.write_span(&job.spans[0], &[], &styles, &mut written)
            .unwrap();
        let written = String::from_utf8(written).unwrap();
        assert!(written.starts_with(r#"<row r="33">"#), "{written}");
        assert!(written.contains("<f>A40*2</f>"), "{written}");
        assert!(!written.contains("si="), "{written}");
    }
}
