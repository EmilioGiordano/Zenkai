// Tab colors and sheet visibility: read from the file by both readers, changed through the
// engine as undoable steps, and written back without losing what the app did not touch.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use rust_xlsxwriter::{Color, Workbook as Source};
use zenkai_engine::{Engine, SaveTarget, Workbook, XlsxReader, open_xlsx_with, save_xlsx};
use zenkai_types::{Rgb, SheetId, SheetVisibility};

const READERS: [XlsxReader; 2] = [XlsxReader::IronCalc, XlsxReader::Fast];
const BLUE: Rgb = Rgb(0x4472C4);
const ORANGE: Rgb = Rgb(0xFFC000);
const GREEN: Rgb = Rgb(0x00B050);

// Summary, Inputs (blue), Lookups (hidden, theme color), Archive (very hidden), Notes.
fn source(path: &Path) {
    let mut book = Source::new();
    book.add_worksheet()
        .set_name("Summary")
        .unwrap()
        .write_string(0, 0, "summary")
        .unwrap();
    let inputs = book.add_worksheet().set_name("Inputs").unwrap();
    inputs.write_number(0, 0, 1.0).unwrap();
    inputs.set_tab_color(Color::RGB(0x4472C4));
    inputs.group_symbols_above(true);
    let lookups = book.add_worksheet().set_name("Lookups").unwrap();
    lookups.write_number(0, 0, 2.0).unwrap();
    lookups.set_tab_color(Color::Theme(4, 3));
    lookups.set_hidden(true);
    let archive = book.add_worksheet().set_name("Archive").unwrap();
    archive.write_number(0, 0, 3.0).unwrap();
    archive.set_very_hidden(true);
    book.add_worksheet()
        .set_name("Notes")
        .unwrap()
        .write_number(0, 0, 4.0)
        .unwrap();
    book.save(path).unwrap();
}

