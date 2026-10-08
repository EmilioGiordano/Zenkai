// Spec: editing recalculates only what an edit affects, with the same results as
// evaluating the whole workbook. Drives the engine through the calls `Workbook` makes and
// compares every cell against a fresh model evaluated from scratch after each step.
#![allow(clippy::unwrap_used)]

use ironcalc::base::cell::CellValue;
use ironcalc::base::expressions::types::Area;
use ironcalc::base::{ClipboardData, Model, Recalculation, UserModel};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

const SHEETS: u32 = 2;
const ROWS: i32 = 6;
const COLUMNS: i32 = 4;
// Inserted rows push contents down, so the comparison looks past the edited grid.
const COMPARED_ROWS: i32 = ROWS + 4;

#[derive(Clone, Debug)]
enum Op {
    Set(u32, i32, i32, String),
    Paste(u32, i32, i32, Vec<Vec<String>>),
    Clear(u32, i32, i32, i32, i32),
    CopyPaste {
        from: (u32, i32, i32, i32, i32),
        to: (u32, i32, i32),
        cut: bool,
    },
    InsertRow(u32, i32),
    DeleteRow(u32, i32),
    HideRow(u32, i32, bool),
    DefineName(String),
    Undo,
    Redo,
}

fn column_name(column: i32) -> char {
    char::from(b'A' + u8::try_from(column - 1).unwrap())
}

fn cell() -> impl Strategy<Value = String> {
    (0..SHEETS, 1..=ROWS, 1..=COLUMNS, 0..4u8).prop_map(|(sheet, row, column, anchoring)| {
        let prefix = if sheet == 0 { "" } else { "Sheet2!" };
        let dollar_column = if anchoring & 1 == 1 { "$" } else { "" };
        let dollar_row = if anchoring & 2 == 2 { "$" } else { "" };
        format!(
            "{prefix}{dollar_column}{}{dollar_row}{row}",
            column_name(column)
        )
    })
}

fn range() -> impl Strategy<Value = String> {
    (0..SHEETS, 1..=ROWS, 1..=COLUMNS, 1..=ROWS, 1..=COLUMNS).prop_map(
        |(sheet, row1, column1, row2, column2)| {
            let prefix = if sheet == 0 { "" } else { "Sheet2!" };
            format!(
                "{prefix}{}{}:{}{}",
                column_name(column1.min(column2)),
                row1.min(row2),
                column_name(column1.max(column2)),
                row1.max(row2)
            )
        },
    )
}

// Volatile functions take part, but in a way that keeps the result deterministic.
fn formula() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => (cell(), cell()).prop_map(|(a, b)| format!("={a}+{b}")),
        3 => (cell(), cell()).prop_map(|(a, b)| format!("={a}*2-{b}")),
        3 => range().prop_map(|r| format!("=SUM({r})")),
        3 => (range(), range()).prop_map(|(a, b)| format!("=SUMIF({a},\">2\",{b})")),
        3 => range().prop_map(|r| format!("=COUNTIF({r},\"<5\")")),
        3 => (cell(), range()).prop_map(|(a, r)| format!("=VLOOKUP({a},{r},2,FALSE)")),
        3 => (cell(), cell(), cell()).prop_map(|(a, b, c)| format!("=IF({a}>3,{b},{c}*2)")),
        3 => cell().prop_map(|a| format!("=IF(RAND()<2,{a},0)")),
        3 => cell().prop_map(|a| format!("=TODAY()-TODAY()+NOW()-NOW()+{a}")),
        3 => cell().prop_map(|a| format!("=INDIRECT(\"B2\")+{a}")),
        3 => cell().prop_map(|a| format!("=OFFSET({a},1,0)")),
        1 => Just("=SEQUENCE(2)".to_string()),
        1 => cell().prop_map(|a| format!("=SEQUENCE(MAX(1,MIN(3,{a})))")),
        2 => range().prop_map(|r| format!("=SUBTOTAL(109,{r})")),
        2 => Just("=SUM(total)*2".to_string()),
        1 => Just("=SUM(A1:T3)".to_string()),
        1 => Just("=SUM(Sheet2!B:B)".to_string()),
    ]
}

