// IronCalc 0.8.3 writes a <row> only for rows that hold cells, so a row with a custom
// height, a row style or the hidden flag but no cells is lost on save. Excel files have
// such rows often (spacer rows, formatted blank rows). This puts them back in the exported
// sheets with the attributes IronCalc itself would write, and writes back the settings
// carried from the original file (`sheet_settings`). Each sheet part is rewritten once.

use std::io::{Cursor, Read, Write};

use ironcalc::base::Model;
use ironcalc::base::types::Worksheet;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::EngineError;
use crate::sheet_settings::{Carried, Outline, SheetSettings, attribute};

const MAX_ROW: i32 = 1_048_576;
// Excel's tallest row, in points.
const MAX_HEIGHT: f64 = 409.5;

struct Patch<'a> {
    part: String,
    rows: String,
    odd_rows: bool,
    settings: Option<&'a SheetSettings>,
}

pub fn patch_sheets(
    model: &Model<'_>,
    carried: &Carried,
    xlsx: Vec<u8>,
) -> Result<Vec<u8>, EngineError> {
    let mut patches = Vec::new();
    for (index, sheet) in model.workbook.worksheets.iter().enumerate() {
        let (rows, odd_rows) = missing_rows(sheet);
        let settings = carried
            .get(sheet.sheet_id)
            .filter(|settings| !settings.is_empty());
        if rows.is_empty() && !odd_rows && settings.is_none() {
            continue;
        }
        patches.push(Patch {
            part: format!("xl/worksheets/sheet{}.xml", index + 1),
            rows,
            odd_rows,
            settings,
        });
    }
    if patches.is_empty() {
        return Ok(xlsx);
    }
    rewrite(xlsx, &patches).map_err(|e| EngineError::Rejected(format!("saving sheet layout: {e}")))
}

