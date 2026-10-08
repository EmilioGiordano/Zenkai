// IronCalc 0.8.3 writes a <row> only for rows that hold cells, so a row with a custom
// height, a row style or the hidden flag but no cells is lost on save. Excel files have
// such rows often (spacer rows, formatted blank rows). This puts them back in the exported
// sheets with the attributes IronCalc itself would write.

use std::io::{Cursor, Read, Write};

use ironcalc::base::Model;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::EngineError;

pub fn restore_empty_rows(model: &Model<'_>, xlsx: Vec<u8>) -> Result<Vec<u8>, EngineError> {
    let mut missing: Vec<(String, String)> = Vec::new();
    for (index, sheet) in model.workbook.worksheets.iter().enumerate() {
        let mut rows: Vec<_> = sheet
            .rows
            .iter()
            .filter(|row| !sheet.sheet_data.contains_key(&row.r))
            .collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|row| row.r);
        let mut xml = String::new();
        for row in rows {
            let hidden = if row.hidden { r#" hidden="1""# } else { "" };
            xml.push_str(&format!(
                r#"<row r="{}" s="{}" ht="{}" customHeight="{}" customFormat="{}"{hidden}/>"#,
                row.r,
                row.s,
                row.height,
                i32::from(row.custom_height),
                i32::from(row.custom_format),
            ));
        }
        missing.push((format!("xl/worksheets/sheet{}.xml", index + 1), xml));
    }
    if missing.is_empty() {
        return Ok(xlsx);
    }
    rewrite(xlsx, &missing).map_err(|e| EngineError::Rejected(format!("saving row sizes: {e}")))
}

fn rewrite(xlsx: Vec<u8>, missing: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut archive = ZipArchive::new(Cursor::new(xlsx)).map_err(|e| e.to_string())?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let rows = missing
            .iter()
            .find(|(name, _)| name == entry.name())
            .map(|(_, rows)| rows);
        let Some(rows) = rows else {
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
            continue;
        };
        let mut sheet = String::new();
        entry
            .read_to_string(&mut sheet)
            .map_err(|e| e.to_string())?;
        let merged = merge_rows(&sheet, rows)?;
        writer
            .start_file(entry.name(), options)
            .map_err(|e| e.to_string())?;
        writer
            .write_all(merged.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    writer
        .finish()
        .map(Cursor::into_inner)
        .map_err(|e| e.to_string())
}

// Rows must stay in ascending order inside <sheetData>, or Excel reports the file as
// damaged; each new row goes before the first written row with a larger index.
fn merge_rows(sheet: &str, rows: &str) -> Result<String, String> {
    let open = sheet
        .find("<sheetData>")
        .ok_or("the exported sheet has no <sheetData>")?
        + "<sheetData>".len();
    let close = sheet[open..]
        .find("</sheetData>")
        .ok_or("the exported sheet has no </sheetData>")?
        + open;
    let mut written: Vec<(i32, &str)> = Vec::new();
    let mut rest = &sheet[open..close];
    while let Some(start) = rest.find("<row r=\"") {
        let from = &rest[start..];
        let end = match from.find("</row>") {
            Some(end) => end + "</row>".len(),
            None => return Err("an exported row is not closed".to_string()),
        };
        written.push((row_number(from)?, &from[..end]));
        rest = &from[end..];
    }
    let mut added: Vec<(i32, &str)> = Vec::new();
    let mut rest = rows;
    while let Some(end) = rest.find("/>") {
        let row = &rest[..end + 2];
        added.push((row_number(row)?, row));
        rest = &rest[end + 2..];
    }
    let mut all: Vec<(i32, &str)> = written.into_iter().chain(added).collect();
    all.sort_by_key(|(r, _)| *r);
    let mut out = String::with_capacity(sheet.len() + rows.len());
    out.push_str(&sheet[..open]);
    for (_, row) in all {
        out.push_str(row);
    }
    out.push_str(&sheet[close..]);
    Ok(out)
}

fn row_number(row: &str) -> Result<i32, String> {
    row.strip_prefix("<row r=\"")
        .and_then(|rest| rest.split('"').next())
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("unexpected row element: {}", &row[..row.len().min(40)]))
}

#[cfg(test)]
mod tests {
    use super::merge_rows;

    #[test]
    fn inserts_rows_in_ascending_order() {
        let sheet =
            r#"<x><sheetData><row r="2"><c r="A2"/></row><row r="9"></row></sheetData></x>"#;
        let rows = r#"<row r="1" s="0"/><row r="5" s="0"/><row r="12" s="0"/>"#;
        let merged = merge_rows(sheet, rows).unwrap();
        assert_eq!(
            merged,
            r#"<x><sheetData><row r="1" s="0"/><row r="2"><c r="A2"/></row><row r="5" s="0"/><row r="9"></row><row r="12" s="0"/></sheetData></x>"#
        );
    }
}
