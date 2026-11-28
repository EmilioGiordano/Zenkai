use zenkai_engine::{Copied, Engine, EngineError, Workbook};
use zenkai_types::{CellPos, Range, SheetId};

use crate::clipboard;
use crate::visible_cells::CopyShape;

// Each block is one engine copy made on the UI thread, so their number is capped, not only
// the cells they hold.
const MAX_COPY_BLOCKS: usize = 2_000;

// A rectangle the engine copied whole. Blocks paste at their offsets, so a copy that leaves
// out hidden lines lands contiguous with formulas and formats intact.
#[derive(Clone)]
struct ClipBlock {
    copied: Copied,
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

pub enum CopyOutcome {
    Copied(InternalClip),
    TooLarge,
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
        let used = shape.within(workbook.used_end(sheet));
        let used = if used.is_empty() {
            CopyShape::whole(Range::single(range.start))
        } else {
            used
        };
        let blocks = used.blocks();
        if used.cell_count() > clipboard::MAX_COPY_CELLS || blocks.len() > MAX_COPY_BLOCKS {
            return Ok(CopyOutcome::TooLarge);
        }
        // Moving several areas one after another can overwrite the source of a later one.
        if cut && blocks.len() > 1 {
            return Ok(CopyOutcome::CutNeedsOneArea);
        }
        let blocks = blocks
            .into_iter()
            .map(|block| {
                Ok(ClipBlock {
                    copied: workbook.copy(sheet, block.source)?,
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
