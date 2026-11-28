use zenkai_i18n::t;
use zenkai_types::{ColIdx, ColumnCriteria, FilterView};

// Excel names a column by its header; with an empty header cell, by its letters.
pub fn column_name(header: &str, col: ColIdx) -> String {
    let header = header.trim();
    if header.is_empty() {
        t!("filter.unnamed_column", letters = col.letters())
    } else {
        header.to_string()
    }
}

// What the button announces and its tooltip says.
pub fn button_label(view: &FilterView, col: ColIdx, name: &str) -> String {
    let criteria = view
        .criteria
        .iter()
        .find(|(column, _)| *column == col)
        .map(|(_, criteria)| criteria);
    let shown = match criteria {
        None => return t!("filter.tooltip_all", column = name),
        Some(ColumnCriteria::Custom) => return t!("filter.tooltip_custom", column = name),
        Some(ColumnCriteria::Values(shown)) => shown,
    };
    let mut values: Vec<&str> = shown.values.iter().map(String::as_str).collect();
    if shown.blanks {
        values.push(t!("filter.blanks"));
    }
    match values.as_slice() {
        [one] => t!("filter.tooltip_one", column = name, value = one),
        [first, second] => t!(
            "filter.tooltip_two",
            column = name,
            first = first,
            second = second
        ),
        many => t!("filter.tooltip_many", count = many.len(), column = name),
    }
}

// The status bar's left text while any column has criteria.
pub fn records_found(view: &FilterView) -> Option<String> {
    if !view.is_filtering() {
        return None;
    }
    Some(if view.found == 0 {
        t!("filter.no_records").to_string()
    } else {
        t!(
            "filter.records_found",
            shown = zenkai_i18n::number(u64::from(view.found)),
            total = zenkai_i18n::number(u64::from(view.total))
        )
    })
}

// A value is cut with an ellipsis in a row of the list, and its full text shows in a tooltip.
const MAX_ROW_CHARS: usize = 28;

pub fn needs_tooltip(text: &str) -> bool {
    text.chars().count() > MAX_ROW_CHARS
}

#[cfg(test)]
mod tests {
    use zenkai_types::{ColIdx, ColumnCriteria, FilterView, Range, ShownValues};

    use super::{button_label, column_name, needs_tooltip, records_found};

    fn view(criteria: Vec<(ColIdx, ColumnCriteria)>, found: u32, total: u32) -> FilterView {
        FilterView {
            range: Range::parse_a1("A1:C9").unwrap(),
            criteria,
            without_button: Vec::new(),
            filtered_out: Vec::new(),
            found,
            total,
        }
    }

    fn values(texts: &[&str], blanks: bool) -> ColumnCriteria {
        ColumnCriteria::Values(ShownValues {
            values: texts.iter().map(|text| (*text).to_string()).collect(),
            blanks,
        })
    }

    #[test]
    fn an_empty_header_names_the_column_by_its_letters() {
        assert_eq!(column_name("Region", ColIdx::clamped(1)), "Region");
        assert_eq!(column_name("  ", ColIdx::clamped(27)), "Column AB");
    }

    #[test]
    fn the_button_names_its_column_and_what_it_shows() {
        let col = ColIdx::clamped(1);
        let tooltip = |criteria| button_label(&view(criteria, 3, 8), col, "Region");
        assert_eq!(tooltip(Vec::new()), "Region: (Showing All)");
        assert_eq!(
            tooltip(vec![(col, values(&["North"], false))]),
            "Region: Equals \"North\""
        );
        assert_eq!(
            tooltip(vec![(col, values(&["North", "West"], false))]),
            "Region: Equals \"North\" or \"West\""
        );
        assert_eq!(
            tooltip(vec![(col, values(&["North", "West"], true))]),
            "Region: 3 values selected"
        );
        assert_eq!(
            tooltip(vec![(col, ColumnCriteria::Custom)]),
            "Region: Custom filter"
        );
    }

    #[test]
    fn another_columns_criteria_leave_this_one_showing_all() {
        let other = vec![(ColIdx::clamped(2), values(&["x"], false))];
        assert_eq!(
            button_label(&view(other, 1, 8), ColIdx::clamped(1), "Region"),
            "Region: (Showing All)"
        );
    }

    #[test]
    fn the_status_bar_counts_visible_data_rows_of_all_of_them() {
        let col = ColIdx::clamped(0);
        let found =
            |found, total| records_found(&view(vec![(col, values(&["x"], false))], found, total));
        assert_eq!(found(3, 8).as_deref(), Some("3 of 8 records found"));
        assert_eq!(
            found(1_200, 10_000).as_deref(),
            Some("1,200 of 10,000 records found")
        );
        assert_eq!(found(0, 8).as_deref(), Some("No records found"));
    }

    #[test]
    fn the_status_bar_says_nothing_without_criteria() {
        assert_eq!(records_found(&view(Vec::new(), 8, 8)), None);
    }

    #[test]
    fn only_a_long_value_needs_the_full_text_in_a_tooltip() {
        assert!(!needs_tooltip("North"));
        assert!(needs_tooltip(
            "A value that is much too long for one row of the list"
        ));
    }
}
