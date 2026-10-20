#![forbid(unsafe_code)]

mod assemble;
mod base;
mod diff;
mod formulas;
mod layout;
pub mod limits;
mod links;
mod nesting;
mod package;
mod parallel;
mod shared_strings;
mod sheet_data;
mod sheet_stream;
mod splice;
mod stream;
mod styles_part;
#[cfg(test)]
mod test_sheets;
mod text;
mod unit_pool;
mod verify;

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use ironcalc::base::types::Workbook;

pub use base::{BaseFacts, BasePackage, CalcChain};
pub use diff::workbook_differences;
pub use layout::{PackageLayout, PartNames, SharedStrings};
pub use limits::CellBudget;
pub use links::{SheetLink, SheetLinks, relationships_part};
pub use shared_strings::SHARED_STRINGS_PART;
pub use splice::{AnchorKind, ReachingAnchor, RowSpan, cannot_splice, is_cannot_splice};
pub use styles_part::{STYLES_PART, StyleAppendix, StyleCounts, StylesShape};
pub use verify::{PartDigest, Written, check_written, check_xml};

use crate::formulas::Names;
use crate::package::{importer_worksheet_paths, read_package, stub_archive};
use crate::parallel::{Job, Open, ReadPlan, SheetStreams, SheetTask};
use crate::sheet_stream::{StreamedSheet, Tuning, stream_config, stream_worksheet};
use crate::stream::{Budgets, CHUNK_BYTES, CompressedMeter, Counted, read_part_if_checked};

pub use stream::has_checked_element;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Unsafe(String),
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("the file uses something this reader leaves to IronCalc: {0}")]
    Unsupported(String),
}

pub type Inspect<'a> = dyn Fn(&str, &[u8]) -> Result<(), ReadError> + Sync + 'a;

pub struct OpenedBook {
    pub book: Workbook,
    pub sheet_configs: Vec<String>,
    pub layout: PackageLayout,
}

// Parts the model never uses and IronCalc's importer never parses are never
// decompressed; the caller still learns about them through `inspect`.
pub fn skipped_part(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "xl/pivotcache/",
        "xl/pivottables/",
        "xl/drawings/",
        "xl/charts/",
        "xl/media/",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

// Builds the same workbook as IronCalc's `load_from_xlsx_bytes`. The zip is read
// from the file without copying it whole into memory, each worksheet streams its
// rows into the model as its XML is read, and every decompressed byte is counted
// against the budgets. Every archive entry is passed to `inspect` once, worksheets
// without their cells, which this reader checks itself while streaming them. Any
// error means the file is left to IronCalc's reader, except `Unsafe`, which refuses
// it. The sheet configurations come back alongside for the caller's settings.
pub fn read_xlsx_file(
    path: &Path,
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    let open = || {
        let file = File::open(path).map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let meter = CompressedMeter::new();
        let archive = zip::ZipArchive::new(meter.wrap(BufReader::new(file)))
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        Ok((archive, meter))
    };
    read_from(&open, &ReadPlan::detect(), name, locale, timezone, inspect)
}

pub fn read_xlsx(
    bytes: &[u8],
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    read_xlsx_planned(bytes, &ReadPlan::detect(), name, locale, timezone, inspect)
}

fn read_xlsx_planned(
    bytes: &[u8],
    plan: &ReadPlan,
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    let open = || {
        let meter = CompressedMeter::new();
        let archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes)))
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        Ok((archive, meter))
    };
    read_from(&open, plan, name, locale, timezone, inspect)
}

// Worksheets stream on several threads, which share the memory budget, so the cell that
// first crosses it, and the figures in its refusal, depend on timing. A refusal is
// therefore read again on one thread, which gives the refusal a single thread gives.
fn read_from<R: Read + Seek>(
    open: &Open<'_, R>,
    plan: &ReadPlan,
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    match read_planned(open, plan, name, locale, timezone, inspect) {
        Err(ReadError::Unsafe(_)) if !plan.is_serial() => read_planned(
            open,
            &plan.single_threaded(),
            name,
            locale,
            timezone,
            inspect,
        ),
        other => other,
    }
}

