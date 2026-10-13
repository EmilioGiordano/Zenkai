use std::io::{Read, Seek};

use crate::ReadError;
use crate::package::{Package, read_entry};
use crate::shared_strings::{SHARED_STRINGS_PART, SST_HEAD_BYTES, sst_root};
use crate::stream::{Budgets, CHUNK_BYTES, CompressedMeter};

// IronCalc's importer reads the styles from this path whatever the relationships say.
const STYLES_PART: &str = "xl/styles.xml";

// Where the parts of an opened file live, for a save that copies the parts it does
// not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageLayout {
    pub worksheets: Vec<String>,
    pub shared_strings: SharedStrings,
    pub styles: StyleCounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedStrings {
    Absent,
    // The part holds exactly the strings the model starts with, in the same order,
    // under an unprefixed <sst> that new <si> elements can be appended to.
    Aligned(usize),
    Unaligned,
}

// The elements of the styles part a cell's s="" and a rule's dxfId="" count into. A sheet
// written by the engine points into the copied part only when the model holds as many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleCounts {
    pub cell_xfs: usize,
    pub dxfs: usize,
}

pub(crate) fn style_counts<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<StyleCounts, ReadError> {
    let unreadable = |e: &dyn std::fmt::Display| ReadError::Unreadable(e.to_string());
    let entry = archive.by_name(STYLES_PART).map_err(|e| unreadable(&e))?;
    let bytes = read_entry(entry, meter, budgets)?;
    let text = std::str::from_utf8(&bytes).map_err(|e| unreadable(&e))?;
    let styles = roxmltree::Document::parse(text).map_err(|e| unreadable(&e))?;
    let count = |list: &str, item: &str| {
        styles
            .root_element()
            .children()
            .find(|node| node.has_tag_name(list))
            .map_or(0, |node| {
                node.children()
                    .filter(|child| child.has_tag_name(item))
                    .count()
            })
    };
    Ok(StyleCounts {
        cell_xfs: count("cellXfs", "xf"),
        dxfs: count("dxfs", "dxf"),
    })
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
    let closed = sst_root(&scan.head).is_some() && scan.ending == b"</sst>";
    Ok(match scan {
        StringScan {
            unprefixed_root: true,
            character_data: false,
            items,
            ..
        } if items == imported && closed => SharedStrings::Aligned(items),
        _ => SharedStrings::Unaligned,
    })
}

// Counts <si> start tags across chunk boundaries. Text holds no '<', so every match is
// a tag, except inside CDATA, whose presence alone makes the part unaligned. The head
// and the last characters other than white space are kept to check the part can be
// appended to the way a save does it.
#[derive(Default)]
struct StringScan {
    carry: Vec<u8>,
    items: usize,
    unprefixed_root: bool,
    character_data: bool,
    head: Vec<u8>,
    ending: Vec<u8>,
}

const LONGEST_PATTERN: usize = 9;
const ENDING: &[u8] = b"</sst>";

impl StringScan {
    fn push(&mut self, chunk: &[u8]) {
        let wanted = SST_HEAD_BYTES
            .saturating_sub(self.head.len())
            .min(chunk.len());
        self.head.extend_from_slice(&chunk[..wanted]);
        let mut last: Vec<u8> = chunk
            .iter()
            .rev()
            .filter(|byte| !byte.is_ascii_whitespace())
            .take(ENDING.len())
            .copied()
            .collect();
        last.reverse();
        self.ending.extend_from_slice(&last);
        let excess = self.ending.len().saturating_sub(ENDING.len());
        self.ending.drain(..excess);
        self.scan(chunk);
    }

    fn scan(&mut self, chunk: &[u8]) {
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
        self.scan(&[b' '; LONGEST_PATTERN]);
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

    #[test]
    fn the_closing_tag_is_seen_past_trailing_white_space() {
        let xml = "<sst><si><t>a</t></si></sst  >
  
";
        for chunk in 1..xml.len() {
            let scan = scanned(xml, chunk);
            assert_eq!(scan.ending, b"</sst>", "chunk {chunk}");
            assert!(scan.head.starts_with(b"<sst>"));
        }
        assert_ne!(scanned("<sst><si/></sst><!-- -->", 3).ending, b"</sst>");
    }
}

#[cfg(test)]
mod read_tests {
    use std::io::Cursor;

    use ironcalc::base::Model;

    use super::{SharedStrings, StyleCounts};

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
        let styles = &opened.book.styles;
        assert_eq!(
            opened.layout.styles,
            StyleCounts {
                cell_xfs: styles.cell_xfs.len(),
                dxfs: styles.dxfs.len(),
            }
        );
    }
}
