use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use zenkai_engine::{Engine, EngineError, ShownCells, Stamp, Workbook};
use zenkai_formats::{DateOrder, day_first_field, decimal_comma_field};
use zenkai_i18n::t;
use zenkai_types::{CellPos, CellStyle, ColIdx, Contents, RowIdx, SheetId, ValueKind};

use crate::csv_options::{CsvBinding, CsvOptions};
use crate::document::{ChangedFile, file_changed_since, read_shared};
use crate::entry::FileSeen;
use crate::files::{self, Baseline, FieldKind};

// Cells copied under the read lock at a time while looking for what a CSV cannot hold, so an
// edit waiting for the write lock is never kept waiting for a whole sheet.
const LOSS_CHUNK: usize = 20_000;

// What the engine's `shown_cells` writes for a cell it could not read.
const UNREADABLE_CELL: &str = "#ERROR";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CsvLoss {
    pub sheets: bool,
    pub formulas: bool,
    pub formatting: bool,
}

impl CsvLoss {
    pub fn any(self) -> bool {
        self.sheets || self.formulas || self.formatting
    }
}

// Number formats are not counted: reading a CSV gives dates and percentages their own, and the
// file holds what they show.
fn is_formatted(mut cell: CellStyle, plain: &CellStyle) -> bool {
    cell.num_fmt.clear();
    cell != *plain
}

// Walks the active sheet's cells, so it runs off the UI thread. Other sheets are lost whatever
// they hold, so with several the cells are not looked at.
pub fn csv_loss(shared: &RwLock<Workbook>, sheets: usize, sheet: SheetId) -> CsvLoss {
    let mut loss = CsvLoss {
        sheets: sheets > 1,
        ..CsvLoss::default()
    };
    if loss.sheets {
        return loss;
    }
    let (positions, plain) = {
        let workbook = read_shared(shared);
        let mut plain = workbook
            .cell(sheet, CellPos::new(RowIdx::LAST, ColIdx::LAST))
            .style;
        plain.num_fmt.clear();
        (workbook.filled_cells(sheet), plain)
    };
    for chunk in positions.chunks(LOSS_CHUNK) {
        let workbook = read_shared(shared);
        for &pos in chunk {
            loss.formulas = loss.formulas || workbook.input(sheet, pos).starts_with('=');
            loss.formatting =
                loss.formatting || is_formatted(workbook.cell(sheet, pos).style, &plain);
        }
        if loss.formulas && loss.formatting {
            break;
        }
    }
    loss
}

pub struct CsvSaveRequest {
    pub path: PathBuf,
    pub binding: CsvBinding,
    pub sheets: usize,
    pub sheet: SheetId,
    pub changed: ChangedFile,
    pub guard: Option<FileSeen>,
}

pub enum CsvSave {
    Written(FileSeen),
    // Nothing was written: the user decides whether the CSV keeps only the active sheet's values.
    WouldLose(CsvLoss),
    // Nothing was written: the file changed on disk since it was read or last saved.
    FileChanged,
    // Nothing was written: decoding the file replaced bytes, so writing it back would change it.
    LossyFile,
}

struct Snapshot {
    shown: ShownCells,
    // What the formula bar shows for each shown cell.
    inputs: Vec<String>,
    // Whether each shown cell holds a number; filled only for a decimal-comma file.
    numeric: Vec<bool>,
}

// One read lock for everything a save needs of the sheet, so it describes a single moment.
fn snapshot(
    shared: &RwLock<Workbook>,
    sheet: SheetId,
    decimal_comma: bool,
) -> Result<Snapshot, EngineError> {
    let workbook = read_shared(shared);
    let shown = workbook.shown_cells(sheet)?;
    let contents = if decimal_comma {
        Some(workbook.contents(sheet)?)
    } else {
        None
    };
    let mut inputs = Vec::with_capacity(shown.cells.len());
    let mut numeric = Vec::new();
    for (pos, text) in &shown.cells {
        if text == UNREADABLE_CELL && workbook.cell(sheet, *pos).kind == ValueKind::Error {
            return Err(EngineError::Rejected(t!("csv.unreadable_cell", cell = pos)));
        }
        inputs.push(workbook.input(sheet, *pos));
        if let Some(contents) = &contents {
            numeric.push(matches!(contents(*pos), Contents::Number(_)));
        }
    }
    Ok(Snapshot {
        shown,
        inputs,
        numeric,
    })
}

