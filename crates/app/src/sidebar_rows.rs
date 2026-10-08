use zenkai_types::WorkbookId;

use crate::spaces::SpaceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    NewWorkbook,
    Space(SpaceId),
    File(WorkbookId),
    NewSpace,
    RecentHeader,
    Recent(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    First,
    Last,
}

pub struct SpaceRows {
    pub id: SpaceId,
    pub collapsed: bool,
    pub files: Vec<WorkbookId>,
}

// The rows the keyboard walks, top to bottom, exactly as they are drawn.
pub fn rows(spaces: &[SpaceRows], recent: usize, recent_open: bool) -> Vec<Row> {
    let mut rows = vec![Row::NewWorkbook];
    for space in spaces {
        rows.push(Row::Space(space.id));
        if !space.collapsed {
            rows.extend(space.files.iter().copied().map(Row::File));
        }
    }
    rows.push(Row::NewSpace);
    if recent > 0 {
        rows.push(Row::RecentHeader);
        if recent_open {
            rows.extend((0..recent).map(Row::Recent));
        }
    }
    rows
}

// A cursor on a row that vanished (a collapsed space, a closed file) restarts from the top.
pub fn step(rows: &[Row], cursor: Option<Row>, movement: Move) -> Option<Row> {
    let last = rows.len().checked_sub(1)?;
    let current = cursor.and_then(|row| rows.iter().position(|candidate| *candidate == row));
    let index = match (movement, current) {
        (Move::First, _) | (_, None) => 0,
        (Move::Last, _) => last,
        (Move::Up, Some(index)) => index.saturating_sub(1),
        (Move::Down, Some(index)) => (index + 1).min(last),
    };
    rows.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: u64) -> WorkbookId {
        WorkbookId(id)
    }

    fn spaces() -> Vec<SpaceRows> {
        vec![
            SpaceRows {
                id: SpaceId(0),
                collapsed: false,
                files: vec![file(1), file(2)],
            },
            SpaceRows {
                id: SpaceId(1),
                collapsed: true,
                files: vec![file(3)],
            },
        ]
    }

    #[test]
    fn rows_follow_the_drawing_order_and_skip_collapsed_files() {
        assert_eq!(
            rows(&spaces(), 2, true),
            [
                Row::NewWorkbook,
                Row::Space(SpaceId(0)),
                Row::File(file(1)),
                Row::File(file(2)),
                Row::Space(SpaceId(1)),
                Row::NewSpace,
                Row::RecentHeader,
                Row::Recent(0),
                Row::Recent(1),
            ]
        );
    }

    #[test]
    fn recent_items_are_hidden_when_folded_and_the_header_when_empty() {
        let folded = rows(&spaces(), 2, false);
        assert_eq!(folded.last(), Some(&Row::RecentHeader));
        let none = rows(&spaces(), 0, true);
        assert_eq!(none.last(), Some(&Row::NewSpace));
    }

    #[test]
    fn stepping_clamps_at_both_ends() {
        let rows = rows(&spaces(), 0, false);
        assert_eq!(
            step(&rows, Some(Row::NewWorkbook), Move::Up),
            Some(Row::NewWorkbook)
        );
        assert_eq!(
            step(&rows, Some(Row::NewSpace), Move::Down),
            Some(Row::NewSpace)
        );
        assert_eq!(
            step(&rows, Some(Row::Space(SpaceId(0))), Move::Down),
            Some(Row::File(file(1)))
        );
    }

    #[test]
    fn first_and_last_jump_to_the_ends() {
        let rows = rows(&spaces(), 0, false);
        assert_eq!(
            step(&rows, Some(Row::File(file(2))), Move::First),
            Some(Row::NewWorkbook)
        );
        assert_eq!(
            step(&rows, Some(Row::File(file(2))), Move::Last),
            Some(Row::NewSpace)
        );
    }

    #[test]
    fn a_vanished_row_restarts_from_the_top() {
        let rows = rows(&spaces(), 0, false);
        assert_eq!(
            step(&rows, Some(Row::File(file(3))), Move::Down),
            Some(Row::NewWorkbook)
        );
        assert_eq!(step(&rows, None, Move::Down), Some(Row::NewWorkbook));
    }
}
