use crate::arithmetic::{BYTES_PER_ELEMENT, MAX_TEXT_ARRAY_BYTES};
use crate::test::util::new_empty_model;

// A range becomes an array only while its nodes fit the 256 MiB an `&` array may hold,
// 5,592,405 elements of 48 bytes: five whole columns or 341 whole rows, not six columns, 342
// rows or the whole sheet. Past it the function is #VALUE! before any node is built.
#[test]
fn ranges_turned_into_arrays_stop_at_the_byte_budget() {
    assert_eq!(BYTES_PER_ELEMENT, 48);
    assert_eq!(MAX_TEXT_ARRAY_BYTES / BYTES_PER_ELEMENT, 5_592_405);
    let mut model = new_empty_model();
    model._set("H1", "=COUNTA(A1:E1048576=0)");
    model._set("H2", "=COUNTA(LEFT(A:A,1))");
    model._set("H3", "=LEFT(A1:F1048576,1)");
    model._set("H4", "=ARRAYTOTEXT(A1:XFD1048576)");
    model._set("H5", "=IF(A1:F1048576,1)");
    model._set("H6", "=A1:F1048576+1");
    model._set("H1000", "=LEFT(1:342,1)");
    model.evaluate();
    assert_eq!(model._get_text("H1"), "5242880");
    assert_eq!(model._get_text("H2"), "1048576");
    for cell in ["H3", "H4", "H5", "H6", "H1000"] {
        assert_eq!(model._get_text(cell), "#VALUE!", "{cell}");
    }
}
