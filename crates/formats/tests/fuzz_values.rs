// Structured fuzzing of the read-only fallback reader (calamine): small valid .xls, .ods and
// .xlsb files are damaged the way a corrupt or hostile file would be. Reading must end in
// values or an error, within a bounded time; a panic inside calamine must come back as
// `ValuesError::Panicked`, never unwind through the caller.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use proptest::prelude::*;
use zenkai_formats::{ValuesError, read_values};

#[path = "../../../test-support/xlsx_damage.rs"]
mod xlsx_damage;

use xlsx_damage::{Damage, apply, corpus, damage, parts_of, zip_of};

const HANG_AFTER: Duration = Duration::from_secs(60);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/compat")
        .join(name)
}

fn record(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = if kind < 0x80 {
        vec![kind as u8]
    } else {
        vec![(kind & 0x7F) as u8 | 0x80, (kind >> 7) as u8]
    };
    let mut length = payload.len();
    loop {
        let low = (length & 0x7F) as u8;
        length >>= 7;
        if length == 0 {
            out.push(low);
            break;
        }
        out.push(low | 0x80);
    }
    out.extend_from_slice(payload);
    out
}

fn wide(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = (units.len() as u32).to_le_bytes().to_vec();
    out.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
    out
}

fn cell(column: u32, tail: &[u8]) -> Vec<u8> {
    let mut out = column.to_le_bytes().to_vec();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(tail);
    out
}

fn xlsb_parts(cells: &[Vec<u8>]) -> Vec<(String, Vec<u8>)> {
    let mut sheet_entry = vec![0u8; 8];
    sheet_entry.extend(wide("rId1"));
    sheet_entry.extend(wide("Data"));
    let workbook = [
        record(0x9C, &sheet_entry),
        record(0x90, &[]),
        record(0x9D, &[]),
    ]
    .concat();
    let strings = [
        record(0x9F, &[2, 0, 0, 0, 2, 0, 0, 0]),
        record(0x13, &[&[0u8][..], &wide("alpha")].concat()),
        record(0x13, &[&[0u8][..], &wide("=1+1")].concat()),
        record(0xA0, &[]),
    ]
    .concat();
    let mut sheet = vec![record(0x81, &[]), record(0x94, &[0; 16]), record(0x91, &[])];
    for (row, cell_records) in (0u32..).zip(cells) {
        sheet.push(record(0x00, &[&row.to_le_bytes()[..], &[0; 12]].concat()));
        sheet.push(cell_records.clone());
    }
    sheet.push(record(0x92, &[]));
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.bin"/></Relationships>"#;
    vec![
        ("xl/workbook.bin".to_string(), workbook),
        ("xl/_rels/workbook.bin.rels".to_string(), rels.into()),
        ("xl/sharedStrings.bin".to_string(), strings),
        ("xl/worksheets/sheet1.bin".to_string(), sheet.concat()),
    ]
}

fn valid_xlsb_cells() -> Vec<Vec<u8>> {
    (0u32..4)
        .map(|row| {
            [
                record(0x05, &cell(0, &f64::from(row).to_le_bytes())),
                record(0x06, &cell(1, &wide("text"))),
                record(0x07, &cell(2, &(row % 2).to_le_bytes())),
                record(0x04, &cell(3, &[1])),
                record(0x03, &cell(4, &[0x07])),
                record(0x02, &cell(5, &[0x0C, 0, 0, 0])),
            ]
            .concat()
        })
        .collect()
}

fn valid_xlsb() -> Vec<u8> {
    zip_of(&xlsb_parts(&valid_xlsb_cells()))
}

fn valid_ods() -> Vec<u8> {
    let content = r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:spreadsheet><table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>text</text:p></table:table-cell><table:table-cell table:number-columns-repeated="3" office:value-type="boolean" office:boolean-value="true"/></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#;
    zip_of(&[
        (
            "mimetype".to_string(),
            b"application/vnd.oasis.opendocument.spreadsheet".to_vec(),
        ),
        ("content.xml".to_string(), content.into()),
        (
            "META-INF/manifest.xml".to_string(),
            br#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>"#.to_vec(),
        ),
    ])
}

fn damaged_parts(mut parts: Vec<(String, Vec<u8>)>, damages: &[Damage]) -> Vec<(String, Vec<u8>)> {
    for damage in damages {
        if !parts.is_empty() {
            apply(&mut parts, damage);
        }
    }
    parts
}

