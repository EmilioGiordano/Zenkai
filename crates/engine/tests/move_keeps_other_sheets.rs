// Spec: a save warns before losing content. A row or column inserted or deleted on one sheet
// keeps what another sheet's relationships point at when it names only that sheet's cells
// (notes, pictures, links out of the workbook), and drops and reports what may name the
// moved cells: charts, tables, pivot tables, and shapes or links that name cells.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use rust_xlsxwriter::{Chart, ChartType, Image, Note, Shape, Workbook as Source, Worksheet};
use zenkai_engine::{
    Engine, SaveTarget, Unsupported, Workbook, open_xlsx, save_xlsx, scan_unsupported,
};
use zenkai_types::{ColIdx, RowIdx, SheetId};

mod support;
use support::PIXEL_PNG;

const HELD: &str = "xl/worksheets/sheet1.xml";
const HELD_RELS: &str = "xl/worksheets/_rels/sheet1.xml.rels";
const CHARTED: &str = "xl/worksheets/sheet2.xml";
const CHARTED_RELS: &str = "xl/worksheets/_rels/sheet2.xml.rels";
const DATA: SheetId = SheetId(2);

struct Saved {
    dropped: Vec<Unsupported>,
    kept: Vec<Unsupported>,
    bytes: Vec<u8>,
    path: PathBuf,
}

fn pixel() -> Image {
    Image::new_from_buffer(&PIXEL_PNG).unwrap()
}

// Sheet1 holds a note, a picture and a link out of the workbook; Sheet2 a chart of Sheet3.
fn source(dir: &Path) -> PathBuf {
    let path = dir.join("source.xlsx");
    let mut book = Source::new();
    let held = book.add_worksheet();
    held.write_string(0, 0, "held").unwrap();
    held.insert_note(0, 0, &Note::new("checked")).unwrap();
    held.insert_image(2, 1, &pixel()).unwrap();
    held.write_url(6, 0, "https://example.com").unwrap();
    let charted = book.add_worksheet();
    charted.write_string(0, 0, "charted").unwrap();
    let mut chart = Chart::new(ChartType::Column);
    chart.add_series().set_values("Sheet3!$A$1:$A$3");
    charted.insert_chart(2, 0, &chart).unwrap();
    let data = book.add_worksheet();
    for row in 0..3 {
        data.write_number(row, 0, f64::from(row + 1)).unwrap();
    }
    book.save(&path).unwrap();
    path
}

fn part(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut content = Vec::new();
    entry.read_to_end(&mut content).unwrap();
    Some(content)
}

fn save(book: &Workbook, path: &Path) -> Saved {
    let snapshot = book.save_snapshot(path, SaveTarget::Document).unwrap();
    let dropped = snapshot.dropped().to_vec();
    let kept = snapshot.kept().to_vec();
    save_xlsx(snapshot).unwrap();
    Saved {
        dropped,
        kept,
        bytes: std::fs::read(path).unwrap(),
        path: path.to_path_buf(),
    }
}

fn set(kinds: &[Unsupported]) -> BTreeSet<Unsupported> {
    kinds.iter().copied().collect()
}

#[derive(Clone, Copy, Debug)]
enum Move {
    InsertRows,
    DeleteRows,
    InsertColumns,
    DeleteColumns,
}

const MOVES: [Move; 4] = [
    Move::InsertRows,
    Move::DeleteRows,
    Move::InsertColumns,
    Move::DeleteColumns,
];

fn apply(book: &mut Workbook, sheet: SheetId, step: Move) {
    let (row, column) = (RowIdx::clamped(0), ColIdx::clamped(0));
    match step {
        Move::InsertRows => book.insert_rows(sheet, row, 1),
        Move::DeleteRows => book.delete_rows(sheet, row, 1),
        Move::InsertColumns => book.insert_columns(sheet, column, 1),
        Move::DeleteColumns => book.delete_columns(sheet, column, 1),
    }
    .unwrap();
}

// The chart names the moved cells, so it goes with the drawing showing it, which is what
// Images reports; the note, picture and link on Sheet1 stay byte for byte.
#[test]
fn a_move_on_one_sheet_keeps_another_sheets_notes_picture_and_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    let original = std::fs::read(&path).unwrap();
    let found = scan_unsupported(&original).unwrap();
    assert_eq!(
        set(&found),
        set(&[
            Unsupported::Charts,
            Unsupported::Images,
            Unsupported::Comments,
            Unsupported::Hyperlinks,
        ])
    );
    for step in MOVES {
        let mut book = open_xlsx(&path).unwrap().workbook;
        apply(&mut book, DATA, step);
        let saved = save(&book, &dir.path().join(format!("{step:?}.xlsx")));
        assert_eq!(
            set(&saved.dropped),
            set(&[Unsupported::Charts, Unsupported::Images]),
            "{step:?}"
        );
        for name in [HELD, HELD_RELS] {
            assert_eq!(
                part(&saved.bytes, name),
                part(&original, name),
                "{step:?} {name}"
            );
        }
        assert_eq!(part(&saved.bytes, CHARTED_RELS), None, "{step:?}");
        assert_eq!(
            saved.kept,
            scan_unsupported(&saved.bytes).unwrap(),
            "{step:?}"
        );
        assert_eq!(
            set(&saved.kept),
            set(&[
                Unsupported::Images,
                Unsupported::Comments,
                Unsupported::Hyperlinks,
            ]),
            "{step:?}"
        );
        open_xlsx(&saved.path).unwrap();
    }
}

