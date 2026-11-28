#![allow(clippy::unwrap_used)]

use crate::{model::Model, test::util::new_empty_model};

// Keys in A1:A3 and three columns of values in B1:D3.
fn table_model<'a>(keys: [&str; 3]) -> Model<'a> {
    let mut model = new_empty_model();
    for (index, key) in keys.iter().enumerate() {
        let row = index + 1;
        model._set(&format!("A{row}"), key);
        model._set(&format!("B{row}"), &(row * 10).to_string());
        model._set(&format!("C{row}"), &(row * 10 + 1).to_string());
        model._set(&format!("D{row}"), &(row * 10 + 2).to_string());
    }
    model
}

fn assert_row(model: &Model, cells: [&str; 4], expected: [&str; 4]) {
    for (cell, value) in cells.iter().zip(expected) {
        assert_eq!(model._get_text(cell), value, "{cell}");
    }
}

#[test]
fn exact_match_spills_the_row_of_whole_columns() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"b\",A:A,B:D)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["20", "21", "22", ""]);
    assert_eq!(model._get_text("F2"), "");
    assert!(model._get_cell("F1").dynamic_range().is_some());
}

#[test]
fn a_single_column_return_array_gives_one_value() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"b\",A1:A3,C1:C3)");
    model._set("F2", "=XLOOKUP(\"c\",A:A,D:D)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "F2", "G2"], ["21", "", "32", ""]);
    assert!(model._get_cell("F1").dynamic_range().is_none());
    assert!(model._get_cell("F2").dynamic_range().is_none());
}

#[test]
fn not_found_gives_na_in_one_cell() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"z\",A1:A3,B1:D3)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "F2"], ["#N/A", "", "", ""]);
}

#[test]
fn not_found_gives_if_not_found_in_one_cell() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"z\",A1:A3,B1:D3,\"none\")");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "F2"], ["none", "", "", ""]);
}

#[test]
fn approximate_match_spills_the_next_smaller_or_larger_row() {
    let mut model = table_model(["10", "20", "30"]);
    model._set("F1", "=XLOOKUP(25,A1:A3,B1:D3,,-1)");
    model._set("F2", "=XLOOKUP(25,A1:A3,B1:D3,,1)");
    model._set("F3", "=XLOOKUP(5,A1:A3,B1:D3,\"none\",-1)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["20", "21", "22", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["30", "31", "32", ""]);
    assert_row(&model, ["F3", "G3", "H3", "I3"], ["none", "", "", ""]);
}

#[test]
fn wildcard_match_spills_the_row() {
    let mut model = table_model(["apple", "banana", "cherry"]);
    model._set("F1", "=XLOOKUP(\"ban*\",A1:A3,B1:D3,,2)");
    model._set("F2", "=XLOOKUP(\"?herry\",A1:A3,B1:D3,,2)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["20", "21", "22", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["30", "31", "32", ""]);
}

#[test]
fn reverse_search_spills_the_last_matching_row() {
    let mut model = table_model(["x", "y", "x"]);
    model._set("F1", "=XLOOKUP(\"x\",A1:A3,B1:D3)");
    model._set("F2", "=XLOOKUP(\"x\",A1:A3,B1:D3,,,-1)");
    model._set("F3", "=XLOOKUP(\"x\",A1:A3,B1:D3,,0,-1)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["10", "11", "12", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["30", "31", "32", ""]);
    assert_row(&model, ["F3", "G3", "H3", "I3"], ["30", "31", "32", ""]);
}

#[test]
fn binary_search_spills_the_row() {
    let mut model = table_model(["10", "20", "30"]);
    model._set("F1", "=XLOOKUP(20,A1:A3,B1:D3,,0,2)");
    model._set("F2", "=XLOOKUP(25,A1:A3,B1:D3,,-1,2)");
    model._set("F3", "=XLOOKUP(25,A1:A3,B1:D3,,1,2)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["20", "21", "22", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["20", "21", "22", ""]);
    assert_row(&model, ["F3", "G3", "H3", "I3"], ["30", "31", "32", ""]);
}

#[test]
fn a_row_lookup_array_spills_the_column() {
    let mut model = new_empty_model();
    model._set("A1", "a");
    model._set("B1", "b");
    model._set("C1", "c");
    model._set("A2", "1");
    model._set("B2", "2");
    model._set("C2", "3");
    model._set("A3", "4");
    model._set("B3", "5");
    model._set("C3", "6");
    model._set("E1", "=XLOOKUP(\"b\",A1:C1,A2:C3)");
    model._set("F1", "=XLOOKUP(\"c\",A1:C1,A2:C2)");
    model.evaluate();
    assert_row(&model, ["E1", "E2", "E3", "F2"], ["2", "5", "", ""]);
    assert_eq!(model._get_text("F1"), "3");
    assert!(model._get_cell("E1").dynamic_range().is_some());
    assert!(model._get_cell("F1").dynamic_range().is_none());
}

#[test]
fn return_array_of_another_length_is_value_error() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"b\",A1:A3,B1:D4)");
    model._set("F2", "=XLOOKUP(\"b\",A1:A3,B2:D3)");
    model._set("F3", "=XLOOKUP(\"b\",A1:C1,A2:B3)");
    model._set("F4", "=XLOOKUP(\"b\",A1:A3,B1:B4)");
    model.evaluate();
    assert_row(&model, ["F1", "F2", "F3", "F4"], ["#VALUE!"; 4]);
    assert_row(&model, ["G1", "G2", "G3", "G4"], [""; 4]);
}

// A one-cell lookup_array is read down when return_array is one row, as for a table with one
// row, and across when it is one column.
#[test]
fn a_one_cell_lookup_array_spills_a_row_or_a_column() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"a\",A1:A1,B1:D1)");
    model._set("F3", "=XLOOKUP(\"a\",A1:A1,B1:B2)");
    model._set("H3", "=XLOOKUP(\"a\",A1:A1,B1:C2)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["10", "11", "12", ""]);
    assert_row(
        &model,
        ["F3", "F4", "F5", "H3"],
        ["10", "20", "", "#VALUE!"],
    );
}

#[test]
fn the_returned_row_is_a_reference() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=SUM(XLOOKUP(\"b\",A1:A3,B1:D3))");
    model._set("F2", "=XLOOKUP(\"c\",A1:A3,B1:D3)*2");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "F2", "G2"], ["63", "", "60", "62"]);
    assert_eq!(model._get_text("H2"), "64");
}

