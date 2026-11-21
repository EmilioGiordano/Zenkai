#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

#[test]
fn test_arraytotext_concise() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("B1", "hello");
    model._set("A2", "2");
    model._set("B2", "world");
    model._set("C1", "=ARRAYTOTEXT(A1:B2)");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "1, hello, 2, world");
}

#[test]
fn test_arraytotext_strict() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("B1", "hello");
    model._set("C1", "=ARRAYTOTEXT(A1:B1, 1)");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "{1,\"hello\"}");
}

#[test]
fn test_arraytotext_separates_rows_and_columns() {
    let mut model = new_empty_model();
    model._set("A1", "=ARRAYTOTEXT({1,\"a\";TRUE,4})");
    model._set("A2", "=ARRAYTOTEXT({1,\"a\";TRUE,4}, 1)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "1, a, TRUE, 4");
    assert_eq!(model._get_text("A2"), "{1,\"a\";TRUE,4}");
}

// A result past Excel's 32,767 characters, separators, quotes and braces included, is #VALUE!
// instead of the whole array joined.
#[test]
fn test_arraytotext_past_the_text_limit_is_value() {
    let mut model = new_empty_model();
    model._set("A1", "=REPT(\"x\",16382)");
    model._set("A2", "=REPT(\"x\",16383)");
    model._set("A3", "=REPT(\"x\",16384)");
    model._set("B1", "=REPT(\"x\",16380)");
    model._set("C1", "=REPT(\"x\",16380)");
    model._set("B2", "=REPT(\"x\",16380)");
    model._set("C2", "=REPT(\"x\",16381)");
    model._set("D1", "=ARRAYTOTEXT(A1:A2)");
    model._set("D2", "=ARRAYTOTEXT(A2:A3)");
    model._set("D3", "=ARRAYTOTEXT(B1:C1, 1)");
    model._set("D4", "=ARRAYTOTEXT(B2:C2, 1)");
    model._set("D5", "=ARRAYTOTEXT(REPT(\"x\",32765), 1)");
    model._set("D6", "=ARRAYTOTEXT(REPT(\"x\",32766), 1)");
    model._set("D7", "=ARRAYTOTEXT(REPT(\"\"\"\",16383), 1)");
    model._set("D8", "=ARRAYTOTEXT(REPT(\"x\",32767))");
    model.evaluate();
    let x = |count: usize| "x".repeat(count);
    assert_eq!(model._get_text("D1"), format!("{}, {}", x(16382), x(16383)));
    assert_eq!(model._get_text("D2"), "#VALUE!");
    assert_eq!(
        model._get_text("D3"),
        format!("{{\"{}\",\"{}\"}}", x(16380), x(16380))
    );
    assert_eq!(model._get_text("D4"), "#VALUE!");
    assert_eq!(model._get_text("D5"), format!("\"{}\"", x(32765)));
    assert_eq!(model._get_text("D6"), "#VALUE!");
    assert_eq!(model._get_text("D7"), "#VALUE!");
    assert_eq!(model._get_text("D8"), x(32767));
}
