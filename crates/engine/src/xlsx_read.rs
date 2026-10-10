use std::sync::Mutex;

use ironcalc::base::types::Workbook as Book;
use zenkai_xlsx_reader::{ReadError, workbook_differences};

use crate::error::EngineError;
use crate::file::{Unsupported, part_unsupported, scan_unsupported};
use crate::preflight::run_with_engine_stack;
use crate::workbook::{read_book, read_book_fast};

const SHADOW_REPORT_LINES: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XlsxReader {
    #[default]
    IronCalc,
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
            Err(ReadError::Unsafe(reason)) => return Err(EngineError::Unsafe(reason)),
            Err(error) => {
                tracing::warn!(%error, "the fast xlsx reader left this file to IronCalc's");
                let (book, unsupported) = read_with_ironcalc(bytes, name)?;
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

fn read_with_ironcalc(bytes: &[u8], name: &str) -> Outcome<EngineError> {
    let unsupported = scan_unsupported(bytes)?;
    let book = run_with_engine_stack(|| read_book(bytes, name))?;
    Ok((book, unsupported))
}

fn read_fast(bytes: &[u8], name: &str) -> Outcome<ReadError> {
    let found = Mutex::new(Vec::new());
    let inspect = |part: &str, data: &[u8]| -> Result<(), String> {
        let kinds = part_unsupported(part, data).map_err(|error| match error {
            EngineError::Unsafe(reason) => reason,
            other => other.to_string(),
        })?;
        found
            .lock()
            .map_err(|_| "a part check stopped".to_string())?
            .extend(kinds);
        Ok(())
    };
    let book = run_with_engine_stack(|| Ok(read_book_fast(bytes, name, &inspect)))
        .map_err(|error| ReadError::Unsupported(error.to_string()))??;
    let mut unsupported = found
        .into_inner()
        .map_err(|_| ReadError::Unsupported("a part check stopped".to_string()))?;
    unsupported.sort();
    unsupported.dedup();
    Ok((book, unsupported))
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
