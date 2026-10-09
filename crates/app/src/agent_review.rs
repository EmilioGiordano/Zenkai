use std::collections::HashSet;

use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{CellPos, CellRef, Range, SheetId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellChange {
    pub cell: CellRef,
    pub old: String,
    pub new: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub agent: String,
    pub sheet_name: String,
    pub description: String,
    cells: Vec<CellChange>,
}

impl Batch {
    pub fn cells(&self) -> &[CellChange] {
        &self.cells
    }

    fn bytes(&self) -> usize {
        self.cells
            .iter()
            .map(|change| change.old.len() + change.new.len())
            .sum()
    }
}

// A write holds at most 5,000 cells (MAX_WRITE_CELLS), so 64 MiB keeps every old and new
// input of several typical writes (a few dozen bytes per cell) while bounding the worst
// case of long texts; past it the oldest writes are kept automatically.
pub const MAX_REVIEW_BYTES: usize = 64 * 1024 * 1024;

fn row_major(change: &CellChange) -> (SheetId, u32, u16) {
    (
        change.cell.sheet,
        change.cell.pos.row.get(),
        change.cell.pos.col.get(),
    )
}

// What the agents changed and the user has not decided on yet, oldest write first. The bar
// works on the oldest batch; a cell written again moves to the newer batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    batches: Vec<Batch>,
    cursor: usize,
    budget: usize,
}

impl Default for Review {
    fn default() -> Review {
        Review::with_budget(MAX_REVIEW_BYTES)
    }
}

impl Review {
    pub fn with_budget(budget: usize) -> Review {
        Review {
            batches: Vec::new(),
            cursor: 0,
            budget,
        }
    }

    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    pub fn current(&self) -> Option<&Batch> {
        self.batches.first()
    }

    pub fn position(&self) -> Option<usize> {
        let batch = self.current()?;
        Some(self.cursor.min(batch.cells.len().saturating_sub(1)) + 1)
    }

    pub fn record(
        &mut self,
        agent: String,
        sheet_name: String,
        description: String,
        changes: Vec<CellChange>,
    ) {
        let mut cells: Vec<CellChange> = changes
            .into_iter()
            .filter(|change| change.old != change.new)
            .collect();
        if cells.is_empty() {
            return;
        }
        cells.sort_by_key(row_major);
        for change in &cells {
            self.forget(change.cell);
        }
        self.batches.push(Batch {
            agent,
            sheet_name,
            description,
            cells,
        });
        while self.batches.iter().map(Batch::bytes).sum::<usize>() > self.budget {
            self.batches.remove(0);
            self.cursor = 0;
        }
    }

    pub fn find(&self, cell: CellRef) -> Option<(&Batch, &CellChange)> {
        self.batches.iter().find_map(|batch| {
            batch
                .cells
                .iter()
                .find(|change| change.cell == cell)
                .map(|change| (batch, change))
        })
    }

    pub fn marks(&self, sheet: SheetId) -> HashSet<CellPos> {
        self.batches
            .iter()
            .flat_map(|batch| &batch.cells)
            .filter(|change| change.cell.sheet == sheet)
            .map(|change| change.cell.pos)
            .collect()
    }

    pub fn forget(&mut self, cell: CellRef) -> Option<CellChange> {
        let mut removed = None;
        for batch in &mut self.batches {
            if let Some(index) = batch.cells.iter().position(|change| change.cell == cell) {
                removed = Some(batch.cells.remove(index));
                break;
            }
        }
        self.tidy();
        removed
    }

    pub fn take_all(&mut self) -> Vec<CellChange> {
        if self.batches.is_empty() {
            return Vec::new();
        }
        self.cursor = 0;
        self.batches.remove(0).cells
    }

    pub fn cursor_cell(&self) -> Option<CellRef> {
        let batch = self.current()?;
        Some(batch.cells[self.cursor.min(batch.cells.len() - 1)].cell)
    }

    pub fn forget_in(&mut self, sheet: SheetId, range: Range) {
        for batch in &mut self.batches {
            batch
                .cells
                .retain(|change| change.cell.sheet != sheet || !range.contains(change.cell.pos));
        }
        self.tidy();
    }

    pub fn follow(&mut self, cell: CellRef) {
        if let Some(index) = self
            .current()
            .and_then(|batch| batch.cells.iter().position(|change| change.cell == cell))
        {
            self.cursor = index;
        }
    }