fn input() -> impl Strategy<Value = String> {
    prop_oneof![
        5 => (-9i32..10).prop_map(|n| n.to_string()),
        1 => "[a-c]{1,2}",
        1 => Just(String::new()),
        4 => formula(),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let row = 1..=ROWS;
    let column = 1..=COLUMNS;
    prop_oneof![
        8 => (0..SHEETS, row.clone(), column.clone(), input())
            .prop_map(|(s, r, c, v)| Op::Set(s, r, c, v)),
        2 => (
            0..SHEETS,
            row.clone(),
            column.clone(),
            (1..3usize, 1..3usize).prop_flat_map(|(height, width)| {
                prop::collection::vec(prop::collection::vec(input(), width), height)
            })
        )
            .prop_map(|(s, r, c, rows)| Op::Paste(s, r, c, rows)),
        2 => (0..SHEETS, row.clone(), column.clone(), 0..3i32, 0..3i32)
            .prop_map(|(s, r, c, h, w)| Op::Clear(s, r, c, h + 1, w + 1)),
        1 => (
            (0..SHEETS, row.clone(), column.clone(), 1..3i32, 1..3i32),
            (0..SHEETS, row.clone(), column.clone()),
            any::<bool>()
        )
            .prop_map(|(from, to, cut)| Op::CopyPaste { from, to, cut }),
        1 => (0..SHEETS, row.clone()).prop_map(|(s, r)| Op::InsertRow(s, r)),
        1 => (0..SHEETS, row.clone()).prop_map(|(s, r)| Op::DeleteRow(s, r)),
        1 => (0..SHEETS, row, any::<bool>()).prop_map(|(s, r, hidden)| Op::HideRow(s, r, hidden)),
        1 => range().prop_map(|r| Op::DefineName(r)),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

fn tsv(rows: &[Vec<String>]) -> String {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .from_writer(Vec::new());
    for row in rows {
        writer.write_record(row).unwrap();
    }
    String::from_utf8(writer.into_inner().unwrap()).unwrap()
}

fn select(model: &mut UserModel, sheet: u32, row: i32, column: i32, height: i32, width: i32) {
    model.set_selected_sheet(sheet).unwrap();
    model.set_selected_cell(row, column).unwrap();
    model
        .set_selected_range(row, column, row + height - 1, column + width - 1)
        .unwrap();
}

fn copy_paste(
    model: &mut UserModel,
    from: (u32, i32, i32, i32, i32),
    to: (u32, i32, i32),
    cut: bool,
) -> Result<(), String> {
    let (sheet, row, column, height, width) = from;
    select(model, sheet, row, column, height, width);
    let payload = serde_json::to_value(model.copy_to_clipboard().unwrap()).unwrap();
    let data: ClipboardData = serde_json::from_value(payload["data"].clone()).unwrap();
    let source_range: (i32, i32, i32, i32) =
        serde_json::from_value(payload["range"].clone()).unwrap();
    select(model, to.0, to.1, to.2, 1, 1);
    model.paste_from_clipboard(sheet, source_range, &data, cut)
}

fn define_name(model: &mut UserModel, range: &str) -> Result<(), String> {
    let formula = if range.contains('!') {
        range.to_string()
    } else {
        format!("Sheet1!{range}")
    };
    if model
        .get_defined_name_list()
        .iter()
        .any(|(name, _, _)| name == "total")
    {
        model.update_defined_name("total", None, "total", None, &formula)
    } else {
        model.new_defined_name("total", None, &formula)
    }
}

// Whether the engine accepted the operation; a refused one (a paste over part of an
// array) leaves the model as it was.
fn apply(model: &mut UserModel, op: &Op) -> bool {
    let result = match op {
        Op::Set(sheet, row, column, value) => model.set_user_input(*sheet, *row, *column, value),
        Op::Paste(sheet, row, column, rows) => {
            let height = i32::try_from(rows.len()).unwrap();
            let width = i32::try_from(rows[0].len()).unwrap();
            select(model, *sheet, *row, *column, height, width);
            let area = Area {
                sheet: *sheet,
                row: *row,
                column: *column,
                width,
                height,
            };
            model.paste_csv_string(&area, &tsv(rows))
        }
        Op::Clear(sheet, row, column, height, width) => model.range_clear_contents(&Area {
            sheet: *sheet,
            row: *row,
            column: *column,
            width: *width,
            height: *height,
        }),
        Op::CopyPaste { from, to, cut } => copy_paste(model, *from, *to, *cut),
        Op::InsertRow(sheet, row) => model.insert_rows(*sheet, *row, 1),
        Op::DeleteRow(sheet, row) => model.delete_rows(*sheet, *row, 1),
        Op::HideRow(sheet, row, hidden) => model.set_rows_hidden(*sheet, *row, *row, *hidden),
        Op::DefineName(range) => define_name(model, range),
        Op::Undo => model.undo(),
        Op::Redo => model.redo(),
    };
    result.is_ok()
}

fn values(model: &UserModel) -> Vec<(u32, i32, i32, CellValue)> {
    let mut values = Vec::new();
    for sheet in 0..SHEETS {
        for row in 1..=COMPARED_ROWS {
            for column in 1..=COLUMNS + 1 {
                let value = model
                    .get_model()
                    .get_cell_value_by_index(sheet, row, column)
                    .unwrap();
                values.push((sheet, row, column, value));
            }
        }
    }
    values
}

fn evaluated_from_scratch(model: &UserModel) -> UserModel<'static> {
    let mut fresh = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
    fresh.evaluate();
    assert_eq!(fresh.get_model().last_recalculation(), Recalculation::Full);
    fresh
}

// Built on the model, so that undo cannot remove the second sheet.
fn two_sheets() -> UserModel<'static> {
    let mut model = Model::new_empty("book", "en", "UTC", "en").unwrap();
    model.new_sheet();
    UserModel::from_model(model)
}

#[test]
fn incremental_recalculation_matches_a_full_evaluation() {
    let config = Config {
        cases: 1000,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let steps = std::cell::Cell::new(0u32);
    let accepted = std::cell::Cell::new(0u32);
    let incremental = std::cell::Cell::new(0u32);
    let result = runner.run(&prop::collection::vec(op(), 1..25), |ops| {
        let mut model = two_sheets();
        for op in &ops {
            if apply(&mut model, op) {
                accepted.set(accepted.get() + 1);
            }
            steps.set(steps.get() + 1);
            if model.get_model().last_recalculation() == Recalculation::Incremental {
                incremental.set(incremental.get() + 1);
            }
            let expected = values(&evaluated_from_scratch(&model));
            prop_assert_eq!(values(&model), expected, "after {:?}", op);
        }
        Ok(())
    });
    if let Err(failure) = result {
        panic!("{failure}");
    }
    assert!(
        accepted.get() * 10 > steps.get() * 9,
        "only {} of {} operations were accepted",
        accepted.get(),
        steps.get()
    );
    // The fast path must be what is under test, not the fallback.
    assert!(
        incremental.get() * 2 > steps.get(),
        "only {} of {} steps were incremental",
        incremental.get(),
        steps.get()
    );
}
