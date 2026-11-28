use zenkai_engine::{Copied, Engine, EngineError, Workbook};
use zenkai_types::{CellPos, MAX_COLS, MAX_ROWS, Range, SheetId, WorkbookId};

use crate::clipboard;
use crate::visible_cells::CopyShape;

// Each block is one engine copy, so their number is capped, not only the cells they hold.
const MAX_COPY_BLOCKS: u64 = 2_000;

// A rectangle the engine copied whole. Blocks paste at their offsets, so a copy that leaves
// out hidden lines lands contiguous with formulas and formats intact.
#[derive(Clone)]
struct ClipBlock {
    copied: Copied,
    source: Range,
    row_offset: u32,
    col_offset: u16,
}

#[derive(Clone)]
pub struct InternalClip {
    blocks: Vec<ClipBlock>,
    pub text: String,
    pub cut: bool,
    pub range: Range,
    pub sheet: SheetId,
    pub shape: CopyShape,
}

// What a background copy was started against; its result lands only if all of it still holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyStamp {
    pub request: u64,
    pub document: WorkbookId,
    pub generation: u64,
}

pub fn copy_still_current(started: CopyStamp, now: Option<CopyStamp>) -> bool {
    now == Some(started)
}

pub enum CopyOutcome {
    Copied(InternalClip),
    NoCells,
    TooLarge,
    TooManyAreas,
    CutNeedsOneArea,
}

