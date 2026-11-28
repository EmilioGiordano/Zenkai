// Spec: a Text column imports every field as a text cell, and saving the CSV writes each field
// exactly as it was read.
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use proptest::prelude::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_formats::{CsvFormat, CsvWriter, Delimiter, Encoding, LineEnding, TextColumns};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, ValueKind};

use super::{CsvSave, CsvSaveRequest, LargeWrite, save_csv_file};
use crate::csv_io;
use crate::csv_options::{CsvBinding, CsvOptions};
use crate::document::ChangedFile;
use crate::entry::CsvLoad;

fn chosen(columns: &[u16]) -> TextColumns {
    TextColumns::chosen(columns.iter().filter_map(|column| ColIdx::new(*column)))
}

fn at_pos(row: u32, col: u16) -> CellPos {
    CellPos::new(RowIdx::new(row).unwrap(), ColIdx::new(col).unwrap())
}

fn at(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

// Opens `original` as the import does once the user chose `text`, with the delimiter and the
// number and date reading the file shows.
fn opened_with(dir: &Path, original: &[u8], text: TextColumns) -> (PathBuf, Workbook, CsvLoad) {
    let path = dir.join("ids.csv");
    std::fs::write(&path, original).unwrap();
    let found = csv_io::load_csv(&path, None).unwrap().csv.unwrap();
    opened_as(
        &path,
        CsvOptions {
            text,
            ..found.options
        },
    )
}

fn opened_as(path: &Path, options: CsvOptions) -> (PathBuf, Workbook, CsvLoad) {
    let mut load = csv_io::load_csv(path, Some(options)).unwrap();
    let csv = load.csv.take().unwrap();
    (path.to_path_buf(), load.workbook, csv)
}

// With the delimiter fixed, since arbitrary fields could make a guess pick another.
fn opened_as_comma(dir: &Path, original: &[u8], text: TextColumns) -> (PathBuf, Workbook, CsvLoad) {
    let path = dir.join("ids.csv");
    std::fs::write(&path, original).unwrap();
    opened_as(
        &path,
        CsvOptions {
            text,
            ..CsvOptions::exported_as(Delimiter::Comma)
        },
    )
}

fn request(path: &Path, csv: &CsvLoad) -> CsvSaveRequest {
    CsvSaveRequest {
        path: path.to_path_buf(),
        binding: CsvBinding::new(csv.options.clone()).with_sheet(csv.sheet),
        sheets: 1,
        sheet: SheetId(0),
        changed: ChangedFile::Ask,
        large: LargeWrite::Ask,
        guard: Some(csv.seen),
    }
}

// The bytes written, and the options the file was written with.
fn saved_as(asked: &CsvSaveRequest, workbook: Workbook) -> (Vec<u8>, CsvOptions) {
    let outcome = save_csv_file(&RwLock::new(workbook), asked, &AtomicBool::new(false)).unwrap();
    let CsvSave::Written(_, options, ..) = outcome else {
        panic!("the save was not written");
    };
    (std::fs::read(&asked.path).unwrap(), options)
}

fn saved(path: &Path, workbook: Workbook, csv: &CsvLoad) -> Vec<u8> {
    saved_as(&request(path, csv), workbook).0
}

fn assert_text(workbook: &Workbook, pos: CellPos, expected: &str) {
    let cell = workbook.cell(SheetId(0), pos);
    assert_eq!(cell.kind, ValueKind::Text, "{expected}");
    assert_eq!(cell.number, None, "{expected}");
    assert_eq!(cell.text, expected);
}

#[test]
fn a_leading_zero_id_and_a_19_digit_id_open_as_shown_and_save_unchanged() {
    let original = b"name,code,id\nAna,00123,9223372036854775807\nLuis,0045,1234567890123456789\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, workbook, csv) = opened_with(dir.path(), original, chosen(&[1, 2]));
    assert_text(&workbook, at(1, 1), "00123");
    assert_text(&workbook, at(1, 2), "9223372036854775807");
    assert_text(&workbook, at(2, 1), "0045");
    assert_text(&workbook, at(2, 2), "1234567890123456789");
    assert_eq!(workbook.cell(SheetId(0), at(1, 0)).kind, ValueKind::Text);
    assert_eq!(saved(&path, workbook, &csv), original);
}

#[test]
fn the_columns_detection_flags_open_as_text_and_save_unchanged() {
    let original = b"name,code,id\nAna,00123,9223372036854775807\nLuis,0045,1234567890123456789\n";
    let rows = zenkai_formats::parse_csv(original, None).unwrap().rows;
    let flagged = zenkai_formats::find_at_risk(&rows);
    assert_eq!(
        flagged.iter().map(|risk| risk.column).collect::<Vec<_>>(),
        [1, 2]
    );
    let text = TextColumns::chosen(flagged.iter().map(|risk| risk.column));
    let dir = tempfile::tempdir().unwrap();
    let (path, workbook, csv) = opened_with(dir.path(), original, text);
    assert_text(&workbook, at(1, 1), "00123");
    assert_text(&workbook, at(1, 2), "9223372036854775807");
    assert_text(&workbook, at(2, 2), "1234567890123456789");
    assert_eq!(saved(&path, workbook, &csv), original);
}

#[test]
fn the_same_file_read_as_general_turns_the_ids_into_numbers() {
    let original = b"code,id\n00123,9223372036854775807\n";
    let dir = tempfile::tempdir().unwrap();
    let (_, workbook, _) = opened_with(dir.path(), original, TextColumns::default());
    let code = workbook.cell(SheetId(0), at(1, 0));
    assert_eq!(code.kind, ValueKind::Number);
    assert_eq!(code.number, Some(123.0));
}

#[test]
fn a_text_column_keeps_what_decimal_and_date_reading_would_rewrite() {
    let original = b"price;when;note\n1,50;08/10/2026;x\n2,25;25/10/2026;y\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, workbook, csv) = opened_with(dir.path(), original, chosen(&[0, 1]));
    assert_text(&workbook, at(1, 0), "1,50");
    assert_text(&workbook, at(1, 1), "08/10/2026");
    assert_eq!(saved(&path, workbook, &csv), original);
}

#[test]
fn keep_everything_as_text_opens_formulas_signs_and_numbers_as_written() {
    let original = b"a,b,c,d,e\n=1+1,-5,TRUE,1E5,'quoted\n007,+34 600,2026-01-31,50%,\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, workbook, csv) = opened_with(dir.path(), original, TextColumns::Everything);
    for (pos, expected) in [
        (at(1, 0), "=1+1"),
        (at(1, 1), "-5"),
        (at(1, 2), "TRUE"),
        (at(1, 3), "1E5"),
        (at(1, 4), "'quoted"),
        (at(2, 0), "007"),
        (at(2, 1), "+34 600"),
        (at(2, 2), "2026-01-31"),
        (at(2, 3), "50%"),
    ] {
        assert_text(&workbook, pos, expected);
    }
    assert_eq!(saved(&path, workbook, &csv), original);
}

#[test]
fn the_choices_stored_in_the_session_read_the_file_the_same_way_on_reopening() {
    let original = b"code,qty\n00123,5\n0007,6\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, _, csv) = opened_with(dir.path(), original, chosen(&[0]));
    let stored = serde_json::to_string(&CsvBinding::new(csv.options.clone())).unwrap();
    let restored: CsvBinding = serde_json::from_str(&stored).unwrap();
    let mut again = csv_io::load_csv(&path, restored.options).unwrap();
    assert_text(&again.workbook, at(1, 0), "00123");
    assert_text(&again.workbook, at(2, 0), "0007");
    let quantity = again.workbook.cell(SheetId(0), at(1, 1));
    assert_eq!(quantity.number, Some(5.0));
    let csv = again.csv.take().unwrap();
    assert_eq!(saved(&path, again.workbook, &csv), original);
}

#[test]
fn an_edited_text_cell_is_written_as_typed_and_the_rest_stays() {
    let original = b"code,qty\n00123,5\n0007,6\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, mut workbook, csv) = opened_with(dir.path(), original, chosen(&[0]));
    workbook.set_input(SheetId(0), at(2, 0), "'0008").unwrap();
    assert_eq!(saved(&path, workbook, &csv), b"code,qty\n00123,5\n0008,6\n");
}

const ALPHABET: &str = "0012345679abZ ,\"\n'=+-.%$/E;|\t@\u{e9}";

fn field() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(ALPHABET.chars().collect::<Vec<_>>()),
        0..10,
    )
    .prop_map(|characters| characters.into_iter().collect())
}

