use zenkai_types::{CellPos, Range};

const GRIP: f32 = 4.0;

pub(crate) fn on_handle(pointer: (f32, f32), corner: (f32, f32), zoom: f32) -> bool {
    let reach = GRIP * zoom;
    (pointer.0 - corner.0).abs() <= reach && (pointer.1 - corner.1).abs() <= reach
}

// Excel extends down when the pointer passes the last row, else right when it passes the
// last column; up and left are not supported yet.
pub(crate) fn target_for(source: Range, pos: CellPos) -> Range {
    if pos.row > source.end.row {
        Range::new(source.start, CellPos::new(pos.row, source.end.col))
    } else if pos.col > source.end.col {
        Range::new(source.start, CellPos::new(source.end.row, pos.col))
    } else {
        source
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::{ColIdx, RowIdx};

    use super::*;

    fn at(row: u32, col: u16) -> CellPos {
        CellPos::new(RowIdx::new(row).unwrap(), ColIdx::new(col).unwrap())
    }

    #[test]
    fn handle_is_grabbed_within_the_grip_of_the_corner() {
        assert!(on_handle((100.0, 50.0), (100.0, 50.0), 1.0));
        assert!(on_handle((103.9, 46.1), (100.0, 50.0), 1.0));
        assert!(!on_handle((104.5, 50.0), (100.0, 50.0), 1.0));
        assert!(!on_handle((100.0, 55.0), (100.0, 50.0), 1.0));
    }

    #[test]
    fn grip_follows_the_zoom() {
        assert!(on_handle((107.0, 50.0), (100.0, 50.0), 2.0));
        assert!(!on_handle((107.0, 50.0), (100.0, 50.0), 1.0));
    }

    #[test]
    fn dragging_below_extends_down_keeping_the_source_columns() {
        let source = Range::new(at(0, 0), at(1, 1));
        assert_eq!(target_for(source, at(5, 1)), Range::new(at(0, 0), at(5, 1)));
        assert_eq!(target_for(source, at(5, 9)), Range::new(at(0, 0), at(5, 1)));
    }

    #[test]
    fn dragging_to_the_right_extends_right_keeping_the_source_rows() {
        let source = Range::new(at(0, 0), at(1, 1));
        assert_eq!(target_for(source, at(1, 4)), Range::new(at(0, 0), at(1, 4)));
    }

    #[test]
    fn dragging_inside_or_before_the_source_changes_nothing() {
        let source = Range::new(at(2, 2), at(3, 3));
        assert_eq!(target_for(source, at(3, 3)), source);
        assert_eq!(target_for(source, at(0, 0)), source);
    }
}
