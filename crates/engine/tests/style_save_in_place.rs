// Spec: a style edit on an opened file saves in place, as Excel does. The styles part of
// the file only grows: every style a sheet already points at keeps its index and its
// bytes, the sheets no edit touched are copied, and nothing the file holds is dropped.
#![allow(clippy::unwrap_used)]

use std::io::Read;
use std::path::{Path, PathBuf};

use rust_xlsxwriter::{Color, Format, FormatPattern, Table, TableColumn, Workbook as Source};
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, NumberFormat, Range, SheetId, StyleChange};

const TABLE_SHEET_PART: &str = "xl/worksheets/sheet1.xml";
const OTHER_SHEET_PART: &str = "xl/worksheets/sheet2.xml";
const STYLES_PART: &str = "xl/styles.xml";
const CUSTOM_FORMAT: &str = "0.000\" kg\"";

fn pos(a1: &str) -> CellPos {
    CellPos::parse_a1(a1).unwrap()
}

fn range(a1: &str) -> Range {
    Range::new(pos(a1), pos(a1))
}

// Written by another application: the styles hold what IronCalc does not model (a pattern
// fill, an unlocked cell), so a styles part written from the model would lose them.
fn other_styles(book: &mut Source) {
    let patterned = Format::new()
        .set_pattern(FormatPattern::DarkGrid)
        .set_foreground_color(Color::RGB(0xC00000))
        .set_background_color(Color::RGB(0xFFFF00));
    let unlocked = Format::new().set_unlocked().set_italic();
    let other = book.add_worksheet().set_name("Other").unwrap();
    other
        .write_number_with_format(0, 0, 1.0, &patterned)
        .unwrap();
    other
        .write_number_with_format(1, 0, 2.0, &unlocked)
        .unwrap();
    other.write_number(2, 0, 3.0).unwrap();
}

fn table_workbook(path: &Path) {
    let mut book = Source::new();
    let kilos = Format::new().set_num_format(CUSTOM_FORMAT);
    let data = book.add_worksheet().set_name("Data").unwrap();
    for row in 1..=4u32 {
        data.write_string(row, 0, format!("item {row}")).unwrap();
        data.write_number_with_format(row, 1, f64::from(row) * 1.5, &kilos)
            .unwrap();
    }
    let columns = [
        TableColumn::new().set_header("Item"),
        TableColumn::new().set_header("Weight"),
    ];
    data.add_table(0, 0, 4, 1, &Table::new().set_columns(&columns))
        .unwrap();
    other_styles(&mut book);
    book.save(path).unwrap();
}

fn plain_workbook(path: &Path) {
    let mut book = Source::new();
    other_styles(&mut book);
    book.save(path).unwrap();
}

fn part(file: &Path, name: &str) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    Some(bytes)
}

fn text(file: &Path, name: &str) -> String {
    String::from_utf8(part(file, name).unwrap()).unwrap()
}

fn part_names(file: &Path) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    (0..archive.len())
        .map(|index| archive.by_index_raw(index).unwrap().name().to_string())
        .collect()
}

// The `<xf>` elements of cellXfs as written, in order.
fn cell_xfs(styles: &str) -> Vec<String> {
    let document = roxmltree::Document::parse(styles).unwrap();
    let list = document
        .root_element()
        .children()
        .find(|node| node.has_tag_name("cellXfs"))
        .unwrap();
    list.children()
        .filter(roxmltree::Node::is_element)
        .map(|node| styles[node.range()].to_string())
        .collect()
}

// Every list's count attribute says how many items it holds.
fn assert_counts_match(styles: &str) {
    let document = roxmltree::Document::parse(styles).unwrap();
    for list in document.root_element().children().filter(|node| {
        ["numFmts", "fonts", "fills", "borders", "cellXfs"]
            .iter()
            .any(|name| node.has_tag_name(*name))
    }) {
        let items = list.children().filter(roxmltree::Node::is_element).count();
        let declared: usize = list.attribute("count").unwrap().parse().unwrap();
        assert_eq!(declared, items, "{}", list.tag_name().name());
    }
}

struct Base {
    _dir: tempfile::TempDir,
    document: PathBuf,
    original: PathBuf,
    book: Workbook,
}

fn opened(write: impl FnOnce(&Path)) -> Base {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    write(&document);
    let original = dir.path().join("original.xlsx");
    std::fs::copy(&document, &original).unwrap();
    let book = open_xlsx(&document).unwrap().workbook;
    Base {
        _dir: dir,
        document,
        original,
        book,
    }
}

// Saves in place and checks what the app checks before it does: nothing is dropped.
fn save_in_place(base: &Base) {
    let snapshot = base
        .book
        .save_snapshot(&base.document, SaveTarget::Document)
        .unwrap();
    assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
    save_xlsx(snapshot).unwrap();
}

