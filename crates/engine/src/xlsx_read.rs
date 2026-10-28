use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ironcalc::base::types::Workbook as Book;
use zenkai_xlsx_reader::{ReadError, SheetLinks, list_sheets, workbook_differences};

pub use zenkai_xlsx_reader::ListedSheet;

use crate::error::EngineError;
use crate::file::{Found, Unsupported, inspect_part, kinds};
use crate::held::Held;

// Only a refused file reports harm; a corrupt or non-zip file is not a valid workbook.
pub(crate) fn engine_error(error: ReadError) -> EngineError {
    match error {
        ReadError::Unsafe(reason) => EngineError::refused(reason),
        ReadError::Unreadable(reason) | ReadError::Unsupported(reason) => {
            EngineError::InvalidFile(reason)
        }
        ReadError::Cancelled => EngineError::Cancelled,
    }
}
use crate::preflight::run_with_engine_stack;
use crate::saved::OpenedParts;
use crate::sheet_elements::ElementBudget;
use crate::sheet_settings::{self, SheetSettings};
use crate::workbook::{read_book, read_book_fast, read_book_streaming};

// The sheets of a workbook that is not open, read from its package metadata alone.
pub fn sheet_names_of(path: &Path) -> Result<Vec<ListedSheet>, EngineError> {
    std::fs::metadata(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    list_sheets(path).map_err(engine_error)
}

const SHADOW_REPORT_LINES: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XlsxReader {
    IronCalc,
    #[default]
    Fast,
    // Development aid: reads with both, logs every difference and keeps IronCalc's result.
    Shadow,
}

// A workbook read from xlsx bytes, before the engine model is built from it.
pub struct ReadBook {
    pub(crate) book: Book,
    pub(crate) unsupported: Vec<Unsupported>,
    pub(crate) fallback: Option<String>,
}

impl ReadBook {
    pub fn fallback(&self) -> Option<&str> {
        self.fallback.as_deref()
    }
}

pub(crate) struct FileRead {
    pub(crate) read: ReadBook,
    pub(crate) settings: Vec<SheetSettings>,
    // Where the file's parts are and what they hold, when a save can copy from it.
    pub(crate) parts: Option<OpenedParts>,
}

type Outcome<E> = Result<(Book, Vec<Unsupported>), E>;

pub fn read_xlsx_bytes(
    bytes: &[u8],
    name: &str,
    reader: XlsxReader,
) -> Result<ReadBook, EngineError> {
    let (book, unsupported, fallback) = match reader {
        XlsxReader::IronCalc => {
            let (book, unsupported) = read_with_ironcalc(bytes, name)?;
            (book, unsupported, None)
        }
        XlsxReader::Fast => match read_fast(bytes, name) {
            Ok((book, unsupported)) => (book, unsupported, None),
            Err(ReadError::Unsafe(reason)) => return Err(EngineError::refused(reason)),
            Err(error) => {
                let (book, unsupported) = fallback_to_ironcalc(bytes, name, &error)?;
                (book, unsupported, Some(error.to_string()))
            }
        },
        XlsxReader::Shadow => {
            let ironcalc = read_with_ironcalc(bytes, name);
            let fast = read_fast(bytes, name);
            match compare(&ironcalc, &fast) {
                ReaderComparison::Same => tracing::info!("shadow read: both xlsx readers agree"),
                other => tracing::warn!(?other, "shadow read: the xlsx readers disagree"),
            }
            let (book, unsupported) = ironcalc?;
            (book, unsupported, None)
        }
    };
    Ok(ReadBook {
        book,
        unsupported,
        fallback,
    })
}

pub(crate) fn read_xlsx_file(
    path: &Path,
    name: &str,
    reader: XlsxReader,
    cancel: &Arc<AtomicBool>,
) -> Result<FileRead, EngineError> {
    let read_error = |source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    };
    let finish = |read: ReadBook, settings| FileRead {
        read,
        settings,
        parts: None,
    };
    match reader {
        XlsxReader::IronCalc => {
            let bytes = std::fs::read(path).map_err(read_error)?;
            check_not_cancelled(cancel)?;
            let read = read_xlsx_bytes(&bytes, name, XlsxReader::IronCalc)?;
            let settings = read_settings(&bytes)?;
            Ok(finish(read, settings))
        }
        XlsxReader::Fast => {
            match tracing::info_span!("open_read_file")
                .in_scope(|| read_streaming(path, name, cancel))
            {
                Ok(read) => Ok(read),
                Err(ReadError::Cancelled) => Err(EngineError::Cancelled),
                Err(ReadError::Unsafe(reason)) => Err(EngineError::refused(reason)),
                Err(error) => {
                    let bytes = std::fs::read(path).map_err(read_error)?;
                    let (book, unsupported) = fallback_to_ironcalc(&bytes, name, &error)?;
                    let settings = read_settings(&bytes)?;
                    Ok(finish(
                        ReadBook {
                            book,
                            unsupported,
                            fallback: Some(error.to_string()),
                        },
                        settings,
                    ))
                }
            }
        }
        XlsxReader::Shadow => {
            let bytes = std::fs::read(path).map_err(read_error)?;
            check_not_cancelled(cancel)?;
            let read = read_xlsx_bytes(&bytes, name, XlsxReader::Shadow)?;
            let settings = read_settings(&bytes)?;
            Ok(finish(read, settings))
        }
    }
}

