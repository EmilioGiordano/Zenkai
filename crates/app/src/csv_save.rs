use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use zenkai_engine::{EngineError, FileCell, FileCells, FormatEdits, ShownCells, Workbook};
use zenkai_formats::{
    FinalNewline, date_serial, iso_date, neutralize, number_text, stored_number_text,
};
use zenkai_i18n::t;
use zenkai_types::{CellPos, LoggedChange, MAX_ROWS, RowIdx, SheetChange, SheetId};

use crate::csv_export_size::ExportSize;
use crate::csv_io::{self, AsRead, BaselineRead, Layout, Shape};
use crate::csv_options::{CsvBinding, CsvOptions, Losses};
use crate::document::{ChangedFile, file_changed_since, read_shared};
use crate::edit_log::Walk;
use crate::entry::FileSeen;

const CANCEL_CHECK_EVERY: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LargeWrite {
    Ask,
    // The size the user agreed to; a write that grows past it asks again.
    Confirmed(u64),
}

pub struct CsvSaveRequest {
    pub path: PathBuf,
    pub binding: CsvBinding,
    pub sheets: usize,
    pub sheet: SheetId,
    pub changed: ChangedFile,
    pub large: LargeWrite,
    pub guard: Option<FileSeen>,
}

pub enum CsvSave {
    // The file now holds the sheet as the engine's counters, which the save read with its cells,
    // describe.
    // The key of the sheet it holds is the last.
    Written(FileSeen, CsvOptions, FormatEdits, u32),
    // Nothing was written, from here on: the user decides, or the file is not ours to write.
    WouldLose(Losses),
    TooLarge(ExportSize),
    FileChanged,
    // Decoding replaced bytes, so writing the file back would change it.
    LossyFile,
    // The file's empty lines could not be placed among its rows.
    ShapeUnknown,
}

fn field_at(rows: &mut [Vec<String>], pos: CellPos) -> Option<&mut String> {
    let row = usize::try_from(pos.row.get()).ok()?;
    rows.get_mut(row)?.get_mut(usize::from(pos.col.get()))
}

// What the cell shows if the file's reader takes it as the same date, else yyyy-mm-dd. A date
// with a time has no text the engine reads back, so it is written as its serial.
fn date_text(shown: &str, serial: f64, options: &CsvOptions) -> String {
    if date_serial(shown, options.date_order) == Some(serial) {
        return shown.to_string();
    }
    iso_date(serial).unwrap_or_else(|| number_text(&serial.to_string(), options.decimal_mark))
}

