use std::fs::{self, File};
use std::io::{BufWriter, Cursor};
use std::ops::Range;
use std::path::Path;

use zenkai_types::{CellPos, SheetId};

use super::{FullPlan, Plan, SaveTarget, verify, write_temp};
use crate::error::EngineError;
use crate::workbook::{Engine, Workbook};

const SHEET: &str = "xl/worksheets/sheet1.xml";

fn plan() -> FullPlan {
    let mut book = Workbook::new_empty().unwrap();
    let rows: Vec<Vec<String>> = (1..=2000)
        .map(|row| vec![row.to_string(), format!("=A{row}*2"), format!("text {row}")])
        .collect();
    book.set_inputs(SheetId(0), CellPos::default(), &rows)
        .unwrap();
    let snapshot = book
        .save_snapshot(Path::new("book.xlsx"), SaveTarget::Copy)
        .unwrap();
    match snapshot.plan {
        Plan::Full(plan) => plan,
        Plan::Patch(_) => panic!("a copy without a saved file is written whole"),
    }
}

fn written_then_verified(
    plan: FullPlan,
    corrupt: impl FnOnce(&mut Vec<u8>),
) -> Result<(), EngineError> {
    let dir = tempfile::tempdir().unwrap();
    let temp = dir.path().join("book.xlsx");
    let file = BufWriter::new(File::create(&temp).unwrap());
    let written = write_temp(Plan::Full(plan), file, &temp, &mut None)?;
    let mut bytes = fs::read(&temp).unwrap();
    corrupt(&mut bytes);
    fs::write(&temp, &bytes).unwrap();
    verify(&temp, None, &written)
}

fn sheet_data(bytes: &[u8]) -> Range<usize> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).unwrap();
        if entry.name() == SHEET {
            assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
            let start = usize::try_from(entry.data_start()).unwrap();
            let length = usize::try_from(entry.compressed_size()).unwrap();
            assert!(length > 1024, "the sheet is large enough to corrupt");
            return start..start + length;
        }
    }
    panic!("the sheet was not written");
}

fn central_header(bytes: &[u8], name: &str) -> usize {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let entry = archive.by_name(name).unwrap();
    usize::try_from(entry.central_header_start()).unwrap()
}

fn rejected_on_verify(result: Result<(), EngineError>) {
    assert!(
        matches!(result, Err(EngineError::VerifyFailed(_))),
        "{result:?}"
    );
}

#[test]
fn an_intact_file_passes() {
    written_then_verified(plan(), |_| {}).unwrap();
}

#[test]
fn a_flipped_byte_in_a_generated_sheet_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let data = sheet_data(bytes);
        bytes[data.start + data.len() / 2] ^= 0x01;
    }));
}

// Inflating did not notice this one: the last byte of a deflate stream can carry
// padding bits.
#[test]
fn a_flipped_last_byte_of_a_generated_sheet_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let data = sheet_data(bytes);
        bytes[data.end - 1] ^= 0x80;
    }));
}

#[test]
fn a_zeroed_tail_of_a_generated_sheet_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let data = sheet_data(bytes);
        bytes[data.end - 512..data.end].fill(0);
    }));
}

#[test]
fn bytes_cut_out_of_a_generated_sheet_fail() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let data = sheet_data(bytes);
        let middle = data.start + data.len() / 2;
        bytes.drain(middle..middle + 64);
    }));
}

#[test]
fn a_truncated_file_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let data = sheet_data(bytes);
        bytes.truncate(data.start + data.len() / 2);
    }));
}

#[test]
fn a_cut_central_directory_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        bytes.truncate(bytes.len() - 30);
    }));
}

#[test]
fn a_wrong_checksum_in_the_central_directory_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let header = central_header(bytes, SHEET);
        bytes[header + 16] ^= 0x01;
    }));
}

// Inflating did not notice this one either: nothing read the uncompressed size.
#[test]
fn a_wrong_size_in_the_central_directory_fails() {
    rejected_on_verify(written_then_verified(plan(), |bytes| {
        let header = central_header(bytes, SHEET);
        bytes[header + 24] ^= 0x01;
    }));
}

#[test]
fn malformed_generated_xml_fails() {
    for malformed in ["<a><b></a>", "<a>", "<a/><b/>", "text"] {
        let mut plan = plan();
        plan.parts
            .push(("xl/extra.xml".to_string(), malformed.to_string()));
        rejected_on_verify(written_then_verified(plan, |_| {}));
    }
}