fn parts_of_bytes(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let (_folder, path) = write_temp("parts.zip", bytes);
    parts_of(&path)
}

fn damaged_zip(source: &Path, damages: &[Damage]) -> Vec<u8> {
    zip_of(&damaged_parts(parts_of(source), damages))
}

fn damaged_blob(bytes: &[u8], damages: &[Damage]) -> Vec<u8> {
    damaged_parts(vec![("blob".to_string(), bytes.to_vec())], damages)
        .into_iter()
        .next()
        .map(|(_, bytes)| bytes)
        .unwrap_or_default()
}

fn read_with_watchdog(extension: &'static str, bytes: Vec<u8>) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(format!("fuzzed.{extension}"));
        std::fs::write(&path, &bytes).unwrap();
        if let Err(error) = read_values(&path, &AtomicBool::new(false)) {
            assert!(!error.to_string().is_empty());
        }
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(HANG_AFTER).is_ok(),
        "the fallback reader hung or died on a damaged .{extension}"
    );
}

fn write_temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (folder, path)
}

fn damages() -> impl Strategy<Value = Vec<Damage>> {
    prop::collection::vec(damage(), 1..4)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn damaged_xlsb_reads_or_fails(damages in damages()) {
        let (_folder, source) = write_temp("valid.xlsb", &valid_xlsb());
        read_with_watchdog("xlsb", damaged_zip(&source, &damages));
    }

    #[test]
    fn damaged_ods_reads_or_fails(damages in damages(), from_fixture in any::<bool>()) {
        let (_folder, source) = if from_fixture {
            (None, fixture("issues.ods"))
        } else {
            let (folder, path) = write_temp("valid.ods", &valid_ods());
            (Some(folder), path)
        };
        read_with_watchdog("ods", damaged_zip(&source, &damages));
    }

    #[test]
    fn damaged_xlsx_reads_or_fails(damages in damages(), pick in any::<prop::sample::Index>()) {
        let corpus = corpus();
        read_with_watchdog("xlsx", damaged_zip(&corpus[pick.index(corpus.len())], &damages));
    }

    #[test]
    fn damaged_xls_reads_or_fails(damages in damages(), date in any::<bool>()) {
        let name = if date { "date.xls" } else { "issues.xls" };
        let source = std::fs::read(fixture(name)).unwrap();
        read_with_watchdog("xls", damaged_blob(&source, &damages));
    }
}

#[test]
fn the_valid_inputs_read_before_any_damage() {
    for (name, bytes) in [
        ("valid.xlsb", valid_xlsb()),
        ("valid.ods", valid_ods()),
        ("valid.xls", std::fs::read(fixture("issues.xls")).unwrap()),
    ] {
        let (_folder, path) = write_temp(name, &bytes);
        let sheets = read_values(&path, &AtomicBool::new(false)).unwrap();
        assert!(!sheets[0].blocks.is_empty(), "{name} read no cells");
    }
}

#[test]
fn a_cancelled_read_stops_with_a_cancelled_error() {
    let outcome = read_values(&fixture("issues.xls"), &AtomicBool::new(true));
    assert!(matches!(outcome, Err(ValuesError::Cancelled)));
}

#[test]
fn a_shared_string_index_out_of_range_is_an_error_not_a_crash() {
    let cells = vec![record(0x07, &cell(0, &99u32.to_le_bytes()))];
    let (_folder, path) = write_temp("bad_index.xlsb", &zip_of(&xlsb_parts(&cells)));
    let outcome = read_values(&path, &AtomicBool::new(false));
    assert!(matches!(outcome, Err(ValuesError::Panicked)));
}

#[test]
fn an_ods_cut_inside_its_table_is_an_error_not_an_endless_loop() {
    let mut parts = parts_of_bytes(&valid_ods());
    let content = &mut parts
        .iter_mut()
        .find(|(name, _)| name == "content.xml")
        .unwrap()
        .1;
    let cut = content
        .windows(b"</table:table>".len())
        .position(|window| window == b"</table:table>")
        .unwrap();
    content.truncate(cut);
    let (_folder, path) = write_temp("cut.ods", &zip_of(&parts));
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        done.send(read_values(&path, &AtomicBool::new(false)))
            .unwrap();
    });
    let outcome = finished.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(matches!(outcome, Err(ValuesError::Unreadable(_))));
}
