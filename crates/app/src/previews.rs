use std::collections::HashMap;

use gpui_kit::SharedString;
use zenkai_grid::{GridCell, MAX_TYPED_PREVIEW_CELLS, typed_preview};
use zenkai_types::{CellPos, Range, SheetId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Queued,
    Running,
}

#[derive(Debug)]
struct Preview {
    generation: u64,
    sheet: SheetId,
    range: Range,
    text: SharedString,
    stage: Stage,
}

// Typed text the workbook does not hold yet. A refresh reads the workbook, which lacks
// the edits still queued or running, so it lays these on top instead of wiping them.
#[derive(Debug, Default)]
pub struct TypedPreviews {
    previews: Vec<Preview>,
}

impl TypedPreviews {
    pub fn add(&mut self, generation: u64, sheet: SheetId, range: Range, text: SharedString) {
        if range.cell_count() > MAX_TYPED_PREVIEW_CELLS {
            return;
        }
        self.previews.push(Preview {
            generation,
            sheet,
            range,
            text,
            stage: Stage::Queued,
        });
    }

    pub fn batch_started(&mut self) {
        for preview in &mut self.previews {
            preview.stage = Stage::Running;
        }
    }

    pub fn batch_finished(&mut self) {
        self.previews
            .retain(|preview| preview.stage == Stage::Queued);
    }

    pub fn clear(&mut self) {
        self.previews.clear();
    }

    pub fn apply(
        &self,
        generation: u64,
        sheet: SheetId,
        ranges: &[Range],
        cells: &mut HashMap<CellPos, GridCell>,
    ) {
        for preview in &self.previews {
            if preview.generation != generation || preview.sheet != sheet {
                continue;
            }
            for visible in ranges
                .iter()
                .filter_map(|range| preview.range.intersection(range))
            {
                for pos in visible.positions() {
                    if let Some(cell) = typed_preview(cells.get(&pos), &preview.text) {
                        cells.insert(pos, cell);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    fn screen() -> Vec<Range> {
        vec![Range::parse_a1("A1:J10").unwrap()]
    }

    fn shown(previews: &TypedPreviews, generation: u64, sheet: u32) -> HashMap<CellPos, GridCell> {
        let mut cells = HashMap::new();
        previews.apply(generation, SheetId(sheet), &screen(), &mut cells);
        cells
    }

    fn typed(previews: &mut TypedPreviews, cell: &str, text: &str) {
        previews.add(1, SheetId(0), Range::single(pos(cell)), text.into());
    }

    #[test]
    fn queued_previews_survive_the_refresh_after_an_earlier_batch() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "a");
        previews.batch_started();
        typed(&mut previews, "B1", "a");
        previews.batch_finished();
        let cells = shown(&previews, 1, 0);
        assert!(!cells.contains_key(&pos("A1")));
        assert_eq!(cells[&pos("B1")].text.as_ref(), "a");
    }

    #[test]
    fn a_preview_is_dropped_when_its_batch_finishes_even_if_it_failed() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "a");
        previews.batch_started();
        previews.batch_finished();
        assert!(shown(&previews, 1, 0).is_empty());
    }

    #[test]
    fn a_later_preview_of_the_same_cell_wins() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "1");
        typed(&mut previews, "A1", "2");
        assert_eq!(shown(&previews, 1, 0)[&pos("A1")].text.as_ref(), "2");
    }

    #[test]
    fn other_documents_and_sheets_show_nothing() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "a");
        assert!(shown(&previews, 2, 0).is_empty());
        assert!(shown(&previews, 1, 1).is_empty());
    }

    #[test]
    fn clearing_leaves_no_ghost_after_a_document_change() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "a");
        previews.clear();
        assert!(shown(&previews, 1, 0).is_empty());
    }

    #[test]
    fn cells_outside_the_cached_ranges_are_not_added() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "Z99", "a");
        assert!(shown(&previews, 1, 0).is_empty());
    }

    #[test]
    fn only_the_visible_part_of_a_large_preview_is_laid_down() {
        let mut previews = TypedPreviews::default();
        previews.add(
            1,
            SheetId(0),
            Range::parse_a1("A1:C3000").unwrap(),
            "a".into(),
        );
        let cells = shown(&previews, 1, 0);
        assert_eq!(cells.len(), 30);
        assert!(cells.contains_key(&pos("C10")));
        assert!(!cells.contains_key(&pos("A11")));
    }

    #[test]
    fn a_preview_waits_for_its_sheet_while_another_is_shown() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "a");
        assert!(shown(&previews, 1, 1).is_empty());
        assert_eq!(shown(&previews, 1, 0)[&pos("A1")].text.as_ref(), "a");
    }

    #[test]
    fn an_edit_queued_behind_a_running_batch_keeps_the_earlier_previews_until_it_ends() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "a");
        previews.batch_started();
        typed(&mut previews, "B1", "b");
        assert_eq!(shown(&previews, 1, 0).len(), 2);
        previews.batch_finished();
        assert_eq!(shown(&previews, 1, 0).len(), 1);
    }

    #[test]
    fn formulas_keep_the_last_value() {
        let mut previews = TypedPreviews::default();
        typed(&mut previews, "A1", "=1+1");
        assert!(shown(&previews, 1, 0).is_empty());
    }
}
