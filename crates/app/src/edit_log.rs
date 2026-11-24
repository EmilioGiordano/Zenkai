use serde::{Deserialize, Serialize};
use zenkai_types::{
    CellPos, ColIdx, ContentSpan, LineAxis, LineChange, LineOp, LoggedChange, RowIdx, SheetChange,
};

pub const MAX_LOGGED: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Logged {
    change: LoggedChange,
    // The number of the latest engine change this one stands for.
    through: u64,
}

// What was written, cleared or moved in a CSV's sheets since it was read or last saved. Past the
// bound, or when the engine says it lost track, the log is `lost` and a save writes every cell
// as it shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedLog", into = "SavedLog")]
pub struct EditLog {
    entries: Vec<Logged>,
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

impl From<SavedLog> for EditLog {
    fn from(saved: SavedLog) -> Self {
        let mut log = EditLog {
            entries: Vec::new(),
            lost_through: saved.lost_through,
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
            changes: log.entries,
            lost_through: log.lost_through,
        }
    }
}

fn end(first: u32, count: u32) -> u64 {
    u64::from(first) + u64::from(count)
}

fn within(outer: ContentSpan, inner: ContentSpan) -> bool {
    outer.row <= inner.row
        && outer.col <= inner.col
        && end(inner.row, inner.rows) <= end(outer.row, outer.rows)
        && end(inner.col, inner.cols) <= end(outer.col, outer.cols)
}

fn touch(first: u32, count: u32, other_first: u32, other_count: u32) -> bool {
    u64::from(first) <= end(other_first, other_count) && u64::from(other_first) <= end(first, count)
}

fn extent(first: u32, other_first: u32, last: u64, other_last: u64) -> (u32, u32) {
    let start = first.min(other_first);
    let count = last.max(other_last) - u64::from(start);
    (start, u32::try_from(count).unwrap_or(u32::MAX))
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
        let (row, rows) = extent(a.row, b.row, end(a.row, a.rows), end(b.row, b.rows));
        return Some(ContentSpan { row, rows, ..a });
    }
    if a.row == b.row && a.rows == b.rows && touch(a.col, a.cols, b.col, b.cols) {
        let (col, cols) = extent(a.col, b.col, end(a.col, a.cols), end(b.col, b.cols));
        return Some(ContentSpan { col, cols, ..a });
    }
    None
}

impl EditLog {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn has_lost(&self) -> bool {
        self.lost_through.is_some()
    }

    pub fn lose(&mut self, through: u64) {
        self.lost_through = Some(self.lost_through.map_or(through, |lost| lost.max(through)));
    }

    pub fn changes(&self) -> impl Iterator<Item = LoggedChange> + '_ {
        self.entries.iter().map(|entry| entry.change)
    }

    fn append(&mut self, change: LoggedChange, through: u64) {
        if self.entries.len() == MAX_LOGGED {
            self.lose(through);
            return;
        }
        self.entries.push(Logged { change, through });
    }

    // A block written next to or inside blocks written since the last row or column was
    // inserted or deleted joins them, so typing down a column is one entry.
    fn mergeable(&self, sheet: u32, span: ContentSpan) -> Option<(usize, ContentSpan)> {
        for (at, entry) in self.entries.iter().enumerate().rev() {
            let logged = entry.change;
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
                    self.entries.remove(at);
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
        self.entries.retain(|entry| entry.through > seq);
        if self.lost_through.is_some_and(|lost| lost <= seq) {
            self.lost_through = None;
        }
    }

    // The workbook was rebuilt, so the engine's numbers start again from the first.
    pub fn rebase(&mut self) {
        for entry in &mut self.entries {
            entry.through = 0;
        }
        if self.lost_through.is_some() {
            self.lost_through = Some(0);
        }
    }
}

enum Layer {
    // Blocks written between two line operations, with the block that holds them all.
    Content {
        bounds: ContentSpan,
        spans: Vec<ContentSpan>,
    },
    Lines(LineOp),
}

