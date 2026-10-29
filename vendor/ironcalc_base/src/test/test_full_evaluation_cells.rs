#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

// The full evaluation visits formula cells only; values, spills and the cells an array
// covers must still come out as when every cell was visited.

#[test]
fn values_formulas_and_spills_in_one_sheet() {
    let mut model = new_empty_model();
    model._set("A1", "10");
    model._set("A2", "text");
    model._set("B1", "=A1*2");
    model._set("B2", "=SEQUENCE(3)");
    model._set("C1", "=SUM(B2:B4)+B1");
    model._set("D5", "=B4+1");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "10");
    assert_eq!(model._get_text("A2"), "text");
    assert_eq!(model._get_text("B1"), "20");
    assert_eq!(model._get_text("B3"), "2");
    assert_eq!(model._get_text("B4"), "3");
    assert_eq!(model._get_text("C1"), "26");
    assert_eq!(model._get_text("D5"), "4");
}

#[test]
fn a_formula_reading_a_later_formula_in_another_sheet() {
    let mut model = new_empty_model();
    model.new_sheet();
    model._set("A1", "=Sheet2!A1+1");
    model.set_user_input(1, 1, 1, "=B1*3".to_string()).unwrap();
    model.set_user_input(1, 1, 2, "4".to_string()).unwrap();
    model.evaluate();
    assert_eq!(model._get_text("A1"), "13");
}

#[test]
fn evaluating_twice_gives_the_same_values() {
    let mut model = new_empty_model();
    model._set("A1", "=SEQUENCE(2,2)");
    model._set("D1", "=SUM(A1:B2)");
    model.evaluate();
    let first = model._get_text("D1");
    model.evaluate();
    assert_eq!(first, "10");
    assert_eq!(model._get_text("D1"), first);
}

#[test]
fn a_sheet_without_formulas_is_skipped_until_it_gets_one() {
    let mut model = new_empty_model();
    model.new_sheet();
    model.set_user_input(1, 1, 1, "5".to_string()).unwrap();
    model._set("A1", "=Sheet2!A1+1");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "6");
    model.set_user_input(1, 2, 1, "=A1*2".to_string()).unwrap();
    model
        .set_user_input(1, 3, 1, "=SEQUENCE(2)".to_string())
        .unwrap();
    model.evaluate();
    assert_eq!(model._get_text_at(1, 2, 1), "10");
    assert_eq!(model._get_text_at(1, 4, 1), "2");
}

#[test]
fn the_formula_cells_are_listed_in_one_allocation() {
    let mut model = new_empty_model();
    model.new_sheet();
    model.new_sheet();
    for row in 1..=5 {
        model._set(&format!("A{row}"), &format!("={row}*2"));
    }
    model._set("B1", "7");
    model.set_user_input(2, 3, 3, "=A1+1".to_string()).unwrap();
    model.set_user_input(1, 1, 1, "text".to_string()).unwrap();
    let cells = model.get_formula_cells();
    assert_eq!(cells.len(), 6);
    assert_eq!(cells.capacity(), cells.len());
    assert_eq!((cells[5].index, cells[5].row, cells[5].column), (2, 3, 3));
}
