#![allow(clippy::unwrap_used)]

use crate::constants::LAST_ROW;
use crate::expressions::types::Area;
use crate::test::user_model::util::new_empty_user_model;
use crate::types::Cell;
use crate::UserModel;

const FORMULAS: [&str; 12] = [
    "=A5+B5",
    "=$A5*B$2",
    "=SUM(A$1:B5)",
    "=SUM($A:$A)",
    "=Sheet2!A5+1",
    "=B5&\"x\"",
    "=IF(A5>0,A5,-A5)",
    "=SUM(A5:C7)",
    "=B5*2",
    "=A$1+$B5",
    "=ROW()+COLUMN()",
    "=A5",
];

fn model_with(source: &str) -> UserModel<'static> {
    let mut model = new_empty_user_model();
    model.new_sheet().unwrap();
    for row in 1..=12 {
        model.set_user_input(0, row, 1, &row.to_string()).unwrap();
        model
            .set_user_input(0, row, 2, &format!("{}%", row * 3))
            .unwrap();
        model.set_user_input(0, row, 3, "text").unwrap();
        model.set_user_input(1, row, 1, &(row * 10).to_string()).unwrap();
    }
    model.set_user_input(0, 5, 6, source).unwrap();
    model.set_user_input(0, 8, 6, "=1+1").unwrap();
    model.set_user_input(0, 9, 7, "kept").unwrap();
    model
}

fn content(model: &UserModel) -> Vec<(i32, i32, Cell)> {
    let mut cells = snapshot(model).0;
    cells.retain(|(_, _, cell)| !matches!(cell, Cell::EmptyCell { .. }));
    cells
}

fn snapshot(model: &UserModel) -> (Vec<(i32, i32, Cell)>, Vec<String>, usize) {
    let worksheet = model.model.workbook.worksheet(0).unwrap();
    let mut cells: Vec<_> = worksheet
        .sheet_data
        .iter()
        .map(|(row, column, cell)| (row, column, cell.clone()))
        .collect();
    cells.sort_by_key(|(row, column, _)| (*row, *column));
    (
        cells,
        worksheet.shared_formulas.clone(),
        model.model.parsed_formulas[0].len(),
    )
}

fn fill_as_text(model: &mut UserModel, source: (i32, i32), target: &Area) {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .from_writer(Vec::new());
    for row in target.row..target.row + target.height {
        let fields: Vec<String> = (target.column..target.column + target.width)
            .map(|column| {
                model
                    .model
                    .extend_to(target.sheet, source.0, source.1, row, column)
                    .unwrap()
            })
            .collect();
        writer.write_record(&fields).unwrap();
    }
    let tsv = String::from_utf8(writer.into_inner().unwrap()).unwrap();
    model.paste_csv_string(target, &tsv).unwrap();
}

fn area(row: i32, column: i32, width: i32, height: i32) -> Area {
    Area {
        sheet: 0,
        row,
        column,
        width,
        height,
    }
}

fn assert_same_fill(source_formula: &str, source: (i32, i32), target: &Area) {
    let mut by_text = model_with(source_formula);
    let mut by_sharing = model_with(source_formula);
    let before = content(&by_sharing);
    fill_as_text(&mut by_text, source, target);
    assert!(
        by_sharing.fill_formula(source, target).unwrap(),
        "{source_formula}"
    );
    let after = snapshot(&by_text);
    assert_eq!(snapshot(&by_sharing), after, "{source_formula} fill");

    by_text.undo().unwrap();
    by_sharing.undo().unwrap();
    assert_eq!(snapshot(&by_sharing), snapshot(&by_text), "{source_formula} undo");
    assert_eq!(content(&by_sharing), before, "{source_formula} undo state");

    by_text.redo().unwrap();
    by_sharing.redo().unwrap();
    assert_eq!(snapshot(&by_sharing), after, "{source_formula} redo");
}

