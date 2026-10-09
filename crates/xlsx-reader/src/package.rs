use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};

use ironcalc::base::expressions::parser::DefinedNameS;

use crate::ReadError;
use crate::limits::MAX_ENTRY_BYTES;

pub(crate) struct WorksheetPart {
    pub(crate) name: String,
    pub(crate) path: String,
}

// What sheet parsing needs from workbook.xml, built the way IronCalc's importer builds it:
// every <sheet> (chart sheets included) names a formula target, and defined names carry
// their raw formulas, scoped by sheet position.
pub(crate) struct Package {
    pub(crate) sheet_names: Vec<String>,
    pub(crate) defined_names: Vec<DefinedNameS>,
    pub(crate) worksheets: Vec<WorksheetPart>,
}

struct SheetEntry {
    name: String,
    sheet_id: u32,
    rel_id: String,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

pub(crate) fn read_entry(entry: zip::read::ZipFile<'_>) -> Result<Vec<u8>, ReadError> {
    let mut bytes =
        Vec::with_capacity(usize::try_from(entry.size().min(MAX_ENTRY_BYTES)).unwrap_or(0));
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(ReadError::Unsupported(
            "a part expands past the size its archive declares".to_string(),
        ));
    }
    Ok(bytes)
}

fn read_text<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String, ReadError> {
    let entry = archive.by_name(name).map_err(unreadable)?;
    String::from_utf8(read_entry(entry)?).map_err(unreadable)
}

pub(crate) fn read_package<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<Package, ReadError> {
    let workbook = read_text(archive, "xl/workbook.xml")?;
    let workbook = roxmltree::Document::parse(&workbook).map_err(unreadable)?;
    let mut sheets = Vec::new();
    for node in workbook.descendants().filter(|n| n.has_tag_name("sheet")) {
        let attribute = |name| {
            node.attribute(name)
                .ok_or_else(|| unreadable("a sheet misses an attribute"))
        };
        sheets.push(SheetEntry {
            name: attribute("name")?.to_string(),
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
    let sheet_ids: Vec<u32> = sheets.iter().map(|s| s.sheet_id).collect();
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
                sheet_ids
                    .iter()
                    .position(|id| *id == sheet_id)
                    .and_then(|position| u32::try_from(position).ok())
            }
            None => None,
        };
        defined_names.push((
            name.to_string(),
            scope,
            node.text().unwrap_or("").to_string(),
        ));
    }

    let rels = read_text(archive, "xl/_rels/workbook.xml.rels")?;
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
    for sheet in &sheets {
        let (kind, target) = targets
            .get(sheet.rel_id.as_str())
            .ok_or_else(|| unreadable("a sheet has no relationship"))?;
        if !kind.ends_with("worksheet") {
            continue;
        }
        let path = match target.strip_prefix('/') {
            Some(absolute) => absolute.to_string(),
            None => format!("xl/{target}"),
        };
        if worksheets.iter().any(|w| w.path == path) {
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
    Ok(Package {
        sheet_names: sheets.into_iter().map(|s| s.name).collect(),
        defined_names,
        worksheets,
    })
}

// The archive IronCalc's own importer reads for everything but the cells: every entry is
// copied untouched except the worksheets, whose <sheetData> is emptied.
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
            None => writer.raw_copy_file(entry).map_err(unreadable)?,
        }
    }
    Ok(writer.finish().map_err(unreadable)?.into_inner())
}
