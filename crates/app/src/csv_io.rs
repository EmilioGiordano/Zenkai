use std::io::{Read, Write};
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use zenkai_agent::protected_view::file_origin;
use zenkai_engine::{
    Engine, EngineError, ShownCells, Workbook, run_with_engine_stack, write_atomic_streaming,
};
use zenkai_formats::{
    CsvError, CsvWriter, Delimiter, MAX_CSV_BYTES, ParsedCsv, apply, guess, parse_csv,
};
use zenkai_i18n::t;
use zenkai_types::{CellPos, SheetId};

use crate::csv_options::CsvOptions;
use crate::document::read_shared;
use crate::entry::{CsvLoad, FileSeen};
use crate::files::{FileLoad, LoadFailure, export_delimiter, tsv_delimiter};

pub struct CsvRead {
    pub bytes: Vec<u8>,
    pub parsed: ParsedCsv,
    pub seen: FileSeen,
}

enum ReadStop {
    Io(std::io::Error),
    TooLarge(usize),
}

impl ReadStop {
    fn message(self, path: &Path) -> String {
        match self {
            ReadStop::Io(error) => t!("file.read_failed", path = path.display(), error = error),
            ReadStop::TooLarge(megabytes) => CsvError::TooLarge(megabytes).to_string(),
        }
    }

    fn into_engine_error(self, path: &Path) -> EngineError {
        match self {
            ReadStop::Io(source) => EngineError::Read {
                path: path.to_path_buf(),
                source,
            },
            ReadStop::TooLarge(megabytes) => {
                EngineError::Rejected(CsvError::TooLarge(megabytes).to_string())
            }
        }
    }
}

// The size is checked on the open file and the read is capped too, so a file that grows
// meanwhile cannot fill memory. The stamp is taken before the read, so a change in between counts.
fn read_with_stamp(path: &Path, limit: usize) -> Result<(Vec<u8>, FileSeen), ReadStop> {
    let seen = FileSeen::of(path);
    let file = std::fs::File::open(path).map_err(ReadStop::Io)?;
    let length = file.metadata().map_err(ReadStop::Io)?.len();
    let megabytes = |bytes: u64| usize::try_from(bytes / 1024 / 1024).unwrap_or(usize::MAX);
    if length > limit as u64 {
        return Err(ReadStop::TooLarge(megabytes(length)));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or_default());
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ReadStop::Io)?;
    if bytes.len() > limit {
        return Err(ReadStop::TooLarge(megabytes(bytes.len() as u64)));
    }
    Ok((bytes, seen))
}

pub fn read_csv(path: &Path) -> Result<CsvRead, String> {
    let (bytes, seen) = read_with_stamp(path, MAX_CSV_BYTES).map_err(|stop| stop.message(path))?;
    let parsed = parse_csv(&bytes, tsv_delimiter(path)).map_err(|e| e.to_string())?;
    Ok(CsvRead {
        bytes,
        parsed,
        seen,
    })
}

pub fn detect_csv_options(path: &Path) -> Result<CsvOptions, String> {
    let read = read_csv(path)?;
    Ok(CsvOptions::from_read(&read.parsed, &guess(&read.parsed)))
}

pub struct Baseline {
    pub rows: Vec<Vec<String>>,
    pub ends_with_newline: bool,
}

pub enum BaselineRead {
    Present(Baseline),
    Absent,
    Lossy,
    ShapeUnknown,
}

// Only a missing file is no baseline: one that cannot be read or parsed fails the save, which
// would otherwise write every cell as shown over a file it could not look at.
pub fn read_baseline(path: &Path, delimiter: Delimiter) -> Result<BaselineRead, EngineError> {
    let (bytes, _) = match read_with_stamp(path, MAX_CSV_BYTES) {
        Ok(read) => read,
        Err(ReadStop::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BaselineRead::Absent);
        }
        Err(stop) => return Err(stop.into_engine_error(path)),
    };
    let parsed = parse_csv(&bytes, Some(delimiter))
        .map_err(|error| EngineError::Rejected(error.to_string()))?;
    Ok(if parsed.lossy {
        BaselineRead::Lossy
    } else if parsed.shape_unknown {
        BaselineRead::ShapeUnknown
    } else {
        BaselineRead::Present(Baseline {
            rows: parsed.rows,
            ends_with_newline: parsed.ends_with_newline,
        })
    })
}

