use std::io::{Read, Seek};

use crate::ReadError;
use crate::package::Package;
use crate::stream::{Budgets, CHUNK_BYTES, CompressedMeter};

// IronCalc's importer reads the shared strings from this path whatever the
// relationships say.
const SHARED_STRINGS_PART: &str = "xl/sharedStrings.xml";

// Where the parts of an opened file live, for a save that copies the parts it does
// not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageLayout {
    pub worksheets: Vec<String>,
    pub shared_strings: SharedStrings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedStrings {
    Absent,
    // The part holds exactly the strings the model starts with, in the same order,
    // under an unprefixed <sst> that new <si> elements can be appended to.
    Aligned(usize),
    Unaligned,
}

pub(crate) fn shared_strings_layout<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
    package: &Package,
    imported: usize,
) -> Result<SharedStrings, ReadError> {
    let exists = archive.by_name(SHARED_STRINGS_PART).is_ok();
    match package.shared_strings.as_deref() {
        None if !exists => return Ok(SharedStrings::Absent),
        Some(SHARED_STRINGS_PART) if exists => {}
        _ => return Ok(SharedStrings::Unaligned),
    }
    let entry = archive
        .by_name(SHARED_STRINGS_PART)
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    let mut guard = budgets.stream_reread(entry, meter, SHARED_STRINGS_PART);
    let mut scan = StringScan::default();
    let mut chunk = [0u8; CHUNK_BYTES];
    loop {
        match guard.read_chunk(&mut chunk)? {
            0 => break,
            read => scan.push(&chunk[..read]),
        }
    }
    scan.finish();
    Ok(match scan {
        StringScan {
            unprefixed_root: true,
            character_data: false,
            items,
            ..
        } if items == imported => SharedStrings::Aligned(items),
        _ => SharedStrings::Unaligned,
    })
}

// Counts <si> start tags across chunk boundaries. Text holds no '<', so every match is
// a tag, except inside CDATA, whose presence alone makes the part unaligned.
#[derive(Default)]
struct StringScan {
    carry: Vec<u8>,
    items: usize,
    unprefixed_root: bool,
    character_data: bool,
}

const LONGEST_PATTERN: usize = 9;

impl StringScan {
    fn push(&mut self, chunk: &[u8]) {
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(chunk);
        let delimited = |at: usize, name: &[u8]| {
            window[at..].starts_with(name)
                && matches!(
                    window.get(at + name.len()),
                    Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r')
                )
        };
        let scanned = window.len().saturating_sub(LONGEST_PATTERN);
        for at in 0..scanned {
            if window[at] != b'<' {
                continue;
            }
            if delimited(at, b"<si") {
                self.items += 1;
            } else if delimited(at, b"<sst") {
                self.unprefixed_root = true;
            } else if window[at..].starts_with(b"<![CDATA[") {
                self.character_data = true;
            }
        }
        self.carry = window[scanned..].to_vec();
    }

    // Flushes the held tail; the padding is no tag, so it adds no match.
    fn finish(&mut self) {
        self.push(&[b' '; LONGEST_PATTERN]);
    }
}

#[cfg(test)]
mod tests {
    use super::StringScan;

    fn scanned(xml: &str, chunk: usize) -> StringScan {
        let mut scan = StringScan::default();
        for piece in xml.as_bytes().chunks(chunk) {
            scan.push(piece);
        }
        scan.finish();
        scan
    }

    #[test]
    fn counts_items_split_across_chunks() {
        let xml = r#"<sst xmlns="m"><si><t>a</t></si><si><r><t>b</t></r></si><si/></sst>"#;
        for chunk in 1..xml.len() {
            let scan = scanned(xml, chunk);
            assert_eq!(scan.items, 3, "chunk {chunk}");
            assert!(scan.unprefixed_root);
            assert!(!scan.character_data);
        }
    }

    #[test]
    fn a_prefixed_root_or_character_data_is_seen() {
        let prefixed = scanned(r#"<x:sst xmlns:x="m"><x:si><x:t>a</x:t></x:si></x:sst>"#, 4);
        assert!(!prefixed.unprefixed_root);
        assert_eq!(prefixed.items, 0);
        let cdata = scanned(r#"<sst><si><t><![CDATA[<si>]]></t></si></sst>"#, 5);
        assert!(cdata.character_data);
    }
}

#[cfg(test)]
mod read_tests {
    use std::io::Cursor;

    use ironcalc::base::Model;

    use super::SharedStrings;

    fn saved(model: &Model) -> Vec<u8> {
        ironcalc::export::save_xlsx_to_writer(model, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner()
    }

    #[test]
    fn an_opened_file_reports_its_parts_and_aligned_strings() {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        model.set_user_input(0, 1, 1, "one".to_string()).unwrap();
        model.set_user_input(0, 2, 1, "two".to_string()).unwrap();
        model.new_sheet();
        model.evaluate();
        let opened = crate::read_xlsx(&saved(&model), "b", "en", "UTC", &|_, _| Ok(())).unwrap();
        assert_eq!(
            opened.layout.worksheets,
            ["xl/worksheets/sheet1.xml", "xl/worksheets/sheet2.xml"]
        );
        assert_eq!(opened.layout.shared_strings, SharedStrings::Aligned(2));
    }
}