fn assert_styles_only_grew(base: &Base) {
    let before = text(&base.original, STYLES_PART);
    let after = text(&base.document, STYLES_PART);
    let (old, new) = (cell_xfs(&before), cell_xfs(&after));
    assert!(new.len() > old.len(), "no style was added");
    assert_eq!(new[..old.len()], old[..], "an existing style changed");
    assert_counts_match(&after);
    // Not modeled by IronCalc, so only a styles part that was never regenerated has them.
    assert!(after.contains(r#"patternType="darkGrid""#));
    assert!(after.contains(r#"locked="0""#));
}

#[test]
fn a_bold_cell_in_a_table_saves_in_place_and_keeps_the_table() {
    let mut base = opened(table_workbook);
    let other_before: Vec<_> = ["A1", "A2", "A3"]
        .map(|a1| base.book.cell(SheetId(1), pos(a1)).style)
        .into();
    base.book
        .apply_style(SheetId(0), range("B3"), StyleChange::Bold(true))
        .unwrap();
    save_in_place(&base);

    assert_eq!(part_names(&base.document), part_names(&base.original));
    assert_eq!(
        part(&base.document, "xl/tables/table1.xml"),
        part(&base.original, "xl/tables/table1.xml")
    );
    assert_eq!(
        part(&base.document, OTHER_SHEET_PART),
        part(&base.original, OTHER_SHEET_PART),
        "a sheet no edit touched is copied"
    );
    let sheet = text(&base.document, TABLE_SHEET_PART);
    assert!(sheet.contains("<tableParts"), "{sheet}");
    assert!(text(&base.document, "xl/worksheets/_rels/sheet1.xml.rels").contains("table1.xml"));
    assert_styles_only_grew(&base);

    let reopened = open_xlsx(&base.document).unwrap().workbook;
    assert!(reopened.cell(SheetId(0), pos("B3")).style.bold);
    assert!(!reopened.cell(SheetId(0), pos("B2")).style.bold);
    assert_eq!(
        reopened.cell(SheetId(0), pos("B3")).style.num_fmt,
        CUSTOM_FORMAT,
        "the cell keeps its number format"
    );
    let other_after: Vec<_> = ["A1", "A2", "A3"]
        .map(|a1| reopened.cell(SheetId(1), pos(a1)).style)
        .into();
    assert_eq!(other_after, other_before);
}

#[test]
fn a_bold_cell_outside_the_table_saves_in_place_too() {
    let mut base = opened(table_workbook);
    base.book
        .apply_style(SheetId(0), range("D10"), StyleChange::Bold(true))
        .unwrap();
    base.book.set_input(SheetId(0), pos("D10"), "note").unwrap();
    save_in_place(&base);
    assert_eq!(
        part(&base.document, "xl/tables/table1.xml"),
        part(&base.original, "xl/tables/table1.xml")
    );
    assert_styles_only_grew(&base);
    let reopened = open_xlsx(&base.document).unwrap().workbook;
    assert!(reopened.cell(SheetId(0), pos("D10")).style.bold);
}

#[test]
fn a_new_number_format_is_added_to_a_file_without_custom_formats() {
    let mut base = opened(plain_workbook);
    base.book
        .apply_style(
            SheetId(0),
            range("A3"),
            StyleChange::NumberFormat(NumberFormat::Date),
        )
        .unwrap();
    base.book
        .apply_style(SheetId(0), range("A1"), StyleChange::Fill(None))
        .unwrap();
    save_in_place(&base);
    assert_styles_only_grew(&base);
    let styles = text(&base.document, STYLES_PART);
    assert!(styles.contains("<numFmts"), "{styles}");
    assert!(styles.contains(r#"formatCode="dd/mm/yyyy""#), "{styles}");

    let reopened = open_xlsx(&base.document).unwrap().workbook;
    assert_eq!(
        reopened.cell(SheetId(0), pos("A3")).style.num_fmt,
        NumberFormat::Date.code()
    );
    assert_eq!(
        reopened.cell(SheetId(0), pos("A2")).style,
        base.book.cell(SheetId(0), pos("A2")).style
    );
}

#[test]
fn a_number_format_is_appended_to_the_custom_formats_the_file_has() {
    let mut base = opened(table_workbook);
    base.book
        .apply_style(
            SheetId(0),
            range("B2"),
            StyleChange::NumberFormat(NumberFormat::Date),
        )
        .unwrap();
    save_in_place(&base);
    assert_styles_only_grew(&base);
    let styles = text(&base.document, STYLES_PART);
    assert!(styles.contains(&format!(
        r#"formatCode="{}""#,
        CUSTOM_FORMAT.replace('"', "&quot;")
    )));
    let reopened = open_xlsx(&base.document).unwrap().workbook;
    assert_eq!(
        reopened.cell(SheetId(0), pos("B2")).style.num_fmt,
        NumberFormat::Date.code()
    );
    assert_eq!(
        reopened.cell(SheetId(0), pos("B3")).style.num_fmt,
        CUSTOM_FORMAT
    );
}

#[test]
fn style_edits_in_a_row_keep_appending() {
    let mut base = opened(table_workbook);
    let first = [
        StyleChange::Bold(true),
        StyleChange::Italic(true),
        StyleChange::Fill(Some(zenkai_types::Rgb(0x0AC81E))),
    ];
    for change in first {
        base.book
            .apply_style(SheetId(0), range("A3"), change)
            .unwrap();
        save_in_place(&base);
        assert_styles_only_grew(&base);
    }
    base.book.set_input(SheetId(1), pos("A3"), "9").unwrap();
    save_in_place(&base);
    assert_counts_match(&text(&base.document, STYLES_PART));
    let reopened = open_xlsx(&base.document).unwrap().workbook;
    let style = reopened.cell(SheetId(0), pos("A3")).style;
    assert!(style.bold && style.italic && style.fill.is_some());
    assert_eq!(reopened.cell(SheetId(1), pos("A3")).text, "9");
}