#[test]
fn filling_down_matches_filling_as_text() {
    for formula in FORMULAS {
        assert_same_fill(formula, (5, 6), &area(6, 6, 1, 7));
    }
}

#[test]
fn filling_right_and_over_a_block_matches_filling_as_text() {
    for formula in FORMULAS {
        assert_same_fill(formula, (5, 6), &area(5, 7, 4, 1));
        assert_same_fill(formula, (5, 6), &area(6, 7, 3, 4));
    }
}

#[test]
fn filling_up_matches_filling_as_text() {
    for formula in FORMULAS {
        assert_same_fill(formula, (5, 6), &area(1, 6, 1, 4));
    }
}

#[test]
fn a_fill_keeps_the_formula_stored_once() {
    let mut model = model_with("=A5+B5");
    let stored = model.model.workbook.worksheet(0).unwrap().shared_formulas.len();
    assert!(model.fill_formula((5, 6), &area(6, 6, 1, 500)).unwrap());
    let worksheet = model.model.workbook.worksheet(0).unwrap();
    assert_eq!(worksheet.shared_formulas.len(), stored);
    assert_eq!(model.get_cell_content(0, 505, 6).unwrap(), "=A505+B505");
}

#[test]
fn a_fill_that_leaves_the_sheet_is_left_to_the_text_path() {
    let mut model = model_with("=A2");
    let before = snapshot(&model);
    assert!(!model.fill_formula((5, 6), &area(1, 6, 1, 3)).unwrap());
    assert_eq!(snapshot(&model), before);

    let mut model = model_with("=SUM(A$5:A6)");
    assert!(!model.fill_formula((5, 6), &area(3, 6, 1, 1)).unwrap());

    let last = LAST_ROW;
    let mut model = new_empty_user_model();
    model
        .set_user_input(0, last - 3, 2, &format!("=A{}", last - 2))
        .unwrap();
    assert!(model.fill_formula((last - 3, 2), &area(last - 2, 2, 1, 2)).unwrap());
    assert!(!model.fill_formula((last - 3, 2), &area(last - 2, 2, 1, 3)).unwrap());
}

#[test]
fn only_a_plain_formula_is_filled_by_sharing() {
    let mut model = model_with("text");
    assert!(!model.fill_formula((5, 6), &area(6, 6, 1, 2)).unwrap());
    model.set_user_input(0, 5, 6, "=SEQUENCE(2)").unwrap();
    assert!(!model.fill_formula((5, 6), &area(6, 7, 1, 2)).unwrap());
    model.set_user_input(0, 5, 6, "=A5").unwrap();
    assert!(!model.fill_formula((5, 6), &area(5, 6, 1, 2)).unwrap());
    model.set_user_array_formula(0, 10, 8, 2, 2, "=A1:B2").unwrap();
    assert!(!model.fill_formula((5, 6), &area(9, 8, 1, 3)).unwrap());
}

#[test]
fn a_group_of_edits_is_one_undo_step() {
    let mut model = model_with("=A5+B5");
    let before = content(&model);
    model.begin_edit_group().unwrap();
    assert!(model.fill_formula((5, 6), &area(6, 6, 1, 4)).unwrap());
    model.set_user_input(0, 1, 9, "13").unwrap();
    model.paste_csv_string(&area(3, 9, 1, 1), "1\t2\n3\t4").unwrap();
    model.end_edit_group().unwrap();
    let after = snapshot(&model);
    assert_eq!(model.get_formatted_cell_value(0, 1, 9).unwrap(), "13");

    model.undo().unwrap();
    assert_eq!(content(&model), before);
    model.redo().unwrap();
    assert_eq!(snapshot(&model), after);
}

#[test]
fn an_empty_group_leaves_no_undo_step() {
    let mut model = new_empty_user_model();
    model.begin_edit_group().unwrap();
    model.end_edit_group().unwrap();
    assert!(!model.can_undo());
    assert!(model.end_edit_group().is_err());
}
