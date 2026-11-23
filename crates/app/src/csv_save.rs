use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use zenkai_engine::{Engine, EngineError, ShownCells, UNREADABLE_CELL, Workbook, is_date_format};
use zenkai_formats::{
    DateOrder, DecimalMark, date_serial, day_first_field, decimal_comma_field, iso_date,
    number_value, same_value,
};
use zenkai_i18n::t;
use zenkai_types::{CellPos, Contents, SheetId, ValueKind};

use crate::csv_io::{self, AsRead, BaselineRead, Layout};
use crate::csv_options::{CsvBinding, CsvOptions, Losses};
use crate::document::{ChangedFile, file_changed_since, read_shared};
use crate::entry::FileSeen;

pub struct CsvSaveRequest {
    pub path: PathBuf,
    pub binding: CsvBinding,
    pub sheets: usize,
    pub sheet: SheetId,
    pub changed: ChangedFile,
    pub guard: Option<FileSeen>,
}

pub enum CsvSave {
    Written(FileSeen, CsvOptions),
    WouldLose(Losses),
    FileChanged,
    LossyFile,
    ShapeUnknown,
}

fn import_input<'a>(field: &'a str, options: &CsvOptions) -> Cow<'a, str> {
    let mut text = Cow::Borrowed(field);
    if options.date_order == DateOrder::DayFirst
        && let Some(rewritten) = day_first_field(&text)
    {
        text = Cow::Owned(rewritten);
    }
    if options.decimal_mark == DecimalMark::Comma
        && let Some(rewritten) = decimal_comma_field(&text)
    {
        text = Cow::Owned(rewritten);
    }
    text
}

fn field_at(rows: &mut [Vec<String>], pos: CellPos) -> Option<&mut String> {
    let row = usize::try_from(pos.row.get()).ok()?;
    rows.get_mut(row)?.get_mut(usize::from(pos.col.get()))
}

// A number the file's reader takes back as the same one: no grouping, which it could not tell
// from decimal commas; in a decimal-comma file a plain decimal, keeping only a percent sign.
fn number_text(source: &str, mark: DecimalMark) -> String {
    let (body, percent) = match source.strip_suffix('%') {
        Some(body) => (body, true),
        None => (source, false),
    };
    let ungrouped = body.replace(',', "");
    let Some(value) = number_value(&ungrouped) else {
        return source.to_string();
    };
    match mark {
        DecimalMark::Dot => format!("{ungrouped}{}", if percent { "%" } else { "" }),
        DecimalMark::Comma => format!(
            "{}{}",
            value.to_string().replace('.', ","),
            if percent { "%" } else { "" }
        ),
    }
}

// A cell whose input stands for what importing its position's field gives is unedited and takes
// that field's text. After rows or columns move, the values differ and the cell counts as edited.
fn take_unedited(
    text: &mut String,
    input: &str,
    field: Option<&mut String>,
    options: &CsvOptions,
) -> bool {
    match field.filter(|field| !field.is_empty()) {
        Some(field) if same_value(input, &import_input(field, options)) => {
            *text = std::mem::take(field);
            true
        }
        _ => false,
    }
}

// What the cell shows if the file's reader takes it as the same date, else yyyy-mm-dd. A date
// with a time has no text the engine reads back, so it is written as its serial.
fn date_text(shown: &str, serial: f64, options: &CsvOptions) -> String {
    if date_serial(shown, options.date_order) == Some(serial) {
        return shown.to_string();
    }
    iso_date(serial).unwrap_or_else(|| number_text(&serial.to_string(), options.decimal_mark))
}

// The text of an edited number, so that reading the file gives the same value back: what the
// cell shows unless that rounds the value (General), then the value. A formula writes its result.
fn edited_number_text(
    workbook: &Workbook,
    sheet: SheetId,
    pos: CellPos,
    shown: &str,
    input: &str,
    options: &CsvOptions,
) -> String {
    let view = workbook.cell(sheet, pos);
    if let Some(serial) = view.number.filter(|_| is_date_format(&view.style.num_fmt)) {
        return date_text(shown, serial, options);
    }
    let value = (!input.starts_with('=') && number_value(input).is_some()).then_some(input);
    let source = match value {
        Some(value) if !same_value(shown, value) => value,
        _ => shown,
    };
    number_text(source, options.decimal_mark)
}

struct Classified {
    shown: ShownCells,
    formulas: bool,
}