    pub fn step(&mut self, forward: bool) -> Option<CellRef> {
        let count = self.current()?.cells.len();
        let at = self.cursor.min(count - 1);
        self.cursor = if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        };
        self.cursor_cell()
    }

    // Anything that no longer holds what the agent wrote (a hand edit, an undo, a reject)
    // is out of the review.
    pub fn retain_unchanged(&mut self, input_of: impl Fn(CellRef) -> String) {
        for batch in &mut self.batches {
            batch
                .cells
                .retain(|change| input_of(change.cell) == change.new);
        }
        self.tidy();
    }

    fn tidy(&mut self) {
        self.batches.retain(|batch| !batch.cells.is_empty());
        let count = self.batches.first().map_or(0, |batch| batch.cells.len());
        self.cursor = self.cursor.min(count.saturating_sub(1));
    }
}

pub fn inputs_in(workbook: &Workbook, sheet: SheetId, block: Range) -> Vec<(CellPos, String)> {
    block
        .positions()
        .map(|pos| (pos, workbook.input(sheet, pos)))
        .collect()
}

pub fn changes_since(
    workbook: &Workbook,
    sheet: SheetId,
    before: Vec<(CellPos, String)>,
) -> Vec<CellChange> {
    before
        .into_iter()
        .filter_map(|(pos, old)| {
            let new = workbook.input(sheet, pos);
            (old != new).then_some(CellChange {
                cell: CellRef { sheet, pos },
                old,
                new,
            })
        })
        .collect()
}

pub struct RestoreBlock {
    pub sheet: SheetId,
    pub origin: CellPos,
    pub rows: Vec<Vec<String>>,
}

impl RestoreBlock {
    fn last_row(&self) -> u32 {
        self.origin.row.get() + self.rows.len() as u32 - 1
    }
}

// A cell that no longer holds what the agent wrote (changed since by anything the review
// did not see) is left alone.
pub fn reject_unchanged(
    workbook: &mut Workbook,
    changes: &[CellChange],
) -> Result<(), EngineError> {
    let still_written: Vec<CellChange> = changes
        .iter()
        .filter(|change| workbook.input(change.cell.sheet, change.cell.pos) == change.new)
        .cloned()
        .collect();
    restore_blocks(&still_written)
        .iter()
        .try_for_each(|block| workbook.set_inputs(block.sheet, block.origin, &block.rows))
}

