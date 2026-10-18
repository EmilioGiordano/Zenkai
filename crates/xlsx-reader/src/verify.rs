use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::ReadError;
use crate::layout::{PartNames, names_in_order, part_names_of};
use crate::limits;
use crate::package::read_package;
use crate::stream::{Budgets, CHUNK_BYTES, CompressedMeter, Counted, Guard};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    // Copied from the base file without inflating it.
    Copied,
    Generated,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn open(
    path: &Path,
    meter: &CompressedMeter,
) -> Result<zip::ZipArchive<Counted<BufReader<File>>>, ReadError> {
    let file = File::open(path).map_err(unreadable)?;
    zip::ZipArchive::new(meter.wrap(BufReader::new(file))).map_err(unreadable)
}

// Proves a file just written reads back without building a model: the archive holds
// exactly the planned parts in order, every copied part is byte for byte the base
// file's, and every generated part inflates with a matching checksum within the
// reader's caps and is well-formed XML. The workbook and its relationships parse,
// so the sheets resolve.
pub fn check_written(
    written: &Path,
    base: Option<&Path>,
    parts: &[(String, Written)],
) -> Result<(), ReadError> {
    let meter = CompressedMeter::new();
    let mut archive = open(written, &meter)?;
    limits::check_archive(&mut archive)?;
    let names = names_in_order(&mut archive)?;
    if part_names_of(&names) == PartNames::Duplicated {
        return Err(unreadable("the written file holds a part twice"));
    }
    if !names.iter().eq(parts.iter().map(|(name, _)| name)) {
        return Err(unreadable(
            "the written file holds other parts than planned",
        ));
    }
    let budgets = Budgets::production();
    let package = read_package(&mut archive, &meter, &budgets)?;
    if let Some(missing) = package
        .worksheets
        .iter()
        .find(|sheet| !names.contains(&sheet.path))
    {
        return Err(unreadable(format!("{} is missing", missing.path)));
    }
    let mut base = match base {
        Some(path) if parts.iter().any(|(_, kind)| *kind == Written::Copied) => {
            let mut archive = open(path, &CompressedMeter::new())?;
            let mut indexes = HashMap::new();
            for index in 0..archive.len() {
                let name = archive
                    .by_index_raw(index)
                    .map_err(unreadable)?
                    .name()
                    .to_string();
                indexes.insert(name, index);
            }
            Some((archive, indexes))
        }
        _ => None,
    };
    for (index, (name, kind)) in parts.iter().enumerate() {
        match kind {
            Written::Copied => {
                let (base, indexes) = base
                    .as_mut()
                    .ok_or_else(|| unreadable("a copied part has no base file"))?;
                let at = *indexes
                    .get(name)
                    .ok_or_else(|| unreadable(format!("{name} is not in the base file")))?;
                same_raw_entry(&mut archive, index, base, at)
                    .map_err(|e| unreadable(format!("{name}: {e}")))?;
            }
            Written::Generated => {
                let entry = archive.by_index(index).map_err(unreadable)?;
                let guard = if package.worksheets.iter().any(|sheet| &sheet.path == name) {
                    budgets.stream_worksheet(entry, &meter, name)
                } else {
                    budgets.stream(entry, &meter, name)
                };
                check_xml(BufReader::with_capacity(CHUNK_BYTES, GuardRead(guard)))
                    .map_err(|e| unreadable(format!("{name}: {e}")))?;
            }
        }
    }
    Ok(())
}

fn same_raw_entry<A: Read + Seek, B: Read + Seek>(
    written: &mut zip::ZipArchive<A>,
    index: usize,
    base: &mut zip::ZipArchive<B>,
    base_index: usize,
) -> Result<(), String> {
    let mut written = written.by_index_raw(index).map_err(|e| e.to_string())?;
    let mut base = base.by_index_raw(base_index).map_err(|e| e.to_string())?;
    let header = |entry: &zip::read::ZipFile<'_>| {
        (
            entry.crc32(),
            entry.compressed_size(),
            entry.size(),
            entry.compression(),
        )
    };
    if header(&written) != header(&base) {
        return Err("differs from the base file".to_string());
    }
    let mut left = vec![0u8; CHUNK_BYTES];
    let mut right = vec![0u8; CHUNK_BYTES];
    loop {
        let read = written.read(&mut left).map_err(|e| e.to_string())?;
        if read == 0 {
            return Ok(());
        }
        base.read_exact(&mut right[..read])
            .map_err(|e| e.to_string())?;
        if left[..read] != right[..read] {
            return Err("differs from the base file".to_string());
        }
    }
}

