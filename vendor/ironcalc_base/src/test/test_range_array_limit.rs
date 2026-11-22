use crate::arithmetic::{BYTES_PER_ELEMENT, MAX_TEXT_ARRAY_BYTES};
use crate::test::util::new_empty_model;

const CELLS_IN_A_COLUMN: usize = 1_048_576;
const CELLS_IN_A_ROW: usize = 16_384;

fn column_letter(column: usize) -> char {
    assert!((1..=26).contains(&column), "column {column}");
    char::from(b'A' + (column - 1) as u8)
}

// A range becomes an array only while its nodes fit the 256 MiB an `&` array may hold, as many
// whole columns or rows as fit there and not one more, nor the whole sheet. Past it the
// function is #VALUE! before any node is built.
#[test]
fn ranges_turned_into_arrays_stop_at_the_byte_budget() {
    let limit = MAX_TEXT_ARRAY_BYTES / BYTES_PER_ELEMENT;
    let columns = limit / CELLS_IN_A_COLUMN;
    let rows = limit / CELLS_IN_A_ROW;
    assert!(columns >= 1 && columns + 1 < 26, "{columns} columns");
    assert!(rows + 1 < 1000, "{rows} rows");
    let fits = column_letter(columns);
    let too_many = column_letter(columns + 1);
    let mut model = new_empty_model();
    model._set("Z1", &format!("=COUNTA(A1:{fits}1048576=0)"));
    model._set("Z2", "=COUNTA(LEFT(A:A,1))");
    model._set("Z3", &format!("=LEFT(A1:{too_many}1048576,1)"));
    model._set("Z4", "=ARRAYTOTEXT(A1:XFD1048576)");
    model._set("Z5", &format!("=IF(A1:{too_many}1048576,1)"));
    model._set("Z6", &format!("=A1:{too_many}1048576+1"));
    model._set("Z1000", &format!("=LEFT(1:{},1)", rows + 1));
    model.evaluate();
    assert_eq!(
        model._get_text("Z1"),
        (columns * CELLS_IN_A_COLUMN).to_string()
    );
    assert_eq!(model._get_text("Z2"), CELLS_IN_A_COLUMN.to_string());
    for cell in ["Z3", "Z4", "Z5", "Z6", "Z1000"] {
        assert_eq!(model._get_text(cell), "#VALUE!", "{cell}");
    }
}

// Cells holding one shared string each get a copy of it in the array, so the texts count
// toward the budget as they are copied: as many copies as fit with their nodes, not one more.
#[test]
fn copies_of_a_shared_string_count_toward_the_byte_budget() {
    let text = "x".repeat(30_000);
    let fit = MAX_TEXT_ARRAY_BYTES / (text.len() + BYTES_PER_ELEMENT);
    let mut model = new_empty_model();
    for row in 1..=fit + 1 {
        model._set(&format!("A{row}"), &text);
    }
    model._set("B1", &format!("=COUNTA(LEFT(A1:A{fit},1))"));
    model._set("B2", &format!("=LEFT(A1:A{},1)", fit + 1));
    model.evaluate();
    let stored = model.workbook.shared_strings.iter().filter(|s| **s == text);
    assert_eq!(stored.count(), 1);
    assert_eq!(model._get_text("B1"), fit.to_string());
    assert_eq!(model._get_text("B2"), "#VALUE!");
}