fn with_every_sheet_hidden(path: &Path) {
    let mut source = Source::new();
    source.add_worksheet().set_name("One").unwrap();
    source.add_worksheet().set_name("Two").unwrap();
    let bytes = source.save_to_buffer().unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut body = String::new();
        entry.read_to_string(&mut body).unwrap();
        if name == "xl/workbook.xml" {
            body = body.replace("<sheet ", r#"<sheet state="hidden" "#);
        }
        out.start_file(name, options).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

fn open(path: &Path, reader: XlsxReader) -> Workbook {
    let opened = open_xlsx_with(path, reader).unwrap();
    assert_eq!(opened.fallback, None, "the reader under test was not used");
    opened.workbook
}

fn save(book: &Workbook, path: &Path) -> Vec<u8> {
    save_xlsx(book.save_snapshot(path, SaveTarget::Document).unwrap()).unwrap();
    std::fs::read(path).unwrap()
}

fn part(xlsx: &[u8], name: &str) -> String {
    let mut archive = zip::ZipArchive::new(Cursor::new(xlsx)).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

// The name and state of every sheet as workbook.xml lists them.
fn states(xlsx: &[u8]) -> Vec<(String, String)> {
    let text = part(xlsx, "xl/workbook.xml");
    let document = roxmltree::Document::parse(&text).unwrap();
    document
        .descendants()
        .filter(|node| node.has_tag_name("sheet"))
        .map(|node| {
            (
                node.attribute("name").unwrap().to_string(),
                node.attribute("state").unwrap_or("visible").to_string(),
            )
        })
        .collect()
}

// The attributes of the sheet's tabColor, empty without one, and the order of sheetPr's children.
fn sheet_properties(xlsx: &[u8], sheet_number: usize) -> (BTreeMap<String, String>, Vec<String>) {
    let text = part(xlsx, &format!("xl/worksheets/sheet{sheet_number}.xml"));
    let document = roxmltree::Document::parse(&text).unwrap();
    let root = document.root_element();
    let first = root.children().find(roxmltree::Node::is_element).unwrap();
    let Some(properties) = root.children().find(|node| node.has_tag_name("sheetPr")) else {
        return (BTreeMap::new(), Vec::new());
    };
    assert_eq!(
        first.tag_name().name(),
        "sheetPr",
        "sheetPr must come first in the worksheet"
    );
    let names = properties
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|node| node.tag_name().name().to_string())
        .collect();
    let attributes = properties
        .children()
        .find(|node| node.has_tag_name("tabColor"))
        .map(|node| {
            node.attributes()
                .map(|attribute| (attribute.name().to_string(), attribute.value().to_string()))
                .collect()
        })
        .unwrap_or_default();
    (attributes, names)
}

fn tab_color(xlsx: &[u8], sheet_number: usize) -> BTreeMap<String, String> {
    sheet_properties(xlsx, sheet_number).0
}

// The tint is compared as a number: it is written back from a parsed value.
fn same_tab_color(left: &BTreeMap<String, String>, right: &BTreeMap<String, String>) -> bool {
    let number = |map: &BTreeMap<String, String>| {
        map.get("tint")
            .map(|tint| tint.parse::<f64>().unwrap().to_bits())
    };
    let plain = |map: &BTreeMap<String, String>| {
        map.iter()
            .filter(|(name, _)| name.as_str() != "tint")
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    plain(left) == plain(right) && number(left) == number(right)
}

fn colors(book: &Workbook) -> Vec<Option<Rgb>> {
    book.sheets().into_iter().map(|sheet| sheet.color).collect()
}

fn visibilities(book: &Workbook) -> Vec<SheetVisibility> {
    book.sheets()
        .into_iter()
        .map(|sheet| sheet.visibility)
        .collect()
}

fn three_sheets() -> Workbook {
    let mut book = Workbook::new_empty().unwrap();
    book.add_sheet().unwrap();
    book.add_sheet().unwrap();
    book
}

#[test]
fn both_readers_read_the_tab_color_and_the_state_of_each_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabs.xlsx");
    source(&path);
    for reader in READERS {
        let book = open(&path, reader);
        assert_eq!(
            visibilities(&book),
            [
                SheetVisibility::Visible,
                SheetVisibility::Visible,
                SheetVisibility::Hidden,
                SheetVisibility::VeryHidden,
                SheetVisibility::Visible,
            ],
            "{reader:?}"
        );
        let colors = colors(&book);
        assert_eq!(colors[0], None, "{reader:?}");
        assert_eq!(colors[1], Some(BLUE), "{reader:?}");
        assert!(colors[2].is_some(), "a theme color resolves: {reader:?}");
        assert_eq!(colors[3], None, "{reader:?}");
        assert_eq!(colors[4], None, "{reader:?}");
    }
}

#[test]
fn open_and_save_without_edits_keeps_every_tab_color_and_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabs.xlsx");
    source(&path);
    let original = std::fs::read(&path).unwrap();
    for reader in READERS {
        let book = open(&path, reader);
        let saved = save(&book, &dir.path().join("saved.xlsx"));
        assert_eq!(states(&saved), states(&original), "{reader:?}");
        for number in 1..=5 {
            assert!(
                same_tab_color(&tab_color(&saved, number), &tab_color(&original, number)),
                "sheet {number}: {reader:?}"
            );
        }
        let theme = tab_color(&saved, 3);
        assert_eq!(theme.get("theme").map(String::as_str), Some("4"));
        assert!(theme.contains_key("tint"), "{theme:?}");
    }
}

#[test]
fn a_color_chosen_in_the_app_is_saved_and_read_back_by_both_readers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabs.xlsx");
    source(&path);
    for reader in READERS {
        let mut book = open(&path, reader);
        book.set_sheet_color(SheetId(0), Some(ORANGE)).unwrap();
        book.set_sheet_color(SheetId(1), None).unwrap();
        book.set_sheet_color(SheetId(2), Some(GREEN)).unwrap();
        let saved_path = dir.path().join("saved.xlsx");
        let saved = save(&book, &saved_path);

        let orange = tab_color(&saved, 1);
        assert_eq!(orange.get("rgb").map(String::as_str), Some("FFFFC000"));
        assert!(tab_color(&saved, 2).is_empty(), "no color was asked for");
        let green = tab_color(&saved, 3);
        assert_eq!(green.get("rgb").map(String::as_str), Some("FF00B050"));
        assert!(!green.contains_key("theme"), "{green:?}");
        assert_eq!(
            sheet_properties(&saved, 2).1,
            ["outlinePr"],
            "the outline summary of a sheet that lost its color stays"
        );

        for reread in READERS {
            let again = open(&saved_path, reread);
            assert_eq!(
                &colors(&again)[..3],
                &[Some(ORANGE), None, Some(GREEN)],
                "{reader:?} then {reread:?}"
            );
            assert_eq!(visibilities(&again), visibilities(&book));
        }
    }
}