// The file as written for a program that reads it back as it is: no field is prefixed.
fn file_of(rows: &[Vec<String>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let format = CsvFormat {
        delimiter: Delimiter::Comma,
        encoding: Encoding::Utf8,
        line_ending: LineEnding::Lf,
    };
    let mut writer = CsvWriter::in_format(&mut bytes, format).unwrap();
    for row in rows {
        for field in row {
            writer.write_verbatim_field(field).unwrap();
        }
        writer.end_row().unwrap();
    }
    writer.finish().unwrap();
    bytes
}

const COLUMNS: usize = 4;

fn grid() -> impl Strategy<Value = Vec<Vec<String>>> {
    prop::collection::vec(prop::collection::vec(field(), COLUMNS..=COLUMNS), 1..6)
}

fn check_text_cells(workbook: &Workbook, rows: &[Vec<String>], text: &TextColumns) {
    for (row, fields) in rows.iter().enumerate() {
        for (column, field) in fields.iter().enumerate() {
            if text.holds_field(column) && !field.is_empty() {
                assert_text(workbook, at(row as i64, column as i64), field);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn text_columns_are_read_and_saved_byte_for_byte(
        rows in grid(),
        mask in 1u8..(1 << COLUMNS),
    ) {
        let columns: Vec<u16> = (0..COLUMNS as u16)
            .filter(|column| (mask >> column) & 1 == 1)
            .collect();
        let text = chosen(&columns);
        let plain = vec!["word".to_string(); COLUMNS];
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|fields| {
                fields
                    .into_iter()
                    .zip(&plain)
                    .enumerate()
                    .map(|(column, (field, other))| {
                        if text.holds_field(column) { field } else { other.clone() }
                    })
                    .collect()
            })
            .collect();
        let original = file_of(&rows);
        let dir = tempfile::tempdir().unwrap();
        let (path, workbook, csv) = opened_as_comma(dir.path(), &original, text.clone());
        check_text_cells(&workbook, &rows, &text);
        prop_assert_eq!(saved(&path, workbook, &csv), original);
    }

    #[test]
    fn keep_everything_as_text_reads_and_saves_every_field_byte_for_byte(rows in grid()) {
        let original = file_of(&rows);
        let dir = tempfile::tempdir().unwrap();
        let (path, workbook, csv) = opened_as_comma(dir.path(), &original, TextColumns::Everything);
        check_text_cells(&workbook, &rows, &TextColumns::Everything);
        prop_assert_eq!(saved(&path, workbook, &csv), original);
    }
}

fn reopened(path: &Path, options: CsvOptions) -> Workbook {
    csv_io::load_csv(path, Some(options)).unwrap().workbook
}

#[test]
fn deleting_a_column_before_the_text_columns_moves_the_choice_with_them() {
    let original = b"name,code,id\nAna,00123,9223372036854775807\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, mut workbook, csv) = opened_with(dir.path(), original, chosen(&[1, 2]));
    workbook
        .delete_columns(SheetId(0), ColIdx::clamped(0), 1)
        .unwrap();
    let (bytes, options) = saved_as(&request(&path, &csv), workbook);
    assert_eq!(bytes, b"code,id\n00123,9223372036854775807\n");
    assert_eq!(options.text, chosen(&[0, 1]));
    let workbook = reopened(&path, options);
    assert_text(&workbook, at(1, 0), "00123");
    assert_text(&workbook, at(1, 1), "9223372036854775807");
}

#[test]
fn inserting_a_column_before_the_text_columns_moves_the_choice_with_them() {
    let original = b"name,code,id\nAna,00123,9223372036854775807\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, mut workbook, csv) = opened_with(dir.path(), original, chosen(&[1, 2]));
    workbook
        .insert_columns(SheetId(0), ColIdx::clamped(0), 1)
        .unwrap();
    let (_, options) = saved_as(&request(&path, &csv), workbook);
    assert_eq!(options.text, chosen(&[2, 3]));
    let workbook = reopened(&path, options);
    assert_text(&workbook, at(1, 2), "00123");
    assert_text(&workbook, at(1, 3), "9223372036854775807");
}

#[test]
fn a_log_that_cannot_be_followed_gives_the_text_columns_the_cells_that_are_text() {
    let original = b"name,qty,code\nAna,5,00123\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, workbook, csv) = opened_with(dir.path(), original, chosen(&[2]));
    let mut asked = request(&path, &csv);
    asked.binding.log.lose(u64::MAX);
    asked.binding.accepted.moved_cells = true;
    let (_, options) = saved_as(&asked, workbook);
    assert!(options.text.holds_field(2) && !options.text.holds_field(1));
    assert_text(&reopened(&path, options), at(1, 2), "00123");
}

#[test]
fn moving_text_cells_keeps_them_text_whatever_they_look_like() {
    let original = b"a,b\n=1+1,00123\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, mut workbook, csv) = opened_with(dir.path(), original, TextColumns::Everything);
    workbook
        .insert_rows(SheetId(0), RowIdx::clamped(1), 1)
        .unwrap();
    assert_text(&workbook, at(2, 0), "=1+1");
    assert_text(&workbook, at(2, 1), "00123");
    assert_eq!(saved(&path, workbook, &csv), b"a,b\n,\n=1+1,00123\n");
}

#[test]
fn a_file_opened_with_nothing_stored_has_its_ids_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ids.csv");
    std::fs::write(&path, b"name,code\nAna,00123\n").unwrap();
    let load = csv_io::load_csv(&path, None).unwrap();
    assert_text(&load.workbook, at(1, 1), "00123");
}

#[test]
fn sorting_text_cells_keeps_them_text_and_the_fields_as_they_were() {
    let original = b"code,name\n00123,Bob\n=1+1,Ana\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, mut workbook, csv) = opened_with(dir.path(), original, TextColumns::Everything);
    let range = Range::new(at_pos(1, 0), at_pos(2, 1));
    workbook
        .sort(SheetId(0), range, ColIdx::clamped(1), false)
        .unwrap();
    assert_text(&workbook, at(1, 0), "=1+1");
    assert_text(&workbook, at(2, 0), "00123");
    let mut asked = request(&path, &csv);
    asked.binding.accepted.moved_cells = true;
    let (bytes, _) = saved_as(&asked, workbook);
    assert_eq!(bytes, b"code,name\n=1+1,Ana\n00123,Bob\n");
}

#[test]
fn filling_down_text_cells_keeps_them_text() {
    let original = b"code\n00123\n=1+1\n";
    let dir = tempfile::tempdir().unwrap();
    let (path, mut workbook, csv) = opened_with(dir.path(), original, TextColumns::Everything);
    workbook
        .fill(SheetId(0), Range::single(at_pos(2, 0)), true)
        .unwrap();
    assert_text(&workbook, at(2, 0), "00123");
    let mut asked = request(&path, &csv);
    asked.binding.accepted.number_formats = true;
    asked.binding.accepted.formatting = true;
    let (bytes, _) = saved_as(&asked, workbook);
    assert_eq!(bytes, b"code\n00123\n00123\n");
}