fn read_planned<R: Read + Seek>(
    open: &Open<'_, R>,
    plan: &ReadPlan,
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    let (mut archive, meter) = open()?;
    let meter = &meter;
    tracing::info_span!("open_check_archive").in_scope(|| limits::check_archive(&mut archive))?;
    let budgets = Budgets::with_cells(plan.cells);
    let package = tracing::info_span!("open_read_package")
        .in_scope(|| read_package(&mut archive, meter, &budgets))?;

    let positions: std::collections::HashMap<&str, usize> = package
        .worksheets
        .iter()
        .enumerate()
        .map(|(position, part)| (part.path.as_str(), position))
        .collect();
    let declared_span = tracing::info_span!("open_declared_sizes").entered();
    let mut declared = 0u64;
    let mut tasks = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        if let Some(&position) = positions.get(entry.name()) {
            declared = declared.saturating_add(entry.size());
            if entry.size() >= plan.min_sheet_bytes {
                tasks.push(SheetTask { index, position });
            }
        }
    }
    drop(declared_span);
    budgets.expect_sheet_bytes(declared);
    let job = Job::new(open, &package.worksheets, &budgets, &tasks, plan);
    let entries_span = tracing::info_span!("open_entries").entered();
    let sheets = std::thread::scope(|scope| {
        let mut streams = SheetStreams::start(scope, &job);
        let sheets = stream_entries(
            &mut archive,
            meter,
            &package,
            &positions,
            &budgets,
            inspect,
            &mut streams,
        );
        if sheets.is_err() {
            budgets.cancel();
        }
        if let Some(streams) = streams {
            streams.finish();
        }
        sheets
    })?;
    drop(entries_span);

    let mut replaced = std::collections::HashMap::new();
    let mut cells = Vec::with_capacity(sheets.len());
    let mut configs = Vec::with_capacity(sheets.len());
    for (part, sheet) in package.worksheets.iter().zip(sheets) {
        let Some(streamed) = sheet else {
            return Err(ReadError::Unreadable(
                "a worksheet part is missing".to_string(),
            ));
        };
        configs.push(
            String::from_utf8(streamed.settings)
                .map_err(|e| ReadError::Unreadable(e.to_string()))?,
        );
        replaced.insert(part.path.clone(), streamed.stub);
        cells.push(streamed.cells);
    }
    let stub = tracing::info_span!("open_stub_archive")
        .in_scope(|| stub_archive(&mut archive, &replaced))?;
    drop(replaced);

    let mut workbook = tracing::info_span!("open_ironcalc_load").in_scope(|| {
        guarded(|| {
            ironcalc::import::load_from_xlsx_bytes(&stub, name, locale, timezone)
                .map_err(|e| ReadError::Unreadable(format!("{e:?}")))
        })
    })?;
    let same_sheets = workbook.worksheets.len() == package.worksheets.len()
        && workbook
            .worksheets
            .iter()
            .zip(&package.worksheets)
            .all(|(built, part)| built.name == part.name);
    if !same_sheets {
        return Err(ReadError::Unsupported(
            "IronCalc read a different list of sheets".to_string(),
        ));
    }
    let worksheet_paths: Vec<String> = package
        .worksheets
        .iter()
        .map(|part| part.path.clone())
        .collect();
    let sheet_links = tracing::info_span!("open_sheet_links")
        .in_scope(|| links::sheet_links(&mut archive, meter, &budgets, &worksheet_paths))?;
    let shared_strings = tracing::info_span!("open_shared_strings_layout").in_scope(|| {
        layout::shared_strings_layout(
            &mut archive,
            meter,
            &budgets,
            &package,
            workbook.shared_strings.len(),
        )
    })?;
    let styles = tracing::info_span!("open_style_counts")
        .in_scope(|| styles_part::style_counts(&mut archive, meter, &budgets))?;
    let part_names = tracing::info_span!("open_part_names").in_scope(|| {
        layout::names_in_order(&mut archive).map(|names| layout::part_names_of(&names))
    })?;
    let layout = PackageLayout {
        worksheets: worksheet_paths,
        shared_strings,
        styles,
        part_names,
        sheet_links,
    };
    let names = Names {
        sheets: &package.sheet_names,
        defined_names: &package.defined_names,
        tables: &workbook.tables,
    };
    let jobs: Vec<(&str, &[crate::sheet_data::FormulaJob])> = package
        .worksheets
        .iter()
        .zip(&cells)
        .map(|(part, cells)| (part.name.as_str(), cells.jobs.as_slice()))
        .collect();
    let converted = tracing::info_span!("open_convert_formulas")
        .in_scope(|| formulas::convert(&jobs, &names))?;
    drop(jobs);
    tracing::info_span!("open_fill")
        .in_scope(|| {
            guarded(|| {
                assemble::fill(&mut workbook, cells, converted)?;
                Ok(workbook)
            })
        })
        .map(|book| OpenedBook {
            book,
            sheet_configs: configs,
            layout,
        })
}

