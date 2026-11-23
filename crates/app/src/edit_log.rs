use serde::{Deserialize, Serialize};
use zenkai_types::{
    CellPos, ColIdx, ContentSpan, LineAxis, LineChange, LineOp, LoggedChange, RowIdx, SheetChange,
};

pub const MAX_LOGGED: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Logged {
    change: LoggedChange,
    // The number of the latest engine change this one stands for.
    through: u64,
}

const NOTHING: Logged = Logged {
    change: LoggedChange::NOTHING,
    through: 0,
};

// What was written, cleared or moved in a CSV's sheets since it was read or last saved, up to a
// bound: past it, or when the engine says it lost track, the log is `lost` and a save writes
// every cell as it shows. A fixed array keeps the binding `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedLog", into = "SavedLog")]
pub struct EditLog {
    entries: [Logged; MAX_LOGGED],
    len: usize,
    // Changes up to this number may be missing from the log.
    lost_through: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct SavedLog {
    #[serde(default)]
    changes: Vec<Logged>,
    #[serde(default)]
    lost_through: Option<u64>,
}

impl Default for EditLog {
    fn default() -> Self {
        EditLog {
            entries: [NOTHING; MAX_LOGGED],
            len: 0,
            lost_through: None,
        }
    }
}

impl From<SavedLog> for EditLog {
    fn from(saved: SavedLog) -> Self {
        let mut log = EditLog {
            lost_through: saved.lost_through,
            ..EditLog::default()
        };
        for entry in saved.changes {
            log.append(entry.change, entry.through);
        }
        log
    }
}

impl From<EditLog> for SavedLog {
    fn from(log: EditLog) -> Self {
        SavedLog {
            changes: log.entries[..log.len].to_vec(),
            lost_through: log.lost_through,
        }
    }
}

fn within(outer: ContentSpan, inner: ContentSpan) -> bool {
    outer.row <= inner.row
        && outer.col <= inner.col
        && u64::from(inner.row) + u64::from(inner.rows)
            <= u64::from(outer.row) + u64::from(outer.rows)
        && u64::from(inner.col) + u64::from(inner.cols)
            <= u64::from(outer.col) + u64::from(outer.cols)
}

fn touch(first: u32, count: u32, other_first: u32, other_count: u32) -> bool {
    first <= other_first.saturating_add(other_count) && other_first <= first.saturating_add(count)
}

// The block both cover when together they are exactly one block; the log never grows an edit
// to cells that were not written.
fn merged(a: ContentSpan, b: ContentSpan) -> Option<ContentSpan> {
    if within(a, b) {
        return Some(a);
    }
    if within(b, a) {
        return Some(b);
    }
    if a.col == b.col && a.cols == b.cols && touch(a.row, a.rows, b.row, b.rows) {
        let row = a.row.min(b.row);
        let end = (a.row + a.rows).max(b.row + b.rows);
        return Some(ContentSpan {
            row,
            rows: end - row,
            ..a
        });
    }
    if a.row == b.row && a.rows == b.rows && touch(a.col, a.cols, b.col, b.cols) {
        let col = a.col.min(b.col);
        let end = (a.col + a.cols).max(b.col + b.cols);
        return Some(ContentSpan {
            col,
            cols: end - col,
            ..a
        });
    }
    None
}

impl EditLog {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn has_lost(&self) -> bool {
        self.lost_through.is_some()
    }

    pub fn lose(&mut self, through: u64) {
        self.lost_through = Some(self.lost_through.map_or(through, |lost| lost.max(through)));
    }

    pub fn changes(&self) -> impl Iterator<Item = LoggedChange> + '_ {
        self.entries[..self.len].iter().map(|entry| entry.change)
    }

    fn append(&mut self, change: LoggedChange, through: u64) {
        if self.len == MAX_LOGGED {
            self.lose(through);
            return;
        }
        self.entries[self.len] = Logged { change, through };
        self.len += 1;
    }

