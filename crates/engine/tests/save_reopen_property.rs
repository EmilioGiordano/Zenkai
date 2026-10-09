// Spec: open, edit, save, reopen never changes what the user sees. Every cell's input and
// value, its style, the sizes, hidden lines, frozen panes and merges survive a save.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use proptest::prelude::*;
use zenkai_engine::{Engine, Workbook, open_xlsx, save_xlsx_atomic};
use zenkai_types::{
    CellPos, ColIdx, HAlign, NumberFormat, Range, Rgb, RowIdx, SheetId, StyleChange,
};

const SIDE: i64 = 6;
const SHEET: SheetId = SheetId(0);

#[derive(Clone, Debug)]
enum Op {
    Set(i64, i64, String),
    Style(i64, i64, i64, i64, StyleChange),
    RowHeight(i64, u16),
    ColumnWidth(i64, u16),
    HideRows(i64, i64, bool),
    HideColumns(i64, i64, bool),
    Freeze(u32, u16),
    Sort(i64, bool),
}

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn range(a: i64, b: i64, c: i64, d: i64) -> Range {
    Range::new(pos(a, b), pos(c, d))
}

fn input() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (-1000i32..1000).prop_map(|n| n.to_string()),
        (-1000i32..1000, 1u32..99).prop_map(|(n, f)| format!("{n}.{f}")),
        "[a-zA-Z ]{1,8}",
        "[a-z]{1,3}[0-9]{1,3}",
        (0..SIDE, 0..SIDE).prop_map(|(r, c)| format!("={}{}+1", char::from(b'A' + c as u8), r + 1)),
        Just("=SUM(A1:C3)".to_string()),
        Just("=1/0".to_string()),
        Just("=\"x\"&A1".to_string()),
        Just("TRUE".to_string()),
    ]
}