// One read lock for the whole pass, so the cells, their inputs and the loss all describe one
// moment; an edit waiting for the write lock waits for this pass only.
fn classify_sheet(
    shared: &RwLock<Workbook>,
    request: &CsvSaveRequest,
    options: &CsvOptions,
    file: &mut [Vec<String>],
) -> Result<Classified, EngineError> {
    let sheet = request.sheet;
    let workbook = read_shared(shared);
    let mut shown = workbook.shown_cells(sheet)?;
    let contents = workbook.contents(sheet)?;
    let mut formulas = false;
    for (pos, text) in &mut shown.cells {
        if text == UNREADABLE_CELL && workbook.cell(sheet, *pos).kind == ValueKind::Error {
            return Err(EngineError::Rejected(t!("csv.unreadable_cell", cell = pos)));
        }
        let input = workbook.input(sheet, *pos);
        if take_unedited(text, &input, field_at(file, *pos), options) {
            continue;
        }
        formulas = formulas || input.starts_with('=');
        if matches!(contents(*pos), Contents::Number(_)) {
            *text = edited_number_text(&workbook, sheet, *pos, text, &input, options);
        }
    }
    Ok(Classified { shown, formulas })
}

pub fn save_csv_file(
    shared: &RwLock<Workbook>,
    request: &CsvSaveRequest,
    cancel: &AtomicBool,
) -> Result<CsvSave, EngineError> {
    let options = match request.binding.options {
        Some(options) => options,
        None => csv_io::detect_csv_options(&request.path).map_err(EngineError::Rejected)?,
    };
    let baseline = match csv_io::read_baseline(&request.path, options.delimiter)? {
        BaselineRead::Present(baseline) => Some(baseline),
        BaselineRead::Absent => None,
        BaselineRead::Lossy => return Ok(CsvSave::LossyFile),
        BaselineRead::ShapeUnknown => return Ok(CsvSave::ShapeUnknown),
    };
    // Checked after the read, so the file compared with is the one that was checked.
    if request.changed == ChangedFile::Ask && file_changed_since(request.guard, &request.path) {
        return Ok(CsvSave::FileChanged);
    }
    let (mut file, ends_with_newline) = baseline.map_or((Vec::new(), true), |baseline| {
        (baseline.rows, baseline.ends_with_newline)
    });
    let row_lengths: Vec<usize> = file.iter().map(Vec::len).collect();
    let content_rows = file
        .iter()
        .rposition(|row| row.iter().any(|field| !field.is_empty()))
        .map_or(0, |last| last + 1);
    let classified = classify_sheet(shared, request, &options, &mut file)?;
    let found = Losses {
        sheets: request.sheets > 1,
        formulas: classified.formulas,
        ..request.binding.introduced
    };
    if found.beyond(request.binding.accepted).any() {
        return Ok(CsvSave::WouldLose(found));
    }
    let layout = Layout::AsRead(AsRead {
        row_lengths: &row_lengths,
        content_rows,
        ends_with_newline,
    });
    csv_io::write_csv_as(&request.path, &classified.shown, &options, &layout, cancel)?;
    Ok(CsvSave::Written(FileSeen::of(&request.path), options))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use zenkai_formats::Delimiter;
    use zenkai_types::{ColIdx, RowIdx};

    use super::*;
    use crate::entry::CsvLoad;

    fn encode_utf16(text: &str, to_bytes: fn(u16) -> [u8; 2], bom: [u8; 2]) -> Vec<u8> {
        bom.into_iter()
            .chain(text.encode_utf16().flat_map(to_bytes))
            .collect()
    }

    fn at(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    fn value_at(workbook: &Workbook, pos: CellPos) -> Option<f64> {
        workbook.cell(SheetId(0), pos).number
    }

    fn opened_csv(dir: &Path, name: &str, original: &[u8]) -> (PathBuf, crate::files::FileLoad) {
        let path = dir.join(name);
        std::fs::write(&path, original).unwrap();
        let load = csv_io::load_csv(&path, None).unwrap();
        (path, load)
    }

    fn request(path: &Path, csv: &CsvLoad, sheets: usize) -> CsvSaveRequest {
        CsvSaveRequest {
            path: path.to_path_buf(),
            binding: CsvBinding::new(csv.options),
            sheets,
            sheet: SheetId(0),
            changed: ChangedFile::Ask,
            guard: Some(csv.seen),
        }
    }

    fn save(path: &Path, workbook: Workbook, csv: &CsvLoad) -> (CsvSave, Vec<u8>) {
        let asked = request(path, csv, 1);
        let saved = save_csv_file(&RwLock::new(workbook), &asked, &AtomicBool::new(false)).unwrap();
        (saved, std::fs::read(path).unwrap())
    }

    // Reads `original` as a CSV file and saves it again without an edit.
    fn open_and_save(dir: &Path, name: &str, original: &[u8]) -> Vec<u8> {
        let (path, mut load) = opened_csv(dir, name, original);
        let csv = load.csv.take().unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)), "{name}");
        bytes
    }

    fn assert_unchanged(name: &str, original: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            open_and_save(dir.path(), name, original),
            original,
            "{}",
            String::from_utf8_lossy(original)
        );
    }

    #[test]
    fn a_semicolon_windows_1252_file_with_decimal_commas_is_saved_as_it_was() {
        assert_unchanged(
            "precios.csv",
            b"nombre;precio;ciudad\nJos\xe9;1,5;M\xe1laga\nAna;2;C\xf3rdoba\n",
        );
    }

    #[test]
    fn a_tab_separated_utf16_file_is_saved_as_it_was() {
        let text = "name\tnote\r\nAna\tcaf\u{e9}\r\nLuis\t3\r\n";
        assert_unchanged(
            "notes.tsv",
            &encode_utf16(text, u16::to_le_bytes, [0xFF, 0xFE]),
        );
        assert_unchanged(
            "notes_be.tsv",
            &encode_utf16(text, u16::to_be_bytes, [0xFE, 0xFF]),
        );
    }

    #[test]
    fn a_bom_is_written_only_if_the_file_had_one_and_line_endings_stay() {
        for original in [
            &b"a,b\n1,2\n"[..],
            b"a,b\r\n1,2\r\n",
            b"\xEF\xBB\xBFa,b\n1,2\n",
            b"\xEF\xBB\xBFa,b\r\n1,2\r\n",
            b"a|b\r1|2\r",
        ] {
            assert_unchanged("plain.csv", original);
        }
    }

    #[test]
    fn numbers_the_engine_would_rewrite_are_saved_as_the_file_had_them() {
        assert_unchanged(
            "numbers.csv",
            b"id,price,pct,ean,exp,cur\n007,1.50,12.345%,1234567890123,1e3,$1.999\n\
              00123,2,0.5%,9780201379624,2e2,$2.5\n\
              x,\"1,234.5678\",-0.10,100000000000,1E5,0.000000001\n",
        );
    }

    #[test]
    fn decimal_comma_numbers_are_saved_as_the_file_had_them() {
        assert_unchanged(
            "comas.csv",
            b"a;b\n1.234.567,89;2\n00123;1,50\n12,345%;1.234\n",
        );
    }

    #[test]
    fn mixed_date_layouts_and_year_first_text_in_a_day_first_file_are_saved_as_they_were() {
        assert_unchanged(
            "fechas.csv",
            b"f;g;h;i;n\n08/10/2026;2026/10/25;8-1-26;8.1.2026;1,5\n\
              25/10/2026;2026/01/02;25-10-26;2026-12-31;2\n",
        );
    }

    #[test]
    fn text_that_looks_like_a_formula_is_saved_over_its_own_csv_as_it_was() {
        assert_unchanged("contacts.csv", b"note,phone\n-,@ana\n@bob,ok\n");
    }

    #[test]
    fn the_shape_of_the_file_is_kept_empty_lines_short_rows_empty_columns_and_the_last_line() {
        for original in [
            &b"a\n\nb\n"[..],
            b"\r\n\r\na,b\r\n\r\nc\r\n",
            b"a,b,\n1,2,\n",
            b"a,b,c\n1\n2,3\n",
            b"a,b\n1,2",
            b"a,b\n\n1,2",
            b"a\r\rb\r",
            b"a\n\n",
            b"a,b\n,,\n",
            b"a,b\n\n\n",
        ] {
            assert_unchanged("shape.csv", original);
        }
    }

    #[test]
    fn a_stray_quote_in_a_plain_field_is_kept_and_only_the_quoting_changes() {
        let dir = tempfile::tempdir().unwrap();
        let saved = open_and_save(
            dir.path(),
            "quotes.csv",
            b"size,note\n5\" pipe,x\n\"a, b\",y\n",
        );
        assert_eq!(saved, b"size,note\n\"5\"\" pipe\",x\n\"a, b\",y\n");
    }

    #[test]
    fn only_the_cell_that_was_edited_changes_in_the_file_and_edited_text_is_never_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) =
            opened_csv(dir.path(), "edit.csv", b"a,b,c\n007,1.50,x\n00123,2,y\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(1, 1), "9").unwrap();
        load.workbook.set_input(SheetId(0), at(2, 2), "-").unwrap();
        load.workbook.set_input(SheetId(0), at(0, 0), "@a").unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"@a,b,c\n007,9,x\n00123,2,-\n");
    }

    #[test]
    fn an_edit_of_the_last_digit_is_an_edit_however_long_the_number() {
        let dir = tempfile::tempdir().unwrap();
        for (edit, expected) in [
            ("9780201379625", &b"ean\n9780201379625\n"[..]),
            ("1.00000000000001", b"ean\n1.00000000000001\n"),
        ] {
            let original: &[u8] = if edit.starts_with('9') {
                b"ean\n9780201379624\n"
            } else {
                b"ean\n1\n"
            };
            let (path, mut load) = opened_csv(dir.path(), "ean.csv", original);
            let csv = load.csv.take().unwrap();
            load.workbook.set_input(SheetId(0), at(1, 0), edit).unwrap();
            let (saved, bytes) = save(&path, load.workbook, &csv);
            assert!(matches!(saved, CsvSave::Written(..)));
            assert_eq!(bytes, expected, "{edit}");
        }
    }

    #[test]
    fn an_edited_number_reopens_as_the_same_number() {
        let dir = tempfile::tempdir().unwrap();
        // File name, file, typed edit, saved file, value read back.
        type Case = (
            &'static str,
            &'static [u8],
            &'static str,
            &'static [u8],
            f64,
        );
        let cases: [Case; 4] = [
            (
                "comma.csv",
                b"a;b\n1,5;2\n",
                "-5.5",
                b"a;b\n1,5;-5,5\n",
                -5.5,
            ),
            (
                "percent.csv",
                b"a,b\n1,2\n",
                "-12%",
                b"a,b\n1,-12%\n",
                -0.12,
            ),
            (
                "negative.csv",
                b"a,b\n1,2\n",
                "-1234.5",
                b"a,b\n1,-1234.5\n",
                -1234.5,
            ),
            (
                "large.csv",
                b"a;b\n1,5;2\n",
                "1234567.5",
                b"a;b\n1,5;1234567,5\n",
                1_234_567.5,
            ),
        ];
        for (name, original, edit, expected, value) in cases {
            let (path, mut load) = opened_csv(dir.path(), name, original);
            let csv = load.csv.take().unwrap();
            load.workbook.set_input(SheetId(0), at(1, 1), edit).unwrap();
            let (saved, bytes) = save(&path, load.workbook, &csv);
            assert!(matches!(saved, CsvSave::Written(..)), "{name}");
            assert_eq!(bytes, expected, "{name}");

            let reopened = csv_io::load_csv(&path, None).unwrap();
            let read = value_at(&reopened.workbook, at(1, 1)).unwrap();
            assert!((read - value).abs() < 1e-9, "{name} {read}");
        }
    }

    #[test]
    fn a_date_edited_in_a_day_first_file_is_written_unambiguously_and_reopens_as_the_same_serial() {
        let dir = tempfile::tempdir().unwrap();
        for (name, typed, format, expected) in [
            ("typed.csv", "10/8/2026", None, "2026-10-08"),
            ("words.csv", "10/8/2026", Some("d mmmm yyyy"), "2026-10-08"),
            ("time.csv", "46303.5", Some("yyyy-mm-dd hh:mm"), "46303,5"),
        ] {
            let (path, mut load) = opened_csv(dir.path(), name, b"f;n\n25/10/2026;1,5\n");
            let csv = load.csv.take().unwrap();
            let cell = at(2, 0);
            load.workbook.set_input(SheetId(0), cell, typed).unwrap();
            if let Some(format) = format {
                load.workbook
                    .set_number_format(SheetId(0), zenkai_types::Range::single(cell), format)
                    .unwrap();
            }
            let serial = value_at(&load.workbook, cell).unwrap();
            let (saved, bytes) = save(&path, load.workbook, &csv);
            assert!(matches!(saved, CsvSave::Written(..)), "{name}");
            let mut written = b"f;n\n25/10/2026;1,5\n".to_vec();
            written.extend_from_slice(expected.as_bytes());
            written.extend_from_slice(b";\n");
            assert_eq!(bytes, written, "{name}");

            let reopened = csv_io::load_csv(&path, None).unwrap();
            assert_eq!(value_at(&reopened.workbook, cell), Some(serial), "{name}");
        }
    }

    #[test]
    fn cells_moved_by_inserting_a_row_are_written_as_shown_and_the_rest_as_they_were() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "insert.csv", b"a,b\n007,1.50\n00123,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(1), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"a,b\n,\n7,1.5\n123,2\n");
    }

    #[test]
    fn a_file_decoded_with_replacements_is_not_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xEF\xBB\xBFa,\xFF\n";
        let (path, mut load) = opened_csv(dir.path(), "broken.csv", original);
        let csv = load.csv.take().unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::LossyFile));
        assert_eq!(bytes, original);
    }

    #[test]
    fn a_file_that_cannot_be_read_back_fails_the_save_and_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "book.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        let unreadable = vec![b'x'; 40_000];
        std::fs::write(&path, &unreadable).unwrap();
        let mut asked = request(&path, &csv, 1);
        asked.changed = ChangedFile::Replace;
        let saved = save_csv_file(&RwLock::new(load.workbook), &asked, &AtomicBool::new(false));
        assert!(matches!(saved, Err(EngineError::Rejected(_))));
        assert_eq!(std::fs::read(&path).unwrap(), unreadable);
    }

    #[test]
    fn a_document_whose_options_were_never_found_reads_them_at_save_or_fails_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "book.csv", b"a;b\r\n1,5;2\r\n");
        let csv = load.csv.take().unwrap();
        let shared = RwLock::new(load.workbook);
        let cancel = AtomicBool::new(false);
        let mut asked = request(&path, &csv, 1);
        asked.binding = CsvBinding::unresolved();
        asked.changed = ChangedFile::Replace;

        let unreadable = vec![b'x'; 40_000];
        std::fs::write(&path, &unreadable).unwrap();
        assert!(matches!(
            save_csv_file(&shared, &asked, &cancel),
            Err(EngineError::Rejected(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), unreadable);

        std::fs::remove_file(&path).unwrap();
        assert!(save_csv_file(&shared, &asked, &cancel).is_err());
        assert!(!path.exists());

        std::fs::write(&path, b"a;b\r\n1,5;2\r\n").unwrap();
        let Ok(CsvSave::Written(_, found)) = save_csv_file(&shared, &asked, &cancel) else {
            panic!("a readable file gives its options");
        };
        assert_eq!(found.delimiter, Delimiter::Semicolon);
        assert_eq!(std::fs::read(&path).unwrap(), b"a;b\r\n1,5;2\r\n");
    }

    #[test]
    fn a_thousand_cells_are_compared_without_the_engine() {
        let options = CsvOptions::exported_as(Delimiter::Comma);
        let mut file = vec![vec!["1.50".to_string(); 40]; 25];
        let mut unedited = 0;
        for n in 0..1_000 {
            let mut text = "1.5".to_string();
            let field = field_at(&mut file, at(n / 40, n % 40));
            unedited += usize::from(take_unedited(&mut text, "1.5", field, &options));
            assert_eq!(text, "1.50");
        }
        assert_eq!(unedited, 1_000);
    }

    fn found_by(shared: &RwLock<Workbook>, asked: &CsvSaveRequest) -> Option<Losses> {
        match save_csv_file(shared, asked, &AtomicBool::new(false)).unwrap() {
            CsvSave::WouldLose(found) => Some(found),
            _ => None,
        }
    }

    #[test]
    fn each_kind_of_loss_is_found_and_asked_until_it_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "book.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        asked.changed = ChangedFile::Replace;
        assert_eq!(found_by(&shared, &asked), None);

        let formatting = Losses {
            formatting: true,
            ..Losses::default()
        };
        asked.binding.introduced = formatting;
        assert_eq!(found_by(&shared, &asked), Some(formatting));

        asked.binding.accepted = formatting;
        assert_eq!(found_by(&shared, &asked), None);

        shared
            .write()
            .unwrap()
            .set_input(SheetId(0), at(0, 2), "=1+1")
            .unwrap();
        let both = Losses {
            formulas: true,
            ..formatting
        };
        assert_eq!(found_by(&shared, &asked), Some(both));

        asked.binding.accepted = both;
        asked.sheets = 2;
        assert_eq!(
            found_by(&shared, &asked),
            Some(Losses {
                sheets: true,
                ..both
            })
        );

        asked.binding.accepted = Losses {
            sheets: true,
            ..both
        };
        asked.binding.introduced.number_formats = true;
        assert!(found_by(&shared, &asked).is_some_and(|found| found.number_formats));
        asked.binding.accepted.number_formats = true;
        assert_eq!(found_by(&shared, &asked), None);
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b,2\n1,2\n");
    }

    #[test]
    fn a_save_that_would_lose_something_writes_nothing_until_it_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "book.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(0, 2), "=1+1")
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let cancel = AtomicBool::new(false);
        let mut asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.formulas));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b\n1,2\n");

        asked.binding.accepted.formulas = true;
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b,2\n1,2\n");
    }

    #[test]
    fn a_file_changed_on_disk_is_not_overwritten_unless_replacing_is_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "book.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        let shared = RwLock::new(load.workbook);
        std::fs::write(&path, b"changed by someone else\n").unwrap();
        let cancel = AtomicBool::new(false);
        let mut asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::FileChanged));
        assert_eq!(std::fs::read(&path).unwrap(), b"changed by someone else\n");

        asked.changed = ChangedFile::Replace;
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b\n1,2\n");
    }

    #[test]
    fn text_the_encoding_cannot_hold_fails_and_leaves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"caf\xe9;x\n";
        let (path, mut load) = opened_csv(dir.path(), "latin.csv", original);
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(0, 1), "\u{65e5}\u{672c}")
            .unwrap();
        let saved = save_csv_file(
            &RwLock::new(load.workbook),
            &request(&path, &csv, 1),
            &AtomicBool::new(false),
        );
        assert!(matches!(saved, Err(EngineError::Rejected(_))));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn a_field_the_engine_reads_as_a_formula_is_asked_about_not_written_over_silently() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"phone\n+34 600\n";
        let (path, mut load) = opened_csv(dir.path(), "phones.csv", original);
        let csv = load.csv.take().unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::WouldLose(found) if found.formulas));
        assert_eq!(bytes, original);
    }

    #[test]
    fn a_number_is_written_without_grouping_and_with_the_files_decimal_mark() {
        assert_eq!(number_text("1,234.50", DecimalMark::Dot), "1234.50");
        assert_eq!(number_text("-0.25%", DecimalMark::Comma), "-0,25%");
        assert_eq!(number_text("v1.2.3", DecimalMark::Comma), "v1.2.3");
    }

    #[test]
    fn a_decimal_comma_number_is_a_plain_decimal_the_import_reads_as_the_same_number() {
        let cases = [
            ("$1,234.50", 1234.5),
            ("1.5E+20", 1.5e20),
            ("-12%", -0.12),
            ("1,234", 1234.0),
            ("0.0000001", 1e-7),
        ];
        for (shown, value) in cases {
            let written = number_text(shown, DecimalMark::Comma);
            assert!(!written.contains(['$', 'E', 'e']), "{written}");
            let read = decimal_comma_field(&written).unwrap_or_else(|| written.clone());
            let read = number_value(&read).unwrap();
            assert!(
                (read - value).abs() <= 1e-12 * value.abs(),
                "{shown} {written}"
            );
        }
    }

    #[test]
    fn a_date_typed_into_a_month_first_file_is_written_as_a_date_and_not_as_its_serial() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "us.csv", b"d,n\n10/25/2026,1.5\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(2, 0), "10/8/2026")
            .unwrap();
        let serial = value_at(&load.workbook, at(2, 0)).unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("d,n\n10/25/2026,1.5\n"), "{text}");
        assert!(!text.contains("46303"), "{text}");
        let reopened = csv_io::load_csv(&path, None).unwrap();
        assert_eq!(value_at(&reopened.workbook, at(2, 0)), Some(serial));
    }

    #[test]
    fn formula_cells_are_written_as_what_they_show_and_only_those_written_from_the_sheet_count() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "f.csv", b"a\n=1+1\n");
        let csv = load.csv.take().unwrap();
        let shared = RwLock::new(load.workbook);
        let cancel = AtomicBool::new(false);
        let mut asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a\n=1+1\n");

        {
            let mut workbook = shared.write().unwrap();
            workbook.set_input(SheetId(0), at(1, 1), "=A2*10").unwrap();
            workbook
                .set_input(SheetId(0), at(1, 2), "=\"x\"&\"y\"")
                .unwrap();
        }
        asked.changed = ChangedFile::Replace;
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.formulas));

        asked.binding.accepted.formulas = true;
        save_csv_file(&shared, &asked, &cancel).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"a\n=1+1,20,xy\n");
    }

    #[test]
    fn an_edited_row_never_loses_fields() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "wide.csv", b"1,2,\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(0, 0), "9").unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"9,2,\n");
    }
}
