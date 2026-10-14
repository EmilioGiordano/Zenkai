use std::collections::HashMap;
use std::rc::Rc;

use zenkai_types::{CellPos, Range};

use crate::grid::GridCell;

pub(crate) const KEPT_SLACK_ROWS: u32 = 100;
pub(crate) const KEPT_SLACK_COLS: u16 = 20;

// Cells inside `covered` are replaced by `fresh`, so deleted ones disappear. Cells outside it
// stay while within the slack, so rows scrolled past come back with their last values
// when the workbook is busy; farther ones go, keeping memory near the viewport.
// Only the kept cells are moved or cloned, never the whole old map.
pub(crate) fn merge(
    old: Rc<HashMap<CellPos, GridCell>>,
    fresh: HashMap<CellPos, GridCell>,
    covered: &[Range],
) -> HashMap<CellPos, GridCell> {
    let keep = |pos: &CellPos| {
        !covered.iter().any(|range| range.contains(*pos))
            && covered.iter().any(|range| near(range, *pos))
    };
    let mut merged = match Rc::try_unwrap(old) {
        Ok(mut owned) => {
            owned.retain(|pos, _| keep(pos));
            owned
        }
        Err(shared) => shared
            .iter()
            .filter(|(pos, _)| keep(pos))
            .map(|(pos, cell)| (*pos, cell.clone()))
            .collect(),
    };
    merged.extend(fresh);
    merged
}

fn near(range: &Range, pos: CellPos) -> bool {
    pos.row >= range.start.row.offset(-i64::from(KEPT_SLACK_ROWS))
        && pos.row <= range.end.row.offset(i64::from(KEPT_SLACK_ROWS))
        && pos.col >= range.start.col.offset(-i64::from(KEPT_SLACK_COLS))
        && pos.col <= range.end.col.offset(i64::from(KEPT_SLACK_COLS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_types::{CellStyle, ValueKind};

    fn pos(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    fn cell(text: &str) -> GridCell {
        GridCell {
            text: text.to_string().into(),
            kind: ValueKind::Text,
            style: CellStyle::default(),
        }
    }

    fn cache(cells: &[(&str, &str)]) -> HashMap<CellPos, GridCell> {
        cells
            .iter()
            .map(|(at, text)| (pos(at), cell(text)))
            .collect()
    }

    fn covered() -> Vec<Range> {
        vec![Range::parse_a1("A501:C520").unwrap()]
    }

    fn merged(
        known: HashMap<CellPos, GridCell>,
        fresh: HashMap<CellPos, GridCell>,
    ) -> HashMap<CellPos, GridCell> {
        merge(Rc::new(known), fresh, &covered())
    }

    #[test]
    fn known_cells_near_the_new_range_keep_their_values() {
        let result = merged(cache(&[("A450", "old")]), cache(&[("A501", "new")]));
        assert_eq!(result[&pos("A450")].text.as_ref(), "old");
        assert_eq!(result[&pos("A501")].text.as_ref(), "new");
    }

    #[test]
    fn cells_far_from_the_new_range_are_dropped() {
        let result = merged(cache(&[("A1", "far"), ("A600", "near")]), HashMap::new());
        assert!(!result.contains_key(&pos("A1")));
        assert!(result.contains_key(&pos("A600")));
    }

    #[test]
    fn stale_values_inside_the_range_are_replaced_or_removed() {
        let result = merged(
            cache(&[("A501", "stale"), ("B501", "deleted")]),
            cache(&[("A501", "fresh")]),
        );
        assert_eq!(result[&pos("A501")].text.as_ref(), "fresh");
        assert!(!result.contains_key(&pos("B501")));
    }

    #[test]
    fn columns_far_to_the_side_are_dropped() {
        assert!(merged(cache(&[("Z510", "far")]), HashMap::new()).is_empty());
    }

    #[test]
    fn nothing_is_kept_without_a_covered_range() {
        let result = merge(Rc::new(cache(&[("A501", "old")])), HashMap::new(), &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn a_cache_still_shared_with_a_frame_is_left_untouched() {
        let shared = Rc::new(cache(&[("A450", "old"), ("A1", "far")]));
        let result = merge(Rc::clone(&shared), cache(&[("A501", "new")]), &covered());
        assert_eq!(result.len(), 2);
        assert_eq!(shared.len(), 2);
    }
}
