// A workbook restored from a recovery copy holds the unsaved work, but its file is still the
// one the user opened: a save copies what the work did not change from that file, never from
// the recovery copy.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::error::EngineError;
use crate::file::refuse_oversized;
use crate::preflight::run_with_engine_stack;
use crate::save::ModelData;
use crate::saved::{FileOnDisk, KnownFile, SavedFile, Stamp, Strings, opened};
use crate::workbook::{Workbook, carried_by_position, engine_model};
use crate::xlsx_read::{FileRead, XlsxReader, check_not_cancelled, read_xlsx_file};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginalFile {
    Adopted,
    // The file is not the one `stamp` describes.
    Changed,
    // A save could not copy from the file into this workbook.
    Unaligned,
}

impl Workbook {
    // Leaves the workbook as it was unless the file is adopted. The file is remembered by the
    // sheets it holds, read again here, so a save rewrites every sheet the work changed.
    pub fn adopt_original_file(
        &mut self,
        path: &Path,
        stamp: Stamp,
        cancel: &Arc<AtomicBool>,
    ) -> Result<OriginalFile, EngineError> {
        if Stamp::of(path).ok() != Some(stamp) {
            return Ok(OriginalFile::Changed);
        }
        refuse_oversized(path)?;
        let workbook = &self.model.get_model().workbook;
        // The book print covers the workbook's name, which a read takes from the file name.
        let FileRead {
            read,
            settings,
            parts,
        } = read_xlsx_file(path, &workbook.name, XlsxReader::Fast, cancel)?;
        check_not_cancelled(cancel)?;
        let Some(parts) = parts else {
            return Ok(OriginalFile::Unaligned);
        };
        if Stamp::of(path).ok() != Some(stamp) {
            return Ok(OriginalFile::Changed);
        }
        let strings = &workbook.shared_strings;
        let file = run_with_engine_stack(|| {
            let model = engine_model(read.book)?;
            let carried = carried_by_position(&model, settings);
            let file = opened(
                FileOnDisk { path, stamp, parts },
                ModelData::of(&model),
                &carried,
            );
            Ok(file.filter(|file| strings_line_up(file, &model.workbook.shared_strings, strings)))
        })?;
        let Some(file) = file else {
            return Ok(OriginalFile::Unaligned);
        };
        self.saved.set_known(Some(KnownFile {
            path: path.to_path_buf(),
            stamp,
        }));
        self.saved.set(Some(file));
        Ok(OriginalFile::Adopted)
    }
}