// Opens a CSV as the document it is, without a preview: with the stored choices, or those read
// from the file. Encoding and line ending are always the file's own as it is now.
pub fn load_csv(path: &Path, stored: Option<CsvOptions>) -> Result<FileLoad, LoadFailure> {
    let (bytes, seen) = read_with_stamp(path, MAX_CSV_BYTES).map_err(|stop| match stop {
        ReadStop::Io(error) if error.kind() == std::io::ErrorKind::NotFound => LoadFailure::Missing,
        other => LoadFailure::Engine(other.into_engine_error(path)),
    })?;
    let hint = stored
        .map(|options| options.delimiter)
        .or_else(|| tsv_delimiter(path));
    let parsed = parse_csv(&bytes, hint)
        .map_err(|e| LoadFailure::Engine(EngineError::Rejected(e.to_string())))?;
    let options = match stored {
        Some(stored) => CsvOptions {
            encoding: parsed.encoding,
            line_ending: parsed.line_ending,
            ..stored
        },
        None => CsvOptions::from_read(&parsed, &guess(&parsed)),
    };
    let mut rows = parsed.rows;
    apply(options.decimal_mark, options.date_order, &mut rows);
    let workbook =
        workbook_from_rows(rows).map_err(|e| LoadFailure::Engine(EngineError::Rejected(e)))?;
    Ok(FileLoad {
        workbook,
        unsupported: Vec::new(),
        read_only: false,
        origin: file_origin(path),
        reader_fallback: None,
        csv: Some(CsvLoad { options, seen }),
    })
}

pub fn workbook_from_rows(rows: Vec<Vec<String>>) -> Result<Workbook, String> {
    run_with_engine_stack(move || {
        let mut workbook = Workbook::new_empty()?;
        workbook.set_inputs(SheetId(0), CellPos::default(), &rows)?;
        workbook.warm_used_areas();
        Ok(workbook)
    })
    .map_err(|e| e.to_string())
}

// The read lock is held only while the shown cells are gathered; the write can take far
// longer. Memory follows the cells, never the area up to the sheet's end.
pub fn gather_export(shared: &RwLock<Workbook>, sheet: SheetId) -> Result<ShownCells, EngineError> {
    read_shared(shared).shown_cells(sheet)
}

pub fn write_csv_file(
    path: &Path,
    shown: &ShownCells,
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    let options = CsvOptions::exported_as(export_delimiter(path));
    write_csv_as(path, shown, &options, &Layout::Rectangle, cancel)
}

pub struct AsRead<'a> {
    pub row_lengths: &'a [usize],
    pub content_rows: usize,
    pub ends_with_newline: bool,
}

pub enum Layout<'a> {
    Rectangle,
    AsRead(AsRead<'a>),
}

pub fn write_csv_as(
    path: &Path,
    shown: &ShownCells,
    options: &CsvOptions,
    layout: &Layout,
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    write_atomic_streaming(path, |out| {
        write_shown_rows(out, shown, options, layout, cancel).map_err(|error| match error {
            ExportStop::Cancelled => EngineError::Cancelled,
            ExportStop::Csv(CsvError::Write(source))
                if source.kind() == std::io::ErrorKind::InvalidData =>
            {
                EngineError::Rejected(t!("csv.cannot_encode", encoding = options.encoding.label()))
            }
            ExportStop::Csv(CsvError::Write(source)) => EngineError::Write {
                path: path.to_path_buf(),
                source,
            },
            ExportStop::Csv(other) => EngineError::Rejected(other.to_string()),
        })
    })
}

enum ExportStop {
    Cancelled,
    Csv(CsvError),
}

impl From<CsvError> for ExportStop {
    fn from(error: CsvError) -> Self {
        ExportStop::Csv(error)
    }
}