// What the import hands the engine for a field of the file.
fn import_input<'a>(field: &'a str, options: &CsvOptions) -> Cow<'a, str> {
    let mut text = Cow::Borrowed(field);
    if options.date_order == DateOrder::DayFirst
        && let Some(rewritten) = day_first_field(&text)
    {
        text = Cow::Owned(rewritten);
    }
    if options.decimal_comma
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

// A cell is unedited when the engine holds what importing its field would give it, and then it
// is written as the file had it. The engine rewrites some inputs ("007" is 7, "1e3" is 1000), so
// what does not match as text is imported on a scratch workbook to see what it becomes. Where
// rows or columns were inserted or deleted the positions no longer line up, the inputs differ,
// and each such cell is written as shown.
fn classify(
    snapshot: &mut Snapshot,
    mut file: Vec<Vec<String>>,
    options: &CsvOptions,
) -> Result<Vec<FieldKind>, EngineError> {
    let Snapshot {
        shown,
        inputs,
        numeric,
    } = snapshot;
    let edited = |index: usize| {
        if numeric.get(index).copied().unwrap_or(false) {
            FieldKind::EditedNumber
        } else {
            FieldKind::Edited
        }
    };
    let mut kinds = Vec::with_capacity(shown.cells.len());
    let mut doubtful: Vec<(usize, String)> = Vec::new();
    for (index, ((pos, _), input)) in shown.cells.iter().zip(inputs.iter()).enumerate() {
        let expected = field_at(&mut file, *pos)
            .filter(|field| !field.is_empty())
            .map(|field| import_input(field, options));
        kinds.push(match expected {
            Some(expected) if expected == input.as_str() => FieldKind::Unedited,
            Some(expected) => {
                doubtful.push((index, expected.into_owned()));
                edited(index)
            }
            None => edited(index),
        });
    }
    if !doubtful.is_empty() {
        let rows = doubtful
            .iter_mut()
            .map(|(_, expected)| vec![std::mem::take(expected)])
            .collect();
        let scratch = files::workbook_from_rows(rows).map_err(EngineError::Rejected)?;
        for (line, (index, _)) in doubtful.iter().enumerate() {
            let at = CellPos::new(
                RowIdx::clamped(i64::try_from(line).unwrap_or(i64::MAX)),
                ColIdx::default(),
            );
            if scratch.input(SheetId(0), at) == inputs[*index] {
                kinds[*index] = FieldKind::Unedited;
            }
        }
    }
    for ((pos, text), kind) in shown.cells.iter_mut().zip(&kinds) {
        if *kind == FieldKind::Unedited
            && let Some(original) = field_at(&mut file, *pos)
        {
            *text = std::mem::take(original);
        }
    }
    Ok(kinds)
}

