#![forbid(unsafe_code)]

mod assemble;
mod diff;
mod formulas;
pub mod limits;
mod package;
mod sheet_data;
mod sheet_stream;
mod stream;
mod text;

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use ironcalc::base::types::Workbook;

pub use diff::workbook_differences;
pub use limits::CellBudget;

use crate::formulas::Names;
use crate::package::{read_entry, read_package, stub_archive};
use crate::sheet_stream::{StreamedSheet, stream_config, stream_worksheet};
use crate::stream::{Budgets, CompressedMeter, Counted, check_fallback_caps};

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
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let entry_name = entry.name().to_string();
        if let Some(&position) = positions.get(entry_name.as_str()) {
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
        let part = read_entry(entry, meter, &budgets)?;
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
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let entry_name = entry.name().to_string();
        if skipped_part(&entry_name) {
            inspect(&entry_name, &[])?;
            continue;
        }
        let part = read_entry(entry, &meter, &budgets)?;
        inspect(&entry_name, &part)?;
    }
    Ok(())
}

// What IronCalc's reader still requires, on the bytes really decompressed: the
// streaming reader never enforces this when opening.
pub fn check_fallback_caps_bytes(bytes: &[u8]) -> Result<(), ReadError> {
    let meter = CompressedMeter::new();
    let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes)))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    check_fallback_caps(&mut archive, &meter)
}

// The worksheet configurations for a reader that parses the rows itself, without
// decompressing any sheetData into memory. A part that fails to stream keeps the
// default settings; only an unsafe part refuses the file.
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
            Err(_) => configs.push(String::new()),
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
