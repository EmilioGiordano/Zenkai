use std::io::Read;

use ironcalc_base::types::{DefinedName, SheetState};
use roxmltree::Node;

use crate::error::XlsxError;

use super::{
    util::get_attribute,
    worksheets::{Sheet, WorkbookXML},
};

pub(super) fn load_workbook<R: Read + std::io::Seek>(
    archive: &mut zip::read::ZipArchive<R>,
) -> Result<WorkbookXML, XlsxError> {
    let mut file = archive.by_name("xl/workbook.xml")?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let doc = roxmltree::Document::parse(&text)?;
    let mut defined_names = Vec::new();
    let mut sheets = Vec::new();
    // Get the sheets
    let sheet_nodes: Vec<Node> = doc
        .descendants()
        .filter(|n| n.has_tag_name("sheet"))
        .collect();
    for sheet in sheet_nodes {
        let name = get_attribute(&sheet, "name")?.to_string();
        let sheet_id = get_attribute(&sheet, "sheetId")?.to_string();
        let sheet_id = sheet_id.parse::<u32>()?;
        let id = get_attribute(
            &sheet,
            (
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                "id",
            ),
        )?
        .to_string();
        let state = match sheet.attribute("state") {
            Some("visible") | None => SheetState::Visible,
            Some("hidden") => SheetState::Hidden,
            Some("veryHidden") => SheetState::VeryHidden,
            Some(state) => return Err(XlsxError::Xml(format!("Unknown sheet state: {state}"))),
        };
        sheets.push(Sheet {
            name,
            sheet_id,
            id,
            state,
        });
    }
    // Get the defined names
    let name_nodes: Vec<Node> = doc
        .descendants()
        .filter(|n| n.has_tag_name("definedName"))
        .collect();
    for node in name_nodes {
        let name = get_attribute(&node, "name")?.to_string();
        let formula = node.text().unwrap_or("").to_string();
        // NOTE: In Excel the `localSheetId` is just the index of the worksheet and unrelated to the sheetId
        let sheet_id = match node.attribute("localSheetId") {
            Some(s) => {
                let index = s.parse::<usize>()?;
                let sheet = sheets.get(index).ok_or_else(|| {
                    XlsxError::Workbook(format!(
                        "The defined name {name} is scoped to sheet {index}, past the last sheet"
                    ))
                })?;
                Some(sheet.sheet_id)
            }
            None => None,
        };
        defined_names.push(DefinedName {
            name,
            formula,
            sheet_id,
        })
    }
    // read the relationships file
    Ok(WorkbookXML {
        worksheets: sheets,
        defined_names,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::load_workbook;

    fn archive_with_workbook(workbook: &str) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("xl/workbook.xml", zip::write::FileOptions::default())
            .unwrap();
        writer.write_all(workbook.as_bytes()).unwrap();
        zip::ZipArchive::new(writer.finish().unwrap()).unwrap()
    }

    #[test]
    fn a_defined_name_scoped_past_the_last_sheet_is_an_error_not_a_panic() {
        let workbook = "<workbook xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"A\" sheetId=\"1\" r:id=\"rId1\"/></sheets><definedNames><definedName name=\"N\" localSheetId=\"1\">A!$A$1</definedName></definedNames></workbook>";
        assert!(load_workbook(&mut archive_with_workbook(workbook)).is_err());
        let inside = workbook.replace("localSheetId=\"1\"", "localSheetId=\"0\"");
        assert!(load_workbook(&mut archive_with_workbook(&inside)).is_ok());
    }
}
