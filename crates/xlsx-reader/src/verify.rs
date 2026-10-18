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
use crate::stream::{Budgets, CHUNK_BYTES, CompressedMeter, Counted};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    // Copied from the base file without inflating it.
    Copied,
    Generated(PartDigest),
}

// What the writer saw of a generated part: its XML, which it checked as it went, and the
// compressed bytes it handed to the file, with where they start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartDigest {
    pub compression: zip::CompressionMethod,
    pub crc32: u32,
    pub size: u64,
    pub data_start: u64,
    pub compressed_size: u64,
    pub compressed_crc32: u32,
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
// file's, and every generated part's compressed bytes, checksum and sizes on disk are
// the ones its writer recorded and inflate to the XML it checked, within the reader's
// caps. The workbook and its relationships parse, so the sheets resolve.
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
            Written::Generated(digest) => {
                if package.worksheets.iter().any(|sheet| &sheet.path == name) {
                    budgets.admit_written_worksheet(digest.size)?;
                } else {
                    budgets.admit_written(name, digest.size)?;
                }
                as_recorded(&mut archive, index, digest)
                    .map_err(|e| unreadable(format!("{name}: {e}")))?;
            }
        }
    }
    Ok(())
}

fn as_recorded<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    digest: &PartDigest,
) -> Result<(), String> {
    let mut entry = archive.by_index_raw(index).map_err(|e| e.to_string())?;
    let on_disk = (
        entry.compression(),
        entry.crc32(),
        entry.size(),
        entry.data_start(),
        entry.compressed_size(),
    );
    let recorded = (
        digest.compression,
        digest.crc32,
        digest.size,
        digest.data_start,
        digest.compressed_size,
    );
    if on_disk != recorded {
        return Err("its zip entry is not the one written".to_string());
    }
    let mut hasher = crc32fast::Hasher::new();
    let mut read = 0u64;
    let mut chunk = vec![0u8; CHUNK_BYTES];
    loop {
        let bytes = entry.read(&mut chunk).map_err(|e| e.to_string())?;
        if bytes == 0 {
            break;
        }
        hasher.update(&chunk[..bytes]);
        read += bytes as u64;
    }
    if read != digest.compressed_size || hasher.finalize() != digest.compressed_crc32 {
        return Err("its bytes on disk are not the ones written".to_string());
    }
    drop(entry);
    let entry = archive.by_index(index).map_err(|e| e.to_string())?;
    inflates_to(entry, digest).map_err(|e| format!("it does not inflate to the XML written: {e}"))
}

