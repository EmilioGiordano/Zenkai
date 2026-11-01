// The real host on real and hostile workbooks. .xls and .ods are fuzzed here, not in process:
// calamine can hang or abort on them, and only a child process turns that into an error.
// Seeds that reproduced such failures are generated in code.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use zenkai_formats::read_values;
use zenkai_values_host::client::{ReadError, ValuesClient};

#[path = "../../../test-support/xlsx_damage.rs"]
mod xlsx_damage;

use xlsx_damage::{Damage, apply, damage, parts_of, zip_of};

const DEADLINE: Duration = Duration::from_secs(3);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/compat")
        .join(name)
}

fn client() -> ValuesClient {
    ValuesClient::new(Some(PathBuf::from(env!(
        "CARGO_BIN_EXE_zenkai-values-host"
    ))))
    .with_deadline(DEADLINE)
}

fn write_temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (folder, path)
}

// Values or any read error is fine; what must never happen is a host that cannot be found or
// started, or a read that outlives its deadline.
fn read_bytes_through_the_child(name: &str, bytes: &[u8]) -> Result<usize, ReadError> {
    let (_folder, path) = write_temp(name, bytes);
    let started = Instant::now();
    let outcome = client().read(&path, &AtomicBool::new(false));
    assert!(
        started.elapsed() < DEADLINE + Duration::from_secs(20),
        "{name} outlived its deadline"
    );
    assert!(
        !matches!(
            outcome,
            Err(ReadError::HostMissing | ReadError::StartFailed(_))
        ),
        "{name}: {outcome:?}"
    );
    outcome.map(|sheets| sheets.len())
}

fn valid_ods() -> Vec<u8> {
    let content = r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:spreadsheet><table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>text</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#;
    let manifest = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>"#;
    zip_of(&[
        (
            "mimetype".to_string(),
            b"application/vnd.oasis.opendocument.spreadsheet".to_vec(),
        ),
        ("content.xml".to_string(), content.into()),
        ("META-INF/manifest.xml".to_string(), manifest.into()),
    ])
}

fn ods_cut_inside_its_table() -> Vec<u8> {
    let mut parts = parts_of_bytes(&valid_ods());
    let content = &mut parts
        .iter_mut()
        .find(|(name, _)| name == "content.xml")
        .unwrap()
        .1;
    let end = b"</table:table>";
    let cut = content
        .windows(end.len())
        .position(|window| window == end)
        .unwrap();
    content.truncate(cut);
    zip_of(&parts)
}

// A 22 byte end-of-central-directory record announcing a 0xFFFF byte comment, appended: one
// zip reader rejects the archive and another accepts it, so a guard that parses the zip
// itself is blind to it.
fn ods_with_a_fake_end_record() -> Vec<u8> {
    let mut bytes = ods_cut_inside_its_table();
    bytes.extend_from_slice(b"PK\x05\x06");
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend_from_slice(&[0xFF, 0xFF]);
    bytes
}

fn parts_of_bytes(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let (_folder, path) = write_temp("parts.zip", bytes);
    parts_of(&path)
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

// The first sector of the difat chain is its own next sector, so the chain never ends.
fn xls_with_a_difat_cycle() -> Vec<u8> {
    let mut bytes = std::fs::read(fixture("date.xls")).unwrap();
    bytes[68..72].copy_from_slice(&0u32.to_le_bytes());
    let last = 512 + 512 - 4;
    bytes[last..last + 4].copy_from_slice(&0u32.to_le_bytes());
    bytes
}

// The directory's first sector is marked in the fat as followed by itself.
fn xls_with_a_self_looping_fat_chain() -> Vec<u8> {
    let mut bytes = std::fs::read(fixture("date.xls")).unwrap();
    let directory = u32_at(&bytes, 48);
    let fat_sector = u32_at(&bytes, 76 + 4 * (directory / 128));
    let entry = 512 + fat_sector * 512 + (directory % 128) * 4;
    bytes[entry..entry + 4].copy_from_slice(&(directory as u32).to_le_bytes());
    bytes
}

#[test]
fn the_host_returns_what_the_in_process_reader_returns() {
    for name in ["issues.xls", "date.xls", "issues.ods", "unicode.xlsx"] {
        let path = fixture(name);
        let expected = read_values(&path).unwrap();
        let received = client().read(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(received.len(), expected.len(), "{name}");
        for (got, want) in received.iter().zip(&expected) {
            assert_eq!(got.name, want.name, "{name}");
            let cells = |blocks: &[zenkai_formats::Block]| {
                blocks
                    .iter()
                    .flat_map(|block| block.rows.iter().flatten())
                    .filter(|cell| !cell.is_empty())
                    .count()
            };
            assert_eq!(cells(&got.blocks), cells(&want.blocks), "{name}");
        }
    }
}

#[test]
fn a_file_that_is_not_a_workbook_is_an_error() {
    let outcome = read_bytes_through_the_child("junk.xls", b"not a workbook at all");
    assert!(matches!(outcome, Err(ReadError::Failed(_))));
}

#[test]
fn the_seeds_that_broke_calamine_end_in_values_or_an_error() {
    let xls = std::fs::read(fixture("date.xls")).unwrap();
    let mut seeds = vec![
        ("cut.ods", ods_cut_inside_its_table()),
        ("fake_end.ods", ods_with_a_fake_end_record()),
        ("difat_cycle.xls", xls_with_a_difat_cycle()),
        ("fat_loop.xls", xls_with_a_self_looping_fat_chain()),
        (
            "ff_header.xls",
            [vec![0xFF; 512], xls[512..].to_vec()].concat(),
        ),
    ];
    for length in [0, 1, 8, 100, 511, 512, 600, 2000] {
        seeds.push(("short.xls", xls[..length].to_vec()));
    }
    for (name, bytes) in seeds {
        read_bytes_through_the_child(name, &bytes).ok();
    }
}

fn damaged_zip(source: &Path, damages: &[Damage]) -> Vec<u8> {
    let mut parts = parts_of(source);
    for damage in damages {
        if !parts.is_empty() {
            apply(&mut parts, damage);
        }
    }
    zip_of(&parts)
}

fn damaged_blob(bytes: &[u8], damages: &[Damage]) -> Vec<u8> {
    let mut parts = vec![("blob".to_string(), bytes.to_vec())];
    for damage in damages {
        if !parts.is_empty() {
            apply(&mut parts, damage);
        }
    }
    parts
        .into_iter()
        .next()
        .map(|(_, bytes)| bytes)
        .unwrap_or_default()
}

fn damages() -> impl Strategy<Value = Vec<Damage>> {
    prop::collection::vec(damage(), 1..4)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 60, max_shrink_iters: 0, ..ProptestConfig::default() })]

    #[test]
    fn damaged_ods_ends_in_values_or_an_error(damages in damages(), from_fixture in any::<bool>()) {
        let (_folder, source) = write_temp("source.zip", &if from_fixture {
            std::fs::read(fixture("issues.ods")).unwrap()
        } else {
            valid_ods()
        });
        read_bytes_through_the_child("fuzzed.ods", &damaged_zip(&source, &damages)).ok();
    }

    #[test]
    fn damaged_xls_ends_in_values_or_an_error(damages in damages(), date in any::<bool>()) {
        let name = if date { "date.xls" } else { "issues.xls" };
        let source = std::fs::read(fixture(name)).unwrap();
        read_bytes_through_the_child("fuzzed.xls", &damaged_blob(&source, &damages)).ok();
    }
}
