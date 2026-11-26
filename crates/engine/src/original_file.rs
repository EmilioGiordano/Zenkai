// A workbook restored from a recovery copy holds the unsaved work, but its file is still the
// one the user opened: a save copies what the work did not change from that file, never from
// the recovery copy.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ironcalc::base::types::Workbook as Book;
use zenkai_xlsx_reader::SheetLinks;

use crate::error::EngineError;
use crate::file::refuse_oversized;
use crate::fingerprint::{DynamicArrays, book_print};
use crate::preflight::run_with_engine_stack;
use crate::save::ModelData;
use crate::saved::{
    FileOnDisk, KnownFile, Metadata, SavedFile, SourceParts, Stamp, Strings, opened, sheet_prints,
};
use crate::sheet_settings::Carried;
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
        check_not_cancelled(cancel)?;
        if Stamp::of(path).ok() != Some(stamp) {
            return Ok(OriginalFile::Changed);
        }
        refuse_oversized(path)?;
        let Some(source_parts) = self.source_parts.as_ref() else {
            return Ok(OriginalFile::Unaligned);
        };
        let model_ref = self.model.get_model();
        let workbook = &model_ref.workbook;
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
        let unsupported = read.unsupported;
        let original_links = parts.layout.sheet_links.clone();
        let workbook_data = ModelData::of(model_ref);
        let workbook_carried = &self.carried;
        let file = run_with_engine_stack(|| {
            let model = engine_model(read.book)?;
            let carried = carried_by_position(&model, settings);
            let Some(file) = opened(
                FileOnDisk { path, stamp, parts },
                ModelData::of(&model),
                &carried,
            ) else {
                return Ok(None);
            };
            let original = Original {
                file: &file,
                book: &model.workbook,
                carried: &carried,
                links: &original_links,
            };
            let aligned = lines_up(&original, workbook_data, workbook_carried, source_parts)?;
            Ok(aligned.then_some(file))
        })?;
        check_not_cancelled(cancel)?;
        let Some(file) = file else {
            return Ok(OriginalFile::Unaligned);
        };
        self.opened = unsupported;
        self.saved.set_known(Some(KnownFile {
            path: path.to_path_buf(),
            stamp,
        }));
        self.saved.set(Some(file));
        Ok(OriginalFile::Adopted)
    }
}

struct Original<'a> {
    file: &'a SavedFile,
    book: &'a Book,
    carried: &'a Carried,
    links: &'a [SheetLinks],
}

fn lines_up(
    original: &Original<'_>,
    workbook: ModelData<'_>,
    carried: &Carried,
    source: &SourceParts,
) -> Result<bool, EngineError> {
    if !strings_line_up(
        original.file,
        &original.book.shared_strings,
        &workbook.book.shared_strings,
    ) {
        return Ok(false);
    }
    if !original
        .file
        .contents
        .styles
        .lines_up(&original.book.styles, &workbook.book.styles)?
    {
        return Ok(false);
    }
    if !book_lines_up(original, workbook.book)? {
        return Ok(false);
    }
    if !carried_lines_up(original, workbook.book, carried, source) {
        return Ok(false);
    }
    dynamic_arrays_line_up(original.file, workbook, carried)
}

// A save appends the workbook's strings after those the file holds, and a rewritten sheet
// points into the workbook's, so the file's must be the workbook's first ones.
fn strings_line_up(file: &SavedFile, read: &[String], workbook: &[String]) -> bool {
    match file.contents.strings {
        Strings::Absent => workbook.is_empty(),
        Strings::Aligned(count) => match (read.get(..count), workbook.get(..count)) {
            (Some(read), Some(workbook)) => read == workbook,
            _ => false,
        },
    }
}

fn book_lines_up(original: &Original<'_>, workbook: &Book) -> Result<bool, EngineError> {
    let read = original.book;
    let sheets_match = read.worksheets.len() == workbook.worksheets.len()
        && read
            .worksheets
            .iter()
            .zip(&workbook.worksheets)
            .all(|(a, b)| a.name == b.name && a.sheet_id == b.sheet_id && a.state == b.state);
    let same = sheets_match
        && read.defined_names == workbook.defined_names
        && read.tables == workbook.tables
        && read.name == workbook.name
        && read.settings == workbook.settings
        && read.metadata == workbook.metadata
        && read.theme == workbook.theme;
    Ok(same && book_print(workbook)? == original.file.contents.book)
}

