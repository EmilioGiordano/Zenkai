use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::error::EngineError;
use crate::preflight;
use crate::saved::{FileOnDisk, KnownFile, Stamp};
use crate::workbook::Workbook;
use crate::xlsx_read::{self, XlsxReader};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unsupported {
    Charts,
    Images,
    PivotTables,
    Macros,
    Comments,
    Tables,
    Hyperlinks,
    DataValidation,
    ExternalLinks,
    AutoFilter,
    SheetProtection,
    Outline,
    PageBreaks,
}

impl Unsupported {
    pub fn label(self) -> &'static str {
        match self {
            Unsupported::Charts => "charts",
            Unsupported::Images => "images and shapes",
            Unsupported::PivotTables => "pivot tables",
            Unsupported::Macros => "macros (VBA)",
            Unsupported::Comments => "comments",
            Unsupported::Tables => "Excel tables",
            Unsupported::Hyperlinks => "hyperlinks",
            Unsupported::DataValidation => "data validation",
            Unsupported::ExternalLinks => "links to other workbooks",
            Unsupported::AutoFilter => "filters (AutoFilter)",
            Unsupported::SheetProtection => "sheet protection",
            Unsupported::Outline => "row and column grouping",
            Unsupported::PageBreaks => "manual page breaks",
        }
    }
}

pub struct Opened {
    pub workbook: Workbook,
    pub unsupported: Vec<Unsupported>,
    pub fallback: Option<String>,
}

pub fn open_xlsx(path: &Path) -> Result<Opened, EngineError> {
    open_xlsx_with(path, XlsxReader::Fast)
}

pub fn open_xlsx_with(path: &Path, reader: XlsxReader) -> Result<Opened, EngineError> {
    open_xlsx_cancellable(path, reader, &Arc::default())
}

// Raising `cancel` stops the open with `Cancelled` at the next checkpoint: while streaming
// the sheets, between the read phases and before the model is built and calculated. The
// calculation inside IronCalc cannot be interrupted, so a cancel during it is honoured once it ends.
pub fn open_xlsx_cancellable(
    path: &Path,
    reader: XlsxReader,
    cancel: &Arc<AtomicBool>,
) -> Result<Opened, EngineError> {
    let read_error = |source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    };
    let size = fs::metadata(path).map_err(read_error)?.len();
    if size > preflight::MAX_FILE_BYTES {
        return Err(EngineError::Unsafe(format!(
            "the file is {} MB, larger than the {} MB supported",
            size / 1024 / 1024,
            preflight::MAX_FILE_BYTES / 1024 / 1024
        )));
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Book".to_string());
    // The streaming reader parses the file straight from disk and brings the sheet
    // settings back from the same pass; the other readers read the file whole first.
    let before = Stamp::of(path);
    let xlsx_read::FileRead {
        read,
        settings,
        parts,
    } = xlsx_read::read_xlsx_file(path, &name, reader, cancel)?;
    xlsx_read::check_not_cancelled(cancel)?;
    // Even a file saves cannot copy from, or one that changed while it was read, is
    // remembered by the stamp taken before the read, so a save over it asks first.
    let known = before.as_ref().ok().map(|stamp| KnownFile {
        path: path.to_path_buf(),
        stamp: *stamp,
    });
    // A file that changed while it was read is never copied from.
    let file = match (parts, before) {
        (Some(parts), Ok(stamp)) if Stamp::of(path).is_ok_and(|after| after == stamp) => {
            Some(FileOnDisk { path, stamp, parts })
        }
        _ => None,
    };
    let held = &read.unsupported;
    let workbook = tracing::info_span!("open_workbook_build").in_scope(|| {
        preflight::run_with_engine_stack(|| {
            Workbook::from_file(read.book, settings, file, known, held)
        })
    })?;
    Ok(Opened {
        workbook,
        unsupported: read.unsupported,
        fallback: read.fallback,
    })
}

pub fn scan_unsupported(bytes: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let found = std::sync::Mutex::new(Vec::new());
    zenkai_xlsx_reader::scan_parts(bytes, &|name, part| inspect_part(&found, name, part))
        .map_err(crate::xlsx_read::engine_error)?;
    let found = found
        .into_inner()
        .map_err(|_| EngineError::InvalidFile("a part check stopped".to_string()))?;
    Ok(kinds(&found))
}

// Each kind found, by the name of the part it was found in.
pub(crate) type Found = std::sync::Mutex<Vec<(String, Unsupported)>>;

pub(crate) fn kinds(found: &[(String, Unsupported)]) -> Vec<Unsupported> {
    let mut kinds: Vec<Unsupported> = found.iter().map(|(_, kind)| *kind).collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

pub(crate) fn inspect_part(
    found: &Found,
    name: &str,
    part: &[u8],
) -> Result<(), zenkai_xlsx_reader::ReadError> {
    use zenkai_xlsx_reader::ReadError;
    let kinds = part_unsupported(name, part).map_err(|error| match error {
        EngineError::Unsafe(reason) => ReadError::Unsafe(reason),
        other => ReadError::Unsupported(other.to_string()),
    })?;
    found
        .lock()
        .map_err(|_| ReadError::Unsupported("a part check stopped".to_string()))?
        .extend(kinds.into_iter().map(|kind| (name.to_string(), kind)));
    Ok(())
}

pub(crate) fn part_unsupported(name: &str, part: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let name = name.to_ascii_lowercase();
    let by_name = [
        ("xl/charts/", Unsupported::Charts),
        ("xl/media/", Unsupported::Images),
        ("xl/drawings/", Unsupported::Images),
        ("xl/pivotcache/", Unsupported::PivotTables),
        ("xl/pivottables/", Unsupported::PivotTables),
        ("xl/vbaproject", Unsupported::Macros),
        ("xl/comments", Unsupported::Comments),
        ("xl/threadedcomments/", Unsupported::Comments),
        ("xl/externallinks/", Unsupported::ExternalLinks),
        ("xl/tables/", Unsupported::Tables),
    ];
    let mut found: Vec<Unsupported> = by_name
        .into_iter()
        .filter(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, kind)| kind)
        .collect();
    found.extend(preflight::check_part(part)?);
    Ok(found)
}

