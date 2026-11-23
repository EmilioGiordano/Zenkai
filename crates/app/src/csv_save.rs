use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use zenkai_engine::{Engine, EngineError, Stamp, Workbook};
use zenkai_formats::FormulaText;
use zenkai_types::{CellPos, CellStyle, ColIdx, RowIdx, SheetId};

use crate::csv_options::CsvBinding;
use crate::document::{ChangedFile, file_changed_since, read_shared};
use crate::entry::FileSeen;
use crate::files;

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
    let workbook = read_shared(shared);
    let mut plain = workbook
        .cell(sheet, CellPos::new(RowIdx::LAST, ColIdx::LAST))
        .style;
    plain.num_fmt.clear();
    for pos in workbook.filled_cells(sheet) {
        loss.formulas = loss.formulas || workbook.input(sheet, pos).starts_with('=');
        loss.formatting = loss.formatting || is_formatted(workbook.cell(sheet, pos).style, &plain);
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
}

pub fn save_csv_file(
    shared: &RwLock<Workbook>,
    request: &CsvSaveRequest,
    cancel: &AtomicBool,
) -> Result<CsvSave, EngineError> {
    if request.changed == ChangedFile::Ask && file_changed_since(request.guard, &request.path) {
        return Ok(CsvSave::FileChanged);
    }
    if !request.binding.loss_accepted {
        let loss = csv_loss(shared, request.sheets, request.sheet);
        if loss.any() {
            return Ok(CsvSave::WouldLose(loss));
        }
    }
    let shown = files::gather_export(shared, request.sheet)?;
    // The file is the user's own, so what is in its cells is not rewritten for another program.
    files::write_csv_as(
        &request.path,
        &shown,
        &request.binding.options,
        FormulaText::Verbatim,
        cancel,
    )?;
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

    // Reads `original` as a CSV file and saves it again without an edit.
    fn open_and_save(dir: &Path, name: &str, original: &[u8]) -> Vec<u8> {
        let path = dir.join(name);
        std::fs::write(&path, original).unwrap();
        let load = files::load_csv(&path, None).unwrap();
        let csv = load.csv.unwrap();
        let request = CsvSaveRequest {
            path: path.clone(),
            binding: CsvBinding::new(csv.options),
            sheets: 1,
            sheet: SheetId(0),
            changed: ChangedFile::Ask,
            guard: Some(csv.seen),
        };
        let saved = save_csv_file(
            &RwLock::new(load.workbook),
            &request,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(matches!(saved, CsvSave::Written(_)), "{name}");
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn a_semicolon_windows_1252_file_with_decimal_commas_is_saved_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"nombre;precio;ciudad\nJos\xe9;1,5;M\xe1laga\nAna;2;C\xf3rdoba\n";
        assert_eq!(open_and_save(dir.path(), "precios.csv", original), original);
    }

    #[test]
    fn a_tab_separated_utf16_file_is_saved_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let text = "name\tnote\r\nAna\tcaf\u{e9}\r\nLuis\t3\r\n";
        let original = encode_utf16(text, u16::to_le_bytes, [0xFF, 0xFE]);
        assert_eq!(open_and_save(dir.path(), "notes.tsv", &original), original);
        let big_endian = encode_utf16(text, u16::to_be_bytes, [0xFE, 0xFF]);
        assert_eq!(
            open_and_save(dir.path(), "notes_be.tsv", &big_endian),
            big_endian
        );
    }

    #[test]
    fn a_bom_is_written_only_if_the_file_had_one_and_line_endings_stay() {
        let dir = tempfile::tempdir().unwrap();
        for original in [
            &b"a,b\n1,2\n"[..],
            b"a,b\r\n1,2\r\n",
            b"\xEF\xBB\xBFa,b\n1,2\n",
            b"\xEF\xBB\xBFa,b\r\n1,2\r\n",
            b"a|b\r1|2\r",
        ] {
            assert_eq!(open_and_save(dir.path(), "plain.csv", original), original);
        }
    }

    #[test]
    fn text_that_looks_like_a_formula_is_saved_over_its_own_csv_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"note,phone\n-,+34 600\n@ana,ok\n";
        assert_eq!(
            open_and_save(dir.path(), "contacts.csv", original),
            original
        );
    }

    fn opened_csv(dir: &Path, original: &[u8]) -> (PathBuf, files::FileLoad) {
        let path = dir.join("book.csv");
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

    #[test]
    fn nothing_is_asked_when_the_csv_holds_everything_the_workbook_has() {
        let dir = tempfile::tempdir().unwrap();
        let (_, load) = opened_csv(dir.path(), b"a,b\n1,2\n");
        let loss = csv_loss(&RwLock::new(load.workbook), 1, SheetId(0));
        assert!(!loss.any());
    }

    #[test]
    fn formulas_formatting_and_more_sheets_are_each_a_loss() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut load) = opened_csv(dir.path(), b"a,b\n1,2\n");
        let at = |row, col| CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col));
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
        let (path, mut load) = opened_csv(dir.path(), b"a,b\n1,2\n");
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(
                SheetId(0),
                CellPos::new(RowIdx::clamped(0), ColIdx::clamped(2)),
                "=1+1",
            )
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
        let (path, load) = opened_csv(dir.path(), b"a,b\n1,2\n");
        let csv = load.csv.unwrap();
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
        let path = dir.path().join("latin.csv");
        let original = b"caf\xe9;x\n";
        std::fs::write(&path, original).unwrap();
        let mut load = files::load_csv(&path, None).unwrap();
        let csv = load.csv.take().unwrap();
        load.workbook
            .set_input(
                SheetId(0),
                CellPos::new(RowIdx::clamped(0), ColIdx::clamped(1)),
                "\u{65e5}\u{672c}",
            )
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
