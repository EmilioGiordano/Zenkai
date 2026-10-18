use std::fs::File;
use std::io::{self, BufReader, Seek, Write};
use std::ops::Range;
use std::path::Path;

use crate::ReadError;
use crate::layout::{
    PartNames, StringScan, StyleCounts, names_in_order, part_names_of, style_counts,
};
use crate::limits;
use crate::package::{part_path, read_entry, read_package};
use crate::shared_strings::{GuardRead, SHARED_STRINGS_PART, copy_appended};
use crate::splice::{ReachingAnchor, RowSpan, splice_rows};
use crate::stream::{Budgets, CHUNK_BYTES, CompressedMeter, Counted};

const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
const WORKBOOK_RELS_PART: &str = "xl/_rels/workbook.xml.rels";

// The file on disk a save copies its unchanged parts from, read under the same caps as
// an open.
pub struct BasePackage {
    archive: zip::ZipArchive<Counted<BufReader<File>>>,
    meter: CompressedMeter,
    budgets: Budgets,
}

pub struct BaseFacts {
    pub worksheets: Vec<String>,
    pub styles: StyleCounts,
}

// The calculation chain lists formula cells and goes stale once a sheet is rewritten;
// Excel rebuilds it when it is missing. A save leaves out `dropped` and writes `rewritten`
// in place of the parts of the same name.
pub struct CalcChain {
    pub dropped: Vec<String>,
    pub rewritten: Vec<(String, Vec<u8>)>,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn without_ranges(text: &str, mut ranges: Vec<Range<usize>>) -> Vec<u8> {
    ranges.sort_by_key(|range| range.start);
    let mut kept = Vec::with_capacity(text.len());
    let mut from = 0;
    for range in ranges {
        kept.extend_from_slice(&text.as_bytes()[from..range.start]);
        from = range.end;
    }
    kept.extend_from_slice(&text.as_bytes()[from..]);
    kept
}

impl BasePackage {
    pub fn open(path: &Path) -> Result<BasePackage, ReadError> {
        let meter = CompressedMeter::new();
        let file = File::open(path).map_err(unreadable)?;
        let mut archive =
            zip::ZipArchive::new(meter.wrap(BufReader::new(file))).map_err(unreadable)?;
        limits::check_archive(&mut archive)?;
        if part_names_of(&names_in_order(&mut archive)?) == PartNames::Duplicated {
            return Err(unreadable("the file holds a part twice"));
        }
        Ok(BasePackage {
            archive,
            meter,
            budgets: Budgets::production(),
        })
    }

    // In archive order, which a save keeps.
    pub fn part_names(&mut self) -> Result<Vec<String>, ReadError> {
        names_in_order(&mut self.archive)
    }

    pub fn copy_raw<W: Write + Seek>(
        &mut self,
        index: usize,
        zip: &mut zip::ZipWriter<W>,
    ) -> io::Result<()> {
        let entry = self.archive.by_index_raw(index).map_err(io::Error::other)?;
        zip.raw_copy_file(entry).map_err(io::Error::other)
    }

    fn read_text(&mut self, name: &str) -> Result<String, ReadError> {
        let entry = self.archive.by_name(name).map_err(unreadable)?;
        String::from_utf8(read_entry(entry, &self.meter, &self.budgets)?).map_err(unreadable)
    }

    pub fn calc_chain(&mut self) -> Result<Option<CalcChain>, ReadError> {
        let rels_text = self.read_text(WORKBOOK_RELS_PART)?;
        let rels = roxmltree::Document::parse(&rels_text).map_err(unreadable)?;
        let chains: Vec<_> = rels
            .descendants()
            .filter(|node| {
                node.has_tag_name("Relationship")
                    && node
                        .attribute("Type")
                        .is_some_and(|kind| kind.ends_with("/calcChain"))
            })
            .collect();
        if chains.is_empty() {
            return Ok(None);
        }
        let parts: Vec<String> = chains
            .iter()
            .filter_map(|node| node.attribute("Target"))
            .map(part_path)
            .collect();
        let workbook_rels = without_ranges(&rels_text, chains.iter().map(|n| n.range()).collect());
        let types_text = self.read_text(CONTENT_TYPES_PART)?;
        let types = roxmltree::Document::parse(&types_text).map_err(unreadable)?;
        let overrides = types
            .descendants()
            .filter(|node| {
                node.has_tag_name("Override")
                    && node.attribute("PartName").is_some_and(|name| {
                        let name = name.trim_start_matches('/');
                        parts.iter().any(|part| part.eq_ignore_ascii_case(name))
                    })
            })
            .map(|node| node.range())
            .collect();
        Ok(Some(CalcChain {
            rewritten: vec![
                (
                    CONTENT_TYPES_PART.to_string(),
                    without_ranges(&types_text, overrides),
                ),
                (WORKBOOK_RELS_PART.to_string(), workbook_rels),
            ],
            dropped: parts,
        }))
    }

    // The shared strings part with `appended`, whole <si> elements, before </sst>. The
    // open checked the part can take them (`SharedStrings::Aligned`).
    pub fn copy_shared_strings(
        &mut self,
        expected: usize,
        appended: &[u8],
        out: &mut impl Write,
    ) -> io::Result<()> {
        let entry = self
            .archive
            .by_name(SHARED_STRINGS_PART)
            .map_err(io::Error::other)?;
        let mut guard = self.budgets.stream(entry, &self.meter, SHARED_STRINGS_PART);
        copy_appended(&mut guard, expected, appended, out)
    }