// Every archive entry in order: worksheets from their thread or streamed here, the other
// parts read here and handed to `inspect`. A sheet streamed here is small, so no workers.
fn stream_entries<R: Read + Seek>(
    archive: &mut zip::ZipArchive<Counted<R>>,
    meter: &CompressedMeter,
    package: &package::Package,
    positions: &std::collections::HashMap<&str, usize>,
    budgets: &Budgets,
    inspect: &Inspect<'_>,
    streams: &mut Option<SheetStreams<'_>>,
) -> Result<Vec<Option<StreamedSheet>>, ReadError> {
    let mut sheets: Vec<Option<StreamedSheet>> =
        (0..package.worksheets.len()).map(|_| None).collect();
    for index in 0..archive.len() {
        let entry_name = {
            let entry = archive
                .by_index(index)
                .map_err(|e| ReadError::Unreadable(e.to_string()))?;
            entry.name().to_string()
        };
        if let Some(&position) = positions.get(entry_name.as_str()) {
            let streamed = match streams.as_mut().and_then(|streams| streams.take(index)) {
                Some(streamed) => streamed?,
                None => {
                    let entry = archive
                        .by_index(index)
                        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
                    let sheet_name = package.worksheets[position].name.clone();
                    let _sheet_span =
                        tracing::info_span!("open_sheet", sheet = position, bytes = entry.size())
                            .entered();
                    stream_worksheet(
                        entry,
                        meter,
                        &entry_name,
                        &sheet_name,
                        budgets,
                        Tuning::production(0),
                    )?
                }
            };
            // The rows were checked while streaming; the configuration carries the
            // same feature flags and validation limits as a whole part would.
            nesting::check_nesting(&entry_name, &streamed.stub)?;
            inspect(&entry_name, &streamed.stub)?;
            sheets[position] = Some(streamed);
            continue;
        }
        if skipped_part(&entry_name) {
            inspect(&entry_name, &[])?;
            continue;
        }
        let (_, part, _) = read_part_if_checked(archive, index, meter, budgets, false)?;
        inspect(&entry_name, &part)?;
    }
    Ok(sheets)
}

// Every part of the archive, streamed once with the budgets enforced and handed to
// `inspect`; parts the model never uses arrive without their bytes. Worksheets are
// read whole here, so this is for readers that parse the rows themselves.
pub fn scan_parts(bytes: &[u8], inspect: &Inspect<'_>) -> Result<(), ReadError> {
    let meter = CompressedMeter::new();
    let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    limits::check_archive(&mut archive)?;
    let budgets = Budgets::production();
    for index in 0..archive.len() {
        let entry_name = {
            let entry = archive
                .by_index(index)
                .map_err(|e| ReadError::Unreadable(e.to_string()))?;
            entry.name().to_string()
        };
        if skipped_part(&entry_name) {
            inspect(&entry_name, &[])?;
            continue;
        }
        let (_, part, _) = read_part_if_checked(&mut archive, index, &meter, &budgets, false)?;
        inspect(&entry_name, &part)?;
    }
    Ok(())
}

// The single walk for readers that inflate every part themselves: each entry
// streams once through the budgets, so even a part this scan never parses cannot
// smuggle past the caps, and parts the preflight checks are still handed over.
pub fn scan_with_caps(bytes: &[u8], inspect: &Inspect<'_>) -> Result<(), ReadError> {
    let meter = CompressedMeter::new();
    let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    limits::check_archive(&mut archive)?;
    scan_archive(&mut archive, &meter, &Budgets::production(), inspect)
}

