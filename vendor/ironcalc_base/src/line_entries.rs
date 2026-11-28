//! Row and column entries changed many at a time, in one pass over the sheet's entries.
//! The per-line setters scan the entries for every line, which is quadratic over a
//! selection of many lines.

use std::collections::{HashMap, HashSet};

use crate::constants::{self, LAST_COLUMN, LAST_ROW};
use crate::types::{Col, Row, Worksheet};
use crate::user_model::history::RowChange;

fn same_column_properties(a: &Col, b: &Col) -> bool {
    a.width == b.width
        && a.custom_width == b.custom_width
        && a.hidden == b.hidden
        && a.style == b.style
}

// Joins neighbouring columns that have the same properties into ranges.
fn compress(slots: &[Option<Col>], first: i32) -> Vec<Col> {
    let mut cols: Vec<Col> = Vec::new();
    for (offset, slot) in slots.iter().enumerate() {
        let Some(col) = slot else { continue };
        let at = first + offset as i32;
        match cols.last_mut() {
            Some(last) if last.max + 1 == at && same_column_properties(last, col) => last.max = at,
            _ => cols.push(Col {
                min: at,
                max: at,
                width: col.width,
                custom_width: col.custom_width,
                hidden: col.hidden,
                style: col.style,
            }),
        }
    }
    cols
}

impl Worksheet {
    /// The changes that give each listed row its height (in the unit of
    /// [crate::Model::set_row_height]) and mark it as set by hand.
    pub(crate) fn plan_row_heights(&self, heights: &[(i32, f64)]) -> Vec<RowChange> {
        let wanted: HashMap<i32, f64> = heights
            .iter()
            .copied()
            .filter(|(row, _)| (1..=LAST_ROW).contains(row))
            .collect();
        let existing: HashMap<i32, &Row> = self.rows.iter().map(|row| (row.r, row)).collect();
        let mut changes: Vec<RowChange> = wanted
            .into_iter()
            .filter_map(|(row, height)| {
                let old = existing.get(&row).map(|row| (*row).clone());
                let mut new = old.clone().unwrap_or(Row {
                    r: row,
                    height: 0.0,
                    custom_format: false,
                    custom_height: true,
                    s: 0,
                    hidden: false,
                });
                new.height = height / constants::ROW_HEIGHT_FACTOR;
                new.custom_height = true;
                (old.as_ref() != Some(&new)).then_some(RowChange {
                    row,
                    old,
                    new: Some(new),
                })
            })
            .collect();
        changes.sort_by_key(|change| change.row);
        changes
    }

    /// The changes that hide or show the rows `first` to `last`. Showing touches only the
    /// rows that have an entry; hiding gives the others one.
    pub(crate) fn plan_rows_hidden(&self, first: i32, last: i32, hidden: bool) -> Vec<RowChange> {
        let (first, last) = (first.max(1), last.min(LAST_ROW));
        let mut changes: Vec<RowChange> = self
            .rows
            .iter()
            .filter(|row| (first..=last).contains(&row.r) && row.hidden != hidden)
            .map(|row| RowChange {
                row: row.r,
                old: Some(row.clone()),
                new: Some(Row {
                    hidden,
                    ..row.clone()
                }),
            })
            .collect();
        if hidden {
            let existing: HashSet<i32> = self.rows.iter().map(|row| row.r).collect();
            changes.extend(
                (first..=last)
                    .filter(|row| !existing.contains(row))
                    .map(|row| RowChange {
                        row,
                        old: None,
                        new: Some(Row {
                            r: row,
                            height: constants::DEFAULT_ROW_HEIGHT / constants::ROW_HEIGHT_FACTOR,
                            custom_format: false,
                            custom_height: false,
                            s: 0,
                            hidden: true,
                        }),
                    }),
            );
        }
        changes.sort_by_key(|change| change.row);
        changes
    }