    // The worksheet `part` with the rows of `spans` replaced by what `write_span` writes and
    // every other row copied byte for byte. An error means the part cannot be spliced
    // safely and has to be written whole.
    pub fn splice_rows<W: Write>(
        &mut self,
        part: &str,
        dimension: &str,
        spans: &[RowSpan],
        out: &mut W,
        write_span: impl FnMut(&RowSpan, &[ReachingAnchor], &mut W) -> io::Result<()>,
    ) -> io::Result<()> {
        let entry = self.archive.by_name(part).map_err(io::Error::other)?;
        let mut guard = self.budgets.stream_worksheet(entry, &self.meter, part);
        splice_rows(GuardRead(&mut guard), out, dimension, spans, write_span)
    }

    // What a save relies on when it writes sheets into this file, read again because a
    // file can change on disk and keep its length and modification time.
    pub fn facts(&mut self) -> Result<BaseFacts, ReadError> {
        let package = read_package(&mut self.archive, &self.meter, &self.budgets)?;
        Ok(BaseFacts {
            worksheets: package
                .worksheets
                .into_iter()
                .map(|part| part.path)
                .collect(),
            styles: style_counts(&mut self.archive, &self.meter, &self.budgets)?,
        })
    }

    // The items of the shared strings part, when a save could append to it.
    pub fn shared_string_count(&mut self) -> Result<Option<usize>, ReadError> {
        let entry = self
            .archive
            .by_name(SHARED_STRINGS_PART)
            .map_err(unreadable)?;
        let mut guard = self.budgets.stream(entry, &self.meter, SHARED_STRINGS_PART);
        let mut scan = StringScan::default();
        let mut chunk = vec![0u8; CHUNK_BYTES];
        loop {
            match guard.read_chunk(&mut chunk)? {
                0 => break,
                read => scan.push(&chunk[..read]),
            }
        }
        Ok(scan.appendable_items())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use super::*;

    const RELS: &str = r#"<Relationships xmlns="r"><Relationship Id="rId1" Type="x/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId9" Type="x/calcChain" Target="calcChain.xml"/></Relationships>"#;
    const TYPES: &str = r#"<Types xmlns="t"><Default Extension="xml" ContentType="a"/><Override PartName="/xl/calcChain.xml" ContentType="c"/><Override PartName="/xl/workbook.xml" ContentType="w"/></Types>"#;

    fn package(dir: &Path, entries: &[(&str, &[u8])]) -> BasePackage {
        let path = dir.join("base.xlsx");
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            writer
                .start_file(*name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(body).unwrap();
        }
        std::fs::write(&path, writer.finish().unwrap().into_inner()).unwrap();
        BasePackage::open(&path).unwrap()
    }

    #[test]
    fn the_calc_chain_is_cut_out_of_the_package_parts() {
        let dir = tempfile::tempdir().unwrap();
        let mut base = package(
            dir.path(),
            &[
                (CONTENT_TYPES_PART, TYPES.as_bytes()),
                (WORKBOOK_RELS_PART, RELS.as_bytes()),
            ],
        );
        let chain = base.calc_chain().unwrap().unwrap();
        assert_eq!(chain.dropped, ["xl/calcChain.xml"]);
        assert_eq!(
            String::from_utf8(chain.rewritten[1].1.clone()).unwrap(),
            r#"<Relationships xmlns="r"><Relationship Id="rId1" Type="x/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
        );
        assert_eq!(
            String::from_utf8(chain.rewritten[0].1.clone()).unwrap(),
            r#"<Types xmlns="t"><Default Extension="xml" ContentType="a"/><Override PartName="/xl/workbook.xml" ContentType="w"/></Types>"#
        );
        assert_eq!(
            base.part_names().unwrap(),
            [CONTENT_TYPES_PART, WORKBOOK_RELS_PART]
        );
    }

    #[test]
    fn a_package_without_a_calc_chain_has_nothing_to_cut() {
        let dir = tempfile::tempdir().unwrap();
        let rels = r#"<Relationships xmlns="r"/>"#;
        let mut base = package(dir.path(), &[(WORKBOOK_RELS_PART, rels.as_bytes())]);
        assert!(base.calc_chain().unwrap().is_none());
    }

    #[test]
    fn strings_are_appended_and_parts_copied_raw() {
        let dir = tempfile::tempdir().unwrap();
        let sst = br#"<?xml version="1.0"?><sst xmlns="m" count="1" uniqueCount="1"><si><t>a</t></si></sst>"#;
        let mut base = package(dir.path(), &[(SHARED_STRINGS_PART, sst), ("x.bin", b"raw")]);
        let mut out = Vec::new();
        base.copy_shared_strings(1, b"<si><t>b</t></si>", &mut out)
            .unwrap();
        assert_eq!(base.shared_string_count().unwrap(), Some(1));
        assert!(base.copy_shared_strings(2, b"", &mut Vec::new()).is_err());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"<?xml version="1.0"?><sst xmlns="m"><si><t>a</t></si><si><t>b</t></si></sst>"#
        );
        let mut copy = zip::ZipWriter::new(Cursor::new(Vec::new()));
        base.copy_raw(1, &mut copy).unwrap();
        let mut copied = zip::ZipArchive::new(copy.finish().unwrap()).unwrap();
        let mut body = String::new();
        copied
            .by_name("x.bin")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "raw");
    }
}