pub(crate) fn check_not_cancelled(cancel: &AtomicBool) -> Result<(), EngineError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(EngineError::Cancelled);
    }
    Ok(())
}

// The IronCalc fallback is never a way past the limits: one walk streams every
// part through the caps and gathers what the file uses, otherwise the file is
// refused without ever reaching IronCalc's uncapped reader. An unsafe walk
// refuses the file as harmful; any other walk failure fails it as an invalid
// workbook, the same mapping as the direct IronCalc read.
fn fallback_to_ironcalc(bytes: &[u8], name: &str, declined: &ReadError) -> Outcome<EngineError> {
    let found: Found = Mutex::new(Vec::new());
    let inspect = |part: &str, data: &[u8]| inspect_part(&found, part, data);
    if let Err(limited) = zenkai_xlsx_reader::scan_with_caps(bytes, &inspect) {
        if let ReadError::Unsafe(reason) = &limited
            && reason.starts_with(ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX)
        {
            return Err(EngineError::TooLarge(reason.clone()));
        }
        let message = format!(
            "the streaming reader does not support this file ({declined}), and it cannot fall back to IronCalc's reader ({limited})"
        );
        return Err(match limited {
            ReadError::Unsafe(_) => EngineError::Unsafe(message),
            ReadError::Cancelled => EngineError::Cancelled,
            ReadError::Unreadable(_) | ReadError::Unsupported(_) => {
                EngineError::InvalidFile(message)
            }
        });
    }
    tracing::warn!(%declined, "the fast xlsx reader left this file to IronCalc's");
    let book = run_with_engine_stack(|| read_book(bytes, name))?;
    Ok((book, take_unsupported(found)?))
}

fn read_settings(bytes: &[u8]) -> Result<Vec<SheetSettings>, EngineError> {
    zenkai_xlsx_reader::sheet_configs_from_bytes(bytes)
        .map(|configs| {
            let mut budget = ElementBudget::workbook();
            configs
                .iter()
                .map(|config| {
                    if config.is_empty() {
                        SheetSettings::default()
                    } else {
                        sheet_settings::read(config, &mut budget)
                    }
                })
                .collect()
        })
        .map_err(engine_error)
}

fn read_with_ironcalc(bytes: &[u8], name: &str) -> Outcome<EngineError> {
    let found: Found = Mutex::new(Vec::new());
    let inspect = |part: &str, data: &[u8]| inspect_part(&found, part, data);
    zenkai_xlsx_reader::scan_with_caps(bytes, &inspect).map_err(engine_error)?;
    let book = run_with_engine_stack(|| read_book(bytes, name))?;
    Ok((book, take_unsupported(found)?))
}

