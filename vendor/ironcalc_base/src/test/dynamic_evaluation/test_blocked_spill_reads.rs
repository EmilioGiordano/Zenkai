#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

// A formula reading an anchor whose spill is blocked sees #SPILL!, as in Excel, even when
// the read is what makes the anchor evaluate.

#[test]
fn formula_reads_blocked_anchor() {
    let mut model = new_empty_model();
    model._set("B1", "=E5*10");
    model._set("E5", "=SEQUENCE(2)");
    model._set("E6", "99");
    model.evaluate();
    assert_eq!(model._get_text("E5"), "#SPILL!");
    assert_eq!(model._get_text("B1"), "#SPILL!");
}

//         ║      A        |   C   |      E       |
// ════════╬═══════════════╪═══════╪══════════════╪
//    1    ║ =SEQUENCE(C1) | =E5   |              |
//    5    ║               |       | =SEQUENCE(2) |
//    6    ║               |       |      99      |
//
// A1 is evaluated before E5 and reaches it through C1, which is not an anchor.
#[test]
fn anchor_reads_blocked_anchor_through_formula() {
    let mut model = new_empty_model();
    model._set("A1", "=SEQUENCE(C1)");
    model._set("C1", "=E5");
    model._set("E5", "=SEQUENCE(2)");
    model._set("E6", "99");
    model.evaluate();
    assert_eq!(model._get_text("E5"), "#SPILL!");
    assert_eq!(model._get_text("C1"), "#SPILL!");
    assert_eq!(model._get_text("A1"), "#SPILL!");
}

#[test]
fn anchor_reads_anchor_past_the_last_row_through_formula() {
    let mut model = new_empty_model();
    model._set("A1", "=SEQUENCE(C1)");
    model._set("C1", "=E1048576");
    model._set("E1048576", "=SEQUENCE(2)");
    model.evaluate();
    assert_eq!(model._get_text("E1048576"), "#SPILL!");
    assert_eq!(model._get_text("C1"), "#SPILL!");
    assert_eq!(model._get_text("A1"), "#SPILL!");
}