#[test]
fn a_new_color_goes_before_the_other_sheet_properties() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabs.xlsx");
    source(&path);
    let mut book = open(&path, XlsxReader::Fast);
    book.set_sheet_color(SheetId(1), Some(GREEN)).unwrap();
    let saved = save(&book, &dir.path().join("saved.xlsx"));
    let (tab, children) = sheet_properties(&saved, 2);
    assert_eq!(tab.get("rgb").map(String::as_str), Some("FF00B050"));
    assert_eq!(children, ["tabColor", "outlinePr"]);
}

#[test]
fn a_color_on_a_sheet_made_in_the_app_is_written() {
    let mut book = Workbook::new_empty().unwrap();
    let added = book.add_sheet().unwrap();
    book.set_sheet_color(added, Some(GREEN)).unwrap();
    let xlsx = book.to_xlsx().unwrap();
    assert!(tab_color(&xlsx, 1).is_empty());
    assert_eq!(
        tab_color(&xlsx, 2).get("rgb").map(String::as_str),
        Some("FF00B050")
    );
}

#[test]
fn choosing_the_color_a_sheet_has_adds_no_undo_step() {
    let mut book = three_sheets();
    book.set_sheet_color(SheetId(1), Some(GREEN)).unwrap();
    book.set_sheet_color(SheetId(1), Some(GREEN)).unwrap();
    book.undo().unwrap();
    assert_eq!(colors(&book), [None, None, None]);
}

#[test]
fn undo_and_redo_of_a_tab_color_are_one_step_each_and_keep_the_sheets() {
    let mut book = three_sheets();
    book.set_sheet_color(SheetId(1), Some(BLUE)).unwrap();
    book.set_sheet_color(SheetId(1), Some(ORANGE)).unwrap();
    book.set_sheet_color(SheetId(1), None).unwrap();
    assert_eq!(colors(&book)[1], None);
    book.undo().unwrap();
    assert_eq!(colors(&book)[1], Some(ORANGE));
    book.undo().unwrap();
    assert_eq!(colors(&book)[1], Some(BLUE));
    book.redo().unwrap();
    assert_eq!(colors(&book)[1], Some(ORANGE));
    assert_eq!(colors(&book)[0], None);
    assert_eq!(colors(&book)[2], None);
}

#[test]
fn undo_and_redo_of_hiding_a_sheet() {
    let mut book = three_sheets();
    book.hide_sheet(SheetId(1)).unwrap();
    assert_eq!(
        visibilities(&book),
        [
            SheetVisibility::Visible,
            SheetVisibility::Hidden,
            SheetVisibility::Visible
        ]
    );
    book.undo().unwrap();
    assert_eq!(visibilities(&book), [SheetVisibility::Visible; 3]);
    book.redo().unwrap();
    assert_eq!(visibilities(&book)[1], SheetVisibility::Hidden);
}

#[test]
fn unhiding_several_sheets_is_one_undo_step() {
    let mut book = three_sheets();
    book.add_sheet().unwrap();
    book.hide_sheet(SheetId(1)).unwrap();
    book.hide_sheet(SheetId(2)).unwrap();
    book.unhide_sheets(&[SheetId(2), SheetId(1)]).unwrap();
    assert_eq!(visibilities(&book), [SheetVisibility::Visible; 4]);

    book.undo().unwrap();
    assert_eq!(
        visibilities(&book),
        [
            SheetVisibility::Visible,
            SheetVisibility::Hidden,
            SheetVisibility::Hidden,
            SheetVisibility::Visible
        ],
        "one undo hides both again"
    );
    book.undo().unwrap();
    assert_eq!(visibilities(&book)[2], SheetVisibility::Visible);
    assert_eq!(visibilities(&book)[1], SheetVisibility::Hidden);

    book.redo().unwrap();
    book.redo().unwrap();
    assert_eq!(visibilities(&book), [SheetVisibility::Visible; 4]);
}

#[test]
fn unhiding_nothing_leaves_the_history_alone() {
    let mut book = three_sheets();
    book.hide_sheet(SheetId(1)).unwrap();
    book.unhide_sheets(&[]).unwrap();
    book.undo().unwrap();
    assert_eq!(visibilities(&book), [SheetVisibility::Visible; 3]);
}