fn take_unsupported(found: Found) -> Result<Vec<Unsupported>, EngineError> {
    let found = found
        .into_inner()
        .map_err(|_| EngineError::InvalidFile("a part check stopped".to_string()))?;
    Ok(kinds(&found))
}

fn read_streaming(
    path: &Path,
    name: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<FileRead, ReadError> {
    let found: Found = Mutex::new(Vec::new());
    let inspect = |part: &str, data: &[u8]| inspect_part(&found, part, data);
    let opened = tracing::info_span!("open_read_streaming")
        .in_scope(|| {
            run_with_engine_stack(|| Ok(read_book_streaming(path, name, &inspect, cancel)))
        })
        .map_err(|error| ReadError::Unsupported(error.to_string()))??;
    let found = found
        .into_inner()
        .map_err(|_| ReadError::Unsupported("a part check stopped".to_string()))?;
    let held = Held::of(&found, &opened.layout);
    let mut budget = ElementBudget::workbook();
    let settings = opened
        .sheet_configs
        .iter()
        .zip(&opened.layout.sheet_links)
        .map(|(config, links)| {
            if config.is_empty() {
                return SheetSettings::default();
            }
            let mut settings = sheet_settings::read(config, &mut budget);
            if matches!(links, SheetLinks::Known(known) if known.is_empty()) {
                settings.elements.nothing_linked();
            }
            settings
        })
        .collect();
    Ok(FileRead {
        read: ReadBook {
            book: opened.book,
            unsupported: kinds(&found),
            fallback: None,
        },
        settings,
        parts: Some(OpenedParts {
            layout: opened.layout,
            held,
        }),
    })
}

fn read_fast(bytes: &[u8], name: &str) -> Outcome<ReadError> {
    let found: Found = Mutex::new(Vec::new());
    let inspect = |part: &str, data: &[u8]| inspect_part(&found, part, data);
    let book = run_with_engine_stack(|| Ok(read_book_fast(bytes, name, &inspect)))
        .map_err(|error| ReadError::Unsupported(error.to_string()))??;
    let found = found
        .into_inner()
        .map_err(|_| ReadError::Unsupported("a part check stopped".to_string()))?;
    Ok((book, kinds(&found)))
}

#[derive(Debug)]
pub enum ReaderComparison {
    Same,
    Different(Vec<String>),
    BothRefused,
    FastDeclined(String),
    FastRefusedAlone(String),
    FastReadAlone(String),
}

pub fn compare_readers(bytes: &[u8]) -> ReaderComparison {
    compare(
        &read_with_ironcalc(bytes, "Book"),
        &read_fast(bytes, "Book"),
    )
}

fn compare(ironcalc: &Outcome<EngineError>, fast: &Outcome<ReadError>) -> ReaderComparison {
    match (ironcalc, fast) {
        (Ok((expected, expected_unsupported)), Ok((actual, actual_unsupported))) => {
            let mut differences = workbook_differences(expected, actual, SHADOW_REPORT_LINES);
            if expected_unsupported != actual_unsupported {
                differences.push(format!(
                    "unsupported features: expected {expected_unsupported:?}, got {actual_unsupported:?}"
                ));
            }
            if differences.is_empty() {
                ReaderComparison::Same
            } else {
                ReaderComparison::Different(differences)
            }
        }
        (Err(_), Err(ReadError::Unsafe(_))) => ReaderComparison::BothRefused,
        (Ok(_), Err(ReadError::Unsafe(reason))) => {
            ReaderComparison::FastRefusedAlone(reason.clone())
        }
        (_, Err(error)) => ReaderComparison::FastDeclined(error.to_string()),
        (Err(error), Ok(_)) => ReaderComparison::FastReadAlone(error.to_string()),
    }
}
