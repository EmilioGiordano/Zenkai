#![forbid(unsafe_code)]

mod assemble;
mod diff;
mod formulas;
pub mod limits;
mod package;
mod sheet_xml;
mod text;

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ironcalc::base::types::Workbook;

pub use diff::workbook_differences;

use crate::formulas::Names;
use crate::package::{read_entry, read_package, stub_archive};
use crate::sheet_xml::{ScannedSheet, scan_worksheet};

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Unsafe(String),
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("the file uses something this reader leaves to IronCalc: {0}")]
    Unsupported(String),
}

pub type Inspect<'a> = dyn Fn(&str, &[u8]) -> Result<(), String> + Sync + 'a;

// Builds the same workbook as IronCalc's `load_from_xlsx_bytes`. Every archive entry is
// passed to `inspect` once, worksheets without their cells, which this reader checks
// itself while streaming them. Any error means the file is left to IronCalc's reader,
// except `Unsafe`, which refuses it.
pub fn read_xlsx(
    bytes: &[u8],
    name: &str,
    locale: &str,
    timezone: &str,
    inspect: &Inspect<'_>,
) -> Result<Workbook, ReadError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    limits::check_archive(&mut archive)?;
    let package = read_package(&mut archive)?;

    let mut worksheet_entries = vec![None; package.worksheets.len()];
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let entry_name = entry.name().to_string();
        if let Some(position) = package.worksheets.iter().position(|w| w.path == entry_name) {
            worksheet_entries[position] = Some(index);
            continue;
        }
        let part = read_entry(entry)?;
        inspect(&entry_name, &part).map_err(ReadError::Unsafe)?;
    }
    let worksheet_entries = worksheet_entries
        .into_iter()
        .collect::<Option<Vec<usize>>>()
        .ok_or_else(|| ReadError::Unreadable("a worksheet part is missing".to_string()))?;

    let texts = in_parallel(worksheet_entries.len(), |position| {
        let mut archive = archive.clone();
        let entry = archive
            .by_index(worksheet_entries[position])
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        String::from_utf8(read_entry(entry)?).map_err(|e| ReadError::Unreadable(e.to_string()))
    })?;
    let scanned: Vec<ScannedSheet<'_>> = in_parallel(texts.len(), |position| {
        let worksheet = &package.worksheets[position];
        let scanned = scan_worksheet(&texts[position], &worksheet.name)?;
        let checked = scanned
            .stub
            .as_deref()
            .unwrap_or(texts[position].as_bytes());
        inspect(&worksheet.path, checked).map_err(ReadError::Unsafe)?;
        Ok(scanned)
    })?;

    let mut replaced = std::collections::HashMap::new();
    let mut sheets = Vec::with_capacity(scanned.len());
    for (worksheet, scanned) in package.worksheets.iter().zip(scanned) {
        if let Some(stub) = scanned.stub {
            replaced.insert(worksheet.path.clone(), stub);
        }
        sheets.push(scanned.cells);
    }
    let stub = stub_archive(&mut archive, &replaced)?;
    drop(replaced);

    guarded(|| {
        let mut workbook = ironcalc::import::load_from_xlsx_bytes(&stub, name, locale, timezone)
            .map_err(|e| ReadError::Unreadable(format!("{e:?}")))?;
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
        let jobs: Vec<(&str, &[sheet_xml::FormulaJob<'_>])> = package
            .worksheets
            .iter()
            .zip(&sheets)
            .map(|(part, cells)| (part.name.as_str(), cells.jobs.as_slice()))
            .collect();
        let converted = formulas::convert(&jobs, &names)?;
        drop(jobs);
        assemble::fill(&mut workbook, sheets, converted)?;
        Ok(workbook)
    })
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

fn in_parallel<T: Send>(
    count: usize,
    job: impl Fn(usize) -> Result<T, ReadError> + Sync,
) -> Result<Vec<T>, ReadError> {
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(count)
        .max(1);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<T, ReadError>>>> =
        Mutex::new((0..count).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let position = next.fetch_add(1, Ordering::Relaxed);
                    if position >= count {
                        break;
                    }
                    let result = guarded(|| job(position));
                    if let Ok(mut results) = results.lock() {
                        results[position] = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_inner()
        .map_err(|_| ReadError::Unsupported("a reader thread stopped".to_string()))?
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|| {
                Err(ReadError::Unsupported(
                    "a reader thread stopped".to_string(),
                ))
            })
        })
        .collect()
}