// A recovery copy that dropped a sheet's relationships, uncarried elements or carried settings
// (for example after a row or column move) would otherwise rewrite that sheet without them
// while keeping the original file's relationships or stale row-spliced parts.
fn carried_lines_up(
    original: &Original<'_>,
    workbook: &Book,
    carried: &Carried,
    source: &SourceParts,
) -> bool {
    if source.held != original.file.contents.held || source.sheet_links.as_slice() != original.links
    {
        return false;
    }
    if carried.losses() != 0 {
        return false;
    }
    original
        .book
        .worksheets
        .iter()
        .zip(&workbook.worksheets)
        .all(|(read, current)| {
            carried.moves(current.sheet_id) == 0
                && original.carried.written(read) == carried.written(current)
        })
}

fn dynamic_arrays_line_up(
    file: &SavedFile,
    workbook: ModelData<'_>,
    carried: &Carried,
) -> Result<bool, EngineError> {
    if file.contents.metadata == Metadata::Written {
        return Ok(true);
    }
    let prints = sheet_prints(workbook, carried)?;
    let needs_metadata = prints
        .iter()
        .zip(&file.contents.sheets)
        .any(|(now, saved)| {
            now.changed_since(saved) && now.dynamic_arrays() == DynamicArrays::Present
        });
    Ok(!needs_metadata)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use rust_xlsxwriter::{Chart, ChartType};
    use zenkai_types::{CellPos, Range, RowIdx, SheetId, StyleChange};

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

    fn rewrite_zip(
        source: &Path,
        target: &Path,
        edit: impl Fn(&str, Vec<u8>) -> Vec<u8>,
        extra: &[(&str, &[u8])],
    ) {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(source).unwrap()).unwrap();
        let mut writer = zip::ZipWriter::new(std::fs::File::create(target).unwrap());
        let options = zip::write::FileOptions::default();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            let bytes = edit(&name, bytes);
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        for (name, bytes) in extra {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
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
    fn an_original_without_shared_strings_is_not_adopted_when_the_workbook_holds_strings() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        source.add_worksheet().write_number(0, 0, 10.0).unwrap();
        source.save(&original).unwrap();
        let book = open_xlsx(&original).unwrap().workbook;
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let mut recovered = open_xlsx(&recovery).unwrap().workbook;
        recovered.set_input(SheetId(0), at("A1"), "hello").unwrap();
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
    }

    #[test]
    fn an_original_whose_styles_do_not_line_up_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        let mut source = Workbook::new_empty().unwrap();
        source.set_input(SheetId(0), at("A1"), "10").unwrap();
        let a1 = Range::single(at("A1"));
        source
            .apply_style(SheetId(0), a1, StyleChange::Bold(true))
            .unwrap();
        save_xlsx(
            source
                .save_snapshot(&original, SaveTarget::Document)
                .unwrap(),
        )
        .unwrap();
        let copy = dir.path().join("autosave-1-1-0.xlsx");
        rewrite_zip(
            &original,
            &copy,
            |name, bytes| {
                if name == "xl/styles.xml" {
                    let xml = String::from_utf8(bytes).unwrap();
                    assert!(xml.contains("<b/>"), "{xml}");
                    xml.replacen("<b/>", "<i/>", 1).into_bytes()
                } else {
                    bytes
                }
            },
            &[],
        );
        let mut recovered = open_xlsx(&copy).unwrap().workbook;
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
        let snapshot = recovered
            .save_snapshot(&original, SaveTarget::Document)
            .unwrap();
        save_xlsx(snapshot).unwrap();
        let reopened = open_xlsx(&original).unwrap().workbook;
        let style = reopened.cell(SheetId(0), at("A1")).style;
        assert!(style.italic && !style.bold);
    }

    #[test]
    fn a_restored_workbook_with_added_styles_still_adopts_and_keeps_unseen_parts() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        charted(&original);
        let chart = raw_part(&original, CHART_PART);
        let mut book = open_xlsx(&original).unwrap().workbook;
        book.set_input(SheetId(0), at("A1"), "kiwi").unwrap();
        let a1 = Range::single(at("A1"));
        book.apply_style(SheetId(0), a1, StyleChange::Bold(true))
            .unwrap();
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let mut recovered = open_xlsx(&recovery).unwrap().workbook;
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Adopted);
        std::fs::write(&recovery, b"corrupted recovery").unwrap();
        let snapshot = recovered
            .save_snapshot(&original, SaveTarget::Document)
            .unwrap();
        assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
        assert!(snapshot.kept().contains(&Unsupported::Charts));
        save_xlsx(snapshot).unwrap();

        let reopened = open_xlsx(&original).unwrap();
        assert_eq!(raw_part(&original, CHART_PART), chart);
        assert_eq!(text(&reopened.workbook, 0, "A1"), "kiwi");
        assert!(reopened.workbook.cell(SheetId(0), at("A1")).style.bold);
    }

    #[test]
    fn an_original_whose_sheets_or_defined_names_differ_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        charted(&original);
        let book = open_xlsx(&original).unwrap().workbook;
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let stamp = Stamp::of(&original).unwrap();

        let mut renamed = open_xlsx(&recovery).unwrap().workbook;
        renamed.rename_sheet(SheetId(0), "Renamed").unwrap();
        renamed.guard_file(&original, Some(stamp));
        let adopted = renamed.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);

        let mut named = open_xlsx(&recovery).unwrap().workbook;
        named
            .model
            .new_defined_name("Total", None, "Sheet1!$A$1")
            .unwrap();
        named.guard_file(&original, Some(stamp));
        let adopted = named.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
    }

    #[test]
    fn a_recovery_copy_that_dropped_a_sheets_relationships_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        charted(&original);
        let mut book = open_xlsx(&original).unwrap().workbook;
        book.insert_rows(SheetId(0), RowIdx::clamped(0), 1).unwrap();
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let mut recovered = open_xlsx(&recovery).unwrap().workbook;
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
        let snapshot = recovered
            .save_snapshot(&original, SaveTarget::Document)
            .unwrap();
        assert!(snapshot.kept().is_empty());
        assert!(snapshot.dropped().contains(&Unsupported::Charts));
    }

    #[test]
    fn a_recovery_copy_that_dropped_sheet_links_without_sheet_elements_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        source.add_worksheet().write_number(0, 0, 10.0).unwrap();
        source.save(&base).unwrap();
        let original = dir.path().join("book.xlsx");
        let rels = concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2017/10/relationships/threadedComment" Target="../threadedComments/threadedComment1.xml"/>"#,
            r#"</Relationships>"#,
        );
        let comments = concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<ThreadedComments xmlns="http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments"/>"#,
        );
        rewrite_zip(
            &base,
            &original,
            |_, bytes| bytes,
            &[
                ("xl/worksheets/_rels/sheet1.xml.rels", rels.as_bytes()),
                (
                    "xl/threadedComments/threadedComment1.xml",
                    comments.as_bytes(),
                ),
            ],
        );
        let opened = open_xlsx(&original).unwrap();
        assert!(opened.unsupported.contains(&Unsupported::Comments));
        let mut book = opened.workbook;
        book.insert_rows(SheetId(0), RowIdx::clamped(0), 1).unwrap();
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let mut recovered = open_xlsx(&recovery).unwrap().workbook;
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
    }

    #[test]
    fn a_recovery_copy_that_dropped_carried_settings_is_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        let sheet = source.add_worksheet();
        sheet.write_number(0, 0, 10.0).unwrap();
        sheet.write_number(1, 0, 20.0).unwrap();
        source.save(&base).unwrap();
        let original = dir.path().join("book.xlsx");
        let breaks = r#"<rowBreaks count="1" manualBreakCount="1"><brk id="1" max="16383" man="1"/></rowBreaks>"#;
        rewrite_zip(
            &base,
            &original,
            |name, bytes| {
                if name == "xl/worksheets/sheet1.xml" {
                    let xml = String::from_utf8(bytes).unwrap();
                    xml.replacen("</worksheet>", &format!("{breaks}</worksheet>"), 1)
                        .into_bytes()
                } else {
                    bytes
                }
            },
            &[],
        );
        let mut book = open_xlsx(&original).unwrap().workbook;
        book.insert_rows(SheetId(0), RowIdx::clamped(0), 1).unwrap();
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let mut recovered = open_xlsx(&recovery).unwrap().workbook;
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
    }

    #[test]
    fn a_changed_sheet_with_a_dynamic_array_is_not_adopted_over_unknown_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        source.add_worksheet().write_number(0, 0, 10.0).unwrap();
        source.save(&original).unwrap();
        let book = open_xlsx(&original).unwrap().workbook;
        let recovery = dir.path().join("autosave-1-1-0.xlsx");
        save_xlsx(book.save_snapshot(&recovery, SaveTarget::Copy).unwrap()).unwrap();
        let mut recovered = open_xlsx(&recovery).unwrap().workbook;
        recovered
            .set_input(SheetId(0), at("B1"), "=SEQUENCE(3)")
            .unwrap();
        let stamp = Stamp::of(&original).unwrap();
        recovered.guard_file(&original, Some(stamp));

        let adopted = recovered.adopt_original_file(&original, stamp, &Arc::default());
        assert_eq!(adopted.unwrap(), OriginalFile::Unaligned);
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