#[test]
fn the_last_visible_sheet_cannot_be_hidden() {
    let mut book = Workbook::new_empty().unwrap();
    assert!(book.hide_sheet(SheetId(0)).is_err());
    book.add_sheet().unwrap();
    book.hide_sheet(SheetId(0)).unwrap();
    assert!(book.hide_sheet(SheetId(1)).is_err());
    assert_eq!(
        visibilities(&book),
        [SheetVisibility::Hidden, SheetVisibility::Visible]
    );
}

#[test]
fn only_hidden_sheets_unhide_and_a_refusal_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabs.xlsx");
    source(&path);
    for reader in READERS {
        let mut book = open(&path, reader);
        let before = visibilities(&book);
        assert!(book.unhide_sheets(&[SheetId(2), SheetId(3)]).is_err());
        assert!(book.unhide_sheets(&[SheetId(0)]).is_err());
        assert!(book.hide_sheet(SheetId(3)).is_err());
        assert!(book.hide_sheet(SheetId(2)).is_err());
        assert_eq!(visibilities(&book), before, "{reader:?}");
        book.unhide_sheets(&[SheetId(2)]).unwrap();
        assert_eq!(visibilities(&book)[2], SheetVisibility::Visible);
        assert_eq!(visibilities(&book)[3], SheetVisibility::VeryHidden);
    }
}

#[test]
fn hiding_and_unhiding_are_saved_and_a_very_hidden_sheet_stays_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabs.xlsx");
    source(&path);
    let original = std::fs::read(&path).unwrap();
    for reader in READERS {
        let mut book = open(&path, reader);
        book.hide_sheet(SheetId(0)).unwrap();
        book.unhide_sheets(&[SheetId(2)]).unwrap();
        let saved_path = dir.path().join("saved.xlsx");
        let saved = save(&book, &saved_path);
        let written = states(&saved);
        assert_eq!(
            written
                .iter()
                .map(|(name, state)| (name.as_str(), state.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Summary", "hidden"),
                ("Inputs", "visible"),
                ("Lookups", "visible"),
                ("Archive", "veryHidden"),
                ("Notes", "visible"),
            ],
            "{reader:?}"
        );
        for number in 1..=5 {
            assert!(
                same_tab_color(&tab_color(&saved, number), &tab_color(&original, number)),
                "visibility edits keep the tab colors: sheet {number}, {reader:?}"
            );
        }
        for reread in READERS {
            assert_eq!(
                visibilities(&open(&saved_path, reread)),
                visibilities(&book),
                "{reader:?} then {reread:?}"
            );
        }
    }
}

#[test]
fn a_file_with_every_sheet_hidden_is_not_edited_by_open_or_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hidden.xlsx");
    with_every_sheet_hidden(&path);
    for reader in READERS {
        let book = open(&path, reader);
        assert_eq!(
            visibilities(&book),
            [SheetVisibility::Hidden; 2],
            "{reader:?}"
        );
        let saved = save(&book, &dir.path().join("saved.xlsx"));
        assert_eq!(
            states(&saved),
            [
                ("One".to_string(), "hidden".to_string()),
                ("Two".to_string(), "hidden".to_string())
            ],
            "{reader:?}"
        );
    }
}

#[test]
fn a_hidden_sheet_cannot_be_hidden_when_none_is_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hidden.xlsx");
    with_every_sheet_hidden(&path);
    let mut book = open(&path, XlsxReader::Fast);
    assert!(book.hide_sheet(SheetId(0)).is_err());
    book.unhide_sheets(&[SheetId(1)]).unwrap();
    assert_eq!(
        visibilities(&book),
        [SheetVisibility::Hidden, SheetVisibility::Visible]
    );
}

#[test]
fn a_sheet_keeps_its_color_when_moved_and_when_a_delete_is_undone() {
    let mut book = three_sheets();
    book.set_sheet_color(SheetId(2), Some(GREEN)).unwrap();
    book.move_sheet(SheetId(2), 0).unwrap();
    assert_eq!(colors(&book), [Some(GREEN), None, None]);
    book.delete_sheet(SheetId(0)).unwrap();
    book.undo().unwrap();
    assert_eq!(colors(&book), [Some(GREEN), None, None]);
}