impl InternalClip {
    pub fn copy(
        workbook: &mut Workbook,
        sheet: SheetId,
        range: Range,
        shape: CopyShape,
        cut: bool,
    ) -> Result<CopyOutcome, EngineError> {
        // Past the used area there is nothing to read, so the first visible cell stands in:
        // a hidden cell must never be copied in its place.
        let used = shape.within(workbook.used_end(sheet));
        let used = if used.is_empty() {
            match shape.first_cell() {
                Some(cell) => CopyShape::whole(Range::single(cell)),
                None => return Ok(CopyOutcome::NoCells),
            }
        } else {
            used
        };
        if used.cell_count() > clipboard::MAX_COPY_CELLS {
            return Ok(CopyOutcome::TooLarge);
        }
        // Moving several areas one after another can overwrite the source of a later one.
        if cut && used.block_count() > 1 {
            return Ok(CopyOutcome::CutNeedsOneArea);
        }
        if used.block_count() > MAX_COPY_BLOCKS {
            return Ok(CopyOutcome::TooManyAreas);
        }
        let blocks = used
            .blocks()
            .into_iter()
            .map(|block| {
                Ok(ClipBlock {
                    copied: workbook.copy(sheet, block.source)?,
                    source: block.source,
                    row_offset: block.row_offset,
                    col_offset: block.col_offset,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let text = match blocks.as_slice() {
            [only] => only.copied.text.clone(),
            _ => clipboard::shown_text(workbook, sheet, &used),
        };
        Ok(CopyOutcome::Copied(InternalClip {
            blocks,
            text,
            cut,
            range,
            sheet,
            shape,
        }))
    }

    // Blocks past the sheet edge would clamp onto the same cells. A single block keeps
    // the engine's own handling of the edge.
    pub fn runs_past_the_edge_at(&self, origin: CellPos) -> bool {
        self.blocks.len() > 1
            && self.blocks.iter().any(|block| {
                origin.row.get() + block.row_offset + block.source.rows() > MAX_ROWS
                    || u32::from(origin.col.get())
                        + u32::from(block.col_offset)
                        + u32::from(block.source.cols())
                        > u32::from(MAX_COLS)
            })
    }

    pub fn paste(
        &self,
        workbook: &mut Workbook,
        sheet: SheetId,
        origin: CellPos,
    ) -> Result<(), EngineError> {
        let paste_block = |workbook: &mut Workbook, block: &ClipBlock| {
            let at = CellPos::new(
                origin.row.offset(i64::from(block.row_offset)),
                origin.col.offset(i64::from(block.col_offset)),
            );
            workbook.paste(sheet, at, &block.copied, self.cut)
        };
        match self.blocks.as_slice() {
            [only] => paste_block(workbook, only),
            blocks => workbook.as_one_undo_step(|workbook| {
                blocks
                    .iter()
                    .try_for_each(|block| paste_block(workbook, block))
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visible_cells::{CopyScope, layout_hiding};
    use zenkai_types::{ColIdx, RowIdx};

    const SHEET: SheetId = SheetId(0);

    fn cell(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    fn ten_rows_with_formulas() -> Workbook {
        let mut book = Workbook::new_empty().unwrap();
        let rows: Vec<Vec<String>> = (1..=10)
            .map(|row| vec![row.to_string(), format!("=A{row}*2")])
            .collect();
        book.set_inputs(SHEET, CellPos::default(), &rows).unwrap();
        book
    }

    fn copy_visible(
        book: &mut Workbook,
        selection: &str,
        hidden_rows: &[u32],
        cut: bool,
    ) -> CopyOutcome {
        let selection = range(selection);
        let layout = layout_hiding(hidden_rows, &[]);
        let shape = CopyShape::of(selection, CopyScope::VisibleOnly, &layout);
        InternalClip::copy(book, SHEET, selection, shape, cut).unwrap()
    }

    fn copied(outcome: CopyOutcome) -> InternalClip {
        match outcome {
            CopyOutcome::Copied(clip) => clip,
            _ => panic!("the copy was refused"),
        }
    }

    fn stamp() -> CopyStamp {
        CopyStamp {
            request: 4,
            document: WorkbookId(1),
            generation: 7,
        }
    }

    #[test]
    fn a_background_copy_lands_when_nothing_changed() {
        assert!(copy_still_current(stamp(), Some(stamp())));
    }

    #[test]
    fn a_background_copy_is_dropped_after_an_edit_a_newer_copy_or_a_switch() {
        let newer_request = CopyStamp {
            request: 5,
            ..stamp()
        };
        assert!(!copy_still_current(stamp(), Some(newer_request)));
        let other_document = CopyStamp {
            document: WorkbookId(2),
            ..stamp()
        };
        assert!(!copy_still_current(stamp(), Some(other_document)));
        let replaced = CopyStamp {
            generation: 8,
            ..stamp()
        };
        assert!(!copy_still_current(stamp(), Some(replaced)));
        assert!(!copy_still_current(stamp(), None));
    }

    #[test]
    fn rows_three_to_five_hidden_paste_as_seven_contiguous_rows_with_shifted_formulas() {
        let mut book = ten_rows_with_formulas();
        let clip = copied(copy_visible(&mut book, "A1:B10", &[2, 3, 4], false));
        assert_eq!(clip.shape.row_count(), 7);
        assert_eq!(clip.text, "1\t2\n2\t4\n6\t12\n7\t14\n8\t16\n9\t18\n10\t20");
        clip.paste(&mut book, SHEET, cell("D1")).unwrap();
        let values: Vec<String> = (1..=8)
            .map(|row| book.input(SHEET, cell(&format!("D{row}"))))
            .collect();
        assert_eq!(values, ["1", "2", "6", "7", "8", "9", "10", ""]);
        assert_eq!(book.input(SHEET, cell("E1")), "=D1*2");
        assert_eq!(book.input(SHEET, cell("E3")), "=D3*2");
        assert_eq!(book.input(SHEET, cell("E7")), "=D7*2");
        assert_eq!(book.input(SHEET, cell("E8")), "");
        book.undo().unwrap();
        assert_eq!(book.input(SHEET, cell("D1")), "");
        assert_eq!(book.input(SHEET, cell("E7")), "");
    }

    #[test]
    fn a_marked_cut_with_several_areas_is_refused_and_one_area_cuts() {
        let mut book = ten_rows_with_formulas();
        assert!(matches!(
            copy_visible(&mut book, "A1:B10", &[2, 3, 4], true),
            CopyOutcome::CutNeedsOneArea
        ));
        assert!(matches!(
            copy_visible(&mut book, "A1:B10", &[], true),
            CopyOutcome::Copied(clip) if clip.cut
        ));
    }

    #[test]
    fn nothing_visible_inside_the_used_area_never_copies_a_hidden_cell() {
        let mut book = Workbook::new_empty().unwrap();
        let rows: Vec<Vec<String>> = (1..=5).map(|_| vec!["secret".to_string()]).collect();
        book.set_inputs(SHEET, CellPos::default(), &rows).unwrap();
        let hidden: Vec<u32> = (0..8).collect();
        let clip = copied(copy_visible(&mut book, "A1:A10", &hidden, false));
        assert!(!clip.text.contains("secret"));
        clip.paste(&mut book, SHEET, cell("C1")).unwrap();
        assert_eq!(book.input(SHEET, cell("C1")), "");
    }

    #[test]
    fn a_selection_with_nothing_visible_has_no_cells() {
        let mut book = ten_rows_with_formulas();
        let hidden: Vec<u32> = (0..10).collect();
        assert!(matches!(
            copy_visible(&mut book, "A1:B10", &hidden, false),
            CopyOutcome::NoCells
        ));
    }

    #[test]
    fn too_many_areas_are_refused_before_any_block_is_built() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SHEET, cell("AX3000"), "far").unwrap();
        let hidden_rows: Vec<u32> = (0..3000).filter(|row| row % 2 == 1).collect();
        let hidden_cols: Vec<u16> = (0..50).filter(|col| col % 2 == 1).collect();
        let selection = range("A1:AX3000");
        let layout = layout_hiding(&hidden_rows, &hidden_cols);
        let shape = CopyShape::of(selection, CopyScope::VisibleOnly, &layout);
        assert_eq!(shape.block_count(), 1_500 * 25);
        assert!(matches!(
            InternalClip::copy(&mut book, SHEET, selection, shape, false).unwrap(),
            CopyOutcome::TooManyAreas
        ));
    }

    #[test]
    fn a_paste_of_several_areas_that_reaches_past_the_last_row_is_flagged() {
        let mut book = ten_rows_with_formulas();
        let clip = copied(copy_visible(&mut book, "A1:B10", &[2, 3, 4], false));
        assert!(!clip.runs_past_the_edge_at(cell("D1")));
        let last_fit = CellPos::new(RowIdx::clamped(i64::from(MAX_ROWS) - 7), cell("D1").col);
        assert!(!clip.runs_past_the_edge_at(last_fit));
        assert!(clip.runs_past_the_edge_at(CellPos::new(
            RowIdx::clamped(i64::from(MAX_ROWS) - 6),
            cell("D1").col
        )));
        assert!(clip.runs_past_the_edge_at(CellPos::new(
            cell("D1").row,
            ColIdx::clamped(i64::from(MAX_COLS) - 1)
        )));
    }
}