#[test]
fn undoing_the_move_restores_the_chart_and_redoing_drops_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    let original = std::fs::read(&path).unwrap();
    let mut book = open_xlsx(&path).unwrap().workbook;
    apply(&mut book, DATA, Move::InsertRows);
    book.undo().unwrap();
    let undone = save(&book, &dir.path().join("undone.xlsx"));
    assert!(undone.dropped.is_empty(), "{:?}", undone.dropped);
    for name in [HELD, HELD_RELS, CHARTED, CHARTED_RELS] {
        assert_eq!(part(&undone.bytes, name), part(&original, name), "{name}");
    }
    assert_eq!(undone.kept, scan_unsupported(&undone.bytes).unwrap());

    let mut redone = open_xlsx(&path).unwrap().workbook;
    apply(&mut redone, DATA, Move::InsertRows);
    redone.undo().unwrap();
    redone.redo().unwrap();
    let saved = save(&redone, &dir.path().join("redone.xlsx"));
    assert_eq!(
        set(&saved.dropped),
        set(&[Unsupported::Charts, Unsupported::Images])
    );
    for name in [HELD, HELD_RELS] {
        assert_eq!(part(&saved.bytes, name), part(&original, name), "{name}");
    }
    assert_eq!(part(&saved.bytes, CHARTED_RELS), None);
}

// A move on the sheet holding the content still drops it: its anchors and cells moved.
#[test]
fn a_move_on_the_sheet_itself_drops_its_notes_picture_and_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    apply(&mut book, SheetId(0), Move::InsertColumns);
    let saved = save(&book, &dir.path().join("moved.xlsx"));
    assert!(
        set(&saved.dropped).is_superset(&set(&[
            Unsupported::Images,
            Unsupported::Comments,
            Unsupported::Hyperlinks,
        ])),
        "{:?}",
        saved.dropped
    );
    assert_eq!(part(&saved.bytes, HELD_RELS), None);
    open_xlsx(&saved.path).unwrap();
}

// Sheet1 holds a note and `linked`, which names a cell on Sheet2.
fn naming_source(dir: &Path, linked: impl FnOnce(&mut Worksheet)) -> PathBuf {
    let path = dir.join("naming.xlsx");
    let mut book = Source::new();
    let held = book.add_worksheet();
    held.write_string(0, 0, "held").unwrap();
    held.insert_note(0, 0, &Note::new("checked")).unwrap();
    linked(held);
    let data = book.add_worksheet();
    data.write_number(0, 0, 1.0).unwrap();
    book.save(&path).unwrap();
    path
}

// Excel shifts the cell a shape's text or a picture's link names when rows move on its
// sheet, which Zenkai cannot, so the drawing and everything beside it go, and the save says so.
#[test]
fn a_shape_or_picture_naming_a_moved_cell_is_dropped_and_reported() {
    let text_link = |held: &mut Worksheet| {
        let shape = Shape::textbox()
            .set_text("total")
            .set_text_link("Sheet2!A1");
        held.insert_shape(2, 1, &shape).unwrap();
    };
    let picture_link = |held: &mut Worksheet| {
        let image = pixel().set_url("internal:Sheet2!A1").unwrap();
        held.insert_image(2, 1, &image).unwrap();
    };
    let cases: [(&str, &dyn Fn(&mut Worksheet), &[Unsupported]); 2] = [
        (
            "text link",
            &text_link,
            &[Unsupported::Images, Unsupported::Comments],
        ),
        (
            "picture link",
            &picture_link,
            &[
                Unsupported::Images,
                Unsupported::Comments,
                Unsupported::Hyperlinks,
            ],
        ),
    ];
    for (case, linked, lost) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = naming_source(dir.path(), linked);
        let mut book = open_xlsx(&path).unwrap().workbook;
        apply(&mut book, SheetId(1), Move::InsertRows);
        let saved = save(&book, &dir.path().join("moved.xlsx"));
        assert_eq!(set(&saved.dropped), set(lost), "{case}");
        assert_eq!(part(&saved.bytes, HELD_RELS), None, "{case}");
        let held = String::from_utf8(part(&saved.bytes, HELD).unwrap()).unwrap();
        assert!(!held.contains("r:id="), "{case}: {held}");
        assert_eq!(
            saved.kept,
            scan_unsupported(&saved.bytes).unwrap(),
            "{case}"
        );
        open_xlsx(&saved.path).unwrap();

        let mut undone = open_xlsx(&path).unwrap().workbook;
        apply(&mut undone, SheetId(1), Move::InsertRows);
        undone.undo().unwrap();
        let saved = save(&undone, &dir.path().join("undone.xlsx"));
        assert!(saved.dropped.is_empty(), "{case}: {:?}", saved.dropped);
        let original = std::fs::read(&path).unwrap();
        assert_eq!(
            part(&saved.bytes, HELD_RELS),
            part(&original, HELD_RELS),
            "{case}"
        );
    }
}