fn scan_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
    inspect: &Inspect<'_>,
) -> Result<(), ReadError> {
    // The importer loads the worksheets the package relationships name, whatever else is
    // wrong with the package, so those parts are the ones counted. Only when the package
    // cannot be parsed at all is every part under the usual folder counted instead; every
    // part still streams through the caps below, and IronCalc still decides the file.
    let package_sheets = importer_worksheet_paths(archive, meter, budgets);
    for index in 0..archive.len() {
        let entry_name = {
            let entry = archive
                .by_index(index)
                .map_err(|e| ReadError::Unreadable(e.to_string()))?;
            entry.name().to_string()
        };
        if skipped_part(&entry_name) {
            let entry = archive
                .by_index(index)
                .map_err(|e| ReadError::Unreadable(e.to_string()))?;
            let mut guard = budgets.stream(entry, meter, &entry_name);
            let mut chunk = [0u8; CHUNK_BYTES];
            while guard.read_chunk(&mut chunk)? != 0 {}
            inspect(&entry_name, &[])?;
            continue;
        }
        // Worksheet cells reach IronCalc's reader past this walk, so they are
        // counted on the same pass and charged to the memory budget cells share
        // with the streaming reader; anything over refuses the file.
        let count_cells = match &package_sheets {
            Some(sheets) => sheets.contains(&entry_name),
            None => entry_name
                .to_ascii_lowercase()
                .starts_with("xl/worksheets/"),
        };
        let (_, part, usage) = read_part_if_checked(archive, index, meter, budgets, count_cells)?;
        if count_cells {
            budgets.check_importer_usage(usage)?;
        }
        inspect(&entry_name, &part)?;
    }
    Ok(())
}

// The worksheet configurations for a reader that parses the rows itself, without
// decompressing any sheetData into memory. A part that fails to stream is reported
// as unsupported content instead of silently keeping the default settings, whose
// layout and view would then be lost on save; only an unsafe part refuses the file.
pub fn sheet_configs_from_bytes(bytes: &[u8]) -> Result<Vec<String>, ReadError> {
    let meter = CompressedMeter::new();
    let archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    sheet_configs_from(archive, &meter)
}

fn sheet_configs_from<R: Read + Seek>(
    mut archive: zip::ZipArchive<Counted<R>>,
    meter: &CompressedMeter,
) -> Result<Vec<String>, ReadError> {
    limits::check_archive(&mut archive)?;
    let budgets = Budgets::production();
    let package = read_package(&mut archive, meter, &budgets)?;
    let mut configs = Vec::with_capacity(package.worksheets.len());
    for part in &package.worksheets {
        let entry = archive
            .by_name(&part.path)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        match stream_config(entry, meter, &part.path, &budgets) {
            Ok(config) => {
                nesting::check_nesting(&part.path, &config)?;
                configs.push(
                    String::from_utf8(config).map_err(|e| ReadError::Unreadable(e.to_string()))?,
                );
            }
            Err(ReadError::Unsafe(reason)) => return Err(ReadError::Unsafe(reason)),
            Err(error) => {
                return Err(ReadError::Unsupported(format!(
                    "a worksheet configuration could not be read ({error})"
                )));
            }
        }
    }
    Ok(configs)
}

pub const MAX_LISTED_SHEETS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedSheet {
    pub name: String,
    pub hidden: bool,
}