// The text of a number the user wrote, so that reading the file gives the same value back.
fn edited_number_text(
    shown: &str,
    stored: f64,
    formula: bool,
    date: Option<f64>,
    options: &CsvOptions,
) -> String {
    if let Some(serial) = date {
        return date_text(shown, serial, options);
    }
    number_text(
        &stored_number_text(shown, stored, formula),
        options.decimal_mark,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormulaCells {
    None,
    // A cell the user wrote holds a formula, which the CSV keeps as its value.
    Written,
}

struct Classified {
    shown: ShownCells,
    formulas: FormulaCells,
}

// A cell nobody wrote since the file was read takes the text of its own field, wherever the
// rows and columns inserted or deleted since have put it; every other cell takes what it shows.
// With a log that cannot be trusted every cell takes what it shows. Needs no lock: the sheet
// was read already.
fn classify(
    found: FileCells,
    walk: &Walk,
    trusted: bool,
    options: &CsvOptions,
    file: &mut [Vec<String>],
    cancel: &AtomicBool,
) -> Result<Classified, EngineError> {
    let FileCells {
        end,
        cells,
        silent,
        spilled,
        ..
    } = found;
    // The references of a formula the file holds were rewritten by the rows and columns moved, so
    // its text is no longer the sheet's.
    let moved_lines = walk.moves_rows() || walk.moves_columns();
    let mut own_field = |pos: CellPos| -> Option<String> {
        if !trusted {
            return None;
        }
        let origin = walk.file_position(pos)?;
        let field = field_at(file, origin)
            .filter(|field| !field.is_empty() && !(moved_lines && field.starts_with('=')))?;
        Some(std::mem::take(field))
    };
    let anchors: HashSet<CellPos> = spilled.iter().map(|(_, anchor)| *anchor).collect();
    let mut matched: HashSet<CellPos> = HashSet::new();
    let mut formulas = FormulaCells::None;
    let mut out: Vec<(CellPos, String)> = Vec::with_capacity(cells.len());
    for (checked, cell) in cells.into_iter().enumerate() {
        if checked.is_multiple_of(CANCEL_CHECK_EVERY) && cancel.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        if let Some(text) = own_field(cell.pos) {
            if anchors.contains(&cell.pos) {
                matched.insert(cell.pos);
            }
            out.push((cell.pos, text));
            continue;
        }
        if cell.unreadable {
            return Err(EngineError::Rejected(t!(
                "csv.unreadable_cell",
                cell = cell.pos
            )));
        }
        let formula = cell.input().starts_with('=');
        if formula {
            formulas = FormulaCells::Written;
        }
        let spilled_value = !spilled.is_empty()
            && spilled
                .binary_search_by_key(&(cell.pos.row, cell.pos.col), |(child, _)| {
                    (child.row, child.col)
                })
                .is_ok();
        let FileCell {
            pos,
            text,
            number,
            date,
            ..
        } = cell;
        let text = match number {
            Some(number) => edited_number_text(&text, number, formula, date, options),
            // The result of a formula is not what the user typed, so a text one is made safe
            // for another program; typed text stays as it is.
            None if formula || spilled_value => neutralize(&text).into_owned(),
            None => text,
        };
        out.push((pos, text));
    }
    // Cells that show nothing are not among the shown ones: a field of the file they still hold
    // is kept, and a formula the user wrote that shows nothing has no text to write.
    for (checked, (pos, input)) in silent.iter().enumerate() {
        if checked.is_multiple_of(CANCEL_CHECK_EVERY) && cancel.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        match own_field(*pos) {
            Some(text) => {
                if anchors.contains(pos) {
                    matched.insert(*pos);
                }
                out.push((*pos, text));
            }
            None if input.starts_with('=') => formulas = FormulaCells::Written,
            None => {}
        }
    }
    out.sort_unstable_by_key(|(pos, _)| (pos.row, pos.col));
    // What a formula the file holds spilled is the formula's own, so it is not written; of a
    // formula the user wrote, the spilled cells are values like any other.
    let mut end = end;
    if !spilled.is_empty() {
        out.retain(|(pos, _)| {
            let Ok(at) = spilled
                .binary_search_by_key(&(pos.row, pos.col), |(child, _)| (child.row, child.col))
            else {
                return true;
            };
            if matched.contains(&spilled[at].1) {
                return false;
            }
            end = CellPos::new(end.row.max(pos.row), end.col.max(pos.col));
            true
        });
    }
    Ok(Classified {
        shown: ShownCells { end, cells: out },
        formulas,
    })
}

pub fn save_csv_file(
    shared: &RwLock<Workbook>,
    request: &CsvSaveRequest,
    cancel: &AtomicBool,
) -> Result<CsvSave, EngineError> {
    let options = match &request.binding.options {
        Some(options) => options.clone(),
        None => csv_io::detect_csv_options(&request.path).map_err(EngineError::Rejected)?,
    };
    let baseline = match csv_io::read_baseline(&request.path, options.delimiter)? {
        BaselineRead::Present(baseline) => Some(baseline),
        BaselineRead::Absent => None,
        BaselineRead::Lossy => return Ok(CsvSave::LossyFile),
        BaselineRead::ShapeUnknown => return Ok(CsvSave::ShapeUnknown),
    };
    // Checked after the read, so the file compared with is the one that was checked. One the user
    // chose to replace is not the file the sheet came from, so none of its fields is the sheet's.
    let changed = file_changed_since(request.guard, &request.path);
    if request.changed == ChangedFile::Ask && changed {
        return Ok(CsvSave::FileChanged);
    }
    let present = baseline.is_some();
    let (mut file, final_newline) = baseline
        .map_or((Vec::new(), FinalNewline::Present), |baseline| {
            (baseline.rows, baseline.final_newline)
        });
    let row_lengths: Vec<usize> = file.iter().map(Vec::len).collect();
    let last_content_row = file
        .iter()
        .rposition(|row| row.iter().any(|field| !field.is_empty()))
        .and_then(|last| RowIdx::new(u32::try_from(last).ok()?));
    // The binding is as of the last batch, which could not read the engine while a
    // recalculation held the workbook; the sheet, the counters and what was logged since are
    // read now, together, in one short look.
    let known = request.binding.seen_edits;
    let mut found = read_shared(shared).file_cells(request.sheet, known.seq)?;
    let (edits, sheet_key) = (found.edits, found.sheet_key);
    let mut pending: Vec<LoggedChange> = request.binding.log.changes().collect();
    let mut unknown = request.binding.log.has_lost();
    match found.changes.take() {
        Some(more) => pending.extend(more),
        None => unknown = true,
    }
    unknown |= pending
        .iter()
        .any(|logged| logged.change == SheetChange::Unknown);
    let walk = Walk::new(pending, sheet_key);
    let own_sheet = request.binding.file_sheet == Some(sheet_key);
    let own_fields = !unknown && !changed && own_sheet;
    let classified = classify(found, &walk, own_fields, &options, &mut file, cancel)?;
    drop(file);
    let losses = Losses {
        sheets: request.sheets > 1,
        formulas: classified.formulas == FormulaCells::Written,
        number_formats: request.binding.introduced.number_formats || edits.numbers > known.numbers,
        formatting: request.binding.introduced.formatting || edits.other > known.other,
        moved_cells: request.binding.introduced.moved_cells || edits.moved > known.moved || unknown,
    };
    if losses.beyond(request.binding.accepted).any() {
        return Ok(CsvSave::WouldLose(losses));
    }
    let content_rows = classified.shown.content_rows();
    let file_rows = u32::try_from(row_lengths.len()).unwrap_or(u32::MAX);
    // Rows inserted can carry the end of the file past the last row a sheet has.
    let mapped_rows = walk.current_end(file_rows).min(MAX_ROWS);
    let mapped_content = last_content_row
        .map_or(0, |last| walk.current_end(last.get() + 1))
        .min(MAX_ROWS);
    let widest = row_lengths.iter().copied().max().unwrap_or(0);
    // Rows keep the length their file row had when only rows moved, and the file's empty rows
    // at its end stay where they are now; with columns moved, or a file that is gone or that
    // the log cannot be followed in, every row is as wide as the used range.
    let mapped: Vec<usize>;
    let shape = if !present || !own_fields {
        Shape::UsedRange { rows_kept: 0 }
    } else if walk.moves_columns() {
        Shape::UsedRange {
            rows_kept: if content_rows >= mapped_content {
                mapped_rows
            } else {
                0
            },
        }
    } else if !walk.moves_rows() {
        Shape::File {
            row_lengths: &row_lengths,
            last_content_row,
        }
    } else {
        let rows = (classified.shown.end.row.get() + 1)
            .max(mapped_rows)
            .min(MAX_ROWS);
        mapped = (0..rows)
            .map(|row| {
                RowIdx::new(row)
                    .and_then(|row| walk.file_row(row))
                    .and_then(|file_row| usize::try_from(file_row.get()).ok())
                    .and_then(|index| row_lengths.get(index).copied())
                    .unwrap_or(widest)
            })
            .collect();
        Shape::File {
            row_lengths: &mapped,
            last_content_row: mapped_content.checked_sub(1).and_then(RowIdx::new),
        }
    };
    let layout = Layout::AsRead(AsRead {
        shape,
        final_newline,
    });
    let size = ExportSize::of_layout(&classified.shown, &layout);
    let agreed = matches!(request.large, LargeWrite::Confirmed(bytes) if size.bytes <= bytes);
    if size.needs_confirmation() && !agreed {
        return Ok(CsvSave::TooLarge(size));
    }
    if classified.shown.content_rows() > 0
        && csv_io::last_row_to_write(&classified.shown, &layout).is_none()
    {
        return Err(EngineError::Rejected(t!("save.csv_no_rows").to_string()));
    }
    csv_io::write_csv_as(&request.path, &classified.shown, &options, &layout, cancel)?;
    Ok(CsvSave::Written(
        FileSeen::of(&request.path),
        options,
        edits,
        sheet_key,
    ))
}

#[cfg(test)]
mod text_columns_tests;

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use zenkai_engine::Engine;
    use zenkai_formats::Delimiter;
    use zenkai_types::{ColIdx, Range, ValueKind};

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
            binding: CsvBinding::new(csv.options.clone()).with_sheet(csv.sheet),
            sheets,
            sheet: SheetId(0),
            changed: ChangedFile::Ask,
            large: LargeWrite::Ask,
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
    fn fields_that_show_nothing_are_kept_while_the_engine_still_holds_them() {
        assert_unchanged("empty.csv", b"a,\"=\"\"\"\"\"\n");
        assert_unchanged("quote.csv", b"a,'\n");
        assert_unchanged("if.csv", b"1,\"=IF(A1>5,\"\"x\"\",\"\"\"\")\"\n");
    }

    #[test]
    fn what_a_formula_spilled_is_not_written_into_the_file() {
        assert_unchanged("spill.csv", b"=SEQUENCE(3)\n");
    }

    #[test]
    fn a_formula_that_shows_nothing_is_asked_about_when_typed() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "typed.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(1, 2), "=\"\"")
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::WouldLose(found) if found.formulas));
        assert_eq!(bytes, b"a,b\n1,2\n");
    }

    #[test]
    fn a_long_id_edited_in_its_last_digit_is_written_whatever_its_length() {
        let dir = tempfile::tempdir().unwrap();
        for (original, edit, expected) in [
            ("1234567890123456", "1234567890123457", "1234567890123457"),
            (
                "12345678901234567",
                "12345678901234580",
                "12345678901234580",
            ),
        ] {
            let file = format!("id\n{original}\n");
            let (path, mut load) = opened_csv(dir.path(), "id.csv", file.as_bytes());
            let csv = load.csv.take().unwrap();
            let (saved, bytes) = save(&path, load.workbook, &csv);
            assert!(matches!(saved, CsvSave::Written(..)));
            assert_eq!(bytes, file.as_bytes(), "{original} unedited");

            let (path, mut load) = opened_csv(dir.path(), "id2.csv", file.as_bytes());
            let csv = load.csv.take().unwrap();
            load.workbook.set_input(SheetId(0), at(1, 0), edit).unwrap();
            let (saved, bytes) = save(&path, load.workbook, &csv);
            assert!(matches!(saved, CsvSave::Written(..)));
            assert_eq!(bytes, format!("id\n{expected}\n").as_bytes(), "{edit}");
        }
    }

    #[test]
    fn a_currency_the_engine_keeps_as_text_is_not_taken_for_the_number() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "pound.csv", "p\n\u{a3}5\n".as_bytes());
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(1, 0), "5").unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"p\n5\n");

        let (path, mut load) = opened_csv(dir.path(), "seven.csv", b"p\n7\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(1, 0), "\u{a3}7")
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, "p\n\u{a3}7\n".as_bytes());
    }

    #[test]
    fn an_edit_the_last_batch_could_not_count_is_found_when_the_save_reads_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "late.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_column_width(SheetId(0), ColIdx::clamped(0), 120.0)
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.formatting));
    }

    #[test]
    fn a_crafted_wide_row_does_not_make_a_save_write_gigabytes_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let wide = format!("x{}\n", ",".repeat(15_999));
        let (path, mut load) = opened_csv(dir.path(), "wide.csv", wide.as_bytes());
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(1_000_000, 0), "y")
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        asked.changed = ChangedFile::Replace;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::TooLarge(_)));
        assert_eq!(std::fs::read(&path).unwrap(), wide.as_bytes());

        asked.large = LargeWrite::Confirmed(1);
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::TooLarge(_)));
    }

    #[test]
    fn a_field_the_user_cleared_is_not_brought_back() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "cleared.csv", b"a,\"=\"\"\"\"\"\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(0, 1), "").unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"a,\n");
    }

    #[test]
    fn formulas_the_engine_writes_back_normalized_are_unedited_and_no_loss() {
        assert_unchanged("lower.csv", b"1,2,=sum(a1:b1)\n");
        assert_unchanged("spaces.csv", b"=1 + 1,\"=\"\"a b\"\"&\"\"c\"\"\"\n");
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
            let mut asked = request(&path, &csv, 1);
            asked.binding.accepted.number_formats = true;
            let saved = save_csv_file(&RwLock::new(load.workbook), &asked, &AtomicBool::new(false))
                .unwrap();
            let bytes = std::fs::read(&path).unwrap();
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
    fn cells_moved_by_inserting_a_row_are_still_written_as_their_own_field() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"a,b\n007,\"1,234.50\"\n1234567890123456789,x\n";
        let (path, mut load) = opened_csv(dir.path(), "insert.csv", original);
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(1), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(
            bytes,
            b"a,b\n,\n007,\"1,234.50\"\n1234567890123456789,x\n".as_slice()
        );
    }

    #[test]
    fn after_rows_move_each_row_keeps_the_length_of_its_file_row() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "moved.csv", b"a,b,\n1,2,\n3\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(1), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"a,b,\n,,\n1,2,\n3\n");
    }

    #[test]
    fn deleting_a_column_leaves_the_fields_right_of_it_as_the_file_had_them() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "cols.csv", b"a,b,c\n1,2,007\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .delete_columns(SheetId(0), ColIdx::clamped(0), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"b,c\n2,007\n");
    }

    #[test]
    fn the_empty_lines_a_file_ends_with_stay_at_its_end_after_rows_or_columns_move() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "tail.csv", b"a,b\n1,2\n\n\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(0), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b",\na,b\n1,2\n\n\n");

        let (path, mut load) = opened_csv(dir.path(), "tail2.csv", b"a,b,c\n1,2,3\n\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .delete_columns(SheetId(0), ColIdx::clamped(0), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"b,c\n2,3\n\n");
    }

    #[test]
    fn a_copy_of_the_sheet_and_a_new_sheet_have_none_of_the_files_fields() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "copy.csv", b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(0, 0), "99").unwrap();
        load.workbook.duplicate_sheet(SheetId(0)).unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 2);
        asked.sheet = SheetId(1);
        asked.binding.accepted.sheets = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.moved_cells));
        asked.binding.accepted.moved_cells = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"99,b\n1,2\n");

        let (path, mut load) = opened_csv(dir.path(), "fresh.csv", b"a\n007\n");
        let csv = load.csv.take().unwrap();
        let fresh = load.workbook.add_sheet().unwrap();
        load.workbook.set_input(fresh, at(0, 0), "new").unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 2);
        asked.sheet = fresh;
        asked.binding.accepted.sheets = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"new\n");
    }

    #[test]
    fn a_formula_of_the_file_is_written_from_the_sheet_once_rows_moved_its_references() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "refs.csv", b"1,x\n2,y\n=A1+A2,z\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(0), 1)
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.formulas));
        assert_eq!(std::fs::read(&path).unwrap(), b"1,x\n2,y\n=A1+A2,z\n");
        asked.binding.accepted.formulas = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b",\n1,x\n2,y\n3,z\n");
    }

    #[test]
    fn the_empty_lines_at_the_end_survive_deleting_the_last_content_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "gone.csv", b"a\nb\nc\n\n\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .delete_rows(SheetId(0), RowIdx::clamped(2), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"a\nb\n\n\n");
    }

    #[test]
    fn sorting_cells_asks_before_they_are_written_as_they_show() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "sort.csv", b"n\n3\n1\n2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .sort(
                SheetId(0),
                Range::new(at(1, 0), at(3, 0)),
                ColIdx::clamped(0),
                false,
            )
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.moved_cells));
        assert_eq!(std::fs::read(&path).unwrap(), b"n\n3\n1\n2\n");

        asked.binding.accepted.moved_cells = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"n\n1\n2\n3\n");
    }

    #[test]
    fn a_log_that_could_not_keep_every_change_makes_the_save_write_cells_as_they_show() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "many.csv", b"a\n007\n");
        let csv = load.csv.take().unwrap();
        for _ in 0..600 {
            load.workbook
                .insert_rows(SheetId(0), RowIdx::clamped(5), 1)
                .unwrap();
            load.workbook
                .delete_rows(SheetId(0), RowIdx::clamped(9), 1)
                .unwrap();
        }
        let mut binding = CsvBinding::new(csv.options.clone()).with_sheet(csv.sheet);
        binding.note(load.workbook.format_edits(), load.workbook.changes_since(0));
        assert!(binding.log.has_lost());
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        asked.binding = binding;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.moved_cells));
        asked.binding.accepted.moved_cells = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a\n7\n");
    }

    #[test]
    fn a_typed_dynamic_array_is_written_as_the_values_it_shows() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "array.csv", b"a\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(0, 1), "=SEQUENCE(3)")
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::WouldLose(found) if found.formulas));
        asked.binding.accepted.formulas = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,1\n,2\n,3\n");
    }

    #[test]
    fn the_result_of_an_edited_formula_that_looks_like_a_formula_is_made_safe() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "safe.csv", b"a\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(0, 1), "=\"-\"&\"x\"")
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        asked.binding.accepted.formulas = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,'-x\n");
    }

    #[test]
    fn a_deleted_file_is_written_with_every_row_as_wide_as_the_used_range() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "gone.csv", b"a,b,c\nx\n");
        let csv = load.csv.take().unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut asked = request(&path, &csv, 1);
        asked.changed = ChangedFile::Replace;
        let outcome = save_csv_file(&RwLock::new(load.workbook), &asked, &AtomicBool::new(false));
        assert!(matches!(outcome, Ok(CsvSave::Written(..))));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b,c\nx,,\n");
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
        let Ok(CsvSave::Written(_, found, _, _)) = save_csv_file(&shared, &asked, &cancel) else {
            panic!("a readable file gives its options");
        };
        assert_eq!(found.delimiter, Delimiter::Semicolon);
        assert_eq!(std::fs::read(&path).unwrap(), b"a;b\r\n1,5;2\r\n");
    }

    fn found_by(shared: &RwLock<Workbook>, asked: &CsvSaveRequest) -> Option<Losses> {
        let asked = &CsvSaveRequest {
            path: asked.path.clone(),
            binding: asked.binding.clone(),
            sheets: asked.sheets,
            sheet: asked.sheet,
            changed: ChangedFile::Ask,
            large: asked.large,
            guard: Some(FileSeen::of(&asked.path)),
        };
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
    fn a_field_the_engine_reads_as_a_formula_is_written_as_it_was_while_nobody_edits_it() {
        assert_unchanged("phones.csv", b"phone\n+34 600\n");
    }

    #[test]
    fn typing_over_a_field_writes_what_was_typed_and_the_cells_around_stay_as_they_were() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "seven.csv", b"a\n007\n008\n1.50\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(1, 0), "7").unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"a\n7\n008\n1.50\n");
    }

    #[test]
    fn a_pasted_range_is_written_as_shown_and_what_is_outside_it_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "paste.csv", b"n\n007\n008\n009\n");
        let csv = load.csv.take().unwrap();
        let copied = load
            .workbook
            .copy(SheetId(0), Range::single(at(2, 0)))
            .unwrap();
        load.workbook
            .paste(SheetId(0), at(3, 0), &copied, false)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, b"n\n007\n008\n8\n");
    }

    #[test]
    fn once_another_sheet_was_saved_over_the_file_the_first_has_none_of_its_fields() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "two.csv", b"a\n007\n");
        let csv = load.csv.take().unwrap();
        let other = load.workbook.add_sheet().unwrap();
        load.workbook.set_input(other, at(0, 0), "bee").unwrap();
        let shared = RwLock::new(load.workbook);
        let mut binding = CsvBinding::new(csv.options.clone()).with_sheet(csv.sheet);
        binding.accepted.sheets = true;

        let mut asked = request(&path, &csv, 2);
        asked.binding = binding.clone();
        asked.sheet = other;
        let CsvSave::Written(_, _, consumed, sheet) =
            save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap()
        else {
            panic!("the other sheet should be written");
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"bee\n");
        binding.saved(consumed, sheet);

        shared
            .write()
            .unwrap()
            .set_input(SheetId(0), at(0, 0), "alpha")
            .unwrap();
        let mut asked = request(&path, &csv, 2);
        asked.binding = binding;
        asked.guard = Some(FileSeen::of(&path));
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"alpha\n7\n");
    }

    #[test]
    fn a_file_of_the_most_rows_keeps_them_all_when_rows_are_inserted_above() {
        let dir = tempfile::tempdir().unwrap();
        let rows = MAX_ROWS as usize;
        let original = format!("a\n{}", "\n".repeat(rows - 1));
        let (path, mut load) = opened_csv(dir.path(), "most.csv", original.as_bytes());
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(0), 10)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        let top = format!("{}a\n", "\"\"\n".repeat(10));
        assert!(bytes.starts_with(top.as_bytes()));
        assert_eq!(bytes.len(), top.len() + (rows - 11));
        assert!(bytes[top.len()..].iter().all(|byte| *byte == b'\n'));
    }

    pub(crate) const SALES: &[u8] = b"id;nombre;precio;fecha\r\n007;Caf\xe9;1.234,50;25/10/2026\r\n9780201379624;T\xe9;12,345%;08/01/2026\r\n00123;Pan;3,5;31/12/2026\r\n";

    #[test]
    fn a_file_opened_through_the_preview_saves_without_a_prompt_and_without_a_byte_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ventas.csv");
        std::fs::write(&path, SALES).unwrap();
        let read = csv_io::read_csv(&path).unwrap();
        let guessed = zenkai_formats::guess(&read.parsed);
        let options = CsvOptions::from_read(&read.parsed, &guessed);
        let mut rows = read.parsed.rows;
        zenkai_formats::apply(
            options.decimal_mark,
            options.date_order,
            &options.text,
            &mut rows,
        );
        let workbook = csv_io::workbook_from_rows(rows, &options.text).unwrap();
        let mut document = crate::document::Document::new(
            zenkai_types::WorkbookId(0),
            crate::spaces::SpaceId(0),
            1,
            workbook,
            Some(path.clone()),
            Vec::new(),
        );
        document.bind_csv(options);
        document.guard_file(read.seen);
        let generation = document.generation();
        assert!(document.finish_batch(generation));
        let binding = document.csv.clone().unwrap();
        assert!(!binding.introduced.any());
        assert!(binding.log.is_empty());
        let asked = CsvSaveRequest {
            path: path.clone(),
            binding,
            sheets: 1,
            sheet: SheetId(0),
            changed: ChangedFile::Ask,
            large: LargeWrite::Ask,
            guard: document.file_guard(),
        };
        let shared = document.shared_workbook();
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), SALES);
    }

    #[test]
    fn a_formula_rewritten_by_a_cut_is_not_written_as_the_file_had_it() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "cut.csv", b"5,x\n=A1,y\n");
        let csv = load.csv.take().unwrap();
        let mut workbook = load.workbook;
        let copied = workbook.copy(SheetId(0), Range::single(at(0, 0))).unwrap();
        workbook.paste(SheetId(0), at(4, 2), &copied, true).unwrap();
        let shared = RwLock::new(workbook);
        let mut asked = request(&path, &csv, 1);
        let found = found_by(&shared, &asked).unwrap();
        assert!(found.moved_cells);
        asked.binding.accepted = found;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        let written = match outcome {
            CsvSave::Written(..) => std::fs::read_to_string(&path).unwrap(),
            CsvSave::WouldLose(more) => {
                asked.binding.accepted = Losses {
                    moved_cells: true,
                    ..more
                };
                save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
                std::fs::read_to_string(&path).unwrap()
            }
            _ => panic!("the save should write once the losses are accepted"),
        };
        assert!(!written.contains("=A1"), "{written}");
        assert_eq!(written, ",x,\n5,y,\n,,\n,,\n,,5\n");
    }

    #[test]
    fn a_reordered_sheet_keeps_its_own_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "sheets.csv", b"a\n007\n008\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(2, 0), "9").unwrap();
        load.workbook.add_sheet().unwrap();
        load.workbook.move_sheet(SheetId(0), 1).unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 2);
        asked.sheet = SheetId(1);
        asked.binding.accepted.sheets = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a\n007\n9\n");
    }

    #[test]
    fn an_edit_made_while_the_file_was_written_is_not_forgotten_and_nothing_is_written_twice() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"a,b\n007,x\n008,y\n010,z\n";
        let (path, mut load) = opened_csv(dir.path(), "meanwhile.csv", original);
        let csv = load.csv.take().unwrap();
        let shared = RwLock::new(load.workbook);
        let cancel = AtomicBool::new(false);
        let mut binding = CsvBinding::new(csv.options.clone()).with_sheet(csv.sheet);
        let batch = |binding: &mut CsvBinding| {
            let workbook = shared.read().unwrap();
            let seen = binding.seen_edits.seq;
            binding.note(workbook.format_edits(), workbook.changes_since(seen));
        };
        let save = |binding: &CsvBinding| {
            let mut asked = request(&path, &csv, 1);
            asked.binding = binding.clone();
            asked.guard = Some(FileSeen::of(&path));
            match save_csv_file(&shared, &asked, &cancel).unwrap() {
                CsvSave::Written(_, _, consumed, sheet) => (consumed, sheet),
                _ => panic!("the save should write"),
            }
        };

        shared
            .write()
            .unwrap()
            .set_input(SheetId(0), at(2, 0), "9")
            .unwrap();
        batch(&mut binding);
        let (consumed, sheet) = save(&binding);
        shared
            .write()
            .unwrap()
            .set_input(SheetId(0), at(3, 0), "11")
            .unwrap();
        batch(&mut binding);
        binding.saved(consumed, sheet);
        shared
            .write()
            .unwrap()
            .insert_rows(SheetId(0), RowIdx::clamped(1), 1)
            .unwrap();
        batch(&mut binding);
        let expected = b"a,b\n,\n007,x\n9,y\n11,z\n";
        for _ in 0..2 {
            let (consumed, sheet) = save(&binding);
            binding.saved(consumed, sheet);
            assert_eq!(std::fs::read(&path).unwrap(), expected);
        }
        assert!(binding.log.is_empty());
    }

    #[test]
    fn negative_and_percent_formula_results_are_written_as_numbers_in_the_files_decimal_mark() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "comma.csv", b"a;b\n1,5;2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(SheetId(0), at(1, 2), "=-A2")
            .unwrap();
        load.workbook
            .set_input(SheetId(0), at(1, 3), "=-A2/10")
            .unwrap();
        load.workbook
            .set_number_format(SheetId(0), Range::single(at(1, 3)), "0%")
            .unwrap();
        let shared = RwLock::new(load.workbook);
        let mut asked = request(&path, &csv, 1);
        asked.binding.accepted.formulas = true;
        asked.binding.accepted.number_formats = true;
        let outcome = save_csv_file(&shared, &asked, &AtomicBool::new(false)).unwrap();
        assert!(matches!(outcome, CsvSave::Written(..)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a;b\n1,5;2;-1,5;-15%\n");
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
        asked.guard = Some(FileSeen::of(&path));
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

    #[test]
    fn signed_text_opens_as_text_signed_numbers_stay_numbers_and_unedited_save_keeps_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"phone;dash;plus;neg\n+34 600;-;+5;-1,5\n";
        let (path, mut load) = opened_csv(dir.path(), "signed.csv", original);
        let csv = load.csv.take().unwrap();

        let phone = load.workbook.cell(SheetId(0), at(1, 0));
        assert_eq!(phone.text, "+34 600");
        assert_eq!(phone.kind, ValueKind::Text);
        assert_eq!(phone.number, None);

        let dash = load.workbook.cell(SheetId(0), at(1, 1));
        assert_eq!(dash.text, "-");
        assert_eq!(dash.kind, ValueKind::Text);
        assert_eq!(dash.number, None);

        let plus = load.workbook.cell(SheetId(0), at(1, 2));
        assert_eq!(plus.kind, ValueKind::Number);
        assert_eq!(plus.number, Some(5.0));

        let neg = load.workbook.cell(SheetId(0), at(1, 3));
        assert_eq!(neg.kind, ValueKind::Number);
        assert_eq!(neg.number, Some(-1.5));

        let (saved, bytes) = save(&path, load.workbook, &csv);
        assert!(matches!(saved, CsvSave::Written(..)));
        assert_eq!(bytes, original);
    }
}