// The encoder is not trusted: the compressed bytes must inflate to exactly the XML the
// writer was fed and checked.
fn inflates_to(mut entry: impl Read, digest: &PartDigest) -> Result<(), String> {
    let mut hasher = crc32fast::Hasher::new();
    let mut inflated = 0u64;
    let mut chunk = vec![0u8; CHUNK_BYTES];
    loop {
        let bytes = entry.read(&mut chunk).map_err(|e| e.to_string())?;
        if bytes == 0 {
            break;
        }
        hasher.update(&chunk[..bytes]);
        inflated += bytes as u64;
        if inflated > digest.size {
            return Err("it is longer".to_string());
        }
    }
    if inflated != digest.size || hasher.finalize() != digest.crc32 {
        return Err("its checksum or length differs".to_string());
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

pub fn check_xml<R: BufRead>(source: R) -> Result<(), ReadError> {
    let mut reader = Reader::from_reader(source);
    reader.config_mut().check_end_names = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        match reader.read_event_into(&mut buf).map_err(unreadable)? {
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
        _ => Err(unreadable("the XML is not one closed element")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    const WORKBOOK: &[u8] = b"<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"S\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";
    const RELS: &[u8] = b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>";
    const SHEET: &[u8] = b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>";

    const SOUND: [(&str, &[u8]); 3] = [
        ("xl/workbook.xml", WORKBOOK),
        ("xl/_rels/workbook.xml.rels", RELS),
        ("xl/worksheets/sheet1.xml", SHEET),
    ];

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            writer
                .start_file(*name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(body).unwrap();
        }
        std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
    }

    // What a writer would have recorded for each part of a file still as it wrote it.
    fn recorded(path: &Path, copied: &[&str]) -> Vec<(String, Written)> {
        let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index_raw(index).unwrap();
                let name = entry.name().to_string();
                if copied.contains(&name.as_str()) {
                    return (name, Written::Copied);
                }
                let mut raw = Vec::new();
                entry.read_to_end(&mut raw).unwrap();
                let digest = PartDigest {
                    compression: entry.compression(),
                    crc32: entry.crc32(),
                    size: entry.size(),
                    data_start: entry.data_start(),
                    compressed_size: entry.compressed_size(),
                    compressed_crc32: crc32fast::hash(&raw),
                };
                (name, Written::Generated(digest))
            })
            .collect()
    }

    fn sound(dir: &Path) -> (std::path::PathBuf, Vec<(String, Written)>) {
        let path = dir.join("ok.xlsx");
        write_zip(&path, &SOUND);
        let parts = recorded(&path, &[]);
        (path, parts)
    }

    #[test]
    fn a_sound_file_passes() {
        let dir = tempfile::tempdir().unwrap();
        let (path, parts) = sound(dir.path());
        check_written(&path, None, &parts).unwrap();
    }

    #[test]
    fn malformed_xml_fails() {
        check_xml(SHEET).unwrap();
        let cut = &SHEET[..SHEET.len() - 20];
        for malformed in [cut, b"<a><b></a>", b"<a/><b/>", b"text", b""] {
            assert!(check_xml(malformed).is_err());
        }
    }

    #[test]
    fn a_truncated_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (path, parts) = sound(dir.path());
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 30]).unwrap();
        assert!(check_written(&path, None, &parts).is_err());
    }

    #[test]
    fn a_flipped_byte_fails_the_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let (path, parts) = sound(dir.path());
        let Written::Generated(sheet) = parts[2].1 else {
            panic!("the sheet was generated");
        };
        let mut bytes = std::fs::read(&path).unwrap();
        let last = usize::try_from(sheet.data_start + sheet.compressed_size - 1).unwrap();
        bytes[last] ^= 0x80;
        std::fs::write(&path, &bytes).unwrap();
        let error = check_written(&path, None, &parts).unwrap_err();
        assert!(
            error.to_string().contains("not the ones written"),
            "{error}"
        );
    }

    // Stands for an encoder bug: the compressed bytes on disk are the ones written and
    // inflate cleanly, to XML other than the one the writer was fed.
    #[test]
    fn compressed_bytes_of_other_xml_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.xlsx");
        let other = String::from_utf8(SHEET.to_vec())
            .unwrap()
            .replace("<v>1</v>", "<v>2</v>");
        write_zip(&path, &[SOUND[0], SOUND[1], (SOUND[2].0, other.as_bytes())]);
        let mut parts = recorded(&path, &[]);
        let fed = crc32fast::hash(SHEET);
        let Written::Generated(sheet) = &mut parts[2].1 else {
            panic!("the sheet was generated");
        };
        sheet.crc32 = fed;
        let mut bytes = std::fs::read(&path).unwrap();
        let (local, central) = {
            let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
            let entry = archive.by_name(SOUND[2].0).unwrap();
            (entry.header_start(), entry.central_header_start())
        };
        for at in [local + 14, central + 16] {
            let at = usize::try_from(at).unwrap();
            bytes[at..at + 4].copy_from_slice(&fed.to_le_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        let error = check_written(&path, None, &parts).unwrap_err();
        assert!(
            error.to_string().contains("inflate to the XML written"),
            "{error}"
        );
    }

    #[test]
    fn a_header_other_than_recorded_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut parts) = sound(dir.path());
        let Written::Generated(sheet) = &mut parts[2].1 else {
            panic!("the sheet was generated");
        };
        sheet.size += 1;
        let error = check_written(&path, None, &parts).unwrap_err();
        assert!(error.to_string().contains("not the one written"), "{error}");
    }

    #[test]
    fn a_part_past_the_per_part_cap_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut parts) = sound(dir.path());
        let Written::Generated(rels) = &mut parts[1].1 else {
            panic!("the relationships were generated");
        };
        rels.size = limits::MAX_ENTRY_BYTES + 1;
        let error = check_written(&path, None, &parts).unwrap_err();
        assert!(error.to_string().contains("per-part limit"), "{error}");
    }

    #[test]
    fn a_part_written_twice_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("twice.xlsx");
        let mut entries = SOUND.to_vec();
        entries.push(("xl/worksheets/sheet1.xml", SHEET));
        write_zip(&path, &entries);
        let parts = recorded(&path, &[]);
        let error = check_written(&path, None, &parts).unwrap_err();
        assert!(error.to_string().contains("twice"), "{error}");
    }

    #[test]
    fn copied_parts_must_match_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.xlsx");
        let mut entries = SOUND.to_vec();
        entries.push(("xl/media/a.bin", b"original"));
        write_zip(&base, &entries);
        let same = dir.path().join("same.xlsx");
        write_zip(&same, &entries);
        let parts = recorded(&same, &["xl/media/a.bin"]);
        check_written(&same, Some(&base), &parts).unwrap();
        let other = dir.path().join("other.xlsx");
        entries[3] = ("xl/media/a.bin", b"replaced");
        write_zip(&other, &entries);
        assert!(check_written(&other, Some(&base), &parts).is_err());
        assert!(check_written(&same, None, &parts).is_err());
    }
}
