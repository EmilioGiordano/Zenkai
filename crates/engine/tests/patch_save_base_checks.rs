// Spec: an edit is never lost. A save that copies from the file on disk must refuse a file
// it cannot trust to be the one it read: a part stored twice, or a file swapped for another
// of the same length and modification time.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::path::Path;

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{
    Engine, EngineError, SaveTarget, Workbook, XlsxReader, open_xlsx, open_xlsx_with, save_xlsx,
};
use zenkai_types::{CellPos, SheetId};

const NOTES_PART: &str = "xl/worksheets/sheet2.xml";
const STRINGS_PART: &str = "xl/sharedStrings.xml";

fn base_file(path: &Path) {
    let mut book = Source::new();
    let data = book.add_worksheet().set_name("Data").unwrap();
    for (row, fruit) in ["apple", "pear", "fig"].iter().enumerate() {
        data.write_string(row as u32, 0, *fruit).unwrap();
        data.write_number(row as u32, 1, row as f64).unwrap();
    }
    let notes = book.add_worksheet().set_name("Notes").unwrap();
    notes.write_number(0, 0, 7.0).unwrap();
    book.save(path).unwrap();
}

fn entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (entry.name().to_string(), bytes)
        })
        .collect()
}

// Stored, not deflated, so a part of the same length gives a file of the same length.
fn write_entries(path: &Path, entries: &[(String, Vec<u8>)], method: zip::CompressionMethod) {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default());
    for (name, bytes) in entries {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
}

fn texts(book: &Workbook, sheet: u32) -> Vec<String> {
    (1..=3)
        .flat_map(|row| ["A", "B"].map(|col| format!("{col}{row}")))
        .map(|a1| {
            book.cell(SheetId(sheet), CellPos::parse_a1(&a1).unwrap())
                .text
        })
        .collect()
}

fn reopened_both_ways(path: &Path) -> Vec<Workbook> {
    [XlsxReader::Fast, XlsxReader::IronCalc]
        .map(|reader| open_xlsx_with(path, reader).unwrap().workbook)
        .into()
}

#[test]
fn a_part_stored_twice_is_never_copied_from() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    base_file(&document);
    let mut parts = entries(&document);
    let notes = parts
        .iter()
        .find(|(name, _)| name == NOTES_PART)
        .cloned()
        .unwrap();
    parts.push(notes);
    write_entries(&document, &parts, zip::CompressionMethod::Deflated);
    let mut book = open_xlsx(&document).unwrap().workbook;
    let a1 = CellPos::default();
    book.set_input(SheetId(1), a1, "mine").unwrap();
    for target in [SaveTarget::Copy, SaveTarget::Document] {
        let path = match target {
            SaveTarget::Copy => dir.path().join("copy.xlsx"),
            SaveTarget::Document => document.clone(),
        };
        save_xlsx(book.save_snapshot(&path, target).unwrap()).unwrap();
        let names: Vec<_> = entries(&path).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names.iter().filter(|name| *name == NOTES_PART).count(), 1);
        for reopened in reopened_both_ways(&path) {
            assert_eq!(reopened.cell(SheetId(1), a1).text, "mine");
        }
    }
}

// Same name length as the Notes part, so renaming one to the other keeps the file's length.
const SPARE_PART: &str = "xl/worksheets/spare2.xml";
const RELS_PART: &str = "xl/_rels/workbook.xml.rels";

type Entries = Vec<(String, Vec<u8>)>;

fn edited(entries: Entries, name: &str, edit: impl Fn(String) -> String) -> Entries {
    entries
        .into_iter()
        .map(|(part, bytes)| match part == name {
            true => {
                let text = String::from_utf8(bytes).unwrap();
                let changed = edit(text.clone());
                assert_ne!(changed, text, "{name} was not changed");
                assert_eq!(changed.len(), text.len());
                (part, changed.into_bytes())
            }
            false => (part, bytes),
        })
        .collect()
}

// Two shared strings merged into one.
fn strings_merged(entries: Entries) -> Entries {
    edited(entries, STRINGS_PART, |text| {
        text.replacen("</si><si>", "<!---->  ", 1)
    })
}

// The Notes part stored a second time, after the first, which readers then read.
fn notes_twice(entries: Entries) -> Entries {
    entries
        .into_iter()
        .map(|(part, bytes)| match part == SPARE_PART {
            true => (NOTES_PART.to_string(), bytes),
            false => (part, bytes),
        })
        .collect()
}

// The Notes sheet pointed at another part.
fn sheet_moved(entries: Entries) -> Entries {
    edited(entries, RELS_PART, |text| {
        text.replacen("worksheets/sheet2.xml", "worksheets/spare2.xml", 1)
    })
}

// Rewrites the file in place keeping its length and modification time, which is all a save
// compares to know the file did not change.
fn swap_keeping_stamp(path: &Path, entries: &Entries) {
    let stamp = std::fs::metadata(path).unwrap();
    write_entries(path, entries, zip::CompressionMethod::Stored);
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(stamp.modified().unwrap()).unwrap();
    drop(file);
    assert_eq!(std::fs::metadata(path).unwrap().len(), stamp.len());
}

#[test]
fn a_file_swapped_with_the_same_stamp_fails_the_save_and_the_next_one_works() {
    let swaps: [(&str, fn(Entries) -> Entries); 3] = [
        ("strings merged", strings_merged),
        ("notes twice", notes_twice),
        ("sheet moved", sheet_moved),
    ];
    for (swap, damage) in swaps {
        for (sheet, edit) in [(0, "99"), (0, "kiwi"), (1, "mine")] {
            let dir = tempfile::tempdir().unwrap();
            let document = dir.path().join("book.xlsx");
            base_file(&document);
            let mut parts = entries(&document);
            let notes = parts.iter().find(|(name, _)| name == NOTES_PART).unwrap();
            parts.push((SPARE_PART.to_string(), notes.1.clone()));
            write_entries(&document, &parts, zip::CompressionMethod::Stored);
            let mut book = open_xlsx(&document).unwrap().workbook;
            swap_keeping_stamp(&document, &damage(parts));

            let b1 = CellPos::parse_a1("B1").unwrap();
            book.set_input(SheetId(sheet), b1, edit).unwrap();
            let refused = save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap());
            assert!(
                matches!(refused, Err(EngineError::BaseUnusable { .. })),
                "{swap}, {edit}: {refused:?}"
            );
            save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
            for reopened in reopened_both_ways(&document) {
                for sheet in [0, 1] {
                    assert_eq!(
                        texts(&reopened, sheet),
                        texts(&book, sheet),
                        "{swap}, {edit}"
                    );
                }
            }
        }
    }
}
