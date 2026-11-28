use zenkai_types::{CellPos, Range};

// Ctrl+Shift+L filters the selection when it holds more than one cell, else the current
// region of the active cell; the first row is the header. A selection that runs past the
// used area (whole columns) is cut to it.
pub fn toggle_range(selection: Range, region: Range, used_end: CellPos) -> Range {
    if selection.cell_count() > 1 {
        selection.clip_to(used_end).unwrap_or(region)
    } else {
        region
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::{CellPos, Range};

    use super::toggle_range;

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    fn end(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    #[test]
    fn one_cell_filters_its_current_region() {
        assert_eq!(
            toggle_range(range("B3"), range("A1:D9"), end("D9")),
            range("A1:D9")
        );
    }

    #[test]
    fn a_selection_of_several_cells_filters_exactly_that() {
        assert_eq!(
            toggle_range(range("B2:C5"), range("A1:D9"), end("D9")),
            range("B2:C5")
        );
    }

    #[test]
    fn whole_columns_are_cut_to_the_used_area() {
        assert_eq!(
            toggle_range(range("B1:C1048576"), range("A1:D9"), end("D9")),
            range("B1:C9")
        );
    }

    #[test]
    fn a_selection_wholly_past_the_used_area_falls_back_to_the_region() {
        assert_eq!(
            toggle_range(range("F20:G30"), range("F20"), end("D9")),
            range("F20")
        );
    }
}