struct GuardRead<'b, R>(Guard<'b, R>);

impl<R: Read> Read for GuardRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_chunk(buf).map_err(std::io::Error::other)
    }
}

pub fn check_xml<R: BufRead>(source: R) -> Result<(), String> {
    let mut reader = Reader::from_reader(source);
    reader.config_mut().check_end_names = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| e.to_string())?
        {
            Event::Start(_) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Empty(_) if depth == 0 => roots += 1,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    match (depth, roots) {
        (0, 1) => Ok(()),
        _ => Err("the XML is not one closed element".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    const WORKBOOK: &[u8] = b"<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"S\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";
    const RELS: &[u8] = b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>";
    const SHEET: &[u8] = b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>";

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            writer
                .start_file(
                    *name,
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(body).unwrap();
        }
        std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
    }

    fn plan(kinds: &[(&str, Written)]) -> Vec<(String, Written)> {
        kinds.iter().map(|(n, k)| (n.to_string(), *k)).collect()
    }

    fn generated() -> Vec<(String, Written)> {
        plan(&[
            ("xl/workbook.xml", Written::Generated),
            ("xl/_rels/workbook.xml.rels", Written::Generated),
            ("xl/worksheets/sheet1.xml", Written::Generated),
        ])
    }

    #[test]
    fn a_sound_file_passes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.xlsx");
        write_zip(
            &path,
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("xl/worksheets/sheet1.xml", SHEET),
            ],
        );
        check_written(&path, None, &generated()).unwrap();
    }

    #[test]
    fn an_unclosed_sheet_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.xlsx");
        let cut = &SHEET[..SHEET.len() - 20];
        write_zip(
            &path,
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("xl/worksheets/sheet1.xml", cut),
            ],
        );
        assert!(check_written(&path, None, &generated()).is_err());
    }

    #[test]
    fn a_truncated_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.xlsx");
        write_zip(
            &path,
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("xl/worksheets/sheet1.xml", SHEET),
            ],
        );
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 30]).unwrap();
        assert!(check_written(&path, None, &generated()).is_err());
    }

    #[test]
    fn a_flipped_byte_fails_the_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flip.xlsx");
        write_zip(
            &path,
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("xl/worksheets/sheet1.xml", SHEET),
            ],
        );
        let mut bytes = std::fs::read(&path).unwrap();
        let at = bytes.windows(4).position(|w| w == b"<v>1").unwrap();
        bytes[at + 3] = b'2';
        std::fs::write(&path, &bytes).unwrap();
        assert!(check_written(&path, None, &generated()).is_err());
    }

    #[test]
    fn a_part_written_twice_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("twice.xlsx");
        write_zip(
            &path,
            &[
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", RELS),
                ("xl/worksheets/sheet1.xml", SHEET),
                ("xl/worksheets/sheet1.xml", SHEET),
            ],
        );
        let mut kinds = generated();
        kinds.push(("xl/worksheets/sheet1.xml".to_string(), Written::Generated));
        let error = check_written(&path, None, &kinds).unwrap_err();
        assert!(error.to_string().contains("twice"), "{error}");
    }

    #[test]
    fn copied_parts_must_match_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.xlsx");
        let entries: [(&str, &[u8]); 4] = [
            ("xl/workbook.xml", WORKBOOK),
            ("xl/_rels/workbook.xml.rels", RELS),
            ("xl/worksheets/sheet1.xml", SHEET),
            ("xl/media/a.bin", b"original"),
        ];
        write_zip(&base, &entries);
        let mut kinds = generated();
        kinds.push(("xl/media/a.bin".to_string(), Written::Copied));
        let same = dir.path().join("same.xlsx");
        write_zip(&same, &entries);
        check_written(&same, Some(&base), &kinds).unwrap();
        let other = dir.path().join("other.xlsx");
        let mut changed = entries;
        changed[3] = ("xl/media/a.bin", b"replaced");
        write_zip(&other, &changed);
        assert!(check_written(&other, Some(&base), &kinds).is_err());
        assert!(check_written(&same, Some(&base), &generated()).is_err());
    }
}
