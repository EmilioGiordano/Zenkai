use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};
use std::num::NonZeroU64;

use ironcalc::base::expressions::parser::DefinedNameS;

use crate::ReadError;
use crate::limits::describe_bytes;
use crate::skipped_part;
use crate::stream::{Budgets, CompressedMeter, read_part, read_part_to_peek};

// Excel writes about 150 bytes per sheet in workbook.xml and 200 in its rels, so the
// 20,000 entries the archive allows fit in 4 MB; 8 MB leaves room for defined names and
// keeps each DOM near 128 MB at the importer's sixteen times.
pub(crate) const PEEK_CAP_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) struct WorksheetPart {
    pub(crate) name: String,
    pub(crate) path: String,
}

// What sheet parsing needs from workbook.xml, built the way IronCalc's importer builds it:
// every <sheet> (chart sheets included) names a formula target, and defined names carry
// their raw formulas, scoped by sheet position.
pub(crate) struct Package {
    pub(crate) sheet_names: Vec<String>,
    pub(crate) hidden: Vec<bool>,
    pub(crate) defined_names: Vec<DefinedNameS>,
    pub(crate) worksheets: Vec<WorksheetPart>,
    pub(crate) shared_strings: Option<String>,
}

pub(crate) fn part_path(target: &str) -> String {
    match target.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None => format!("xl/{target}"),
    }
}

