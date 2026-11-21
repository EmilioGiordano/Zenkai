#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

#[test]
fn test_base_pads_to_the_minimum_length() {
    let mut model = new_empty_model();
    model._set("A1", "=BASE(5,2)");
    model._set("A2", "=BASE(5,2,8)");
    model._set("A3", "=BASE(255,16,1)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "101");
    assert_eq!(model._get_text("A2"), "00000101");
    assert_eq!(model._get_text("A3"), "FF");
}

// Excel allows a minimum length up to 255 and gives #NUM! past it; a file asking for billions
// of characters used to allocate them.
#[test]
fn test_base_minimum_length_past_255_is_num() {
    let mut model = new_empty_model();
    model._set("A1", "=BASE(1,2,255)");
    model._set("A2", "=BASE(1,2,255.9)");
    model._set("A3", "=BASE(1,2,256)");
    model._set("A4", "=BASE(1,2,1E+300)");
    model._set("A5", "=BASE(1,2,-1)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), format!("{}1", "0".repeat(254)));
    assert_eq!(model._get_text("A2"), format!("{}1", "0".repeat(254)));
    assert_eq!(model._get_text("A3"), "#NUM!");
    assert_eq!(model._get_text("A4"), "#NUM!");
    assert_eq!(model._get_text("A5"), "#NUM!");
}