    // A block written next to or inside blocks written since the last row or column was
    // inserted or deleted joins them, so typing down a column is one entry.
    fn mergeable(&self, sheet: u32, span: ContentSpan) -> Option<(usize, ContentSpan)> {
        for at in (0..self.len).rev() {
            let logged = self.entries[at].change;
            match logged.change {
                SheetChange::Content(other) if logged.sheet == sheet => {
                    if let Some(union) = merged(other, span) {
                        return Some((at, union));
                    }
                }
                SheetChange::Lines(_) if logged.sheet == sheet => return None,
                _ => {}
            }
        }
        None
    }

    pub fn push(&mut self, change: LoggedChange, through: u64) {
        match change.change {
            SheetChange::Unknown => self.lose(through),
            SheetChange::Lines(_) => self.append(change, through),
            SheetChange::Content(mut span) => {
                let mut through = through;
                while let Some((at, union)) = self.mergeable(change.sheet, span) {
                    span = union;
                    through = through.max(self.entries[at].through);
                    self.entries.copy_within(at + 1..self.len, at);
                    self.len -= 1;
                }
                let content = SheetChange::Content(span);
                self.append(
                    LoggedChange {
                        sheet: change.sheet,
                        change: content,
                    },
                    through,
                );
            }
        }
    }

    // A save made the file what the sheet was at change `seq`: what was logged up to it is
    // in the file, and what is lost up to it no longer matters.
    pub fn clear_through(&mut self, seq: u64) {
        let mut kept = 0;
        for at in 0..self.len {
            if self.entries[at].through > seq {
                self.entries[kept] = self.entries[at];
                kept += 1;
            }
        }
        self.len = kept;
        if self.lost_through.is_some_and(|lost| lost <= seq) {
            self.lost_through = None;
        }
    }

    // The workbook was rebuilt, so the engine's numbers start again from the first.
    pub fn rebase(&mut self) {
        for entry in &mut self.entries[..self.len] {
            entry.through = 0;
        }
        if self.lost_through.is_some() {
            self.lost_through = Some(0);
        }
    }
}

// Where the cells of a sheet were in the file before the changes of its log, and whether the
// log says the user wrote them.
pub struct Walk {
    changes: Vec<SheetChange>,
}

fn origin(op: LineOp, mut index: u32) -> Option<u32> {
    match op.change {
        LineChange::Insert if index >= op.at.saturating_add(op.count) => index -= op.count,
        LineChange::Insert if index >= op.at => return None,
        LineChange::Insert => {}
        LineChange::Delete if index >= op.at => index = index.saturating_add(op.count),
        LineChange::Delete => {}
    }
    Some(index)
}

