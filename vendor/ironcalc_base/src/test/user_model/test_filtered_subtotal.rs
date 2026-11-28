#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;
use crate::types::{AutoFilter, FilterColumn, FilterCriteria};
use crate::UserModel;

fn filter_on_column_a(filtered_rows: Vec<i32>) -> AutoFilter {
    AutoFilter {
        first_row: 1,
        first_column: 1,
        last_row: 10,
        last_column: 1,
        columns: vec![FilterColumn {
            column: 0,
            criteria: FilterCriteria::Values {
                values: vec!["2".to_string()],
                blank: false,
            },
            hidden_button: false,
            show_button: true,
        }],
        filtered_rows,
        sort_state: None,
    }
}

// Excel's SUBTOTAL 1 to 11 count rows hidden by hand but never rows a filter hides.
#[test]
fn subtotal_skips_filtered_rows_but_counts_hidden_ones() {
    let mut model = UserModel::from_model(new_empty_model());
    for row in 2..=6 {
        model.set_user_input(0, row, 1, &row.to_string()).unwrap();
    }
    model.set_user_input(0, 8, 1, "=SUBTOTAL(9,A2:A6)").unwrap();
    model
        .set_user_input(0, 9, 1, "=SUBTOTAL(109,A2:A6)")
        .unwrap();
    model
        .set_scattered_rows_hidden(0, &[(3, true), (5, true)])
        .unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 8, 1).unwrap(), "20");
    assert_eq!(model.get_formatted_cell_value(0, 9, 1).unwrap(), "12");
    model
        .set_auto_filter(0, Some(filter_on_column_a(vec![5])))
        .unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 8, 1).unwrap(), "15");
    assert_eq!(model.get_formatted_cell_value(0, 9, 1).unwrap(), "12");
    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 8, 1).unwrap(), "20");
}