// A save appends the workbook's strings after those the file holds, and a rewritten sheet
// points into the workbook's, so the file's must be the workbook's first ones.
fn strings_line_up(file: &SavedFile, read: &[String], workbook: &[String]) -> bool {
    match file.contents.strings {
        Strings::Absent => true,
        Strings::Aligned(count) => match (read.get(..count), workbook.get(..count)) {
            (Some(read), Some(workbook)) => read == workbook,
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use rust_xlsxwriter::{Chart, ChartType};
    use zenkai_types::{CellPos, SheetId};

    use super::OriginalFile;
    use crate::error::EngineError;
    use crate::file::{Unsupported, open_xlsx};
    use crate::save::{SaveTarget, save_xlsx};
    use crate::saved::Stamp;
    use crate::workbook::{Engine, Workbook};

    const CHART_PART: &str = "xl/charts/chart1.xml";

    fn at(a1: &str) -> CellPos {
        CellPos::parse_a1(a1).unwrap()
    }

    fn text(book: &Workbook, sheet: u32, a1: &str) -> String {
        book.cell(SheetId(sheet), at(a1)).text
    }

    fn raw_part(path: &Path, name: &str) -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }

    fn charted(path: &Path) {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_number(0, 0, 10.0).unwrap();
        sheet.write_string(0, 1, "apple").unwrap();
        sheet.write_number(1, 0, 20.0).unwrap();
        let mut chart = Chart::new(ChartType::Column);
        chart.add_series().set_values("Sheet1!$A$1:$A$2");
        sheet.insert_chart(4, 3, &chart).unwrap();
        let other = book.add_worksheet().set_name("Other").unwrap();
        other.write_string(0, 0, "untouched").unwrap();
        book.save(path).unwrap();
    }

    // The original, and the workbook read back from a recovery copy of edits made on it.
    fn restored(dir: &Path) -> (PathBuf, PathBuf, Workbook) {
        let original = dir.join("book.xlsx");
        charted(&original);
        let mut book = open_xlsx(&original).unwrap().workbook;
        book.set_input(SheetId(0), at("A1"), "kiwi").unwrap();
        book.set_input(SheetId(0), at("A3"), "7").unwrap();
        let recovery = dir.join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let recovered = open_xlsx(&recovery).unwrap().workbook;
        (original, recovery, recovered)
    }

    #[test]
    fn a_restored_workbook_copies_untouched_parts_from_its_original_and_writes_its_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (original, recovery, mut book) = restored(dir.path());
        let stamp = Stamp::of(&original).unwrap();
        book.guard_file(&original, Some(stamp));
        let adopted = book.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Adopted);
        assert_eq!(book.file_stamp(), Some(stamp));
        std::fs::write(&recovery, b"corrupted recovery").unwrap();
        let chart = raw_part(&original, CHART_PART);

        let snapshot = book.save_snapshot(&original, SaveTarget::Document).unwrap();
        assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
        assert!(snapshot.kept().contains(&Unsupported::Charts));
        save_xlsx(snapshot).unwrap();

        let reopened = open_xlsx(&original).unwrap();
        assert!(reopened.unsupported.contains(&Unsupported::Charts));
        assert_eq!(raw_part(&original, CHART_PART), chart);
        for (sheet, a1) in [(0, "A1"), (0, "A2"), (0, "A3"), (0, "B1"), (1, "A1")] {
            assert_eq!(
                text(&reopened.workbook, sheet, a1),
                text(&book, sheet, a1),
                "{sheet} {a1}"
            );
        }
        assert_eq!(text(&reopened.workbook, 0, "A1"), "kiwi");
        assert_eq!(text(&reopened.workbook, 0, "A3"), "7");
    }

    #[test]
    fn an_original_changed_since_its_stamp_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let (original, _, mut book) = restored(dir.path());
        let stamp = Stamp::of(&original).unwrap();
        book.guard_file(&original, Some(stamp));
        std::fs::write(&original, b"theirs, longer than the stamp it replaces").unwrap();

        let adopted = book.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Changed);
        let snapshot = book.save_snapshot(&original, SaveTarget::Document).unwrap();
        assert!(snapshot.kept().is_empty());
        assert!(matches!(
            save_xlsx(snapshot),
            Err(EngineError::ChangedOnDisk(_))
        ));
    }

    fn strings_in_order(path: &Path, first: &str, second: &str) {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_string(0, 0, first).unwrap();
        sheet.write_string(1, 0, second).unwrap();
        book.save(path).unwrap();
    }

    #[test]
    fn an_original_whose_strings_are_not_the_workbooks_first_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        strings_in_order(&original, "a", "b");
        let copy = dir.path().join("book-copy.xlsx");
        strings_in_order(&copy, "b", "a");
        let mut book = open_xlsx(&copy).unwrap().workbook;
        let stamp = Stamp::of(&original).unwrap();
        book.guard_file(&original, Some(stamp));

        let adopted = book.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
        let snapshot = book.save_snapshot(&original, SaveTarget::Document).unwrap();
        save_xlsx(snapshot).unwrap();
        let reopened = open_xlsx(&original).unwrap().workbook;
        assert_eq!(text(&reopened, 0, "A1"), "b");
        assert_eq!(text(&reopened, 0, "A2"), "a");
    }

    #[test]
    fn a_cancelled_adoption_leaves_the_workbook_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let (original, _, mut book) = restored(dir.path());
        let stamp = Stamp::of(&original).unwrap();
        book.guard_file(&original, Some(stamp));

        let cancel = Arc::new(AtomicBool::new(true));
        let adopted = book.adopt_original_file(&original, stamp, &cancel);
        assert!(matches!(adopted, Err(EngineError::Cancelled)));
        let snapshot = book.save_snapshot(&original, SaveTarget::Document).unwrap();
        assert!(snapshot.kept().is_empty());
    }
}
