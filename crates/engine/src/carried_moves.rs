use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::file::Unsupported;
use crate::model::HistoryStep;
use crate::sheet_elements::Elements;
use crate::sheet_settings::{Indexed, SheetSettings};

// What a row or column move took from one sheet. Exchanged with the sheet's settings by
// every undo and redo of the move, so the other side of the exchange is what it took next.
#[derive(Clone, Debug, Default)]
pub(crate) struct Taken {
    pub(crate) rows: Option<Indexed>,
    pub(crate) columns: Option<Indexed>,
    pub(crate) elements: Option<Elements>,
}

impl Taken {
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_none() && self.columns.is_none() && self.elements.is_none()
    }

    pub(crate) fn exchange(self, settings: &mut SheetSettings) -> Taken {
        Taken {
            rows: self
                .rows
                .map(|rows| std::mem::replace(&mut settings.rows, rows)),
            columns: self
                .columns
                .map(|columns| std::mem::replace(&mut settings.columns, columns)),
            elements: self
                .elements
                .map(|elements| std::mem::replace(&mut settings.elements, elements)),
        }
    }
}

// A kind a move recorded as lost, with the loss count it had before (`None` if it had
// none), so undoing the move can put that back.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LostKind {
    pub(crate) kind: Unsupported,
    pub(crate) before: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MoveTaken {
    pub(crate) sheets: Vec<(u32, Taken)>,
    pub(crate) kinds: Vec<LostKind>,
}

// What each row or column move took, for the undo and redo of that move. The entries line
// up with the moves in the model's own history, so every move must be its own undo step
// (the workbook never groups them) and only a new edit empties the redo side.
#[derive(Clone, Debug, Default)]
pub(crate) struct Moves {
    undone: Vec<MoveTaken>,
    redone: Vec<MoveTaken>,
}

impl Moves {
    pub(crate) fn push(&mut self, taken: MoveTaken) {
        self.redone.clear();
        self.undone.push(taken);
    }

    pub(crate) fn redo_emptied(&mut self) {
        self.redone.clear();
    }

    pub(crate) fn pop(&mut self, step: HistoryStep) -> Option<MoveTaken> {
        match step {
            HistoryStep::Undo => self.undone.pop(),
            HistoryStep::Redo => self.redone.pop(),
        }
    }

    pub(crate) fn push_inverse(&mut self, step: HistoryStep, inverse: MoveTaken) {
        match step {
            HistoryStep::Undo => self.redone.push(inverse),
            HistoryStep::Redo => self.undone.push(inverse),
        }
    }

    pub(crate) fn forgo(&mut self, step: HistoryStep) {
        if self.pop(step).is_some() {
            self.push_inverse(step, MoveTaken::default());
        }
    }
}

// How many times an undo or redo put each kind of element back into each sheet (by sheet
// id) since the last save into the workbook's own file that succeeded. A save warns about
// those among them that it cannot write, once. Shared with the snapshot so that save can
// forget what it saw without the workbook's lock.
#[derive(Clone, Debug, Default)]
pub(crate) struct Restored(Arc<Mutex<Counts>>);

pub(crate) type Counts = BTreeMap<u32, BTreeMap<Unsupported, u64>>;

impl Restored {
    fn counts(&self) -> MutexGuard<'_, Counts> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn add(&self, sheet_id: u32, kinds: BTreeSet<Unsupported>) {
        let mut counts = self.counts();
        let sheet = counts.entry(sheet_id).or_default();
        for kind in kinds {
            *sheet.entry(kind).or_default() += 1;
        }
    }

    pub(crate) fn of(&self, sheet_id: u32) -> BTreeSet<Unsupported> {
        self.counts()
            .get(&sheet_id)
            .map(|sheet| sheet.keys().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn seen(&self) -> Counts {
        self.counts().clone()
    }

    // Takes back only the counts `seen` holds, so a restore made after the snapshot keeps
    // its warning even when its kind was already there.
    pub(crate) fn forget(&self, seen: &Counts) {
        let mut counts = self.counts();
        for (sheet_id, gone) in seen {
            let Some(sheet) = counts.get_mut(sheet_id) else {
                continue;
            };
            for (kind, seen_count) in gone {
                if let Some(count) = sheet.get_mut(kind) {
                    *count = count.saturating_sub(*seen_count);
                }
            }
            sheet.retain(|_, count| *count > 0);
        }
        counts.retain(|_, sheet| !sheet.is_empty());
    }
}
