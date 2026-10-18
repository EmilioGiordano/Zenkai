use crate::file::Unsupported;
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