impl Walk {
    pub fn new(logged: impl IntoIterator<Item = LoggedChange>, sheet: u32) -> Walk {
        Walk {
            changes: logged
                .into_iter()
                .filter(|logged| logged.sheet == sheet)
                .map(|logged| logged.change)
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn is_untouched(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn moves_columns(&self) -> bool {
        self.changes
            .iter()
            .any(|change| matches!(change, SheetChange::Lines(op) if op.axis == LineAxis::Column))
    }

    pub fn moves_rows(&self) -> bool {
        self.changes
            .iter()
            .any(|change| matches!(change, SheetChange::Lines(op) if op.axis == LineAxis::Row))
    }

    // The position in the file of a cell nobody wrote since: `None` for one that was, or that
    // sits in a row or column inserted since, which have nothing of the file's.
    pub fn file_position(&self, pos: CellPos) -> Option<CellPos> {
        let (mut row, mut col) = (pos.row.get(), u32::from(pos.col.get()));
        for change in self.changes.iter().rev() {
            match change {
                SheetChange::Content(span) if span.contains(row, col) => return None,
                SheetChange::Lines(op) => match op.axis {
                    LineAxis::Row => row = origin(*op, row)?,
                    LineAxis::Column => col = origin(*op, col)?,
                },
                SheetChange::Content(_) | SheetChange::Unknown => {}
            }
        }
        let col = ColIdx::new(u16::try_from(col).ok()?)?;
        Some(CellPos::new(RowIdx::new(row)?, col))
    }

    // The row of the file a row of the sheet comes from, whatever was written in it.
    pub fn file_row(&self, row: RowIdx) -> Option<RowIdx> {
        let mut row = row.get();
        for change in self.changes.iter().rev() {
            if let SheetChange::Lines(op) = change
                && op.axis == LineAxis::Row
            {
                row = origin(*op, row)?;
            }
        }
        RowIdx::new(row)
    }

    // Where a row of the file is now; a row that was deleted is where the deletion was.
    pub fn current_row(&self, file_row: u32) -> u32 {
        let mut row = file_row;
        for change in &self.changes {
            let SheetChange::Lines(op) = change else {
                continue;
            };
            if op.axis != LineAxis::Row {
                continue;
            }
            match op.change {
                LineChange::Insert if row >= op.at => row = row.saturating_add(op.count),
                LineChange::Delete if row >= op.at.saturating_add(op.count) => row -= op.count,
                LineChange::Delete if row >= op.at => row = op.at,
                _ => {}
            }
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(row: u32, col: u32, rows: u32, cols: u32) -> ContentSpan {
        ContentSpan {
            row,
            col,
            rows,
            cols,
        }
    }

    fn content(sheet: u32, row: u32, col: u32, rows: u32, cols: u32) -> LoggedChange {
        LoggedChange {
            sheet,
            change: SheetChange::Content(span(row, col, rows, cols)),
        }
    }

    fn lines(sheet: u32, axis: LineAxis, change: LineChange, at: u32, count: u32) -> LoggedChange {
        LoggedChange {
            sheet,
            change: SheetChange::Lines(LineOp {
                axis,
                change,
                at,
                count,
            }),
        }
    }

    fn spans(log: &EditLog) -> Vec<LoggedChange> {
        log.changes().collect()
    }

    #[test]
    fn blocks_that_together_are_one_block_are_one_entry() {
        let mut log = EditLog::default();
        log.push(content(1, 0, 0, 1, 1), 1);
        log.push(content(1, 1, 0, 1, 1), 2);
        log.push(content(1, 2, 0, 1, 1), 3);
        assert_eq!(spans(&log), [content(1, 0, 0, 3, 1)]);
        log.push(content(1, 0, 1, 3, 1), 4);
        assert_eq!(spans(&log), [content(1, 0, 0, 3, 2)]);
        log.push(content(1, 1, 1, 1, 1), 5);
        assert_eq!(spans(&log), [content(1, 0, 0, 3, 2)]);
        log.push(content(1, 5, 5, 1, 1), 6);
        log.push(content(2, 3, 0, 1, 1), 7);
        assert_eq!(log.changes().count(), 3);
    }

    #[test]
    fn a_block_never_grows_past_what_was_written() {
        let mut log = EditLog::default();
        log.push(content(1, 0, 0, 1, 1), 1);
        log.push(content(1, 2, 0, 1, 1), 2);
        assert_eq!(log.changes().count(), 2);
        log.push(content(1, 1, 1, 1, 1), 3);
        assert_eq!(log.changes().count(), 3);
    }

    #[test]
    fn blocks_do_not_join_across_an_inserted_or_deleted_line() {
        let mut log = EditLog::default();
        log.push(content(1, 0, 0, 1, 1), 1);
        log.push(lines(1, LineAxis::Row, LineChange::Insert, 0, 1), 2);
        log.push(content(1, 1, 0, 1, 1), 3);
        assert_eq!(log.changes().count(), 3);
        log.push(lines(2, LineAxis::Row, LineChange::Insert, 0, 1), 4);
        log.push(content(1, 2, 0, 1, 1), 5);
        assert_eq!(log.changes().count(), 4);
        assert_eq!(log.changes().last(), Some(content(1, 1, 0, 2, 1)));
    }

    #[test]
    fn a_full_log_and_an_unknown_change_are_lost_never_dropped_silently() {
        let mut log = EditLog::default();
        for n in 0..MAX_LOGGED as u32 {
            log.push(content(1, n * 2, n * 2, 1, 1), u64::from(n) + 1);
        }
        assert!(!log.has_lost());
        log.push(content(1, 500, 500, 1, 1), 99);
        assert!(log.has_lost());
        log.clear_through(98);
        assert!(log.has_lost());
        log.clear_through(99);
        assert!(!log.has_lost());
        assert!(log.is_empty());

        log.push(
            LoggedChange {
                sheet: 0,
                change: SheetChange::Unknown,
            },
            7,
        );
        assert!(log.has_lost());
        log.rebase();
        log.clear_through(0);
        assert!(!log.has_lost());
    }

    #[test]
    fn a_save_drops_what_it_consumed_and_keeps_what_came_after() {
        let mut log = EditLog::default();
        log.push(content(1, 0, 0, 1, 1), 1);
        log.push(content(1, 5, 5, 1, 1), 2);
        log.push(lines(1, LineAxis::Row, LineChange::Insert, 0, 1), 3);
        log.push(content(1, 9, 9, 1, 1), 4);
        log.clear_through(3);
        assert_eq!(spans(&log), [content(1, 9, 9, 1, 1)]);
        log.rebase();
        log.clear_through(0);
        assert!(log.is_empty());
    }

    #[test]
    fn a_log_survives_the_session_file_and_one_that_is_too_long_is_lost() {
        let mut log = EditLog::default();
        log.push(content(1, 0, 0, 2, 2), 1);
        log.push(lines(1, LineAxis::Column, LineChange::Delete, 3, 1), 2);
        log.lose(1);
        let json = serde_json::to_string(&log).unwrap();
        assert_eq!(serde_json::from_str::<EditLog>(&json).unwrap(), log);
        assert_eq!(
            serde_json::from_str::<EditLog>("{}").unwrap(),
            EditLog::default()
        );

        let many = SavedLog {
            changes: (0..=MAX_LOGGED as u32)
                .map(|n| Logged {
                    change: content(1, n * 2, 0, 1, 1),
                    through: u64::from(n) + 1,
                })
                .collect(),
            lost_through: None,
        };
        let json = serde_json::to_string(&many).unwrap();
        let read = serde_json::from_str::<EditLog>(&json).unwrap();
        assert!(read.has_lost());
        assert_eq!(read.changes().count(), MAX_LOGGED);
    }

    fn at(row: u32, col: u16) -> CellPos {
        CellPos::new(RowIdx::new(row).unwrap(), ColIdx::new(col).unwrap())
    }

    #[test]
    fn a_cell_is_the_files_unless_it_was_written_and_lines_map_it_back() {
        let walk = Walk::new(
            [
                content(1, 2, 0, 1, 1),
                lines(1, LineAxis::Row, LineChange::Insert, 1, 2),
                content(1, 9, 0, 1, 1),
                lines(2, LineAxis::Row, LineChange::Delete, 0, 5),
            ],
            1,
        );
        assert!(!walk.is_untouched());
        assert_eq!(walk.file_position(at(0, 0)), Some(at(0, 0)));
        assert_eq!(walk.file_position(at(1, 0)), None);
        assert_eq!(walk.file_position(at(3, 0)), Some(at(1, 0)));
        assert_eq!(walk.file_position(at(5, 3)), Some(at(3, 3)));
        assert_eq!(walk.file_position(at(9, 0)), None);
        assert_eq!(walk.file_position(at(4, 0)), None);
        assert!(Walk::new([lines(2, LineAxis::Row, LineChange::Insert, 0, 1)], 1).is_untouched());
    }

    #[test]
    fn rows_and_columns_are_mapped_apart_and_deletions_pull_the_rest_up() {
        let walk = Walk::new(
            [
                lines(1, LineAxis::Column, LineChange::Delete, 0, 1),
                lines(1, LineAxis::Row, LineChange::Delete, 1, 2),
            ],
            1,
        );
        assert!(walk.moves_columns() && walk.moves_rows());
        assert_eq!(walk.file_position(at(0, 0)), Some(at(0, 1)));
        assert_eq!(walk.file_position(at(1, 2)), Some(at(3, 3)));
        assert_eq!(walk.file_row(RowIdx::new(1).unwrap()), RowIdx::new(3));
        assert_eq!(walk.current_row(0), 0);
        assert_eq!(walk.current_row(2), 1);
        assert_eq!(walk.current_row(5), 3);
        let inserted = Walk::new([lines(1, LineAxis::Row, LineChange::Insert, 1, 2)], 1);
        assert_eq!(inserted.file_row(RowIdx::new(2).unwrap()), None);
        assert_eq!(inserted.current_row(1), 3);
        assert_eq!(inserted.current_row(0), 0);
    }
}
