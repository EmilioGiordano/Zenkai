use std::sync::Arc;

use ironcalc::base::types::Cell;

use super::{FormulaEvent, SheetCells, unreadable, unsupported};
use crate::ReadError;

impl SheetCells {
    pub(crate) fn first_row(&self) -> i32 {
        self.first_row
    }

    pub(crate) fn last_row(&self) -> i32 {
        self.last_row
    }

    // Takes in a run of rows scanned on its own, leaving the sheet as if those rows had
    // been scanned right after the ones it holds: strings and formulas are numbered in
    // document order, so the piece's local numbers shift by what came before.
    pub(crate) fn absorb(&mut self, mut piece: SheetCells) -> Result<(), ReadError> {
        if piece.first_row != 0 && piece.first_row <= self.last_row {
            return Err(unsupported("rows out of order"));
        }
        let strings: Vec<i32> = piece
            .strings
            .iter()
            .map(|text| self.intern_with(text, || Arc::clone(text)))
            .collect();
        let events_start = self.events.len();
        let jobs_start = self.jobs.len();
        if !piece.events.is_empty() {
            i32::try_from(events_start + piece.events.len() - 1).map_err(unreadable)?;
        }
        let events_shift = i32::try_from(events_start).map_err(unreadable)?;
        for slot in &piece.string_cells {
            let Some(Cell::SharedString { si, .. }) = piece.data.get_mut(slot.row, slot.column)
            else {
                return Err(unsupported("a lost string"));
            };
            *si = *usize::try_from(*si)
                .ok()
                .and_then(|index| strings.get(index))
                .ok_or_else(|| unsupported("a lost string"))?;
        }
        for slot in &piece.formula_cells {
            let Some(f) = piece
                .data
                .get_mut(slot.row, slot.column)
                .and_then(Cell::formula_mut)
            else {
                return Err(unsupported("a lost formula"));
            };
            *f += events_shift;
        }
        for event in &mut piece.events {
            match event {
                FormulaEvent::Convert { job } | FormulaEvent::SharedAnchor { job, .. } => {
                    *job += jobs_start;
                }
                FormulaEvent::SharedChild { .. } => {}
            }
        }
        self.rows.append(&mut piece.rows);
        self.data.append(piece.data);
        self.string_cells.append(&mut piece.string_cells);
        self.formula_cells.append(&mut piece.formula_cells);
        self.events.append(&mut piece.events);
        self.jobs.append(&mut piece.jobs);
        self.highest_style = self.highest_style.max(piece.highest_style);
        self.cell_count += piece.cell_count;
        self.job_bytes += piece.job_bytes;
        if piece.first_row != 0 {
            if self.first_row == 0 {
                self.first_row = piece.first_row;
            }
            self.last_row = piece.last_row;
        }
        Ok(())
    }
}