// The old inputs back as few rectangles as possible, so each is one engine paste and one
// undo step.
pub fn restore_blocks(changes: &[CellChange]) -> Vec<RestoreBlock> {
    let mut sorted: Vec<&CellChange> = changes.iter().collect();
    sorted.sort_by_key(|change| row_major(change));
    let mut runs: Vec<RestoreBlock> = Vec::new();
    for change in sorted {
        let pos = change.cell.pos;
        let extends = runs.last_mut().filter(|run| {
            run.sheet == change.cell.sheet
                && run.origin.row == pos.row
                && u32::from(run.origin.col.get()) + run.rows[0].len() as u32
                    == u32::from(pos.col.get())
        });
        match extends {
            Some(run) => run.rows[0].push(change.old.clone()),
            None => runs.push(RestoreBlock {
                sheet: change.cell.sheet,
                origin: pos,
                rows: vec![vec![change.old.clone()]],
            }),
        }
    }
    let mut blocks: Vec<RestoreBlock> = Vec::new();
    for run in runs {
        let below = blocks.iter_mut().rev().find(|block| {
            block.sheet == run.sheet
                && block.origin.col == run.origin.col
                && block.rows[0].len() == run.rows[0].len()
                && block.last_row() + 1 == run.origin.row.get()
        });
        match below {
            Some(block) => block.rows.extend(run.rows),
            None => blocks.push(run),
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use zenkai_agent::tools::{WriteCells, WriteRequest, plan_write};
    use zenkai_types::WorkbookId;

    use super::*;

    const SHEET: SheetId = SheetId(0);

    fn cell(a1: &str) -> CellRef {
        CellRef {
            sheet: SHEET,
            pos: CellPos::parse_a1(a1).unwrap(),
        }
    }

    fn change(a1: &str, old: &str, new: &str) -> CellChange {
        CellChange {
            cell: cell(a1),
            old: old.into(),
            new: new.into(),
        }
    }

    fn review_of(changes: Vec<CellChange>) -> Review {
        let mut review = Review::default();
        review.record(
            "Claude Code".into(),
            "Sales".into(),
            "Write".into(),
            changes,
        );
        review
    }

    fn three() -> Review {
        review_of(vec![
            change("B3", "a", "1"),
            change("A1", "b", "2"),
            change("B1", "c", "3"),
        ])
    }

    #[test]
    fn a_write_is_recorded_in_row_major_order_without_unchanged_cells() {
        let mut review = three();
        review.record(
            "Codex".into(),
            "Sales".into(),
            "Write".into(),
            vec![change("D9", "same", "same")],
        );
        let batch = review.current().unwrap();
        assert_eq!(batch.agent, "Claude Code");
        let order: Vec<_> = batch.cells().iter().map(|c| c.cell).collect();
        assert_eq!(order, vec![cell("A1"), cell("B1"), cell("B3")]);
        assert_eq!(review.position(), Some(1));
    }

    #[test]
    fn keeping_a_cell_clears_its_mark_and_the_last_one_ends_the_review() {
        let mut review = three();
        assert!(review.forget(cell("B1")).is_some());
        assert!(review.find(cell("B1")).is_none());
        assert!(!review.marks(SHEET).contains(&cell("B1").pos));
        review.forget(cell("A1"));
        review.forget(cell("B3"));
        assert!(review.is_empty());
    }

    #[test]
    fn rejecting_a_cell_hands_back_its_old_input() {
        let mut review = three();
        let removed = review.forget(cell("A1")).unwrap();
        assert_eq!((removed.old.as_str(), removed.new.as_str()), ("b", "2"));
        assert_eq!(review.current().unwrap().cells().len(), 2);
    }

    #[test]
    fn reject_all_returns_every_remaining_cell_and_empties_the_review() {
        let mut review = three();
        review.forget(cell("A1"));
        let rejected = review.take_all();
        assert_eq!(rejected.len(), 2);
        assert!(review.is_empty());
        assert!(review.take_all().is_empty());
    }

    #[test]
    fn keeping_all_empties_the_review() {
        let mut review = three();
        review.take_all();
        assert!(review.is_empty());
        assert!(review.marks(SHEET).is_empty());
    }

    #[test]
    fn a_hand_edit_or_an_undo_takes_the_cell_out() {
        let mut review = three();
        let inputs = [("A1", "2"), ("B1", "typed by hand"), ("B3", "a")];
        review.retain_unchanged(|c| {
            inputs
                .iter()
                .find(|(a1, _)| cell(a1) == c)
                .map(|(_, input)| (*input).to_string())
                .unwrap()
        });
        let left: Vec<_> = review
            .current()
            .unwrap()
            .cells()
            .iter()
            .map(|c| c.cell)
            .collect();
        assert_eq!(left, vec![cell("A1")]);
    }

    #[test]
    fn next_and_previous_follow_the_order_and_wrap() {
        let mut review = three();
        assert_eq!(review.step(true), Some(cell("B1")));
        assert_eq!(review.position(), Some(2));
        assert_eq!(review.step(true), Some(cell("B3")));
        assert_eq!(review.step(true), Some(cell("A1")));
        assert_eq!(review.step(false), Some(cell("B3")));
        review.follow(cell("B1"));
        assert_eq!(review.position(), Some(2));
    }

    #[test]
    fn a_cell_written_again_moves_to_the_newer_batch() {
        let mut review = three();
        review.record(
            "Codex".into(),
            "Sales".into(),
            "Write".into(),
            vec![change("A1", "2", "9"), change("C1", "", "x")],
        );
        assert_eq!(review.current().unwrap().cells().len(), 2);
        assert_eq!(review.find(cell("A1")).unwrap().1.new, "9");
        review.take_all();
        assert_eq!(review.current().unwrap().agent, "Codex");
    }

    #[test]
    fn restore_blocks_join_contiguous_cells_into_rectangles() {
        let changes = [
            change("A1", "1", "x"),
            change("B1", "2", "x"),
            change("A2", "3", "x"),
            change("B2", "4", "x"),
            change("D2", "5", "x"),
        ];
        let blocks = restore_blocks(&changes);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].origin, cell("A1").pos);
        assert_eq!(blocks[0].rows, vec![vec!["1", "2"], vec!["3", "4"]]);
        assert_eq!(blocks[1].origin, cell("D2").pos);
    }

    #[test]
    fn one_undo_restores_every_cell_of_an_agent_write() {
        let mut workbook = Workbook::new_empty().unwrap();
        let sheet = workbook.sheets().remove(0);
        workbook
            .set_input(sheet.id, cell("A2").pos, "kept by the user")
            .unwrap();
        let rows: Vec<Vec<String>> = (1..=4)
            .map(|row| (1..=3).map(|col| format!("{}", row * 10 + col)).collect())
            .collect();
        let request = WriteRequest::WriteCells(WriteCells {
            workbook: WorkbookId(0),
            sheet: sheet.name.clone(),
            start: "A1".into(),
            rows,
        });
        let plan = plan_write(&request, &workbook.sheets()).unwrap();
        let block = plan.input_block().unwrap();
        let before = inputs_in(&workbook, sheet.id, block);
        plan.apply(&mut workbook).unwrap();
        let changes = changes_since(&workbook, sheet.id, before);
        assert_eq!(changes.len(), 12);
        assert_eq!(
            changes.iter().find(|c| c.cell == cell("A2")).unwrap().old,
            "kept by the user"
        );
        workbook.undo().unwrap();
        for change in &changes {
            assert_eq!(workbook.input(sheet.id, change.cell.pos), change.old);
        }
    }

    #[test]
    fn rejecting_all_restores_the_old_inputs_in_one_undo_step() {
        let mut workbook = Workbook::new_empty().unwrap();
        let sheet = workbook.sheets().remove(0);
        let rows = vec![vec!["a".to_string(), "b".to_string()]; 3];
        workbook
            .set_inputs(sheet.id, cell("A1").pos, &rows)
            .unwrap();
        let rows = vec![vec!["x".to_string(), "y".to_string()]; 3];
        let request = WriteRequest::WriteCells(WriteCells {
            workbook: WorkbookId(0),
            sheet: sheet.name.clone(),
            start: "A1".into(),
            rows,
        });
        let plan = plan_write(&request, &workbook.sheets()).unwrap();
        let before = inputs_in(&workbook, sheet.id, plan.input_block().unwrap());
        plan.apply(&mut workbook).unwrap();
        let changes = changes_since(&workbook, sheet.id, before);
        let blocks = restore_blocks(&changes);
        assert_eq!(blocks.len(), 1);
        for block in &blocks {
            workbook
                .set_inputs(block.sheet, block.origin, &block.rows)
                .unwrap();
        }
        assert_eq!(workbook.input(sheet.id, cell("B3").pos), "b");
        workbook.undo().unwrap();
        assert_eq!(workbook.input(sheet.id, cell("B3").pos), "y");
    }

    #[test]
    fn a_cell_names_the_batch_that_owns_it() {
        let mut review = three();
        review.record(
            "Codex".into(),
            "Sales".into(),
            "Write".into(),
            vec![change("A1", "2", "9")],
        );
        assert_eq!(review.find(cell("A1")).unwrap().0.agent, "Codex");
        assert_eq!(review.find(cell("B1")).unwrap().0.agent, "Claude Code");
    }

    #[test]
    fn reject_leaves_a_cell_that_is_no_longer_what_the_agent_wrote() {
        let mut workbook = Workbook::new_empty().unwrap();
        let sheet = workbook.sheets().remove(0).id;
        let rows = vec![vec!["a".to_string(), "b".to_string()]];
        workbook.set_inputs(sheet, cell("A1").pos, &rows).unwrap();
        let changes = vec![change("A1", "a", "x"), change("B1", "b", "y")];
        let rows = vec![vec!["x".to_string(), "y".to_string()]];
        workbook.set_inputs(sheet, cell("A1").pos, &rows).unwrap();
        workbook
            .set_input(sheet, cell("B1").pos, "by hand")
            .unwrap();
        reject_unchanged(&mut workbook, &changes).unwrap();
        assert_eq!(workbook.input(sheet, cell("A1").pos), "a");
        assert_eq!(workbook.input(sheet, cell("B1").pos), "by hand");
    }

    #[test]
    fn the_oldest_writes_are_kept_when_the_budget_is_exceeded() {
        let mut review = Review::with_budget(8);
        review.record(
            "A".into(),
            "S".into(),
            "w".into(),
            vec![change("A1", "ab", "cd")],
        );
        review.record(
            "B".into(),
            "S".into(),
            "w".into(),
            vec![change("A2", "ef", "gh")],
        );
        assert_eq!(review.batch_count(), 2);
        review.record(
            "C".into(),
            "S".into(),
            "w".into(),
            vec![change("A3", "i", "j")],
        );
        assert_eq!(review.batch_count(), 2);
        assert_eq!(review.current().unwrap().agent, "B");
        review.record(
            "D".into(),
            "S".into(),
            "w".into(),
            vec![change("A4", "klmno", "pqrst")],
        );
        assert_eq!(review.batch_count(), 0);
    }
}