pub fn save_csv_file(
    shared: &RwLock<Workbook>,
    request: &CsvSaveRequest,
    cancel: &AtomicBool,
) -> Result<CsvSave, EngineError> {
    let options = &request.binding.options;
    if request.changed == ChangedFile::Ask && file_changed_since(request.guard, &request.path) {
        return Ok(CsvSave::FileChanged);
    }
    let file = match files::read_baseline(&request.path, options.delimiter) {
        Baseline::Rows(rows) => rows,
        Baseline::Absent => Vec::new(),
        Baseline::Lossy => return Ok(CsvSave::LossyFile),
    };
    if !request.binding.loss_accepted {
        let loss = csv_loss(shared, request.sheets, request.sheet);
        if loss.any() {
            return Ok(CsvSave::WouldLose(loss));
        }
    }
    let mut snapshot = snapshot(shared, request.sheet, options.decimal_comma)?;
    let kinds = classify(&mut snapshot, file, options)?;
    files::write_csv_as(&request.path, &snapshot.shown, options, &kinds, cancel)?;
    Ok(CsvSave::Written(FileSeen::from_stamp(
        Stamp::of(&request.path).ok(),
    )))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use zenkai_types::StyleChange;

    use super::*;
    use crate::csv_options::CsvLoad;

    fn encode_utf16(text: &str, to_bytes: fn(u16) -> [u8; 2], bom: [u8; 2]) -> Vec<u8> {
        bom.into_iter()
            .chain(text.encode_utf16().flat_map(to_bytes))
            .collect()
    }

    fn at(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    fn opened_csv(dir: &Path, name: &str, original: &[u8]) -> (PathBuf, files::FileLoad) {
        let path = dir.join(name);
        std::fs::write(&path, original).unwrap();
        let load = files::load_csv(&path, None).unwrap();
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

    fn save(
        path: &Path,
        workbook: Workbook,
        csv: &CsvLoad,
        changed: ChangedFile,
    ) -> (CsvSave, Vec<u8>) {
        let mut asked = request(path, csv, 1);
        asked.changed = changed;
        let saved = save_csv_file(&RwLock::new(workbook), &asked, &AtomicBool::new(false)).unwrap();
        (saved, std::fs::read(path).unwrap())
    }

    // Reads `original` as a CSV file and saves it again without an edit.
    fn open_and_save(dir: &Path, name: &str, original: &[u8]) -> Vec<u8> {
        let (path, mut load) = opened_csv(dir, name, original);
        let csv = load.csv.take().unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv, ChangedFile::Ask);
        assert!(matches!(saved, CsvSave::Written(_)), "{name}");
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
        assert_unchanged("contacts.csv", b"note,phone\n-,+34 600\n@ana,ok\n");
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
    fn only_the_cell_that_was_edited_changes_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) =
            opened_csv(dir.path(), "edit.csv", b"a,b,c\n007,1.50,x\n00123,2,y\n");
        let csv = load.csv.take().unwrap();
        load.workbook.set_input(SheetId(0), at(1, 1), "9").unwrap();
        load.workbook.set_input(SheetId(0), at(2, 2), "-z").unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv, ChangedFile::Ask);
        assert!(matches!(saved, CsvSave::Written(_)));
        assert_eq!(bytes, b"a,b,c\n007,9,x\n00123,2,'-z\n");
    }

    #[test]
    fn cells_moved_by_inserting_a_row_are_written_as_shown_and_the_rest_as_they_were() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut load) = opened_csv(dir.path(), "insert.csv", b"a,b\n007,1.50\n00123,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .insert_rows(SheetId(0), RowIdx::clamped(1), 1)
            .unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv, ChangedFile::Ask);
        assert!(matches!(saved, CsvSave::Written(_)));
        assert_eq!(bytes, b"a,b\n,\n7,1.5\n123,2\n");
    }

    #[test]
    fn a_file_decoded_with_replacements_is_not_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xEF\xBB\xBFa,\xFF\n";
        let (path, mut load) = opened_csv(dir.path(), "broken.csv", original);
        let csv = load.csv.take().unwrap();
        let (saved, bytes) = save(&path, load.workbook, &csv, ChangedFile::Ask);
        assert!(matches!(saved, CsvSave::LossyFile));
        assert_eq!(bytes, original);
    }

    #[test]
    fn nothing_is_asked_when_the_csv_holds_everything_the_workbook_has() {
        let dir = tempfile::tempdir().unwrap();
        let (_, load) = opened_csv(dir.path(), "book.csv", b"a,b\n1,2\n");
        let loss = csv_loss(&RwLock::new(load.workbook), 1, SheetId(0));
        assert!(!loss.any());
    }

    #[test]
    fn formulas_formatting_and_more_sheets_are_each_a_loss() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut load) = opened_csv(dir.path(), "book.csv", b"a,b\n1,2\n");
        load.workbook
            .set_input(SheetId(0), at(0, 2), "=A2+B2")
            .unwrap();
        let shared = RwLock::new(load.workbook);
        assert_eq!(
            csv_loss(&shared, 1, SheetId(0)),
            CsvLoss {
                formulas: true,
                ..CsvLoss::default()
            }
        );
        assert_eq!(
            csv_loss(&shared, 2, SheetId(0)),
            CsvLoss {
                sheets: true,
                ..CsvLoss::default()
            }
        );
        let range = zenkai_types::Range::single(at(0, 0));
        shared
            .write()
            .unwrap()
            .apply_style(SheetId(0), range, StyleChange::Bold(true))
            .unwrap();
        assert!(csv_loss(&shared, 1, SheetId(0)).formatting);
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
        assert!(matches!(outcome, CsvSave::WouldLose(loss) if loss.formulas));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b\n1,2\n");

        asked.binding.loss_accepted = true;
        let outcome = save_csv_file(&shared, &asked, &cancel).unwrap();
        assert!(matches!(outcome, CsvSave::Written(_)));
        assert_eq!(std::fs::read(&path).unwrap(), b"a,b,2\n1,2,\n");
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
        assert!(matches!(outcome, CsvSave::Written(_)));
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
}