// Every write over a user file goes through here: sibling temp file, read back
// and verified, then renamed over the target.
pub fn write_atomic(
    path: &Path,
    bytes: &[u8],
    verify: impl Fn(&[u8]) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let (file, temp) = create_temp(path)?;
    if let Err(source) = write_new(file, bytes) {
        remove_temp(&temp);
        return Err(EngineError::Write { path: temp, source });
    }
    let verified = fs::read(&temp)
        .map_err(|e| EngineError::VerifyFailed(e.to_string()))
        .and_then(|written| {
            if written != bytes {
                return Err(EngineError::VerifyFailed(
                    "the file on disk differs from what was written".to_string(),
                ));
            }
            verify(&written).map_err(|e| EngineError::VerifyFailed(e.to_string()))
        });
    if let Err(error) = verified {
        remove_temp(&temp);
        return Err(error);
    }
    if let Err(source) = fs::rename(&temp, path) {
        remove_temp(&temp);
        return Err(EngineError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn write_new(mut file: fs::File, bytes: &[u8]) -> std::io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

pub(crate) fn remove_temp(temp: &Path) {
    if let Err(error) = fs::remove_file(temp) {
        tracing::warn!(?temp, %error, "could not remove temp file after failed save");
    }
}

pub(crate) fn temp_path(path: &Path) -> PathBuf {
    sibling_temp(path, &std::process::id().to_string())
}

fn sibling_temp(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "book.xlsx".to_string());
    path.with_file_name(format!("~${name}.{suffix}.tmp"))
}

const TEMP_ATTEMPTS: u32 = 32;

#[cfg(test)]
const PLANTED_LEFTOVERS: u64 = 16;

#[cfg(not(test))]
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// Each test thread counts on its own, so a test can plant leftovers over the exact
// names its save will try without touching another test's temp files.
#[cfg(test)]
thread_local! {
    static TEMP_COUNTER: AtomicU64 = const { AtomicU64::new(0) };
}

// A killed save leaves its temp file behind. Every attempt takes a fresh name and
// `create_new` still refuses a file we did not create, so a leftover is skipped, never
// reused or deleted.
pub(crate) fn create_temp(path: &Path) -> Result<(fs::File, PathBuf), EngineError> {
    #[cfg(not(test))]
    return create_temp_counting(path, &TEMP_COUNTER);
    #[cfg(test)]
    TEMP_COUNTER.with(|counter| create_temp_counting(path, counter))
}

fn create_temp_counting(
    path: &Path,
    counter: &AtomicU64,
) -> Result<(fs::File, PathBuf), EngineError> {
    let mut last = None;
    for _ in 0..TEMP_ATTEMPTS {
        let count = counter.fetch_add(1, Ordering::Relaxed);
        let temp = sibling_temp(path, &format!("{}.{count}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((file, temp)),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                last = Some(EngineError::Write { path: temp, source });
            }
            Err(source) => return Err(EngineError::Write { path: temp, source }),
        }
    }
    Err(last.unwrap_or_else(|| EngineError::Write {
        path: path.to_path_buf(),
        source: std::io::ErrorKind::AlreadyExists.into(),
    }))
}

// Plants leftovers over the names the next attempts would take, so a test that saves
// right after it collides; `skipped_all` says the save settled on the first free name.
#[cfg(test)]
pub(crate) struct Leftovers {
    paths: Vec<PathBuf>,
    first: u64,
}

#[cfg(test)]
impl Leftovers {
    pub(crate) fn plant(dir: &Path, file_name: &str) -> Self {
        let first = TEMP_COUNTER.with(|counter| counter.load(Ordering::Relaxed));
        let paths = (first..first + PLANTED_LEFTOVERS)
            .map(|count| {
                sibling_temp(
                    &dir.join(file_name),
                    &format!("{}.{count}", std::process::id()),
                )
            })
            .collect::<Vec<_>>();
        for path in &paths {
            fs::write(path, b"theirs").unwrap();
        }
        Self { paths, first }
    }

    pub(crate) fn skipped_all(&self) -> bool {
        TEMP_COUNTER.with(|counter| counter.load(Ordering::Relaxed))
            == self.first + PLANTED_LEFTOVERS + 1
    }

    pub(crate) fn assert_untouched(&self) {
        for path in &self.paths {
            assert_eq!(fs::read(path).unwrap(), b"theirs");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;
    use crate::workbook::Engine;
    use zenkai_types::{CellPos, Contents, Range, SheetId};

    fn number_at(book: &Workbook, pos: CellPos) -> Option<f64> {
        match book.contents(SheetId(0)).unwrap()(pos) {
            Contents::Number(n) => Some(n),
            _ => None,
        }
    }

    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("book.xlsx");
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_number(0, 0, 2.0).unwrap();
        sheet
            .write_formula(1, 0, rust_xlsxwriter::Formula::new("=A1*21").set_result(""))
            .unwrap();
        book.save(&path).unwrap();
        path
    }

    #[test]
    fn open_edit_save_reopen_keeps_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        let mut opened = open_xlsx(&path).unwrap();
        assert!(opened.unsupported.is_empty());
        let a2 = CellPos::parse_a1("A2").unwrap();
        assert_eq!(opened.workbook.cell(SheetId(0), a2).text, "42");

        opened
            .workbook
            .set_input(SheetId(0), CellPos::parse_a1("A1").unwrap(), "3")
            .unwrap();
        crate::save_xlsx(
            opened
                .workbook
                .save_snapshot(&path, crate::SaveTarget::Document)
                .unwrap(),
        )
        .unwrap();

        let reopened = open_xlsx(&path).unwrap();
        assert_eq!(reopened.workbook.cell(SheetId(0), a2).text, "63");
        assert_eq!(reopened.workbook.input(SheetId(0), a2), "=A1*21");
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "temp file left behind");
    }

    fn planted(dir: &Path, count: u64) -> PathBuf {
        dir.join(format!("~$book.xlsx.{}.{count}.tmp", std::process::id()))
    }

    #[test]
    fn a_leftover_temp_is_skipped_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        for count in 0..3 {
            fs::write(planted(dir.path(), count), b"theirs").unwrap();
        }
        let (_file, temp) = create_temp_counting(&path, &AtomicU64::new(0)).unwrap();
        assert_eq!(temp, planted(dir.path(), 3));
        for count in 0..3 {
            assert_eq!(fs::read(planted(dir.path(), count)).unwrap(), b"theirs");
        }
    }

    #[test]
    fn temp_attempts_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        for count in 0..u64::from(TEMP_ATTEMPTS) {
            fs::write(planted(dir.path(), count), b"theirs").unwrap();
        }
        let failed = create_temp_counting(&path, &AtomicU64::new(0));
        assert!(matches!(
            failed,
            Err(EngineError::Write { ref source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists
        ));
    }

    #[test]
    fn a_leftover_temp_does_not_block_write_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        let leftovers = Leftovers::plant(dir.path(), "book.xlsx");
        write_atomic(&path, b"new", |_| Ok(())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(leftovers.skipped_all());
        leftovers.assert_untouched();
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("empty.xlsx", Vec::new()),
            ("text.xlsx", b"not a zip at all".to_vec()),
            ("truncated.xlsx", {
                let full = fs::read(fixture(dir.path())).unwrap();
                full[..full.len() / 2].to_vec()
            }),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            let error = open_xlsx(&path)
                .err()
                .expect("{name} must fail")
                .to_string();
            assert!(
                error.contains("not a valid xlsx workbook"),
                "{name}: {error}"
            );
            assert!(!error.contains("harm"), "{name}: {error}");
        }
    }

    #[test]
    fn inputs_that_do_not_fit_are_rejected_not_clamped() {
        let mut book = Workbook::new_empty().unwrap();
        let near_end = CellPos::new(zenkai_types::RowIdx::LAST, zenkai_types::ColIdx::default());
        let rows = vec![vec!["1".to_string()], vec!["2".to_string()]];
        assert!(book.set_inputs(SheetId(0), near_end, &rows).is_err());
    }

    #[test]
    fn oversized_typed_formula_is_rejected() {
        let mut book = Workbook::new_empty().unwrap();
        let formula = format!("={}1", "1+".repeat(5_000));
        assert!(
            book.set_input(SheetId(0), CellPos::default(), &formula)
                .is_err()
        );
        let rows = vec![vec![formula]];
        assert!(
            book.set_inputs(SheetId(0), CellPos::default(), &rows)
                .is_err()
        );
    }

    #[test]
    fn paste_inside_zenkai_shifts_relative_references() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![
            vec!["1".to_string(), "=A1*2".to_string()],
            vec!["5".to_string(), String::new()],
        ];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        let copied = book.copy(sheet, zenkai_types::Range::single(b1)).unwrap();
        assert_eq!(copied.text.trim(), "2");
        let b2 = CellPos::parse_a1("B2").unwrap();
        book.paste(sheet, b2, &copied, false).unwrap();
        assert_eq!(book.input(sheet, b2), "=A2*2");
        assert_eq!(book.cell(sheet, b2).text, "10");
    }

    #[test]
    fn inserting_a_row_shifts_formulas_like_excel() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![vec!["2".to_string()], vec!["=A1*3".to_string()]];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        book.insert_rows(sheet, zenkai_types::RowIdx::default(), 1)
            .unwrap();
        let a3 = CellPos::parse_a1("A3").unwrap();
        assert_eq!(book.input(sheet, a3), "=A2*3");
        assert_eq!(book.cell(sheet, a3).text, "6");
        book.delete_columns(sheet, zenkai_types::ColIdx::default(), 1)
            .unwrap();
        assert_eq!(book.cell(sheet, a3).text, "");
        book.set_frozen(sheet, 1, 2).unwrap();
        assert_eq!(book.frozen(sheet), (1, 2));
    }

    #[test]
    fn fill_down_and_right_copy_with_shifted_references() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![
            vec!["1".to_string(), "=A1*10".to_string()],
            vec!["2".to_string(), String::new()],
            vec!["3".to_string(), String::new()],
        ];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        let b1_b3 = zenkai_types::Range::parse_a1("B1:B3").unwrap();
        book.fill(sheet, b1_b3, true).unwrap();
        let b3 = CellPos::parse_a1("B3").unwrap();
        assert_eq!(book.input(sheet, b3), "=A3*10");
        assert_eq!(book.cell(sheet, b3).text, "30");
        let c1 = zenkai_types::Range::parse_a1("C1").unwrap();
        book.fill(sheet, c1, false).unwrap();
        assert_eq!(
            book.input(sheet, CellPos::parse_a1("C1").unwrap()),
            "=B1*10"
        );
        book.undo().unwrap();
        book.undo().unwrap();
        assert_eq!(
            book.input(sheet, b3),
            "",
            "the whole fill undoes in one step"
        );
    }

    #[test]
    fn set_inputs_keeps_quotes_tabs_newlines_and_blanks() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let tricky = vec![
            vec!["say \"hi\"".to_string(), "a	b".to_string()],
            vec![
                String::new(),
                "line 1
line 2"
                    .to_string(),
            ],
            vec!["last".to_string()],
        ];
        book.set_inputs(sheet, CellPos::default(), &tricky).unwrap();
        let at = |a1: &str| book.input(sheet, CellPos::parse_a1(a1).unwrap());
        assert_eq!(at("A1"), "say \"hi\"");
        assert_eq!(at("B1"), "a	b");
        assert_eq!(at("A2"), "");
        assert_eq!(
            at("B2"),
            "line 1
line 2"
        );
        assert_eq!(at("A3"), "last");
    }

    #[test]
    fn column_width_round_trips_in_pixels() {
        let mut book = Workbook::new_empty().unwrap();
        let col = zenkai_types::ColIdx::new(2).unwrap();
        book.set_column_width(SheetId(0), col, 145.0).unwrap();
        let sizes = book.sizes(SheetId(0));
        let span = sizes
            .columns
            .iter()
            .find(|s| s.first <= col && col <= s.last)
            .unwrap();
        assert!((span.width - 145.0).abs() < 1.0, "{}", span.width);
    }

    #[test]
    fn filled_cells_lists_populated_cells_in_order() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = vec![vec!["1".to_string(), String::new(), "x".to_string()]];
        book.set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let cells: Vec<String> = book
            .filled_cells(SheetId(0))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(cells, ["A1", "C1"]);
    }

    #[test]
    fn plus_and_minus_formulas_get_the_formula_limits() {
        let mut book = Workbook::new_empty().unwrap();
        let deep = format!("+{}1{}", "(".repeat(300), ")".repeat(300));
        assert!(
            book.set_input(SheetId(0), CellPos::default(), &deep)
                .is_err()
        );
        let rows = vec![vec![deep.replacen('+', "-", 1)]];
        assert!(
            book.set_inputs(SheetId(0), CellPos::default(), &rows)
                .is_err()
        );
        book.set_input(SheetId(0), CellPos::default(), "-5")
            .unwrap();
        assert_eq!(number_at(&book, CellPos::default()), Some(-5.0));
    }

    #[test]
    fn hidden_sheets_report_their_visibility() {
        let mut source = rust_xlsxwriter::Workbook::new();
        source.add_worksheet().write_string(0, 0, "shown").unwrap();
        source.add_worksheet().set_hidden(true);
        let bytes = source.save_to_buffer().unwrap();
        let book = Workbook::from_xlsx_bytes(&bytes, "hidden").unwrap();
        let visibility: Vec<_> = book.sheets().iter().map(|s| s.visibility).collect();
        assert_eq!(
            visibility,
            [
                zenkai_types::SheetVisibility::Visible,
                zenkai_types::SheetVisibility::Hidden
            ]
        );
    }

    #[test]
    fn empty_hidden_row_on_a_later_sheet_survives_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("two.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        source.add_worksheet().write_string(0, 0, "first").unwrap();
        let second = source.add_worksheet();
        second.write_string(0, 0, "second").unwrap();
        second.set_row_hidden(4).unwrap();
        second.set_row_height(6, 33).unwrap();
        source.save(&path).unwrap();
        let opened = open_xlsx(&path).unwrap();
        let saved = opened.workbook.to_xlsx().unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(saved.clone())).unwrap();
        let mut xml = String::new();
        archive
            .by_name("xl/worksheets/sheet2.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(
            xml.contains(r#"<row r="5""#) && xml.contains(r#"hidden="1""#),
            "{xml}"
        );
        let reopened = Workbook::from_xlsx_bytes(&saved, "two").unwrap();
        let sizes = reopened.sizes(SheetId(1));
        let row = |r| zenkai_types::RowIdx::new(r).unwrap();
        assert!(sizes.rows.iter().any(|(r, _)| *r == row(4)));
        assert!(sizes.rows.iter().any(|(r, h)| *r == row(6) && *h == 44.0));
        assert_eq!(reopened.input(SheetId(1), CellPos::default()), "second");
    }

    #[test]
    fn row_heights_survive_save_even_on_empty_rows() {
        let mut book = Workbook::new_empty().unwrap();
        // Row 4 stays empty: IronCalc alone drops the height of rows without cells.
        let empty = zenkai_types::RowIdx::new(3).unwrap();
        let filled = zenkai_types::RowIdx::new(9).unwrap();
        book.set_row_height(SheetId(0), empty, 48.0).unwrap();
        book.set_row_height(SheetId(0), filled, 30.0).unwrap();
        let a = |row| CellPos::new(row, zenkai_types::ColIdx::clamped(0));
        book.set_input(SheetId(0), a(filled), "x").unwrap();
        book.set_input(SheetId(0), CellPos::default(), "top")
            .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "Book1").unwrap();
        let sizes = reopened.sizes(SheetId(0));
        let height = |row| sizes.rows.iter().find(|(r, _)| *r == row).map(|(_, h)| *h);
        assert_eq!(height(empty), Some(48.0));
        assert_eq!(height(filled), Some(30.0));
        assert_eq!(reopened.input(SheetId(0), a(filled)), "x");
        assert_eq!(reopened.input(SheetId(0), CellPos::default()), "top");
    }

    #[test]
    fn font_and_fill_colours_apply_and_survive_save() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        book.set_input(SheetId(0), a1, "x").unwrap();
        let range = zenkai_types::Range::single(a1);
        let red = zenkai_types::Rgb(0xFF_00_00);
        let yellow = zenkai_types::Rgb(0xFF_FF_00);
        book.apply_style(
            SheetId(0),
            range,
            zenkai_types::StyleChange::FontColor(Some(red)),
        )
        .unwrap();
        book.apply_style(
            SheetId(0),
            range,
            zenkai_types::StyleChange::Fill(Some(yellow)),
        )
        .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "b").unwrap();
        let style = reopened.cell(SheetId(0), a1).style;
        assert_eq!((style.font_color, style.fill), (Some(red), Some(yellow)));
        book.apply_style(SheetId(0), range, zenkai_types::StyleChange::Fill(None))
            .unwrap();
        assert_eq!(book.cell(SheetId(0), a1).style.fill, None);
    }

    #[test]
    fn extend_continues_number_trends_repeats_the_rest_and_undoes_once() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let rows = vec![
            vec!["1".to_string(), "=A1*10".to_string()],
            vec!["2".to_string(), "=A2*10".to_string()],
        ];
        book.set_inputs(SheetId(0), at(0, 0), &rows).unwrap();
        let source = zenkai_types::Range::new(at(0, 0), at(1, 1));
        let target = zenkai_types::Range::new(at(0, 0), at(4, 1));
        book.extend(SheetId(0), source, target).unwrap();
        let column = |c| {
            (0..5)
                .map(|r| book.input(SheetId(0), at(r, c)))
                .collect::<Vec<_>>()
        };
        assert_eq!(column(0), ["1", "2", "3", "4", "5"]);
        assert_eq!(
            column(1),
            ["=A1*10", "=A2*10", "=A3*10", "=A4*10", "=A5*10"]
        );
        book.undo().unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 0)), "");
        let right = zenkai_types::Range::new(at(0, 0), at(1, 3));
        book.extend(SheetId(0), source, right).unwrap();
        assert_eq!(book.input(SheetId(0), at(0, 2)), "1");
        assert_eq!(book.input(SheetId(0), at(1, 3)), "=C2*10");
        book.undo().unwrap();
        let tenths = vec![
            vec!["0.1".to_string()],
            vec!["0.2".to_string()],
            vec!["x".to_string()],
        ];
        book.set_inputs(SheetId(0), at(0, 5), &tenths).unwrap();
        let numbers = zenkai_types::Range::new(at(0, 5), at(1, 5));
        book.extend(
            SheetId(0),
            numbers,
            zenkai_types::Range::new(at(0, 5), at(2, 5)),
        )
        .unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 5)), "0.3");
        let mixed = vec![vec!["a".to_string()], vec!["1".to_string()]];
        book.set_inputs(SheetId(0), at(0, 7), &mixed).unwrap();
        let source = zenkai_types::Range::new(at(0, 7), at(1, 7));
        let target = zenkai_types::Range::new(at(0, 7), at(3, 7));
        book.extend(SheetId(0), source, target).unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 7)), "a");
        assert_eq!(book.input(SheetId(0), at(3, 7)), "1");
        let words = vec![vec!["inf".to_string()], vec!["nan".to_string()]];
        book.set_inputs(SheetId(0), at(0, 9), &words).unwrap();
        let source = zenkai_types::Range::new(at(0, 9), at(1, 9));
        let target = zenkai_types::Range::new(at(0, 9), at(2, 9));
        book.extend(SheetId(0), source, target).unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 9)), "inf");
    }

    #[test]
    fn strike_and_font_size_apply() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        book.set_input(SheetId(0), a1, "x").unwrap();
        let range = zenkai_types::Range::single(a1);
        book.apply_style(SheetId(0), range, zenkai_types::StyleChange::Strike(true))
            .unwrap();
        book.apply_style(SheetId(0), range, zenkai_types::StyleChange::FontSize(14))
            .unwrap();
        let style = book.cell(SheetId(0), a1).style;
        assert!(style.strike);
        assert_eq!(style.font_size, Some(14.0));
    }

    #[test]
    fn border_presets_apply_survive_save_and_clear() {
        use zenkai_types::{BorderPreset, StyleChange};
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let block = zenkai_types::Range::new(at(0, 0), at(1, 1));
        book.apply_style(SheetId(0), block, StyleChange::Borders(BorderPreset::All))
            .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "b").unwrap();
        for pos in [at(0, 0), at(1, 1)] {
            let style = reopened.cell(SheetId(0), pos).style;
            assert!(
                style.border_top && style.border_left && style.border_bottom && style.border_right
            );
        }
        book.apply_style(SheetId(0), block, StyleChange::Borders(BorderPreset::None))
            .unwrap();
        let style = book.cell(SheetId(0), at(1, 1)).style;
        assert!(!style.border_top && !style.border_bottom);
    }

    #[test]
    fn autofill_counts_up_dates_and_numbered_text() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let rows = vec![vec![
            "2026-12-30".to_string(),
            "Item 9".to_string(),
            "Q01".to_string(),
            "hola".to_string(),
        ]];
        book.set_inputs(SheetId(0), at(0, 0), &rows).unwrap();
        let source = zenkai_types::Range::new(at(0, 0), at(0, 3));
        let target = zenkai_types::Range::new(at(0, 0), at(3, 3));
        book.extend(SheetId(0), source, target).unwrap();
        let row = |r| {
            (0..4)
                .map(|c| book.cell(SheetId(0), at(r, c)).text)
                .collect::<Vec<_>>()
        };
        assert_eq!(row(1), ["2026-12-31", "Item 10", "Q02", "hola"]);
        assert_eq!(row(3), ["2027-01-02", "Item 12", "Q04", "hola"]);
    }

    #[test]
    fn autofill_counts_early_1900_dates_as_excel() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(0),
            )
        };
        book.set_inputs(SheetId(0), at(0), &[vec!["1900-02-27".to_string()]])
            .unwrap();
        let source = zenkai_types::Range::new(at(0), at(0));
        let target = zenkai_types::Range::new(at(0), at(3));
        book.extend(SheetId(0), source, target).unwrap();
        let serials = (0..4)
            .map(|r| book.cell(SheetId(0), at(r)).number)
            .collect::<Vec<_>>();
        assert_eq!(serials, [Some(58.0), Some(59.0), Some(60.0), Some(61.0)]);
        let texts = (0..4)
            .map(|r| book.cell(SheetId(0), at(r)).text)
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            ["1900-02-27", "1900-02-28", "1900-02-29", "1900-03-01"]
        );
    }

    fn fill_two_values_down(first: &str, second: &str, format: &str) -> Vec<(Option<f64>, String)> {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(0),
            )
        };
        book.set_inputs(
            SheetId(0),
            at(0),
            &[vec![first.to_string()], vec![second.to_string()]],
        )
        .unwrap();
        let source = zenkai_types::Range::new(at(0), at(1));
        book.set_number_format(SheetId(0), source, format).unwrap();
        let target = zenkai_types::Range::new(at(0), at(3));
        book.extend(SheetId(0), source, target).unwrap();
        (0..4)
            .map(|r| {
                let cell = book.cell(SheetId(0), at(r));
                (cell.number, cell.text)
            })
            .collect()
    }

    #[test]
    fn autofill_ignores_date_letters_inside_literals_and_brackets() {
        for format in ["0\" days\"", "0;[Red]0", r"\d0"] {
            let numbers = fill_two_values_down("1", "2", format)
                .into_iter()
                .map(|(number, _)| number)
                .collect::<Vec<_>>();
            assert_eq!(
                numbers,
                [Some(1.0), Some(2.0), Some(3.0), Some(4.0)],
                "{format}"
            );
        }
    }

    #[test]
    fn autofill_steps_dates_across_the_1900_leap_day() {
        let filled = fill_two_values_down("58", "59", "[$-409]d-mmm-yy");
        let texts = filled
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(texts, ["27-Feb-00", "28-Feb-00", "29-Feb-00", "1-Mar-00"]);
        for (first, expected) in [
            (59.0, [59.0, 60.0, 61.0, 62.0]),
            (60.0, [60.0, 61.0, 62.0, 63.0]),
        ] {
            let second = (first + 1.0).to_string();
            let filled = fill_two_values_down(&first.to_string(), &second, "[$-409]d-mmm-yy");
            let serials = filled
                .into_iter()
                .map(|(number, _)| number)
                .collect::<Vec<_>>();
            assert_eq!(serials, expected.map(Some), "{first}");
        }
    }

    #[test]
    fn autofill_counts_up_from_serial_zero_in_the_source_format() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(0),
            )
        };
        book.set_inputs(SheetId(0), at(0), &[vec!["0".to_string()]])
            .unwrap();
        let source = zenkai_types::Range::new(at(0), at(0));
        book.set_number_format(SheetId(0), source, "m/d/yyyy")
            .unwrap();
        let target = zenkai_types::Range::new(at(0), at(3));
        book.extend(SheetId(0), source, target).unwrap();
        let serials = (0..4)
            .map(|r| book.cell(SheetId(0), at(r)).number)
            .collect::<Vec<_>>();
        assert_eq!(serials, [Some(0.0), Some(1.0), Some(2.0), Some(3.0)]);
        let texts = (0..4)
            .map(|r| book.cell(SheetId(0), at(r)).text)
            .collect::<Vec<_>>();
        assert_eq!(texts, ["1/0/1900", "1/1/1900", "1/2/1900", "1/3/1900"]);
    }

    #[test]
    fn sort_orders_rows_like_excel_and_moves_formulas() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let rows: Vec<Vec<String>> = [
            ["banana", "=B1"],
            ["", "x"],
            ["10", "y"],
            ["Apple", "z"],
            ["2", "w"],
        ]
        .iter()
        .map(|r| r.iter().map(ToString::to_string).collect())
        .collect();
        book.set_inputs(SheetId(0), at(0, 0), &rows).unwrap();
        let range = zenkai_types::Range::new(at(0, 0), at(4, 1));
        book.sort(SheetId(0), range, zenkai_types::ColIdx::clamped(0), false)
            .unwrap();
        let column = |book: &Workbook, c| {
            (0..5)
                .map(|r| book.input(SheetId(0), at(r, c)))
                .collect::<Vec<_>>()
        };
        assert_eq!(column(&book, 0), ["2", "10", "Apple", "banana", ""]);
        assert_eq!(column(&book, 1), ["w", "y", "z", "=B4", "x"]);
        book.undo().unwrap();
        assert_eq!(book.input(SheetId(0), at(0, 0)), "banana");
        book.sort(SheetId(0), range, zenkai_types::ColIdx::clamped(0), true)
            .unwrap();
        assert_eq!(column(&book, 0), ["banana", "Apple", "10", "2", ""]);
    }

    #[test]
    fn sort_refuses_ranges_with_merged_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        let sheet = source.add_worksheet();
        sheet.write_string(0, 0, "b").unwrap();
        sheet.write_string(1, 0, "a").unwrap();
        sheet
            .merge_range(2, 0, 2, 1, "merged", &rust_xlsxwriter::Format::new())
            .unwrap();
        source.save(&path).unwrap();
        let mut book = open_xlsx(&path).unwrap().workbook;
        let range = zenkai_types::Range::parse_a1("A1:B3").unwrap();
        let sorted = book.sort(SheetId(0), range, zenkai_types::ColIdx::clamped(0), false);
        assert!(sorted.is_err());
        assert_eq!(book.input(SheetId(0), CellPos::default()), "b");
    }

    #[test]
    fn wrap_and_vertical_alignment_load_and_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        let format = rust_xlsxwriter::Format::new()
            .set_text_wrap()
            .set_align(rust_xlsxwriter::FormatAlign::Top);
        source
            .add_worksheet()
            .write_string_with_format(0, 0, "long header", &format)
            .unwrap();
        source.save(&path).unwrap();
        let mut book = open_xlsx(&path).unwrap().workbook;
        let style = book.cell(SheetId(0), CellPos::default()).style;
        assert!(style.wrap);
        assert_eq!(style.valign, zenkai_types::VAlign::Top);
        let a1 = zenkai_types::Range::single(CellPos::default());
        book.apply_style(SheetId(0), a1, zenkai_types::StyleChange::Wrap(false))
            .unwrap();
        assert!(!book.cell(SheetId(0), CellPos::default()).style.wrap);
    }

    #[test]
    fn hidden_rows_and_columns_have_no_size_and_unhide() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = zenkai_types::Range::parse_a1("A2:A3").unwrap();
        let cols = zenkai_types::Range::parse_a1("C1:D1").unwrap();
        book.set_rows_hidden(SheetId(0), rows, true).unwrap();
        book.set_columns_hidden(SheetId(0), cols, true).unwrap();
        let sizes = book.sizes(SheetId(0));
        assert!(sizes.rows.iter().any(|(r, h)| r.get() == 1 && *h == 0.0));
        assert!(
            sizes
                .columns
                .iter()
                .any(|s| s.first.get() <= 2 && 2 <= s.last.get() && s.width == 0.0)
        );
        book.set_rows_hidden(SheetId(0), rows, false).unwrap();
        assert!(!book.sizes(SheetId(0)).rows.iter().any(|(_, h)| *h == 0.0));
        book.undo().unwrap();
        assert!(book.sizes(SheetId(0)).rows.iter().any(|(_, h)| *h == 0.0));
    }

    #[test]
    fn scattered_inputs_write_cells_and_recalculate_once_done() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::parse_a1("A1").unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        let c3 = CellPos::parse_a1("C3").unwrap();
        book.set_input(SheetId(0), a1, "1").unwrap();
        book.set_input(SheetId(0), b1, "=A1*2").unwrap();
        let changes = vec![(a1, "5".to_string()), (c3, "hola".to_string())];
        book.set_scattered_inputs(SheetId(0), &changes).unwrap();
        assert_eq!(number_at(&book, b1), Some(10.0));
        assert_eq!(book.input(SheetId(0), c3), "hola");
        let deep = vec![(c3, format!("={}1{}", "(".repeat(300), ")".repeat(300)))];
        assert!(book.set_scattered_inputs(SheetId(0), &deep).is_err());
    }

    #[test]
    fn format_preview_matches_excel_codes() {
        use crate::workbook::format_preview;
        assert_eq!(format_preview(1234.5, "#,##0.00").unwrap(), "1,234.50");
        assert_eq!(format_preview(0.256, "0.0%").unwrap(), "25.6%");
        assert!(format_preview(1.0, &"0".repeat(256)).is_err());
    }

    #[test]
    fn duplicate_sheet_copies_cells_next_to_the_source() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), CellPos::default(), "hola")
            .unwrap();
        book.add_sheet().unwrap();
        book.duplicate_sheet(SheetId(0)).unwrap();
        let names: Vec<String> = book.sheets().into_iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 3);
        assert!(names[1].starts_with("Sheet1 ("), "{names:?}");
        assert_eq!(book.input(SheetId(1), CellPos::default()), "hola");
        book.undo().unwrap();
        assert_eq!(book.sheets().len(), 2);
        book.redo().unwrap();
        let row = zenkai_types::RowIdx::new(4).unwrap();
        book.set_row_height(SheetId(1), row, 40.0).unwrap();
        book.set_input(
            SheetId(1),
            CellPos::new(row, zenkai_types::ColIdx::clamped(1)),
            "=A1",
        )
        .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "b").unwrap();
        assert_eq!(reopened.sheets().len(), 3);
        assert_eq!(reopened.input(SheetId(1), CellPos::default()), "hola");
        let sizes = reopened.sizes(SheetId(1));
        assert!(sizes.rows.iter().any(|(r, h)| *r == row && *h == 40.0));
    }

    #[test]
    fn clear_formats_keeps_values_and_clear_all_empties() {
        use zenkai_types::StyleChange;
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        book.set_input(SheetId(0), a1, "7").unwrap();
        book.apply_style(SheetId(0), Range::single(a1), StyleChange::Bold(true))
            .unwrap();
        book.clear_formats(SheetId(0), Range::single(a1)).unwrap();
        assert!(!book.cell(SheetId(0), a1).style.bold);
        assert_eq!(book.input(SheetId(0), a1), "7");
        book.clear_all(SheetId(0), Range::single(a1)).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        book.set_input(SheetId(0), a1, "8").unwrap();
        let sheet = Range::parse_a1("A1:XFD1048576").unwrap();
        book.clear(SheetId(0), sheet).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        book.set_input(SheetId(0), a1, "9").unwrap();
        book.clear_all(SheetId(0), sheet).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        // Clearing far past the data must not create cells that grow the used area.
        book.set_input(SheetId(0), a1, "1").unwrap();
        let before = book.used_end(SheetId(0));
        let far = Range::parse_a1("XFD1048576").unwrap();
        book.clear(SheetId(0), far).unwrap();
        book.clear_all(SheetId(0), far).unwrap();
        assert_eq!(book.used_end(SheetId(0)), before);
        // A formatted far cell makes the used area the whole sheet; Delete over it must
        // still only touch the contents.
        book.apply_style(SheetId(0), far, StyleChange::Bold(true))
            .unwrap();
        book.set_input(SheetId(0), CellPos::parse_a1("XFD1048576").unwrap(), "far")
            .unwrap();
        book.clear(SheetId(0), sheet).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        assert_eq!(
            book.input(SheetId(0), CellPos::parse_a1("XFD1048576").unwrap()),
            ""
        );
        let huge = Range::parse_a1("B2:XFD1048576").unwrap();
        assert!(
            book.apply_style(SheetId(0), huge, StyleChange::Bold(true))
                .is_err()
        );
    }

    #[test]
    fn fill_with_puts_one_entry_in_every_cell_shifting_formulas() {
        let mut book = Workbook::new_empty().unwrap();
        let range = zenkai_types::Range::parse_a1("B1:C2").unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        book.fill_with(SheetId(0), b1, "=A1*2", range).unwrap();
        let column = zenkai_types::Range::parse_a1("D1:D1048576").unwrap();
        assert!(
            book.fill_with(SheetId(0), CellPos::parse_a1("D1").unwrap(), "x", column)
                .is_err()
        );
        assert_eq!(book.input(SheetId(0), CellPos::parse_a1("D1").unwrap()), "");
        let at = |a1: &str| book.input(SheetId(0), CellPos::parse_a1(a1).unwrap());
        assert_eq!(
            (at("B2"), at("C1"), at("C2")),
            ("=A2*2".into(), "=B1*2".into(), "=B2*2".into())
        );
    }

    #[test]
    fn fill_with_is_one_undo_step_that_keeps_earlier_edits() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |a1: &str| CellPos::parse_a1(a1).unwrap();
        book.set_input(SheetId(0), at("C1"), "seed").unwrap();
        let range = zenkai_types::Range::parse_a1("A1:A3").unwrap();
        book.fill_with(SheetId(0), at("A1"), "=B1*2", range)
            .unwrap();
        book.undo().unwrap();
        for a1 in ["A1", "A2", "A3"] {
            assert_eq!(book.input(SheetId(0), at(a1)), "");
        }
        assert_eq!(book.input(SheetId(0), at("C1")), "seed");
        book.redo().unwrap();
        assert_eq!(book.input(SheetId(0), at("A3")), "=B3*2");
        book.undo().unwrap();
        book.undo().unwrap();
        assert_eq!(book.input(SheetId(0), at("C1")), "");
    }

    #[test]
    fn fill_with_puts_in_every_cell_what_typing_it_there_would() {
        let at = |a1: &str| CellPos::parse_a1(a1).unwrap();
        let range = zenkai_types::Range::parse_a1("A1:A3").unwrap();
        for typed in ["1/2/2024", "5%", "007", "'007"] {
            let mut filled = Workbook::new_empty().unwrap();
            filled
                .fill_with(SheetId(0), at("A1"), typed, range)
                .unwrap();
            let mut keyed = Workbook::new_empty().unwrap();
            for a1 in ["A1", "A2", "A3"] {
                keyed.set_input(SheetId(0), at(a1), typed).unwrap();
            }
            for a1 in ["A1", "A2", "A3"] {
                assert_eq!(
                    filled.cell(SheetId(0), at(a1)),
                    keyed.cell(SheetId(0), at(a1)),
                    "{typed} at {a1}"
                );
                assert_eq!(
                    filled.input(SheetId(0), at(a1)),
                    keyed.input(SheetId(0), at(a1)),
                    "{typed} at {a1}"
                );
            }
        }
    }

    #[test]
    fn used_end_follows_every_edit() {
        let mut book = Workbook::new_empty().unwrap();
        let (c5, e9) = (
            CellPos::parse_a1("C5").unwrap(),
            CellPos::parse_a1("E9").unwrap(),
        );
        book.set_input(SheetId(0), c5, "1").unwrap();
        assert_eq!(book.used_end(SheetId(0)), c5);
        book.set_input(SheetId(0), e9, "2").unwrap();
        assert_eq!(book.used_end(SheetId(0)), e9);
    }

    #[test]
    fn a_profiled_edit_writes_like_set_input_and_reports_the_recalculation() {
        let mut book = Workbook::new_empty().unwrap();
        let (a1, a2) = (
            CellPos::parse_a1("A1").unwrap(),
            CellPos::parse_a1("A2").unwrap(),
        );
        book.set_input(SheetId(0), a2, "=A1*2").unwrap();
        let profile = book.set_input_profiled(SheetId(0), a1, "4").unwrap();
        assert_eq!(profile.mode, crate::RecalcMode::Incremental);
        assert_eq!(book.input(SheetId(0), a1), "4");
        assert_eq!(book.cell(SheetId(0), a2).text, "8");
        assert_eq!(book.used_end(SheetId(0)), a2);
    }

    #[test]
    fn contents_tell_values_and_formulas_from_styled_blanks() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |text: &str| CellPos::parse_a1(text).unwrap();
        book.set_input(SheetId(0), at("A1"), "1").unwrap();
        book.set_input(SheetId(0), at("A2"), "=\"\"").unwrap();
        book.apply_style(
            SheetId(0),
            zenkai_types::Range::single(at("A3")),
            zenkai_types::StyleChange::Bold(true),
        )
        .unwrap();
        let contents = book.contents(SheetId(0)).unwrap();
        assert_eq!(contents(at("A1")), Contents::Number(1.0));
        assert_eq!(contents(at("A2")), Contents::NonNumeric);
        assert_eq!(contents(at("A3")), Contents::Empty);
        assert_eq!(contents(at("A4")), Contents::Empty);
    }

    #[test]
    fn contents_of_a_missing_sheet_is_an_error() {
        let book = Workbook::new_empty().unwrap();
        assert!(matches!(
            book.contents(SheetId(7)),
            Err(EngineError::UnknownSheet(SheetId(7)))
        ));
    }

    #[test]
    fn clearing_a_missing_sheet_is_an_error() {
        let mut book = Workbook::new_empty().unwrap();
        let range = Range::new(CellPos::default(), CellPos::default());
        assert!(matches!(
            book.clear(SheetId(7), range),
            Err(EngineError::UnknownSheet(SheetId(7)))
        ));
    }

    #[test]
    fn undo_restores_previous_value() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::parse_a1("A1").unwrap();
        book.set_input(SheetId(0), a1, "1").unwrap();
        book.set_input(SheetId(0), a1, "2").unwrap();
        book.undo().unwrap();
        assert_eq!(book.cell(SheetId(0), a1).text, "1");
        book.redo().unwrap();
        assert_eq!(book.cell(SheetId(0), a1).text, "2");
    }
}
