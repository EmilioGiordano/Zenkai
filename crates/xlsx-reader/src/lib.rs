#![forbid(unsafe_code)]

mod assemble;
mod base;
mod diff;
mod formulas;
mod layout;
pub mod limits;
mod links;
mod package;
mod shared_strings;
mod sheet_data;
mod sheet_stream;
mod stream;
mod text;
mod verify;

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use ironcalc::base::types::Workbook;

pub use base::{BaseFacts, BasePackage, CalcChain};
pub use diff::workbook_differences;
pub use layout::{PackageLayout, PartNames, SharedStrings, StyleCounts};
pub use links::{SheetLink, SheetLinks, relationships_part};
pub use limits::CellBudget;
pub use shared_strings::SHARED_STRINGS_PART;
pub use verify::{Written, check_written};

use crate::formulas::Names;
use crate::package::{read_package, stub_archive};
use crate::sheet_stream::{StreamedSheet, stream_config, stream_worksheet};
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
    let file = File::open(path).map_err(|e| ReadError::Unreadable(e.to_string()))?;
    let meter = CompressedMeter::new();
    let archive = zip::ZipArchive::new(meter.wrap(BufReader::new(file)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    read_from(archive, &meter, name, locale, timezone, inspect)
}

pub fn read_xlsx(
    bytes: &[u8],
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    let meter = CompressedMeter::new();
    let archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    read_from(archive, &meter, name, locale, timezone, inspect)
}

fn read_from<R: Read + Seek>(
    mut archive: zip::ZipArchive<Counted<R>>,
    meter: &CompressedMeter,
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<OpenedBook, ReadError> {
    limits::check_archive(&mut archive)?;
    let budgets = Budgets::production();
    let package = read_package(&mut archive, meter, &budgets)?;

    let positions: std::collections::HashMap<&str, usize> = package
        .worksheets
        .iter()
        .enumerate()
        .map(|(position, part)| (part.path.as_str(), position))
        .collect();
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
            let entry = archive
                .by_index(index)
                .map_err(|e| ReadError::Unreadable(e.to_string()))?;
            let sheet_name = package.worksheets[position].name.clone();
            let streamed = stream_worksheet(entry, meter, &entry_name, &sheet_name, &budgets)?;
            // The rows were checked while streaming; the configuration carries the
            // same feature flags and validation limits as a whole part would.
            inspect(&entry_name, &streamed.stub)?;
            sheets[position] = Some(streamed);
            continue;
        }
        if skipped_part(&entry_name) {
            inspect(&entry_name, &[])?;
            continue;
        }
        let (_, part, _) = read_part_if_checked(&mut archive, index, meter, &budgets, false)?;
        inspect(&entry_name, &part)?;
    }

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
    let stub = stub_archive(&mut archive, &replaced)?;
    drop(replaced);

    let mut workbook = guarded(|| {
        ironcalc::import::load_from_xlsx_bytes(&stub, name, locale, timezone)
            .map_err(|e| ReadError::Unreadable(format!("{e:?}")))
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
    let sheet_links = links::sheet_links(&mut archive, meter, &budgets, &worksheet_paths)?;
    let layout = PackageLayout {
        worksheets: worksheet_paths,
        shared_strings: layout::shared_strings_layout(
            &mut archive,
            meter,
            &budgets,
            &package,
            workbook.shared_strings.len(),
        )?,
        styles: layout::style_counts(&mut archive, meter, &budgets)?,
        part_names: layout::part_names_of(&layout::names_in_order(&mut archive)?),
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
    let converted = formulas::convert(&jobs, &names)?;
    drop(jobs);
    guarded(|| {
        assemble::fill(&mut workbook, cells, converted)?;
        Ok(workbook)
    })
    .map(|book| OpenedBook {
        book,
        sheet_configs: configs,
        layout,
    })
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
    // Worksheet paths come from the package relationships, so sheets outside the
    // usual folder are still counted. When the package itself cannot be read the
    // usual folder is the fallback; every part still streams through the caps
    // below, and IronCalc still decides the file.
    let sheets = read_package(archive, meter, budgets)
        .ok()
        .map(|package| {
            package
                .worksheets
                .into_iter()
                .map(|sheet| sheet.path)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
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
        let count_cells = sheets.iter().any(|path| path == &entry_name)
            || entry_name
                .to_ascii_lowercase()
                .starts_with("xl/worksheets/");
        let (_, part, usage) = read_part_if_checked(archive, index, meter, budgets, count_cells)?;
        if count_cells {
            budgets.check_usage(usage)?;
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
            Ok(config) => configs
                .push(String::from_utf8(config).map_err(|e| ReadError::Unreadable(e.to_string()))?),
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
        assert!(error.to_string().contains("needs at least"), "{error}");
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
        assert!(error.to_string().contains("needs at least"), "{error}");
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
        assert!(error.to_string().contains("needs at least"), "{error}");
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