fn missing_rows(sheet: &Worksheet) -> (String, bool) {
    // The file's values are not validated on import: rows outside Excel's grid, odd
    // heights or a repeated row would make the saved file "damaged" for Excel. The
    // last entry for a row wins, as in IronCalc's own export.
    let mut by_row = std::collections::BTreeMap::new();
    for row in &sheet.rows {
        let valid = (1..=MAX_ROW).contains(&row.r) && !sheet.sheet_data.contains_key(&row.r);
        if valid {
            by_row.insert(row.r, row);
        }
    }
    // IronCalc exports rows it imported even when they are outside the grid or too
    // tall; those sheets are rewritten too, and merge_rows drops or clamps them.
    let odd = sheet.sheet_data.keys().any(|r| !(1..=MAX_ROW).contains(r))
        || sheet.rows.iter().any(|row| {
            sheet.sheet_data.contains_key(&row.r) && !(0.0..=MAX_HEIGHT).contains(&row.height)
        });
    let mut xml = String::new();
    for row in by_row.into_values() {
        let hidden = if row.hidden { r#" hidden="1""# } else { "" };
        xml.push_str(&format!(
            r#"<row r="{}" s="{}" ht="{}" customHeight="{}" customFormat="{}"{hidden}/>"#,
            row.r,
            row.s,
            if row.height.is_finite() {
                row.height.clamp(0.0, MAX_HEIGHT)
            } else {
                15.0
            },
            i32::from(row.custom_height),
            i32::from(row.custom_format),
        ));
    }
    (xml, odd)
}

fn rewrite(xlsx: Vec<u8>, patches: &[Patch<'_>]) -> Result<Vec<u8>, String> {
    let mut archive = ZipArchive::new(Cursor::new(xlsx)).map_err(|e| e.to_string())?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(patch) = patches.iter().find(|patch| patch.part == entry.name()) else {
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
            continue;
        };
        let mut sheet = String::new();
        entry
            .read_to_string(&mut sheet)
            .map_err(|e| e.to_string())?;
        let patched = patch_sheet(sheet, patch)?;
        writer
            .start_file(entry.name(), options)
            .map_err(|e| e.to_string())?;
        writer
            .write_all(patched.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    writer
        .finish()
        .map(Cursor::into_inner)
        .map_err(|e| e.to_string())
}

fn patch_sheet(sheet: String, patch: &Patch<'_>) -> Result<String, String> {
    let no_outline = std::collections::BTreeMap::new();
    let row_outline = patch.settings.map_or(&no_outline, |s| &s.rows.outline);
    let sheet = if patch.rows.is_empty() && !patch.odd_rows && row_outline.is_empty() {
        sheet
    } else {
        merge_rows(&sheet, &patch.rows, row_outline)?
    };
    match patch.settings {
        Some(settings) => write_settings(sheet, settings),
        None => Ok(sheet),
    }
}

// The places below are where IronCalc's exporter writes these elements' neighbours;
// CT_Worksheet requires its children in a fixed order.
fn write_settings(sheet: String, settings: &SheetSettings) -> Result<String, String> {
    let mut sheet = sheet;
    let root = sheet
        .find("<worksheet")
        .ok_or("the exported sheet has no root")?;
    let root_end = sheet[root..]
        .find('>')
        .ok_or("the exported root is not closed")?
        + root
        + 1;
    sheet.insert_str(root_end, &settings.properties);
    let view = r#"<sheetView workbookViewId="0""#;
    let at = sheet.find(view).ok_or("the exported sheet has no view")? + view.len();
    sheet.insert_str(at, &settings.view);
    let views_end = "</sheetViews>";
    let at = sheet
        .find(views_end)
        .ok_or("the exported sheet has no views")?
        + views_end.len();
    sheet.insert_str(at, &settings.format_element());
    if !settings.columns.outline.is_empty() {
        sheet = outline_columns(&sheet, &settings.columns.outline)?;
    }
    let data_end = sheet
        .find("</sheetData>")
        .ok_or("the exported sheet has no </sheetData>")?;
    let at = sheet[data_end..]
        .find("<extLst")
        .map(|at| data_end + at)
        .or_else(|| sheet.rfind("</worksheet>"))
        .ok_or("the exported sheet is not closed")?;
    sheet.insert_str(at, &settings.trailing_elements());
    Ok(sheet)
}

// Each exported <col min max .../> is split where the outline changes inside its range.
fn outline_columns(
    sheet: &str,
    outline: &std::collections::BTreeMap<u32, Outline>,
) -> Result<String, String> {
    let (Some(open), Some(close)) = (sheet.find("<cols>"), sheet.find("</cols>")) else {
        return Ok(sheet.to_string());
    };
    let open = open + "<cols>".len();
    let mut columns = String::new();
    for col in sheet[open..close].split_inclusive("/>") {
        let bound = |name| {
            attribute(col, name)
                .and_then(|n| n.parse::<u32>().ok())
                .ok_or_else(|| format!("unexpected column element: {col}"))
        };
        let (min, max) = (bound("min")?, bound("max")?);
        let rest = col
            .split_once(&format!(r#" max="{max}""#))
            .map(|(_, rest)| rest.trim_end_matches("/>"))
            .ok_or_else(|| format!("unexpected column element: {col}"))?;
        // Imported ranges are not validated, so the walk visits outlined columns only.
        let mut starts = std::collections::BTreeSet::from([min]);
        for index in outline.range(min..=max).map(|(index, _)| *index) {
            starts.insert(index);
            starts.insert(index.saturating_add(1));
        }
        let starts: Vec<u32> = starts.into_iter().filter(|s| *s <= max).collect();
        let mut first = min;
        for (i, start) in starts.iter().enumerate() {
            let next = starts.get(i + 1).copied();
            let current = outline.get(start);
            if next.is_some_and(|next| outline.get(&next) == current) {
                continue;
            }
            let last = next.map_or(max, |next| next - 1);
            let extra = current.map(|o| o.attributes()).unwrap_or_default();
            columns.push_str(&format!(
                r#"<col min="{first}" max="{last}"{rest}{extra}/>"#
            ));
            first = last.saturating_add(1);
        }
    }
    Ok(format!("{}{columns}{}", &sheet[..open], &sheet[close..]))
}

// Rows must stay in ascending order inside <sheetData>, or Excel reports the file as
// damaged; each new row goes before the first written row with a larger index.
fn merge_rows(
    sheet: &str,
    rows: &str,
    outline: &std::collections::BTreeMap<u32, Outline>,
) -> Result<String, String> {
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
    let mut all: Vec<(i32, String)> = written
        .into_iter()
        .chain(added)
        .filter(|(r, _)| (1..=MAX_ROW).contains(r))
        .map(|(r, row)| (r, clamp_height(row)))
        .collect();
    for (row, element) in &mut all {
        if let Some(level) = u32::try_from(*row).ok().and_then(|r| outline.get(&r)) {
            *element = with_outline(element, *row, *level);
        }
    }
    let present: std::collections::BTreeSet<i32> = all.iter().map(|(r, _)| *r).collect();
    for (row, level) in outline {
        let Ok(r) = i32::try_from(*row) else {
            continue;
        };
        if !present.contains(&r) {
            all.push((r, format!(r#"<row r="{r}"{}/>"#, level.attributes())));
        }
    }
    all.sort_by_key(|(r, _)| *r);
    let mut out = String::with_capacity(sheet.len() + rows.len());
    out.push_str(&sheet[..open]);
    for (_, row) in &all {
        out.push_str(row);
    }
    out.push_str(&sheet[close..]);
    Ok(drop_odd_dimension(out))
}

fn with_outline(element: &str, row: i32, outline: Outline) -> String {
    let number = format!(r#"<row r="{row}""#);
    match element.strip_prefix(&number) {
        Some(rest) => format!("{number}{}{rest}", outline.attributes()),
        None => element.to_string(),
    }
}

// IronCalc derives <dimension> from every imported row, so rows outside the grid leave a
// reference like "A0:A2000000000". The element is optional and Excel recomputes it, so an
// out-of-grid one is removed.
fn drop_odd_dimension(sheet: String) -> String {
    let Some(start) = sheet.find("<dimension ") else {
        return sheet;
    };
    let Some(len) = sheet[start..].find("/>") else {
        return sheet;
    };
    let element = &sheet[start..start + len + 2];
    let rows_ok = element
        .split(|c: char| !c.is_ascii_digit())
        .filter(|digits| !digits.is_empty())
        .all(|digits| {
            digits
                .parse::<i64>()
                .is_ok_and(|r| (1..=i64::from(MAX_ROW)).contains(&r))
        });
    if rows_ok {
        return sheet;
    }
    format!("{}{}", &sheet[..start], &sheet[start + len + 2..])
}

// A row tag with ht outside Excel's 0..=409.5 points gets the nearest valid height.
fn clamp_height(row: &str) -> String {
    // Only the opening <row ...> tag is searched, never the cells inside it.
    let tag_end = row.find('>').unwrap_or(row.len());
    let Some(start) = row[..tag_end].find(" ht=\"").map(|at| at + " ht=\"".len()) else {
        return row.to_string();
    };
    let Some(len) = row[start..].find('"') else {
        return row.to_string();
    };
    let height = row[start..start + len].parse::<f64>().unwrap_or(f64::NAN);
    if (0.0..=MAX_HEIGHT).contains(&height) {
        return row.to_string();
    }
    let valid = if height.is_finite() {
        height.clamp(0.0, MAX_HEIGHT)
    } else {
        15.0
    };
    format!("{}{valid}{}", &row[..start], &row[start + len..])
}

fn row_number(row: &str) -> Result<i32, String> {
    row.strip_prefix("<row r=\"")
        .and_then(|rest| rest.split('"').next())
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("unexpected row element: {}", &row[..row.len().min(40)]))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Outline, clamp_height, merge_rows, outline_columns};

    #[test]
    fn drops_only_an_out_of_grid_dimension() {
        let odd = r#"<worksheet><dimension ref="A0:A2000000000"/><sheetData/></worksheet>"#;
        assert_eq!(
            super::drop_odd_dimension(odd.to_string()),
            "<worksheet><sheetData/></worksheet>"
        );
        let fine = r#"<worksheet><dimension ref="A1:C9"/><sheetData/></worksheet>"#;
        assert_eq!(super::drop_odd_dimension(fine.to_string()), fine);
    }

    #[test]
    fn clamps_only_the_row_height_attribute() {
        assert_eq!(
            clamp_height(r#"<row r="2" ht="5000"></row>"#),
            r#"<row r="2" ht="409.5"></row>"#
        );
        let cells = r#"<row r="2"><c r="A2" t="str"><v> ht="9999"</v></c></row>"#;
        assert_eq!(clamp_height(cells), cells);
        assert_eq!(
            clamp_height(r#"<row r="2" ht="15"/>"#),
            r#"<row r="2" ht="15"/>"#
        );
    }

    #[test]
    fn inserts_rows_in_ascending_order() {
        let sheet =
            r#"<x><sheetData><row r="2"><c r="A2"/></row><row r="9"></row></sheetData></x>"#;
        let rows = r#"<row r="1" s="0"/><row r="5" s="0"/><row r="12" s="0"/>"#;
        let merged = merge_rows(sheet, rows, &BTreeMap::new()).unwrap();
        assert_eq!(
            merged,
            r#"<x><sheetData><row r="1" s="0"/><row r="2"><c r="A2"/></row><row r="5" s="0"/><row r="9"></row><row r="12" s="0"/></sheetData></x>"#
        );
    }

    #[test]
    fn outline_goes_on_written_rows_and_adds_bare_ones() {
        let sheet = r#"<x><sheetData><row r="2" s="0"><c r="A2"/></row></sheetData></x>"#;
        let level = |level, collapsed| Outline { level, collapsed };
        let outline = BTreeMap::from([
            (2, level(1, false)),
            (3, level(1, false)),
            (4, level(0, true)),
        ]);
        let merged = merge_rows(sheet, r#"<row r="3" s="0"/>"#, &outline).unwrap();
        assert_eq!(
            merged,
            r#"<x><sheetData><row r="2" outlineLevel="1" s="0"><c r="A2"/></row><row r="3" outlineLevel="1" s="0"/><row r="4" collapsed="1"/></sheetData></x>"#
        );
    }

    #[test]
    fn column_ranges_split_where_the_outline_changes() {
        let sheet = r#"<a><cols><col min="1" max="4" width="9" customWidth="1"/></cols></a>"#;
        let outline = BTreeMap::from([
            (
                2,
                Outline {
                    level: 1,
                    collapsed: false,
                },
            ),
            (
                3,
                Outline {
                    level: 1,
                    collapsed: false,
                },
            ),
        ]);
        assert_eq!(
            outline_columns(sheet, &outline).unwrap(),
            r#"<a><cols><col min="1" max="1" width="9" customWidth="1"/><col min="2" max="3" width="9" customWidth="1" outlineLevel="1"/><col min="4" max="4" width="9" customWidth="1"/></cols></a>"#
        );
    }
}
