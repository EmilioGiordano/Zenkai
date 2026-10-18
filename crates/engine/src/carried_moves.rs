use crate::file::Unsupported;
use crate::model::HistoryStep;
use crate::sheet_elements::Elements;
use crate::sheet_settings::{Indexed, SheetSettings};

// What a row or column move took from one sheet. Exchanged with the sheet's settings by
// every undo and redo of the move, so the other side of the exchange is what it took next.
#[derive(Debug, Default)]
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

// The sheets a move changed, and the kinds it recorded as lost with the count each had
// before, so undoing it can forget them.
#[derive(Debug, Default)]
pub(crate) struct MoveTaken {
    pub(crate) sheets: Vec<(u32, Taken)>,
    pub(crate) kinds: Vec<(Unsupported, Option<u64>)>,
}

// What each row or column move took, for the undo and redo of that move. The entries line
// up with the moves in the model's own history, so every move must be its own undo step
// (the workbook never groups them) and only a new edit empties the redo side.
#[derive(Debug, Default)]
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

    // The entry of the move `step` replays.
    pub(crate) fn pop(&mut self, step: HistoryStep) -> Option<MoveTaken> {
        match step {
            HistoryStep::Undo => self.undone.pop(),
            HistoryStep::Redo => self.redone.pop(),
        }
    }

    // What a replayed `step` leaves to replay the other way.
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