struct SheetEntry {
    name: String,
    sheet_id: u32,
    rel_id: String,
    hidden: bool,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

pub(crate) fn read_entry(
    entry: zip::read::ZipFile<'_>,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<Vec<u8>, ReadError> {
    let name = entry.name().to_string();
    read_part(entry, meter, &name, budgets)
}

fn read_text<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<String, ReadError> {
    let entry = archive.by_name(name).map_err(unreadable)?;
    String::from_utf8(read_entry(entry, meter, budgets)?).map_err(unreadable)
}

pub(crate) fn read_package<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<Package, ReadError> {
    let workbook = read_text(archive, "xl/workbook.xml", meter, budgets)?;
    let workbook = roxmltree::Document::parse(&workbook).map_err(unreadable)?;
    let mut sheets = Vec::new();
    for node in workbook.descendants().filter(|n| n.has_tag_name("sheet")) {
        let attribute = |name| {
            node.attribute(name)
                .ok_or_else(|| unreadable("a sheet misses an attribute"))
        };
        sheets.push(SheetEntry {
            name: attribute("name")?.to_string(),
            hidden: matches!(node.attribute("state"), Some("hidden" | "veryHidden")),
            sheet_id: attribute("sheetId")?.parse::<u32>().map_err(unreadable)?,
            rel_id: node
                .attribute((
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                    "id",
                ))
                .ok_or_else(|| unreadable("a sheet misses its relationship"))?
                .to_string(),
        });
    }
    let mut first_position: HashMap<u32, usize> = HashMap::new();
    for (position, sheet) in sheets.iter().enumerate() {
        first_position.entry(sheet.sheet_id).or_insert(position);
    }
    let mut defined_names = Vec::new();
    for node in workbook
        .descendants()
        .filter(|n| n.has_tag_name("definedName"))
    {
        let name = node
            .attribute("name")
            .ok_or_else(|| unreadable("a defined name has no name"))?;
        let scope = match node.attribute("localSheetId") {
            Some(local) => {
                let index = local.parse::<usize>().map_err(unreadable)?;
                let sheet_id = sheets
                    .get(index)
                    .ok_or_else(|| unreadable("a defined name points past the last sheet"))?
                    .sheet_id;
                first_position
                    .get(&sheet_id)
                    .and_then(|position| u32::try_from(*position).ok())
            }
            None => None,
        };
        defined_names.push((
            name.to_string(),
            scope,
            node.text().unwrap_or("").to_string(),
        ));
    }

    let rels = read_text(archive, "xl/_rels/workbook.xml.rels", meter, budgets)?;
    let rels = roxmltree::Document::parse(&rels).map_err(unreadable)?;
    let mut targets = HashMap::new();
    for node in rels
        .descendants()
        .filter(|n| n.has_tag_name("Relationship"))
    {
        let attribute = |name| {
            node.attribute(name)
                .ok_or_else(|| unreadable("a relationship misses an attribute"))
        };
        targets.insert(attribute("Id")?, (attribute("Type")?, attribute("Target")?));
    }
    let mut worksheets: Vec<WorksheetPart> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();
    for sheet in &sheets {
        let (kind, target) = targets
            .get(sheet.rel_id.as_str())
            .ok_or_else(|| unreadable("a sheet has no relationship"))?;
        if !kind.ends_with("worksheet") {
            continue;
        }
        let path = part_path(target);
        if !seen_paths.insert(path.clone()) {
            return Err(ReadError::Unsupported("two sheets share one part".into()));
        }
        worksheets.push(WorksheetPart {
            name: sheet.name.clone(),
            path,
        });
    }
    // IronCalc reads a formula's own cell as "<sheet>!<cell>" and splits at the first '!'.
    if sheets.iter().any(|s| s.name.contains('!')) {
        return Err(ReadError::Unsupported("a sheet name contains '!'".into()));
    }
    let shared_strings = targets
        .values()
        .find(|(kind, _)| kind.ends_with("/sharedStrings"))
        .map(|(_, target)| part_path(target));
    Ok(Package {
        hidden: sheets.iter().map(|s| s.hidden).collect(),
        sheet_names: sheets.into_iter().map(|s| s.name).collect(),
        defined_names,
        worksheets,
        shared_strings,
    })
}

// The parts IronCalc's importer loads as worksheets, with how many sheets load each,
// found the way it finds them but tolerating what `read_package` refuses (two sheets on
// one part, odd names, a missing attribute on a sheet the importer would reject anyway).
// Counting a part too many is safe; missing one lets its cells in uncharged. The two
// files are only peeked at: the walk reads and charges them afterwards. A file past the
// cap is refused, since the sheets it names, and how often, could not be known; a file
// that is missing or does not parse gives None, as the importer fails on it too.
pub(crate) fn importer_worksheet_paths<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<Option<HashMap<String, NonZeroU64>>, ReadError> {
    // The workbook's DOM is gone before the rels' is built, so the two never coexist.
    let sheet_ids: Vec<String> = {
        let Some(workbook) = peek_part(archive, "xl/workbook.xml", meter, budgets)? else {
            return Ok(None);
        };
        let Ok(workbook) = roxmltree::Document::parse(&workbook) else {
            return Ok(None);
        };
        workbook
            .descendants()
            .filter(|n| n.has_tag_name("sheet"))
            .filter_map(|node| {
                node.attribute((
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                    "id",
                ))
                .map(str::to_string)
            })
            .collect()
    };
    let Some(rels) = peek_part(archive, "xl/_rels/workbook.xml.rels", meter, budgets)? else {
        return Ok(None);
    };
    let Ok(rels) = roxmltree::Document::parse(&rels) else {
        return Ok(None);
    };
    let mut targets = HashMap::new();
    for node in rels
        .descendants()
        .filter(|n| n.has_tag_name("Relationship"))
    {
        if let (Some(id), Some(kind), Some(target)) = (
            node.attribute("Id"),
            node.attribute("Type"),
            node.attribute("Target"),
        ) {
            // The importer keeps the last relationship of a repeated Id.
            targets.insert(id, (kind, target));
        }
    }
    let mut paths: HashMap<String, NonZeroU64> = HashMap::new();
    for id in &sheet_ids {
        if let Some((kind, target)) = targets.get(id.as_str())
            && kind.ends_with("worksheet")
        {
            paths
                .entry(part_path(target))
                .and_modify(|loads| *loads = loads.saturating_add(1))
                .or_insert(NonZeroU64::MIN);
        }
    }
    Ok(Some(paths))
}

fn peek_part<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<Option<String>, ReadError> {
    let Ok(entry) = archive.by_name(name) else {
        return Ok(None);
    };
    let bytes = match read_part_to_peek(entry.take(PEEK_CAP_BYTES + 1), meter, name, budgets) {
        Ok(bytes) => bytes,
        Err(ReadError::Unsafe(reason)) => return Err(ReadError::Unsafe(reason)),
        Err(_) => return Ok(None),
    };
    if bytes.len() as u64 > PEEK_CAP_BYTES {
        return Err(ReadError::Unsafe(format!(
            "{name} is larger than the {} a workbook index may take",
            describe_bytes(PEEK_CAP_BYTES)
        )));
    }
    Ok(String::from_utf8(bytes).ok())
}

// The comments parts the importer loads, with how many times: once for every comments
// relationship of every load of a sheet, each load keeping its own copy. Targets are
// resolved the way the importer does, its first two characters replaced by the sheet's
// base folder. A relationships part that is missing or does not parse is skipped, as the
// importer fails on it too.
pub(crate) fn importer_comment_loads<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
    sheets: &HashMap<String, NonZeroU64>,
) -> Result<HashMap<String, NonZeroU64>, ReadError> {
    let mut loads: HashMap<String, u64> = HashMap::new();
    for (sheet, sheet_loads) in sheets {
        let Some((base, file)) = sheet.split_once("/worksheets/") else {
            continue;
        };
        let rels_name = format!("{base}/worksheets/_rels/{file}.rels");
        let Some(rels) = peek_part(archive, &rels_name, meter, budgets)? else {
            continue;
        };
        let Ok(rels) = roxmltree::Document::parse(&rels) else {
            continue;
        };
        for node in rels
            .descendants()
            .filter(|n| n.has_tag_name("Relationship"))
        {
            let (Some(kind), Some(target)) = (node.attribute("Type"), node.attribute("Target"))
            else {
                continue;
            };
            if !kind.ends_with("comments") || !target.is_char_boundary(2) {
                continue;
            }
            let mut target = target.to_string();
            target.replace_range(..2, base);
            let count = loads.entry(target).or_default();
            *count = count.saturating_add(sheet_loads.get());
        }
    }
    Ok(loads
        .into_iter()
        .filter_map(|(path, count)| NonZeroU64::new(count).map(|count| (path, count)))
        .collect())
}