// A row never has fewer fields than it had; one past the file's end has as many as its widest
// row, as Excel pads to the used range.
fn field_count(cells: &[&(CellPos, String)], file_length: Option<usize>, widest: usize) -> usize {
    let extent = cells
        .last()
        .map_or(0, |(pos, _)| usize::from(pos.col.get()) + 1);
    extent.max(file_length.unwrap_or(widest))
}

// Every row up to the sheet's end has as many fields as the sheet has columns, as when
// the whole rectangle was written; gaps are written without building a string each.
fn write_shown_rows(
    out: &mut dyn Write,
    shown: &ShownCells,
    options: &CsvOptions,
    layout: &Layout,
    cancel: &AtomicBool,
) -> Result<(), ExportStop> {
    let sheet_width = usize::from(shown.end.col.get()) + 1;
    let as_read = match layout {
        Layout::Rectangle => None,
        Layout::AsRead(as_read) => Some(as_read),
    };
    let widest = as_read.map_or(0, |read| {
        read.row_lengths.iter().copied().max().unwrap_or(0)
    });
    let mut writer = CsvWriter::in_format(out, options.format())?;
    let mut cells = shown.cells.iter().peekable();
    let mut row_cells = Vec::new();
    // The file's empty rows after its last cell stay while the content still ends where it did.
    let rows = as_read.map_or(shown.end.row.get() + 1, |read| {
        let file_rows = u32::try_from(read.row_lengths.len()).unwrap_or(u32::MAX);
        let content_end = if shown.cells.is_empty() {
            0
        } else {
            shown.end.row.get() + 1
        };
        match usize::try_from(content_end) {
            Ok(end) if end >= read.content_rows => content_end.max(file_rows),
            _ => content_end,
        }
    });
    for row in 0..rows {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportStop::Cancelled);
        }
        row_cells.clear();
        while let Some(cell) = cells.next_if(|(pos, _)| pos.row.get() == row) {
            row_cells.push(cell);
        }
        let width = match as_read {
            None => sheet_width,
            Some(as_read) => field_count(
                &row_cells,
                usize::try_from(row)
                    .ok()
                    .and_then(|index| as_read.row_lengths.get(index).copied()),
                widest,
            ),
        };
        if width == 0 {
            writer.write_blank_line()?;
            continue;
        }
        let mut next_col = 0;
        for (pos, text) in &row_cells {
            let col = usize::from(pos.col.get());
            writer.write_empty_fields(col.saturating_sub(next_col))?;
            // Over its own file the text stays as typed, as in Excel: a prefix would corrupt it
            // on reopening. Only a file for another program neutralizes.
            if as_read.is_some() {
                writer.write_verbatim_field(text)?;
            } else {
                writer.write_field(text)?;
            }
            next_col = col + 1;
        }
        writer.write_empty_fields(width.saturating_sub(next_col))?;
        let unterminated = row + 1 == rows && as_read.is_some_and(|read| !read.ends_with_newline);
        if !unterminated {
            writer.end_row()?;
        }
    }
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use zenkai_formats::{DecimalMark, Encoding};
    use zenkai_types::{ColIdx, Contents, RowIdx};

    use super::*;

    fn number_at(workbook: &Workbook, col: u16) -> Option<f64> {
        let pos = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(i64::from(col)));
        match workbook.contents(SheetId(0)).unwrap()(pos) {
            Contents::Number(n) => Some(n),
            _ => None,
        }
    }

    #[test]
    fn csv_import_then_export_keeps_values() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("in.csv");
        std::fs::write(&source, "name;qty\nAna;3\nLuis;4\n").unwrap();
        let parsed = read_csv(&source).unwrap().parsed;
        assert_eq!(parsed.delimiter, Delimiter::Semicolon);
        let workbook = workbook_from_rows(parsed.rows).unwrap();
        let target = dir.path().join("out.csv");
        let shown = workbook.shown_cells(SheetId(0)).unwrap();
        write_csv_file(&target, &shown, &AtomicBool::new(false)).unwrap();
        let written = std::fs::read(&target).unwrap();
        assert_eq!(&written[3..], b"name,qty\r\nAna,3\r\nLuis,4\r\n");
    }

    fn at(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    fn sheet_with(cells: &[(CellPos, &str)]) -> Workbook {
        let mut workbook = Workbook::new_empty().unwrap();
        for (pos, text) in cells {
            workbook.set_input(SheetId(0), *pos, text).unwrap();
        }
        workbook
    }

    // The export before it streamed: one string per cell of the used rectangle.
    fn whole_rectangle_export(workbook: &Workbook, delimiter: Delimiter) -> Vec<u8> {
        let end = workbook.used_end(SheetId(0));
        let rows: Vec<Vec<String>> = (0..=i64::from(end.row.get()))
            .map(|row| {
                (0..=i64::from(end.col.get()))
                    .map(|col| workbook.cell(SheetId(0), at(row, col)).text)
                    .collect()
            })
            .collect();
        zenkai_formats::write_csv(&rows, delimiter).unwrap()
    }

    fn exported(workbook: Workbook, name: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(name);
        let shown = gather_export(&RwLock::new(workbook), SheetId(0)).unwrap();
        write_csv_file(&target, &shown, &AtomicBool::new(false)).unwrap();
        std::fs::read(&target).unwrap()
    }

    #[test]
    fn streamed_export_writes_the_bytes_of_the_whole_rectangle() {
        let sheets: Vec<(&str, Vec<(CellPos, &str)>)> = vec![
            (
                "gaps.csv",
                vec![
                    (at(0, 0), "name"),
                    (at(0, 3), "a,b"),
                    (at(1, 1), "say \"hi\""),
                    (at(3, 0), "@user"),
                    (at(3, 2), "=SEQUENCE(3)"),
                    (at(4, 4), "-x"),
                ],
            ),
            (
                "one column.csv",
                vec![(at(0, 0), "top"), (at(3, 0), "bottom")],
            ),
            ("tabs.tsv", vec![(at(0, 1), "a\tb"), (at(2, 0), "=1/0")]),
            ("empty.csv", vec![]),
        ];
        for (name, cells) in sheets {
            let workbook = sheet_with(&cells);
            let delimiter = if name.ends_with(".tsv") {
                Delimiter::Tab
            } else {
                Delimiter::Comma
            };
            let expected = whole_rectangle_export(&workbook, delimiter);
            assert_eq!(exported(workbook, name), expected, "{name}");
        }
    }

    #[test]
    fn a_cancelled_export_removes_its_temp_and_keeps_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.csv");
        std::fs::write(&target, "old contents").unwrap();
        let workbook = sheet_with(&[(at(0, 0), "a"), (at(5, 3), "b")]);
        let shown = gather_export(&RwLock::new(workbook), SheetId(0)).unwrap();
        let result = write_csv_file(&target, &shown, &AtomicBool::new(true));
        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert_eq!(std::fs::read(&target).unwrap(), b"old contents");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn a_cancelled_export_to_a_new_name_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new.csv");
        let workbook = sheet_with(&[(at(0, 0), "a")]);
        let shown = gather_export(&RwLock::new(workbook), SheetId(0)).unwrap();
        let result = write_csv_file(&target, &shown, &AtomicBool::new(true));
        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn spills_gaps_and_empty_rows_export_as_the_sheet_shows_them() {
        let workbook = sheet_with(&[(at(0, 0), "=SEQUENCE(3)"), (at(1, 2), "x")]);
        assert_eq!(
            &exported(workbook, "spill.csv")[3..],
            b"1,,\r\n2,,x\r\n3,,\r\n"
        );
        let workbook = sheet_with(&[(at(0, 0), "top"), (at(2, 0), "bottom")]);
        assert_eq!(
            &exported(workbook, "column.csv")[3..],
            b"top\r\n\"\"\r\nbottom\r\n"
        );
    }

    #[test]
    fn a_sheet_reaching_the_last_column_exports_full_width_rows() {
        let last_col = i64::from(zenkai_types::MAX_COLS - 1);
        let workbook = sheet_with(&[(at(0, 0), "first"), (at(49, last_col), "last")]);
        let bytes = exported(workbook, "wide.csv");
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.split_terminator("\r\n").collect();
        let gap = ",".repeat(usize::from(zenkai_types::MAX_COLS) - 1);
        assert_eq!(lines.len(), 50);
        assert_eq!(lines[0], format!("first{gap}"));
        assert_eq!(lines[1], gap);
        assert_eq!(lines[49], format!("{gap}last"));
    }

    #[test]
    fn decimal_comma_fields_import_as_numbers() {
        let mut rows = vec![vec!["1.234,5".to_string(), "=A1*2".to_string()]];
        zenkai_formats::normalize_decimal_comma(&mut rows);
        let workbook = workbook_from_rows(rows).unwrap();
        let doubled = number_at(&workbook, 1);
        assert_eq!(doubled, Some(2469.0));
    }

    #[test]
    fn day_first_dates_import_as_the_right_day() {
        let mut rows = vec![vec!["08/10/2026".to_string(), "25/10/2026".to_string()]];
        zenkai_formats::normalize_day_first(&mut rows);
        let workbook = workbook_from_rows(rows).unwrap();
        let serial = |col| number_at(&workbook, col);
        // 8 October 2026 and 25 October 2026 as Excel serials.
        assert_eq!(serial(0), Some(46303.0));
        assert_eq!(serial(1), Some(46320.0));
    }

    #[test]
    fn a_csv_loads_with_the_options_found_in_it_and_keeps_the_choices_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prices.csv");
        std::fs::write(&path, "1,5;2\r\n3;4\r\n").unwrap();

        let found = load_csv(&path, None).unwrap();
        let csv = found.csv.unwrap();
        assert_eq!(csv.options.delimiter, Delimiter::Semicolon);
        assert_eq!(csv.options.encoding, zenkai_formats::Encoding::Utf8);
        assert_eq!(csv.options.line_ending, zenkai_formats::LineEnding::Crlf);
        assert_eq!(csv.options.decimal_mark, DecimalMark::Comma);
        assert_eq!(number_at(&found.workbook, 0), Some(1.5));

        let stored = CsvOptions {
            decimal_mark: DecimalMark::Dot,
            ..csv.options
        };
        let again = load_csv(&path, Some(stored)).unwrap();
        assert_eq!(again.csv.unwrap().options, stored);
        assert_eq!(number_at(&again.workbook, 0), None);
    }

    fn lf_options(target: &Path) -> CsvOptions {
        CsvOptions {
            encoding: Encoding::Utf8,
            line_ending: zenkai_formats::LineEnding::Lf,
            ..CsvOptions::exported_as(export_delimiter(target))
        }
    }

    #[test]
    fn only_text_edited_since_the_read_is_neutralized_and_an_export_neutralizes_all() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.csv");
        let shown = ShownCells {
            end: at(0, 4),
            cells: vec![
                (at(0, 0), "-".to_string()),
                (at(0, 1), "+34 600".to_string()),
                (at(0, 2), "=x".to_string()),
                (at(0, 3), "-12%".to_string()),
                (at(0, 4), "ok".to_string()),
            ],
        };
        let cancel = AtomicBool::new(false);
        let layout = Layout::AsRead(AsRead {
            row_lengths: &[5],
            content_rows: 1,
            ends_with_newline: true,
        });
        write_csv_as(&target, &shown, &lf_options(&target), &layout, &cancel).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"-,+34 600,=x,-12%,ok\n");
        write_csv_file(&target, &shown, &cancel).unwrap();
        assert_eq!(
            &std::fs::read(&target).unwrap()[3..],
            b"'-,'+34 600,'=x,'-12%,ok\r\n"
        );
    }

    #[test]
    fn a_file_keeps_its_row_lengths_empty_lines_and_missing_final_line_break() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.csv");
        let shown = ShownCells {
            end: at(3, 2),
            cells: vec![
                (at(0, 0), "a".to_string()),
                (at(0, 1), "b".to_string()),
                (at(2, 0), "c".to_string()),
                (at(3, 2), "z".to_string()),
            ],
        };
        let layout = Layout::AsRead(AsRead {
            row_lengths: &[3, 0, 1, 1],
            content_rows: 4,
            ends_with_newline: false,
        });
        let cancel = AtomicBool::new(false);
        write_csv_as(&target, &shown, &lf_options(&target), &layout, &cancel).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"a,b,\n\nc\n,,z");

        write_csv_as(
            &target,
            &shown,
            &lf_options(&target),
            &Layout::Rectangle,
            &cancel,
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"a,b,\n,,\nc,,\n,,z\n");
    }

    #[test]
    fn empty_rows_after_the_last_cell_stay_while_the_content_still_ends_there() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.csv");
        let shown = ShownCells {
            end: at(0, 1),
            cells: vec![(at(0, 0), "a".to_string()), (at(0, 1), "b".to_string())],
        };
        let cancel = AtomicBool::new(false);
        let write = |row_lengths: &[usize], content_rows: usize| {
            let layout = Layout::AsRead(AsRead {
                row_lengths,
                content_rows,
                ends_with_newline: true,
            });
            write_csv_as(&target, &shown, &lf_options(&target), &layout, &cancel).unwrap();
            std::fs::read(&target).unwrap()
        };
        assert_eq!(write(&[2, 3], 1), b"a,b\n,,\n");
        assert_eq!(write(&[2, 0, 0], 1), b"a,b\n\n\n");
        assert_eq!(write(&[2, 2, 2], 3), b"a,b\n");
    }

    #[test]
    fn a_row_never_loses_fields_and_a_new_row_is_as_wide_as_the_widest() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.csv");
        let shown = ShownCells {
            end: at(1, 1),
            cells: vec![
                (at(0, 0), "9".to_string()),
                (at(0, 1), "2".to_string()),
                (at(1, 0), "x".to_string()),
            ],
        };
        let layout = Layout::AsRead(AsRead {
            row_lengths: &[3],
            content_rows: 1,
            ends_with_newline: true,
        });
        let cancel = AtomicBool::new(false);
        write_csv_as(&target, &shown, &lf_options(&target), &layout, &cancel).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"9,2,\nx,,\n");
    }

    #[test]
    fn a_missing_csv_is_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let failure = load_csv(&dir.path().join("gone.csv"), None).err();
        assert!(matches!(failure, Some(LoadFailure::Missing)));
    }

    #[test]
    fn a_csv_over_the_size_cap_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.csv");
        std::fs::write(&path, b"a,b\n1,2\n").unwrap();
        assert!(read_with_stamp(&path, 8).is_ok());
        assert!(matches!(
            read_with_stamp(&path, 7),
            Err(ReadStop::TooLarge(_))
        ));
        assert_eq!(
            ReadStop::TooLarge(600).message(&path),
            CsvError::TooLarge(600).to_string()
        );
    }

    #[test]
    fn only_a_missing_file_is_no_baseline_and_anything_unreadable_fails() {
        let dir = tempfile::tempdir().unwrap();
        let comma = Delimiter::Comma;
        let missing = dir.path().join("gone.csv");
        assert!(matches!(
            read_baseline(&missing, comma),
            Ok(BaselineRead::Absent)
        ));

        let fine = dir.path().join("fine.csv");
        std::fs::write(&fine, b"a,b\n\n1,2").unwrap();
        let Ok(BaselineRead::Present(baseline)) = read_baseline(&fine, comma) else {
            panic!("a readable file is a baseline");
        };
        assert_eq!(baseline.rows.len(), 3);
        assert!(!baseline.ends_with_newline);

        let wide = dir.path().join("wide.csv");
        std::fs::write(&wide, vec![b'x'; 40_000]).unwrap();
        assert!(matches!(
            read_baseline(&wide, comma),
            Err(EngineError::Rejected(_))
        ));

        let broken = dir.path().join("broken.csv");
        std::fs::write(&broken, b"\xEF\xBB\xBFa,\xFF\n").unwrap();
        assert!(matches!(
            read_baseline(&broken, comma),
            Ok(BaselineRead::Lossy)
        ));

        assert!(matches!(
            read_baseline(dir.path(), comma),
            Err(EngineError::Read { .. })
        ));
    }
}