// Where the cells of a sheet were in the file before the changes of its log, and whether the
// log says the user wrote them. Built once per save, so a cell is looked up in the layers and
// not in every change.
pub struct Walk {
    layers: Vec<Layer>,
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
        let mut layers: Vec<Layer> = Vec::new();
        for logged in logged.into_iter().filter(|logged| logged.sheet == sheet) {
            match logged.change {
                SheetChange::Lines(op) => layers.push(Layer::Lines(op)),
                SheetChange::Content(span) => match layers.last_mut() {
                    Some(Layer::Content { bounds, spans }) => {
                        let (row, rows) = extent(
                            bounds.row,
                            span.row,
                            end(bounds.row, bounds.rows),
                            end(span.row, span.rows),
                        );
                        let (col, cols) = extent(
                            bounds.col,
                            span.col,
                            end(bounds.col, bounds.cols),
                            end(span.col, span.cols),
                        );
                        *bounds = ContentSpan {
                            row,
                            col,
                            rows,
                            cols,
                        };
                        spans.push(span);
                    }
                    _ => layers.push(Layer::Content {
                        bounds: span,
                        spans: vec![span],
                    }),
                },
                SheetChange::Unknown => {}
            }
        }
        Walk { layers }
    }

    #[cfg(test)]
    pub fn is_untouched(&self) -> bool {
        self.layers.is_empty()
    }

    fn moves(&self, wanted: LineAxis) -> bool {
        self.layers
            .iter()
            .any(|layer| matches!(layer, Layer::Lines(op) if op.axis == wanted))
    }

    pub fn moves_columns(&self) -> bool {
        self.moves(LineAxis::Column)
    }

    pub fn moves_rows(&self) -> bool {
        self.moves(LineAxis::Row)
    }

    // The position in the file of a cell nobody wrote since: `None` for one that was, or that
    // sits in a row or column inserted since, which have nothing of the file's.
    pub fn file_position(&self, pos: CellPos) -> Option<CellPos> {
        let (mut row, mut col) = (pos.row.get(), u32::from(pos.col.get()));
        for layer in self.layers.iter().rev() {
            match layer {
                Layer::Content { bounds, spans } => {
                    if bounds.contains(row, col) && spans.iter().any(|span| span.contains(row, col))
                    {
                        return None;
                    }
                }
                Layer::Lines(op) => match op.axis {
                    LineAxis::Row => row = origin(*op, row)?,
                    LineAxis::Column => col = origin(*op, col)?,
                },
            }
        }
        let col = ColIdx::new(u16::try_from(col).ok()?)?;
        Some(CellPos::new(RowIdx::new(row)?, col))
    }

    // The row of the file a row of the sheet comes from, whatever was written in it.
    pub fn file_row(&self, row: RowIdx) -> Option<RowIdx> {
        let mut row = row.get();
        for layer in self.layers.iter().rev() {
            if let Layer::Lines(op) = layer
                && op.axis == LineAxis::Row
            {
                row = origin(*op, row)?;
            }
        }
        RowIdx::new(row)
    }

    // Where the end of a run of the file's rows is now, the end being the number of rows
    // before it: rows deleted from the end of the run pull it up, and rows inserted at it
    // leave it where it was.
    pub fn current_end(&self, file_end: u32) -> u32 {
        let mut end = file_end;
        for layer in &self.layers {
            let Layer::Lines(op) = layer else {
                continue;
            };
            if op.axis != LineAxis::Row {
                continue;
            }
            match op.change {
                LineChange::Insert if end > op.at => end = end.saturating_add(op.count),
                LineChange::Delete if end > op.at.saturating_add(op.count) => end -= op.count,
                LineChange::Delete if end > op.at => end = op.at,
                _ => {}
            }
        }
        end
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
        log.push(content(1, u32::MAX - 1, 0, 1, 1), 4);
        log.push(content(1, u32::MAX, 0, u32::MAX, 1), 5);
        assert_eq!(log.changes().count(), 4);
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
        log.push(content(1, 5000, 5000, 1, 1), 5000);
        assert!(log.has_lost());
        log.clear_through(4999);
        assert!(log.has_lost());
        log.clear_through(5000);
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
    fn blocks_between_the_same_two_operations_are_looked_up_together() {
        let walk = Walk::new(
            [
                content(1, 0, 0, 1, 1),
                content(1, 40, 7, 2, 2),
                content(1, 100, 0, 1, 1),
            ],
            1,
        );
        assert_eq!(walk.file_position(at(0, 0)), None);
        assert_eq!(walk.file_position(at(41, 8)), None);
        assert_eq!(walk.file_position(at(41, 6)), Some(at(41, 6)));
        assert_eq!(walk.file_position(at(50, 0)), Some(at(50, 0)));
        assert_eq!(walk.file_position(at(100, 0)), None);
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
        assert_eq!(walk.current_end(0), 0);
        assert_eq!(walk.current_end(2), 1);
        assert_eq!(walk.current_end(5), 3);
        let inserted = Walk::new([lines(1, LineAxis::Row, LineChange::Insert, 1, 2)], 1);
        assert_eq!(inserted.file_row(RowIdx::new(2).unwrap()), None);
        assert_eq!(inserted.current_end(1), 1);
        assert_eq!(inserted.current_end(2), 4);
        assert_eq!(inserted.current_end(0), 0);
    }
}