// The sheet names of one file, in workbook order, from the package metadata alone:
// no worksheet, shared strings or styles part is ever inflated. The archive goes
// through the same entry and size caps as a full open.
pub fn list_sheets(path: &Path) -> Result<Vec<ListedSheet>, ReadError> {
    let file = File::open(path).map_err(|e| ReadError::Unreadable(e.to_string()))?;
    let meter = CompressedMeter::new();
    let mut archive = zip::ZipArchive::new(meter.wrap(BufReader::new(file)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    list_sheets_from(&mut archive, &meter)
}

fn list_sheets_from<R: Read + Seek>(
    archive: &mut zip::ZipArchive<Counted<R>>,
    meter: &CompressedMeter,
) -> Result<Vec<ListedSheet>, ReadError> {
    limits::check_archive(archive)?;
    let package = read_package(archive, meter, &Budgets::production())?;
    if package.sheet_names.len() > MAX_LISTED_SHEETS {
        return Err(ReadError::Unsafe(format!(
            "{} sheets in the workbook",
            package.sheet_names.len()
        )));
    }
    Ok(package
        .sheet_names
        .into_iter()
        .zip(package.hidden)
        .map(|(name, hidden)| ListedSheet { name, hidden })
        .collect())
}

// IronCalc's importer and parser can panic on malformed input; here that only means the
// file goes to the regular reader.
fn guarded<T>(job: impl FnOnce() -> Result<T, ReadError>) -> Result<T, ReadError> {
    catch_unwind(AssertUnwindSafe(job)).unwrap_or_else(|_| {
        Err(ReadError::Unsupported(
            "IronCalc stopped on part of this file".to_string(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Mutex;

    use super::*;

    fn zipped(entries: &[(&str, &[u8])], deflated: bool) -> Vec<u8> {
        let mut options = zip::write::FileOptions::default();
        if deflated {
            options = options.compression_method(zip::CompressionMethod::Deflated);
        }
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

    fn listing_archive(sheets: &[(&str, &str)], extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut workbook = String::new();
        let mut rels = String::new();
        for (index, (name, state)) in sheets.iter().enumerate() {
            let id = index + 1;
            workbook.push_str(&format!(
                "<sheet name=\"{name}\" sheetId=\"{id}\" {state} r:id=\"rId{id}\"/>"
            ));
            rels.push_str(&format!(
                "<Relationship Id=\"rId{id}\" Type=\"{REL_NS}/worksheet\" Target=\"worksheets/sheet{id}.xml\"/>"
            ));
        }
        let workbook = format!(
            "<workbook xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\"><sheets>{workbook}</sheets></workbook>"
        );
        let rels = format!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{rels}</Relationships>"
        );
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("xl/workbook.xml", workbook.as_bytes()),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ];
        entries.extend_from_slice(extra);
        zipped(&entries, true)
    }

    fn listed(bytes: &[u8]) -> Result<Vec<ListedSheet>, ReadError> {
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes))).unwrap();
        list_sheets_from(&mut archive, &meter)
    }

    #[test]
    fn listing_returns_names_in_order_with_the_hidden_state() {
        let bytes = listing_archive(
            &[
                ("Ventas", ""),
                ("Oculta", "state=\"hidden\""),
                ("Año 2024 日本", ""),
                ("Muy", "state=\"veryHidden\""),
            ],
            &[],
        );
        let sheets = listed(&bytes).unwrap();
        let summary: Vec<(&str, bool)> =
            sheets.iter().map(|s| (s.name.as_str(), s.hidden)).collect();
        assert_eq!(
            summary,
            [
                ("Ventas", false),
                ("Oculta", true),
                ("Año 2024 日本", false),
                ("Muy", true),
            ]
        );
    }

    #[test]
    fn listing_never_inflates_a_worksheet_or_the_other_big_parts() {
        let bomb = vec![0u8; 70 * 1024 * 1024];
        let bytes = listing_archive(
            &[("A", ""), ("B", "")],
            &[
                ("xl/worksheets/sheet1.xml", &bomb),
                ("xl/worksheets/sheet2.xml", b"not xml at all <<<"),
                ("xl/sharedStrings.xml", &bomb),
                ("xl/styles.xml", &bomb),
            ],
        );
        assert_eq!(listed(&bytes).unwrap().len(), 2);
    }

    #[test]
    fn listing_refuses_a_workbook_part_that_is_a_zip_bomb() {
        let mut bomb = b"<workbook><sheets>".to_vec();
        bomb.resize(70 * 1024 * 1024, b' ');
        let bytes = zipped(&[("xl/workbook.xml", &bomb)], true);
        let error = listed(&bytes).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
    }

    #[test]
    fn listing_refuses_more_sheets_than_the_cap() {
        let names: Vec<String> = (0..=MAX_LISTED_SHEETS).map(|n| format!("S{n}")).collect();
        let sheets: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "")).collect();
        let error = listed(&listing_archive(&sheets, &[])).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        let within = &sheets[..MAX_LISTED_SHEETS];
        assert_eq!(
            listed(&listing_archive(within, &[])).unwrap().len(),
            MAX_LISTED_SHEETS
        );
    }

    #[test]
    fn listing_a_package_without_a_workbook_is_unreadable() {
        let bytes = zipped(&[("hello.txt", b"hi")], false);
        let error = listed(&bytes).unwrap_err();
        assert!(matches!(error, ReadError::Unreadable(_)), "{error}");
    }

    #[test]
    fn the_single_scan_walk_refuses_a_bomb_it_never_parses() {
        let bomb = vec![0u8; 70 * 1024 * 1024];
        let bytes = zipped(&[("xl/media/blob.bin", &bomb)], true);
        let error = scan_with_caps(&bytes, &|_, _| Ok(())).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("150"), "{error}");
    }

    #[test]
    fn an_unreadable_sheet_config_is_unsupported_not_empty_settings() {
        const WORKBOOK: &[u8] = b"<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Good\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Broken\" sheetId=\"2\" r:id=\"rId2\"/></sheets></workbook>";
        const RELS: &[u8] = b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>";
        const GOOD: &[u8] = b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>";
        const BROKEN: &[u8] = b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row>";
        let bytes = zipped(
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("xl/worksheets/sheet1.xml", GOOD),
                ("xl/worksheets/sheet2.xml", BROKEN),
            ],
            false,
        );
        let error = sheet_configs_from_bytes(&bytes).unwrap_err();
        assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
        assert!(error.to_string().contains("configuration"), "{error}");
    }

    fn sheet_with_cells(cells: u32) -> String {
        let mut rows = String::new();
        for row in 1..=cells {
            rows.push_str(&format!(
                "<row r=\"{row}\"><c r=\"A{row}\"><v>1</v></c></row>"
            ));
        }
        format!(
            "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>"
        )
    }

    #[test]
    fn the_fallback_walk_charges_worksheet_cells_to_the_budget() {
        let sheet = sheet_with_cells(300);
        let bytes = zipped(&[("xl/worksheets/sheet1.xml", sheet.as_bytes())], false);
        let tiny = Budgets::with_cells(CellBudget::max_cells_for_test(100));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        let error = scan_archive(&mut archive, &meter, &tiny, &|_, _| Ok(())).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("needs about"), "{error}");
        let roomy = Budgets::with_cells(CellBudget::max_cells_for_test(10_000));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        scan_archive(&mut archive, &meter, &roomy, &|_, _| Ok(())).unwrap();
    }

    #[test]
    fn the_fallback_walk_charges_prefixed_worksheet_cells_to_the_budget() {
        let mut rows = String::new();
        for row in 1..=300 {
            rows.push_str(&format!(
                "<x:row r=\"{row}\"><x:c r=\"A{row}\"><v>1</v></x:c></x:row>"
            ));
        }
        let sheet = format!(
            "<worksheet xmlns:x=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>"
        );
        let bytes = zipped(&[("xl/worksheets/sheet1.xml", sheet.as_bytes())], false);
        let tiny = Budgets::with_cells(CellBudget::max_cells_for_test(100));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        let error = scan_archive(&mut archive, &meter, &tiny, &|_, _| Ok(())).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("needs about"), "{error}");
        let roomy = Budgets::with_cells(CellBudget::max_cells_for_test(10_000));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        scan_archive(&mut archive, &meter, &roomy, &|_, _| Ok(())).unwrap();
    }

    #[test]
    fn the_fallback_walk_charges_the_dom_of_the_bytes_really_inflated() {
        let padding = " ".repeat(5_000);
        let sheet = format!(
            "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{padding}<sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>"
        );
        let bytes = zipped(&[("xl/worksheets/sheet1.xml", sheet.as_bytes())], true);
        let tiny = Budgets::with_cells(CellBudget::max_cells_for_test(100));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        let error = scan_archive(&mut archive, &meter, &tiny, &|_, _| Ok(())).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(
            error.to_string().contains("xl/worksheets/sheet1.xml"),
            "{error}"
        );
        let roomy = Budgets::with_cells(CellBudget::max_cells_for_test(10_000));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        scan_archive(&mut archive, &meter, &roomy, &|_, _| Ok(())).unwrap();
    }

    #[test]
    fn the_fallback_walk_counts_sheets_outside_the_usual_folder() {
        const WORKBOOK: &[u8] = b"<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Odd\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";
        const RELS: &[u8] = b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"/data/worksheets/s1.xml\"/></Relationships>";
        let sheet = sheet_with_cells(300);
        let bytes = zipped(
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("data/worksheets/s1.xml", sheet.as_bytes()),
            ],
            false,
        );
        let tiny = Budgets::with_cells(CellBudget::max_cells_for_test(100));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        let error = scan_archive(&mut archive, &meter, &tiny, &|_, _| Ok(())).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("needs about"), "{error}");
    }

    // Empty cells are 4 bytes of XML but a full cell each to the budget, so a part of
    // them trips the cell count long before the DOM charge on its bytes would.
    fn bare_cells(count: usize) -> String {
        format!(
            "<worksheet xmlns=\"{MAIN_NS}\"><sheetData>{}</sheetData></worksheet>",
            "<c/>".repeat(count)
        )
    }

    fn package_naming(target: &str) -> (Vec<u8>, Vec<u8>) {
        let workbook = format!(
            "<workbook xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
        );
        let rels = format!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{REL_NS}/worksheet\" Target=\"{target}\"/></Relationships>"
        );
        (workbook.into_bytes(), rels.into_bytes())
    }

    fn walk_refused(entries: &[(&str, &[u8])], max_cells: u64) -> bool {
        let bytes = zipped(entries, false);
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(max_cells));
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        match scan_archive(&mut archive, &meter, &budgets, &|_, _| Ok(())) {
            Ok(()) => false,
            Err(error) => {
                assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
                true
            }
        }
    }

    #[test]
    fn a_part_the_package_does_not_name_as_a_sheet_is_not_counted_as_one() {
        let (workbook, rels) = package_naming("worksheets/sheet1.xml");
        let small = bare_cells(1);
        let orphan = bare_cells(200);
        let named = [
            ("xl/workbook.xml", workbook.as_slice()),
            ("xl/_rels/workbook.xml.rels", rels.as_slice()),
            ("xl/worksheets/sheet1.xml", small.as_bytes()),
        ];
        let with_orphan = [
            named.to_vec(),
            vec![("xl/worksheets/orphan.xml", orphan.as_bytes())],
        ]
        .concat();
        assert!(!walk_refused(&with_orphan, 1_000));
        let as_the_sheet = [
            ("xl/workbook.xml", workbook.as_slice()),
            ("xl/_rels/workbook.xml.rels", rels.as_slice()),
            ("xl/worksheets/sheet1.xml", orphan.as_bytes()),
        ];
        assert!(walk_refused(&as_the_sheet, 1_000));
    }

    #[test]
    fn every_way_a_relationship_can_name_a_sheet_is_still_counted() {
        let sheet = bare_cells(200);
        // Entry names are matched as written, the way the importer opens them.
        for (target, entry) in [
            ("worksheets/sheet1.xml", "xl/worksheets/sheet1.xml"),
            ("/xl/worksheets/sheet1.xml", "xl/worksheets/sheet1.xml"),
            ("/data/s1.xml", "data/s1.xml"),
            ("./worksheets/s1.xml", "xl/./worksheets/s1.xml"),
            ("worksheets/./s1.xml", "xl/worksheets/./s1.xml"),
            ("../data/s1.xml", "xl/../data/s1.xml"),
            ("sheets/s1.dat", "xl/sheets/s1.dat"),
            ("Worksheets/Sheet1.XML", "xl/Worksheets/Sheet1.XML"),
        ] {
            let (workbook, rels) = package_naming(target);
            let entries = [
                ("xl/workbook.xml", workbook.as_slice()),
                ("xl/_rels/workbook.xml.rels", rels.as_slice()),
                (entry, sheet.as_bytes()),
            ];
            assert!(walk_refused(&entries, 1_000), "{target}");
        }
    }

    fn archive_with_package(workbook: &str, rels: &str, entry: &str) -> bool {
        let sheet = bare_cells(200);
        let entries = [
            ("xl/workbook.xml", workbook.as_bytes()),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
            (entry, sheet.as_bytes()),
        ];
        walk_refused(&entries, 1_000)
    }

    fn rels_of(items: &[(&str, &str)]) -> String {
        let items: String = items
            .iter()
            .map(|(id, target)| {
                format!(
                    "<Relationship Id=\"{id}\" Type=\"{REL_NS}/worksheet\" Target=\"{target}\"/>"
                )
            })
            .collect();
        format!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{items}</Relationships>"
        )
    }

    fn workbook_of(sheets: &[(&str, &str)]) -> String {
        let sheets: String = sheets
            .iter()
            .enumerate()
            .map(|(index, (name, id))| {
                format!(
                    "<sheet name=\"{name}\" sheetId=\"{}\" r:id=\"{id}\"/>",
                    index + 1
                )
            })
            .collect();
        format!(
            "<workbook xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\"><sheets>{sheets}</sheets></workbook>"
        )
    }

    #[test]
    fn sheets_of_a_package_read_package_refuses_are_still_counted() {
        // Two sheets on one part: read_package refuses it, the importer still loads it.
        let workbook = workbook_of(&[("A", "rId1"), ("B", "rId2")]);
        let rels = rels_of(&[("rId1", "/data/s1.xml"), ("rId2", "/data/s1.xml")]);
        assert!(archive_with_package(&workbook, &rels, "data/s1.xml"));
        // A '!' in a sheet name is refused by read_package too.
        let workbook = workbook_of(&[("A!B", "rId1")]);
        let rels = rels_of(&[("rId1", "/data/s1.xml")]);
        assert!(archive_with_package(&workbook, &rels, "data/s1.xml"));
    }

    #[test]
    fn the_last_of_a_repeated_relationship_id_decides_the_sheet() {
        let workbook = workbook_of(&[("A", "rId1")]);
        let rels = rels_of(&[("rId1", "/data/small.xml"), ("rId1", "/data/big.xml")]);
        assert!(archive_with_package(&workbook, &rels, "data/big.xml"));
        let rels = rels_of(&[("rId1", "/data/big.xml"), ("rId1", "/data/small.xml")]);
        assert!(!archive_with_package(&workbook, &rels, "data/big.xml"));
    }

    #[test]
    fn without_a_readable_package_every_part_under_the_usual_folder_is_counted() {
        let sheet = bare_cells(200);
        for broken in [&b"<workbook"[..], b""] {
            let entries = [
                ("xl/workbook.xml", broken),
                ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
                ("xl/Worksheets/other.xml", sheet.as_bytes()),
            ];
            assert!(walk_refused(&entries, 1_000));
        }
        let entries = [("xl/worksheets/sheet1.xml", sheet.as_bytes())];
        assert!(walk_refused(&entries, 1_000));
        assert!(!walk_refused(&entries, 10_000));
    }

    #[test]
    fn the_single_scan_walk_hands_checked_parts_over() {
        const WORKBOOK: &[u8] = b"<workbook><definedNames><definedName name=\"x\">1</definedName></definedNames></workbook>";
        let bytes = zipped(
            &[
                (
                    "xl/styles.xml",
                    b"<styleSheet><fonts count=\"1\"/></styleSheet>",
                ),
                ("xl/workbook.xml", WORKBOOK),
                ("xl/media/note.bin", b"plain"),
            ],
            false,
        );
        let seen = Mutex::new(Vec::new());
        scan_with_caps(&bytes, &|name, part| {
            seen.lock().unwrap().push((name.to_string(), part.len()));
            Ok(())
        })
        .unwrap();
        let seen = seen.into_inner().unwrap();
        assert_eq!(
            seen.iter()
                .find(|(name, _)| name == "xl/workbook.xml")
                .map(|(_, len)| *len),
            Some(WORKBOOK.len()),
            "{seen:?}"
        );
        for (name, len) in &seen {
            if name == "xl/workbook.xml" {
                continue;
            }
            assert_eq!(*len, 0, "{name} needs no bytes");
        }
    }
}
