use std::num::NonZero;
use std::thread;

use zenkai_grid::{Direction, step};
use zenkai_types::{CellPos, ColIdx, RowIdx};

// Short jumps stay on the calling thread; only a long scan, such as reaching the end of
// a column of hundreds of thousands of rows, is worth waking the other cores.
const SEQUENTIAL_STEPS: u32 = 4096;
const CHUNK_STEPS: u32 = 16_384;

// Ctrl+Arrow as in Excel: inside a block go to its last filled cell, otherwise
// to the next filled cell, otherwise to the sheet edge.
pub fn jump_target(
    from: CellPos,
    direction: Direction,
    used_end: CellPos,
    filled: impl Fn(CellPos) -> bool + Sync,
) -> CellPos {
    let edge = match direction {
        Direction::Up => CellPos::new(RowIdx::default(), from.col),
        Direction::Down => CellPos::new(RowIdx::LAST, from.col),
        Direction::Left => CellPos::new(from.row, ColIdx::default()),
        Direction::Right => CellPos::new(from.row, ColIdx::LAST),
    };
    let to_edge = steps_to(from, edge, direction);
    if to_edge == 0 {
        return from;
    }
    let at = |steps: u32| step(from, direction, i64::from(steps));
    if filled(from) && filled(at(1)) {
        return match first_step(to_edge, |steps| !filled(at(steps))) {
            Some(steps) => at(steps - 1),
            None => edge,
        };
    }
    let data_end = match direction {
        Direction::Down | Direction::Right => used_end,
        Direction::Up | Direction::Left => edge,
    };
    let to_data_end = steps_to(from, data_end, direction).min(to_edge);
    first_step(to_data_end, |steps| filled(at(steps))).map_or(edge, at)
}

fn steps_to(from: CellPos, target: CellPos, direction: Direction) -> u32 {
    match direction {
        Direction::Down => target.row.get().saturating_sub(from.row.get()),
        Direction::Up => from.row.get().saturating_sub(target.row.get()),
        Direction::Right => u32::from(target.col.get().saturating_sub(from.col.get())),
        Direction::Left => u32::from(from.col.get().saturating_sub(target.col.get())),
    }
}

// The smallest step in 1..=steps that satisfies `hit`.
fn first_step(steps: u32, hit: impl Fn(u32) -> bool + Sync) -> Option<u32> {
    let sequential_end = steps.min(SEQUENTIAL_STEPS);
    if let Some(found) = (1..=sequential_end).find(|&n| hit(n)) {
        return Some(found);
    }
    let workers = thread::available_parallelism().map_or(1, NonZero::get) as u32;
    let hit = &hit;
    let mut start = sequential_end + 1;
    while start <= steps {
        let found = thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|worker| {
                    let first = start + worker * CHUNK_STEPS;
                    let last = (first + CHUNK_STEPS - 1).min(steps);
                    scope.spawn(move || (first..=last).find(|&n| hit(n)))
                })
                .collect();
            handles.into_iter().find_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
        });
        if found.is_some() {
            return found;
        }
        start += workers * CHUNK_STEPS;
    }
    None
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

    #[test]
    fn long_scans_find_the_same_cells_as_short_ones() {
        let filled = |p: CellPos| p.col.get() == 0 && p.row.get() <= 300_000;
        let edge = CellPos::new(RowIdx::LAST, ColIdx::default());
        let end = CellPos::new(RowIdx::new(300_000).unwrap(), ColIdx::default());
        assert_eq!(jump_target(pos("A1"), Direction::Down, end, filled), end);
        assert_eq!(jump_target(end, Direction::Down, end, filled), edge);
        assert_eq!(jump_target(edge, Direction::Up, end, filled), end);
        assert_eq!(
            jump_target(pos("A1"), Direction::Up, end, filled),
            pos("A1")
        );
        let sparse = |p: CellPos| p.col.get() == 0 && p.row.get() == 250_000;
        assert_eq!(
            jump_target(pos("A1"), Direction::Down, end, sparse),
            CellPos::new(RowIdx::new(250_000).unwrap(), ColIdx::default())
        );
    }
}