    /// Sets each changed row to its new entry, or to its old one when `undo`.
    pub(crate) fn apply_row_changes(&mut self, changes: &[RowChange], undo: bool) {
        let mut set: HashMap<i32, &Row> = HashMap::new();
        let mut removed: HashSet<i32> = HashSet::new();
        for change in changes {
            match if undo { &change.old } else { &change.new } {
                Some(row) => {
                    set.insert(change.row, row);
                }
                None => {
                    removed.insert(change.row);
                }
            }
        }
        let mut rows = Vec::with_capacity(self.rows.len() + set.len());
        let mut placed: HashSet<i32> = HashSet::new();
        for entry in self.rows.drain(..) {
            if removed.contains(&entry.r) {
                continue;
            }
            match set.get(&entry.r) {
                Some(row) => {
                    if placed.insert(entry.r) {
                        rows.push((*row).clone());
                    }
                }
                None => rows.push(entry),
            }
        }
        rows.extend(
            set.into_iter()
                .filter(|(row, _)| !placed.contains(row))
                .map(|(_, row)| row.clone()),
        );
        rows.sort_by_key(|row| row.r);
        self.rows = rows;
    }

    /// The columns the runs `(first, last, width)` span, and the entries that cover them
    /// before and after each column of a run takes its width (in the unit of
    /// [crate::Model::set_column_width]), keeping its style and whether it is hidden.
    /// `None` when nothing changes.
    pub(crate) fn plan_column_widths(
        &self,
        runs: &[(i32, i32, f64)],
    ) -> Option<(i32, i32, Vec<Col>, Vec<Col>)> {
        let runs: Vec<(i32, i32, f64)> = runs
            .iter()
            .map(|(first, last, width)| ((*first).max(1), (*last).min(LAST_COLUMN), *width))
            .filter(|(first, last, _)| first <= last)
            .collect();
        let first = runs.iter().map(|(first, _, _)| *first).min()?;
        let last = runs.iter().map(|(_, last, _)| *last).max()?;
        let mut slots: Vec<Option<Col>> = vec![None; (last - first + 1) as usize];
        let mut filled = 0;
        for col in &self.cols {
            if filled == slots.len() {
                break;
            }
            for at in col.min.max(first)..=col.max.min(last) {
                let slot = &mut slots[(at - first) as usize];
                if slot.is_none() {
                    *slot = Some(Col {
                        min: at,
                        max: at,
                        width: col.width,
                        custom_width: col.custom_width,
                        hidden: col.hidden,
                        style: col.style,
                    });
                    filled += 1;
                }
            }
        }
        let old = compress(&slots, first);
        for (run_first, run_last, width) in &runs {
            let width = width / constants::COLUMN_WIDTH_FACTOR;
            for at in *run_first..=*run_last {
                match &mut slots[(at - first) as usize] {
                    Some(col) => {
                        col.width = width;
                        col.custom_width = true;
                    }
                    slot => {
                        *slot = Some(Col {
                            min: at,
                            max: at,
                            width,
                            custom_width: true,
                            hidden: false,
                            style: None,
                        })
                    }
                }
            }
        }
        let new = compress(&slots, first);
        (old != new).then_some((first, last, old, new))
    }

    /// Replaces what the entries say about the columns `first` to `last` with `pieces`,
    /// which lie inside them; an entry that reaches outside is cut at the edge.
    pub(crate) fn replace_cols_within(&mut self, first: i32, last: i32, pieces: &[Col]) {
        let mut cols = Vec::with_capacity(self.cols.len() + pieces.len() + 2);
        for col in self.cols.drain(..) {
            if col.max < first || col.min > last {
                cols.push(col);
                continue;
            }
            if col.min < first {
                cols.push(Col {
                    max: first - 1,
                    ..col.clone()
                });
            }
            if col.max > last {
                cols.push(Col {
                    min: last + 1,
                    ..col
                });
            }
        }
        cols.extend(pieces.iter().cloned());
        cols.sort_by_key(|col| col.min);
        self.cols = cols;
    }
}