#[test]
fn a_blocked_row_is_spill_error() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"b\",A1:A3,B1:D3)");
    model._set("H1", "taken");
    model.evaluate();
    assert_row(
        &model,
        ["F1", "G1", "H1", "I1"],
        ["#SPILL!", "", "taken", ""],
    );
}

// A plain formula cannot hold several values, so every shape the parser cannot see is
// stored as a dynamic formula and spills.
#[test]
fn a_return_array_of_unknown_shape_spills() {
    let mut model = table_model(["a", "b", "c"]);
    model
        .new_defined_name("Data", None, "Sheet1!$B$1:$D$3")
        .unwrap();
    model._set("F1", "=XLOOKUP(\"b\",A1:A3,Data)");
    model._set("F2", "=XLOOKUP(\"c\",A1:A3,OFFSET(B1,0,0,3,3))");
    model._set("F3", "=XLOOKUP(\"a\",A1:A3,INDIRECT(\"B1:D3\"))");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["20", "21", "22", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["30", "31", "32", ""]);
    assert_row(&model, ["F3", "G3", "H3", "I3"], ["10", "11", "12", ""]);
}

#[test]
fn a_lookup_array_of_unknown_shape_spills() {
    let mut model = table_model(["a", "b", "c"]);
    model
        .new_defined_name("Key", None, "Sheet1!$A$1:$A$1")
        .unwrap();
    model._set("F1", "=XLOOKUP(\"a\",A$1:A1,B1:D1)");
    model._set("F2", "=XLOOKUP(\"a\",Key,B1:D1)");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["10", "11", "12", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["10", "11", "12", ""]);
    assert!(model._get_cell("F1").dynamic_range().is_some());
    assert!(model._get_cell("F2").dynamic_range().is_some());
}

#[test]
fn a_multi_cell_if_not_found_spills() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"z\",A1:A3,B1:B3,{1,2})");
    model._set("F2", "=XLOOKUP(\"z\",A1:A3,B1:B3,C1:D1)");
    model._set("F3", "=XLOOKUP(\"a\",A1:A3,B1:B3,{1,2})");
    model._set("F4", "=XLOOKUP(\"z\",A1:A3,B1:B3,SWITCH(1,1,C1:D1))");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "F2"], ["1", "2", "", "11"]);
    assert_row(&model, ["G2", "H2", "F3", "G3"], ["12", "", "10", ""]);
    assert_row(&model, ["F4", "G4", "H4", "F5"], ["11", "12", "", ""]);
}

// SWITCH, IFS and IF return the range they are given, whatever their own analysis says.
#[test]
fn a_return_array_from_a_function_spills() {
    let mut model = table_model(["a", "b", "c"]);
    model._set("F1", "=XLOOKUP(\"a\",A1:A3,SWITCH(1,1,B1:D3))");
    model._set("F2", "=XLOOKUP(\"a\",A1:A3,IFS(TRUE,B1:D3))");
    model._set("F3", "=XLOOKUP(\"a\",A1:A1,IF(TRUE,$B1:D1,C1))");
    model.evaluate();
    assert_row(&model, ["F1", "G1", "H1", "I1"], ["10", "11", "12", ""]);
    assert_row(&model, ["F2", "G2", "H2", "I2"], ["10", "11", "12", ""]);
    assert_row(&model, ["F3", "G3", "H3", "I3"], ["10", "11", "12", ""]);
}
