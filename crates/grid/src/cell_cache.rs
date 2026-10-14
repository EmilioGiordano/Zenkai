use std::collections::HashMap;

use zenkai_types::{CellPos, Range};

use crate::grid::GridCell;

// Cells inside `covered` are replaced by `fresh`, so deleted ones disappear. Cells outside it
// stay while within the slack, so rows scrolled past come back with their last values
// when the workbook is busy; farther ones go, keeping memory near the viewport.
pub(crate) fn merge(
    cache: &mut HashMap<CellPos, GridCell>,
    fresh: HashMap<CellPos, GridCell>,
    covered: &[Range],
    slack_rows: u32,
    slack_cols: u16,
) {
    cache.retain(|pos, _| {
        !covered.iter().any(|range| range.contains(*pos))
            && covered
                .iter()
                .any(|range| near(range, *pos, slack_rows, slack_cols))
    });
    cache.extend(fresh);
}

fn near(range: &Range, pos: CellPos, slack_rows: u32, slack_cols: u16) -> bool {
    pos.row >= range.start.row.offset(-i64::from(slack_rows))
        && pos.row <= range.end.row.offset(i64::from(slack_rows))
        && pos.col >= range.start.col.offset(-i64::from(slack_cols))
        && pos.col <= range.end.col.offset(i64::from(slack_cols))
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
        cells.iter().map(|(at, text)| (pos(at), cell(text))).collect()
    }

    fn covered() -> Vec<Range> {
        vec![Range::parse_a1("A101:C120").unwrap()]
    }

    #[test]
    fn known_cells_near_the_new_range_keep_their_values() {
        let mut known = cache(&[("A95", "old")]);
        merge(&mut known, cache(&[("A101", "new")]), &covered(), 10, 2);
        assert_eq!(known[&pos("A95")].text.as_ref(), "old");
        assert_eq!(known[&pos("A101")].text.as_ref(), "new");
    }

    #[test]
    fn cells_far_from_the_new_range_are_dropped() {
        let mut known = cache(&[("A1", "far"), ("A130", "near")]);
        merge(&mut known, HashMap::new(), &covered(), 10, 2);
        assert!(!known.contains_key(&pos("A1")));
        assert!(known.contains_key(&pos("A130")));
    }

    #[test]
    fn stale_values_inside_the_range_are_replaced_or_removed() {
        let mut known = cache(&[("A101", "stale"), ("B101", "deleted")]);
        merge(&mut known, cache(&[("A101", "fresh")]), &covered(), 10, 2);
        assert_eq!(known[&pos("A101")].text.as_ref(), "fresh");
        assert!(!known.contains_key(&pos("B101")));
    }

    #[test]
    fn nothing_is_kept_without_a_covered_range() {
        let mut known = cache(&[("A101", "old")]);
        merge(&mut known, HashMap::new(), &[], 10, 2);
        assert!(known.is_empty());
    }

    #[test]
    fn columns_far_to_the_side_are_dropped() {
        let mut known = cache(&[("Z110", "far")]);
        merge(&mut known, HashMap::new(), &covered(), 10, 2);
        assert!(known.is_empty());
    }
}