// The archive IronCalc's own importer reads for everything but the cells: every entry is
// copied untouched except the worksheets, whose <sheetData> is emptied, and the parts
// the model never uses, which stay out.
pub(crate) fn stub_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    replaced: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, ReadError> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut names = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(unreadable)?;
        let name = entry.name().to_string();
        if !names.insert(name.clone()) {
            return Err(ReadError::Unsupported(format!("{name} appears twice")));
        }
        match replaced.get(&name) {
            Some(bytes) => {
                drop(entry);
                writer.start_file(name, stored).map_err(unreadable)?;
                writer.write_all(bytes).map_err(unreadable)?;
            }
            // Parts the model never uses stay out of the stub: IronCalc's import
            // never reads them, so copying them only costs memory.
            None if skipped_part(&name) => {}
            None => writer.raw_copy_file(entry).map_err(unreadable)?,
        }
    }
    Ok(writer.finish().map_err(unreadable)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zipped(names: &[&str]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::FileOptions::default();
        for name in names {
            writer.start_file(*name, options).unwrap();
            writer.write_all(b"placeholder").unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn listed(bytes: &[u8]) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect()
    }

    fn package_archive(sheets: usize, shared_part: bool) -> Vec<u8> {
        let mut workbook = String::from(
            "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
        );
        let mut rels = String::from(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for id in 1..=sheets {
            workbook.push_str(&format!(
                "<sheet name=\"S{id}\" sheetId=\"{id}\" r:id=\"rId{id}\"/>"
            ));
            let target = if shared_part { 1 } else { id };
            rels.push_str(&format!(
                "<Relationship Id=\"rId{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{target}.xml\"/>"
            ));
        }
        workbook.push_str("</sheets><definedNames>");
        for id in 0..sheets {
            workbook.push_str(&format!(
                "<definedName name=\"N{id}\" localSheetId=\"{id}\">S1!$A$1</definedName>"
            ));
        }
        workbook.push_str("</definedNames></workbook>");
        rels.push_str("</Relationships>");
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::FileOptions::default();
        for (name, body) in [
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", rels),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn package_of(bytes: &[u8]) -> Result<Package, ReadError> {
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes))).unwrap();
        read_package(&mut archive, &meter, &Budgets::production())
    }

    #[test]
    fn a_workbook_with_many_sheets_is_read_in_linear_time() {
        let bytes = package_archive(20_000, false);
        let started = std::time::Instant::now();
        let package = package_of(&bytes).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(package.worksheets.len(), 20_000);
        assert_eq!(package.defined_names.len(), 20_000);
        assert_eq!(package.defined_names[19_999].1, Some(19_999));
    }

    #[test]
    fn two_sheets_sharing_one_part_are_still_refused() {
        let error = package_of(&package_archive(3, true)).err().unwrap();
        assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
    }

    #[test]
    fn skipped_parts_stay_out_of_the_stub() {
        let bytes = zipped(&[
            "xl/workbook.xml",
            "xl/drawings/drawing1.xml",
            "xl/worksheets/sheet1.xml",
        ]);
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        let mut replaced = HashMap::new();
        replaced.insert(
            "xl/worksheets/sheet1.xml".to_string(),
            b"<worksheet/>".to_vec(),
        );
        let stub = stub_archive(&mut archive, &replaced).unwrap();
        let names = listed(&stub);
        assert!(names.contains(&"xl/workbook.xml".to_string()));
        assert!(names.contains(&"xl/worksheets/sheet1.xml".to_string()));
        assert!(
            !names.iter().any(|name| name.contains("drawing")),
            "{names:?}"
        );
    }
}
