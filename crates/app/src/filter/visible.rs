use zenkai_grid::Layout;
use zenkai_types::{FilterView, Range, RowIdx};

// Where a paste goes while the sheet has an AutoFilter that hides rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteRoute {
    // As without a filter.
    Plain,
    // One value over a selection fills the visible rows only.
    FillSelection,
    // Several rows would land in the rows the filter hides, as Excel refuses.
    Refuse,
}

// The runs of data rows the filter hides, which edits, copies and totals leave out.
pub fn filtered_out(view: Option<&FilterView>) -> &[(RowIdx, RowIdx)] {
    view.map_or(&[], |view| view.filtered_out.as_slice())
}

fn hides_rows(view: &FilterView, layout: &Layout, target: Range) -> bool {
    let first = target.start.row.max(view.range.start.row.offset(1));
    let last = target.end.row.min(view.range.end.row);
    if first > last {
        return false;
    }
    let shown: u32 = layout
        .shown_row_runs(first, last)
        .iter()
        .map(|(from, to)| to.get() - from.get() + 1)
        .sum();
    shown < last.get() - first.get() + 1
}

// `payload` is the pasted block's rows and columns; `target` is where it would land.
pub fn paste_route(
    view: Option<&FilterView>,
    layout: &Layout,
    selection: Range,
    payload: (u32, u16),
    target: Range,
) -> PasteRoute {
    let Some(view) = view.filter(|view| !view.filtered_out.is_empty()) else {
        return PasteRoute::Plain;
    };
    if payload == (1, 1) && selection.cell_count() > 1 {
        PasteRoute::FillSelection
    } else if target.rows() > 1 && hides_rows(view, layout, target) {
        PasteRoute::Refuse
    } else {
        PasteRoute::Plain
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::{ColumnCriteria, FilterView, Range, RowIdx, ShownValues};

    use super::{PasteRoute, filtered_out, paste_route};
    use crate::visible_cells::layout_hiding;

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    // A1:B10 filtered, rows 3 to 5 (1-based) hidden by the filter.
    fn filtered() -> FilterView {
        FilterView {
            range: range("A1:B10"),
            criteria: vec![(
                zenkai_types::ColIdx::default(),
                ColumnCriteria::Values(ShownValues::default()),
            )],
            without_button: Vec::new(),
            filtered_out: vec![(RowIdx::clamped(2), RowIdx::clamped(4))],
            found: 6,
            total: 9,
        }
    }

    fn route(selection: &str, payload: (u32, u16), target: &str) -> PasteRoute {
        let layout = layout_hiding(&[2, 3, 4], &[]);
        paste_route(
            Some(&filtered()),
            &layout,
            range(selection),
            payload,
            range(target),
        )
    }

    #[test]
    fn without_hidden_rows_a_paste_goes_as_usual() {
        let layout = layout_hiding(&[], &[]);
        let mut view = filtered();
        view.filtered_out.clear();
        let target = range("A2:A6");
        assert_eq!(
            paste_route(Some(&view), &layout, range("A2"), (5, 1), target),
            PasteRoute::Plain
        );
        assert_eq!(
            paste_route(None, &layout, range("A2"), (5, 1), target),
            PasteRoute::Plain
        );
    }

    #[test]
    fn one_value_over_a_selection_fills_the_visible_rows() {
        assert_eq!(route("A2:A8", (1, 1), "A2"), PasteRoute::FillSelection);
    }

    #[test]
    fn one_value_over_one_cell_is_a_plain_paste() {
        assert_eq!(route("A2", (1, 1), "A2"), PasteRoute::Plain);
    }

    #[test]
    fn several_rows_over_hidden_rows_are_refused() {
        assert_eq!(route("A2", (4, 1), "A2:A5"), PasteRoute::Refuse);
    }

    #[test]
    fn several_rows_over_visible_rows_only_are_allowed() {
        assert_eq!(route("A6", (3, 1), "A6:A8"), PasteRoute::Plain);
        assert_eq!(route("A1", (2, 1), "A1:A2"), PasteRoute::Plain);
    }

    #[test]
    fn one_row_of_several_columns_is_allowed_anywhere() {
        assert_eq!(route("A6", (1, 2), "A6:B6"), PasteRoute::Plain);
    }

    #[test]
    fn the_refusal_names_the_way_out() {
        assert_eq!(
            zenkai_i18n::t!("filter.paste_refused"),
            "Can't paste several rows into a filtered range. Clear the filter or paste into one row."
        );
    }

    #[test]
    fn the_filtered_runs_come_from_the_view() {
        assert_eq!(filtered_out(Some(&filtered())).len(), 1);
        assert!(filtered_out(None).is_empty());
    }
}
