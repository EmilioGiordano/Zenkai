// A cancelled open must end as `Cancelled`, never as a read failure that the callers
// answer with a second full read through a fallback reader.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rust_xlsxwriter::{Formula, Workbook as Source};
use zenkai_engine::{EngineError, XlsxReader, open_xlsx_cancellable};

fn sample(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("book.xlsx");
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    for row in 0..2_000u32 {
        sheet.write_number(row, 0, f64::from(row)).unwrap();
        sheet
            .write_formula(row, 1, Formula::new(format!("=A{}*2", row + 1)))
            .unwrap();
    }
    book.save(&path).unwrap();
    path
}

#[test]
fn a_raised_flag_ends_every_reader_as_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let path = sample(&dir);
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc, XlsxReader::Shadow] {
        let cancel = Arc::new(AtomicBool::new(true));
        let error = open_xlsx_cancellable(&path, reader, &cancel).err();
        assert!(matches!(error, Some(EngineError::Cancelled)), "{reader:?}");
    }
}

#[test]
fn a_flag_that_stays_down_opens_the_workbook() {
    let dir = tempfile::tempdir().unwrap();
    let path = sample(&dir);
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let opened = open_xlsx_cancellable(&path, reader, &Arc::default());
        assert!(opened.is_ok(), "{reader:?}");
    }
}