fn style() -> impl Strategy<Value = StyleChange> {
    prop_oneof![
        any::<bool>().prop_map(StyleChange::Bold),
        any::<bool>().prop_map(StyleChange::Italic),
        any::<bool>().prop_map(StyleChange::Underline),
        any::<bool>().prop_map(StyleChange::Strike),
        any::<bool>().prop_map(StyleChange::Wrap),
        (8u16..40).prop_map(StyleChange::FontSize),
        prop::sample::select(vec![HAlign::Left, HAlign::Center, HAlign::Right])
            .prop_map(StyleChange::Align),
        prop::sample::select(vec![
            NumberFormat::General,
            NumberFormat::Number,
            NumberFormat::Currency,
            NumberFormat::Percent,
            NumberFormat::Date,
            NumberFormat::Time,
        ])
        .prop_map(StyleChange::NumberFormat),
        prop::option::of(0u32..0x0100_0000).prop_map(|c| StyleChange::Fill(c.map(Rgb))),
        prop::option::of(0u32..0x0100_0000).prop_map(|c| StyleChange::FontColor(c.map(Rgb))),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let at = || 0..SIDE;
    prop_oneof![
        4 => (at(), at(), input()).prop_map(|(r, c, v)| Op::Set(r, c, v)),
        3 => (at(), at(), at(), at(), style()).prop_map(|(a, b, c, d, s)| Op::Style(a, b, c, d, s)),
        1 => (at(), 10u16..120).prop_map(|(r, h)| Op::RowHeight(r, h)),
        1 => (at(), 20u16..300).prop_map(|(c, w)| Op::ColumnWidth(c, w)),
        1 => (at(), at(), any::<bool>()).prop_map(|(a, b, h)| Op::HideRows(a, b, h)),
        1 => (at(), at(), any::<bool>()).prop_map(|(a, b, h)| Op::HideColumns(a, b, h)),
        1 => (0u32..4, 0u16..4).prop_map(|(r, c)| Op::Freeze(r, c)),
        1 => (0..SIDE, any::<bool>()).prop_map(|(key, desc)| Op::Sort(key, desc)),
    ]
}

fn apply(book: &mut Workbook, op: &Op) {
    // An edit the engine refuses is not part of the state being checked.
    let _ = match op {
        Op::Set(r, c, v) => book.set_input(SHEET, pos(*r, *c), v),
        Op::Style(a, b, c, d, change) => book.apply_style(SHEET, range(*a, *b, *c, *d), *change),
        Op::RowHeight(r, h) => book.set_row_height(SHEET, RowIdx::clamped(*r), f32::from(*h)),
        Op::ColumnWidth(c, w) => book.set_column_width(SHEET, ColIdx::clamped(*c), f32::from(*w)),
        Op::HideRows(a, b, hidden) => book.set_rows_hidden(SHEET, range(*a, 0, *b, 0), *hidden),
        Op::HideColumns(a, b, hidden) => {
            book.set_columns_hidden(SHEET, range(0, *a, 0, *b), *hidden)
        }
        Op::Freeze(rows, cols) => book.set_frozen(SHEET, *rows, *cols),
        Op::Sort(key, desc) => book.sort(
            SHEET,
            range(0, 0, SIDE - 1, SIDE - 1),
            ColIdx::clamped(*key),
            *desc,
        ),
    };
}

fn snapshot(book: &Workbook) -> Vec<String> {
    let mut lines = Vec::new();
    for sheet in book.sheets() {
        lines.push(format!("sheet {:?}", sheet));
        for row in 0..SIDE + 2 {
            for col in 0..SIDE + 2 {
                let at = pos(row, col);
                lines.push(format!(
                    "{row},{col}|{}|{:?}",
                    book.input(sheet.id, at),
                    book.cell(sheet.id, at)
                ));
            }
        }
        let mut sizes = book.sizes(sheet.id);
        sizes.rows.sort_by_key(|(row, _)| row.get());
        lines.push(format!("sizes {sizes:?}"));
        lines.push(format!("frozen {:?}", book.frozen(sheet.id)));
        lines.push(format!("merged {:?}", book.merged(sheet.id)));
    }
    lines
}

fn difference(expected: &[String], actual: &[String]) -> String {
    expected
        .iter()
        .zip(actual)
        .filter(|(a, b)| a != b)
        .map(|(a, b)| {
            format!(
                "expected {a}
  actual {b}"
            )
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn save_and_reopen(book: &Workbook, dir: &Path, name: &str) -> Workbook {
    let path = dir.join(name);
    save_xlsx_atomic(book, &path).unwrap();
    open_xlsx(&path).unwrap().workbook
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn save_then_reopen_keeps_every_cell_style_size_and_pane(ops in prop::collection::vec(op(), 1..20)) {
        let dir = tempfile::tempdir().unwrap();
        let mut book = Workbook::new_empty().unwrap();
        for op in &ops {
            apply(&mut book, op);
        }
        let before = snapshot(&book);
        let reopened = save_and_reopen(&book, dir.path(), "first.xlsx");
        let after = snapshot(&reopened);
        prop_assert!(after == before, "{}", difference(&before, &after));
        let again = save_and_reopen(&reopened, dir.path(), "second.xlsx");
        let after = snapshot(&again);
        prop_assert!(after == before, "second save: {}", difference(&before, &after));
    }

    #[test]
    fn save_then_reopen_keeps_cells_of_every_sheet(ops in prop::collection::vec(op(), 1..10), names in prop::collection::vec("[A-Za-z][A-Za-z0-9 ]{0,10}", 1..3)) {
        let dir = tempfile::tempdir().unwrap();
        let mut book = Workbook::new_empty().unwrap();
        for op in &ops {
            apply(&mut book, op);
        }
        for name in &names {
            let id = book.add_sheet().unwrap();
            let _ = book.rename_sheet(id, name);
            book.set_input(id, pos(0, 0), "=Sheet1!A1").unwrap();
            book.set_input(id, pos(1, 0), "7").unwrap();
        }
        let before = snapshot(&book);
        let reopened = save_and_reopen(&book, dir.path(), "multi.xlsx");
        let after = snapshot(&reopened);
        prop_assert!(after == before, "{}", difference(&before, &after));
    }
}

#[test]
fn merges_sizes_and_panes_from_a_file_survive_a_save() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/compat/merges-freeze-sizes.xlsx");
    let dir = tempfile::tempdir().unwrap();
    let original = open_xlsx(&fixture).unwrap().workbook;
    assert!(
        !original.merged(SHEET).is_empty(),
        "the fixture must hold merges"
    );
    let reopened = save_and_reopen(&original, dir.path(), "copy.xlsx");
    assert_eq!(snapshot(&reopened), snapshot(&original));
}
