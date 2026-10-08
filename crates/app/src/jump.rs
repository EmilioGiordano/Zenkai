use zenkai_grid::{Direction, step};
use zenkai_types::{CellPos, ColIdx, RowIdx};

// Ctrl+Arrow as in Excel: inside a block go to its last filled cell, otherwise
// to the next filled cell, otherwise to the sheet edge.
pub fn jump_target(
    from: CellPos,
    direction: Direction,
    used_end: CellPos,
    filled: impl Fn(CellPos) -> bool,
) -> CellPos {
    let at_edge = |pos: CellPos| match direction {
        Direction::Up => pos.row == RowIdx::default(),
        Direction::Left => pos.col == ColIdx::default(),
        Direction::Down => pos.row == RowIdx::LAST,
        Direction::Right => pos.col == ColIdx::LAST,
    };
    let beyond_data = |pos: CellPos| match direction {
        Direction::Down => pos.row > used_end.row,
        Direction::Right => pos.col > used_end.col,
        Direction::Up | Direction::Left => false,
    };
    let edge = || match direction {
        Direction::Up => CellPos::new(RowIdx::default(), from.col),
        Direction::Down => CellPos::new(RowIdx::LAST, from.col),
        Direction::Left => CellPos::new(from.row, ColIdx::default()),
        Direction::Right => CellPos::new(from.row, ColIdx::LAST),
    };
    if at_edge(from) {
        return from;
    }
    let next = step(from, direction, 1);
    let mut pos = from;
    if filled(from) && filled(next) {
        while !at_edge(pos) && filled(step(pos, direction, 1)) {
            pos = step(pos, direction, 1);
        }
        return pos;
    }
    pos = next;
    loop {
        if filled(pos) {
            return pos;
        }
        if at_edge(pos) || beyond_data(pos) {
            return edge();
        }
        pos = step(pos, direction, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    #[test]
    fn jumps_like_excel() {
        let filled = |p: CellPos| matches!(p.row.get(), 0..=2 | 6..=7) && p.col.get() == 0;
        let end = pos("A8");
        assert_eq!(
            jump_target(pos("A1"), Direction::Down, end, filled),
            pos("A3")
        );
        assert_eq!(
            jump_target(pos("A3"), Direction::Down, end, filled),
            pos("A7")
        );
        assert_eq!(
            jump_target(pos("A7"), Direction::Down, end, filled),
            pos("A8")
        );
        assert_eq!(
            jump_target(pos("A8"), Direction::Down, end, filled).row,
            RowIdx::LAST
        );
        assert_eq!(
            jump_target(pos("A7"), Direction::Up, end, filled),
            pos("A3")
        );
        assert_eq!(
            jump_target(pos("B5"), Direction::Up, end, filled),
            pos("B1")
        );
    }
}
