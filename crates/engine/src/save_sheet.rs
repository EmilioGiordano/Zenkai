// IronCalc 0.8.3 writes a <row> only for rows that hold cells, so a row with a custom
// height, a row style or the hidden flag but no cells is lost on save. Excel files have
// such rows often (spacer rows, formatted blank rows). `prepare` puts them back in the
// sheet copy being saved, and `SheetWriter` writes back the settings carried from the
// original file (`sheet_settings`) as the sheet streams past, so no sheet is ever held
// whole as text.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use ironcalc::base::expressions::utils::number_to_column;
use ironcalc::base::types::Worksheet;

use crate::fingerprint::UsedBounds;
use crate::sheet_elements::Place;
use crate::sheet_settings::{Outline, SheetSettings, attribute};

const MAX_ROW: i32 = 1_048_576;
// Excel's tallest row, in points.
const MAX_HEIGHT: f64 = 409.5;
// Rows stream through in pieces about this large; a piece always ends at a tag end, so
// a row start tag is never split.
const PIECE_BYTES: usize = 64 * 1024;
const SHEET_DATA: &str = "<sheetData>";
const MAX_COLUMN: u32 = 16_384;
const SHEET_DATA_END: &str = "</sheetData>";

fn other(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

// The file's values are not validated on import: rows outside Excel's grid, odd heights
// or rows Excel needs but IronCalc does not write would make the saved file "damaged" or
// lose formatting. Returns the sheet's <dimension> reference.
pub(crate) fn prepare(sheet: &mut Worksheet, settings: Option<&SheetSettings>) -> String {
    let in_grid = |row: &i32| (1..=MAX_ROW).contains(row);
    let outside: Vec<i32> = sheet
        .sheet_data
        .rows()
        .filter(|row| !in_grid(row))
        .collect();
    for row in outside {
        sheet.sheet_data.remove_row(row);
    }
    sheet.rows.retain(|row| in_grid(&row.r));
    for row in &mut sheet.rows {
        row.height = written_height(row.height);
    }
    let reference = dimension_reference(sheet);
    let outlined = settings
        .map(|s| s.rows.outline.keys().filter_map(|r| i32::try_from(*r).ok()))
        .into_iter()
        .flatten();
    let bare: Vec<i32> = sheet.rows.iter().map(|row| row.r).chain(outlined).collect();
    for row in bare {
        if in_grid(&row) && !sheet.sheet_data.has_row(row) {
            sheet.sheet_data.insert_row(row, []);
        }
    }
    reference
}

pub(crate) fn written_height(height: f64) -> f64 {
    if height.is_finite() {
        height.clamp(0.0, MAX_HEIGHT)
    } else {
        15.0
    }
}

// The <dimension> reference of the sheet's cells, which must all lie in Excel's grid.
pub(crate) fn dimension_reference(sheet: &Worksheet) -> String {
    let dimension = sheet.dimension();
    bounds_reference(&UsedBounds {
        min_row: dimension.min_row,
        max_row: dimension.max_row,
        min_column: dimension.min_column,
        max_column: dimension.max_column,
    })
}

pub(crate) fn bounds_reference(bounds: &UsedBounds) -> String {
    match (
        number_to_column(bounds.min_column),
        number_to_column(bounds.max_column),
    ) {
        (Some(first), Some(last)) => {
            format!("{first}{}:{last}{}", bounds.min_row, bounds.max_row)
        }
        _ => "A1".to_string(),
    }
}

enum Stage {
    Head(Vec<u8>),
    Rows(Vec<u8>),
    Tail(Vec<u8>),
}

// Sits between IronCalc's worksheet writer and the zip entry. The part before
// <sheetData> and the part after it are small and edited whole; rows only gain their
// outline attributes on the way through.
pub(crate) struct SheetWriter<'a, W: Write> {
    inner: W,
    settings: Option<&'a SheetSettings>,
    stage: Stage,
}

impl<'a, W: Write> SheetWriter<'a, W> {
    pub(crate) fn new(inner: W, settings: Option<&'a SheetSettings>) -> Self {
        SheetWriter {
            inner,
            settings,
            stage: Stage::Head(Vec::new()),
        }
    }

    pub(crate) fn finish(mut self) -> io::Result<W> {
        let Stage::Tail(tail) = std::mem::replace(&mut self.stage, Stage::Tail(Vec::new())) else {
            return Err(other("the written sheet has no </sheetData>"));
        };
        let tail = String::from_utf8(tail).map_err(other_utf8)?;
        match self.settings {
            Some(settings) => write_tail(&mut self.inner, &tail, settings)?,
            None => self.inner.write_all(tail.as_bytes())?,
        }
        Ok(self.inner)
    }

    fn row_outline(&self) -> Option<&'a BTreeMap<u32, Outline>> {
        self.settings
            .map(|settings| &settings.rows.outline)
            .filter(|outline| !outline.is_empty())
    }

    fn write_rows(&mut self, rows: &[u8]) -> io::Result<()> {
        match self.row_outline() {
            Some(outline) => self.inner.write_all(&with_row_outline(rows, outline)?),
            None => self.inner.write_all(rows),
        }
    }

    fn advance(&mut self) -> io::Result<()> {
        loop {
            match &mut self.stage {
                Stage::Head(head) => {
                    let Some(at) = find(head, SHEET_DATA.as_bytes()) else {
                        return Ok(());
                    };
                    let rest = head.split_off(at + SHEET_DATA.len());
                    let head = String::from_utf8(std::mem::take(head)).map_err(other_utf8)?;
                    let head = match self.settings {
                        Some(settings) => with_head_settings(head, settings)?,
                        None => head,
                    };
                    self.inner.write_all(head.as_bytes())?;
                    self.stage = Stage::Rows(rest);
                }
                Stage::Rows(rows) => {
                    if let Some(at) = find(rows, SHEET_DATA_END.as_bytes()) {
                        let tail = rows.split_off(at);
                        let rows = std::mem::take(rows);
                        self.write_rows(&rows)?;
                        self.stage = Stage::Tail(tail);
                        continue;
                    }
                    if rows.len() < PIECE_BYTES {
                        return Ok(());
                    }
                    let Some(end) = rows.iter().rposition(|b| *b == b'>') else {
                        return Ok(());
                    };
                    let rest = rows.split_off(end + 1);
                    let piece = std::mem::replace(rows, rest);
                    self.write_rows(&piece)?;
                    return Ok(());
                }
                Stage::Tail(_) => return Ok(()),
            }
        }
    }
}

impl<W: Write> Write for SheetWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.stage {
            Stage::Head(bytes) | Stage::Rows(bytes) | Stage::Tail(bytes) => {
                bytes.extend_from_slice(buf)
            }
        }
        self.advance()?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn other_utf8(error: std::string::FromUtf8Error) -> io::Error {
    other(error.to_string())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// Text and formulas are escaped, so `<row r="` only ever starts a row tag.
pub(crate) fn with_row_outline(
    rows: &[u8],
    outline: &BTreeMap<u32, Outline>,
) -> io::Result<Vec<u8>> {
    const OPEN: &[u8] = b"<row r=\"";
    let mut out = Vec::with_capacity(rows.len() + 64);
    let mut rest = rows;
    while let Some(at) = find(rest, OPEN) {
        let number_start = at + OPEN.len();
        let Some(quote) = rest[number_start..].iter().position(|b| *b == b'"') else {
            return Err(other("a written row tag is not closed"));
        };
        let number_end = number_start + quote + 1;
        out.extend_from_slice(&rest[..number_end]);
        let row = std::str::from_utf8(&rest[number_start..number_end - 1])
            .ok()
            .and_then(|n| n.parse::<u32>().ok())
            .ok_or_else(|| other("a written row has no number"))?;
        if let Some(level) = outline.get(&row) {
            out.extend_from_slice(level.attributes().as_bytes());
        }
        rest = &rest[number_end..];
    }
    out.extend_from_slice(rest);
    Ok(out)
}

// The places below are where IronCalc's exporter writes these elements' neighbours;
// CT_Worksheet requires its children in a fixed order.
fn with_head_settings(head: String, settings: &SheetSettings) -> io::Result<String> {
    let mut head = head;
    let root = head
        .find("<worksheet")
        .ok_or_else(|| other("the written sheet has no root"))?;
    let root_end = head[root..]
        .find('>')
        .ok_or_else(|| other("the written root is not closed"))?
        + root
        + 1;
    head.insert_str(root_end, &settings.properties);
    let view = r#"<sheetView workbookViewId="0""#;
    let at = head
        .find(view)
        .ok_or_else(|| other("the written sheet has no view"))?
        + view.len();
    head.insert_str(at, &settings.view);
    let views_end = "</sheetViews>";
    let at = head
        .find(views_end)
        .ok_or_else(|| other("the written sheet has no views"))?
        + views_end.len();
    head.insert_str(at, &settings.format_element());
    if settings.columns.outline.is_empty() {
        return Ok(head);
    }
    outline_columns(
        &head,
        &settings.columns.outline,
        &settings.default_column_width(),
    )
    .map_err(other)
}

// `tail` starts with </sheetData>; IronCalc writes <mergeCells>, <conditionalFormatting>
// and its own <extLst> after it.
fn write_tail(out: &mut impl Write, tail: &str, settings: &SheetSettings) -> io::Result<()> {
    let rest = tail
        .strip_prefix(SHEET_DATA_END)
        .ok_or_else(|| other("the written sheet data is not closed"))?;
    let at = rest
        .find("<extLst")
        .or_else(|| rest.rfind("</worksheet>"))
        .ok_or_else(|| other("the written sheet is not closed"))?;
    let (ironcalc_elements, closing) = rest.split_at(at);
    out.write_all(SHEET_DATA_END.as_bytes())?;
    settings.elements.write_at(out, Place::AfterCellData)?;
    out.write_all(ironcalc_elements.as_bytes())?;
    settings.write_trailing_elements(out)?;
    out.write_all(closing.as_bytes())
}

// Each exported <col min max .../> is split where the outline changes inside its range.
// An outlined column the model writes no <col> for (IronCalc drops the entry of a default
// column) gets its own, so hiding and showing a column keeps its outline.
fn outline_columns(
    sheet: &str,
    outline: &BTreeMap<u32, Outline>,
    default_width: &str,
) -> Result<String, String> {
    let (open, close) = match (sheet.find("<cols>"), sheet.find("</cols>")) {
        (Some(open), Some(close)) => (open + "<cols>".len(), close),
        _ => {
            let at = sheet
                .find(SHEET_DATA)
                .ok_or_else(|| "the written sheet has no data".to_string())?;
            return outline_columns(
                &format!("{}<cols></cols>{}", &sheet[..at], &sheet[at..]),
                outline,
                default_width,
            );
        }
    };
    let mut columns: Vec<(u32, String)> = Vec::new();
    let mut written: BTreeMap<u32, u32> = BTreeMap::new();
    for col in sheet[open..close].split_inclusive("/>") {
        let bound = |name| {
            attribute(col, name)
                .and_then(|n| n.parse::<u32>().ok())
                .ok_or_else(|| format!("unexpected column element: {col}"))
        };
        let (min, max) = (bound("min")?, bound("max")?);
        written.insert(min, max);
        let rest = col
            .split_once(&format!(r#" max="{max}""#))
            .map(|(_, rest)| rest.trim_end_matches("/>"))
            .ok_or_else(|| format!("unexpected column element: {col}"))?;
        // The exporter writes 1 <= min <= max <= 16384, but a range can span the whole
        // sheet, so the walk visits outlined columns only.
        let mut starts = BTreeSet::from([min]);
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
            columns.push((
                first,
                format!(r#"<col min="{first}" max="{last}"{rest}{extra}/>"#),
            ));
            first = last.saturating_add(1);
        }
    }
    for (index, level) in outline {
        let covered = written
            .range(..=*index)
            .next_back()
            .is_some_and(|(_, max)| index <= max);
        let extra = level.attributes();
        if !covered && !extra.is_empty() && (1..=MAX_COLUMN).contains(index) {
            columns.push((
                *index,
                format!(r#"<col min="{index}" max="{index}" width="{default_width}"{extra}/>"#),
            ));
        }
    }
    columns.sort_by_key(|(first, _)| *first);
    let columns: String = columns.into_iter().map(|(_, col)| col).collect();
    Ok(format!("{}{columns}{}", &sheet[..open], &sheet[close..]))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Outline, SheetWriter, outline_columns, with_row_outline};
    use crate::sheet_settings::SheetSettings;

    fn level(level: u32, collapsed: bool) -> Outline {
        Outline { level, collapsed }
    }

    #[test]
    fn outline_goes_on_row_tags_only() {
        let outline = BTreeMap::from([(2, level(1, false)), (4, level(0, true))]);
        let rows = br#"<row r="2" s="0"><c r="A2" t="str"><v>&lt;row r="4"</v></c></row><row r="4"></row>"#;
        assert_eq!(
            String::from_utf8(with_row_outline(rows, &outline).unwrap()).unwrap(),
            r#"<row r="2" outlineLevel="1" s="0"><c r="A2" t="str"><v>&lt;row r="4"</v></c></row><row r="4" collapsed="1"></row>"#
        );
    }

    fn streamed(xml: &str, settings: Option<&SheetSettings>, piece: usize) -> String {
        let mut writer = SheetWriter::new(Vec::new(), settings);
        for chunk in xml.as_bytes().chunks(piece) {
            std::io::Write::write_all(&mut writer, chunk).unwrap();
        }
        String::from_utf8(writer.finish().unwrap()).unwrap()
    }

    #[test]
    fn settings_land_in_place_whatever_the_write_sizes() {
        let mut settings = SheetSettings {
            properties: "<sheetPr/>".to_string(),
            view: r#" zoomScale="90""#.to_string(),
            print: "<pageMargins/>".to_string(),
            ..SheetSettings::default()
        };
        settings.rows.outline.insert(3, level(2, false));
        let xml = r#"<worksheet a="1"><sheetViews><sheetView workbookViewId="0"></sheetView></sheetViews><sheetData><row r="3"><c r="A3"/></row></sheetData><mergeCells/></worksheet>"#;
        let expected = r#"<worksheet a="1"><sheetPr/><sheetViews><sheetView workbookViewId="0" zoomScale="90"></sheetView></sheetViews><sheetFormatPr defaultRowHeight="15" outlineLevelRow="2"/><sheetData><row r="3" outlineLevel="2"><c r="A3"/></row></sheetData><mergeCells/><pageMargins/></worksheet>"#;
        for piece in [1, 3, 7, 64, xml.len()] {
            assert_eq!(streamed(xml, Some(&settings), piece), expected, "{piece}");
        }
        assert_eq!(streamed(xml, None, 5), xml);
    }

    #[test]
    fn a_sheet_without_its_data_end_is_an_error() {
        let mut writer = SheetWriter::new(Vec::new(), None);
        std::io::Write::write_all(&mut writer, b"<worksheet><sheetData><row r=\"1\">").unwrap();
        assert!(writer.finish().is_err());
    }

    #[test]
    fn carried_elements_go_around_what_ironcalc_writes_in_schema_order() {
        let source = concat!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
            r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
            r#"<sheetData/><sheetProtection sheet="1"/><autoFilter ref="A1:B2"/>"#,
            r#"<hyperlinks><hyperlink ref="A1" r:id="rId2"/></hyperlinks><pageMargins left="0.7" "#,
            r#"right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><rowBreaks "#,
            r#"count="1" manualBreakCount="1"><brk id="5" max="16383" man="1"/></rowBreaks>"#,
            r#"<drawing r:id="rId1"/></worksheet>"#
        );
        let mut budget = crate::sheet_elements::ElementBudget::workbook();
        let settings = crate::sheet_settings::read(source, &mut budget);
        let xml = concat!(
            "<worksheet><sheetViews><sheetView workbookViewId=\"0\"></sheetView></sheetViews>",
            "<sheetData><row r=\"1\"/></sheetData><mergeCells/>",
            "<conditionalFormatting/><extLst/></worksheet>"
        );
        let order = [
            "</sheetData>",
            "<sheetProtection",
            "<autoFilter",
            "<mergeCells",
            "<conditionalFormatting",
            "<hyperlinks",
            "<pageMargins",
            "<rowBreaks",
            "<drawing",
            "<extLst",
            "</worksheet>",
        ];
        for piece in [1, 5, xml.len()] {
            let written = streamed(xml, Some(&settings), piece);
            let positions: Vec<Option<usize>> = order.iter().map(|tag| written.find(tag)).collect();
            assert!(positions.iter().all(Option::is_some), "{written}");
            assert!(positions.is_sorted(), "{written}");
        }
    }

    #[test]
    fn missing_rows_come_back_and_odd_rows_go() {
        use ironcalc::base::types::Row;
        let mut model = ironcalc::base::Model::new_empty("b", "en", "UTC", "en").unwrap();
        model.set_user_input(0, 2, 1, "1".to_string()).unwrap();
        let sheet = &mut model.workbook.worksheets[0];
        let row = |r, height| Row {
            r,
            height,
            custom_format: false,
            custom_height: true,
            s: 0,
            hidden: false,
        };
        sheet.rows = vec![row(5, 5000.0), row(0, 20.0), row(9, f64::NAN)];
        sheet.sheet_data.insert_row(2_000_000, []);
        let mut settings = SheetSettings::default();
        settings.rows.outline.insert(7, level(1, false));
        let dimension = super::prepare(sheet, Some(&settings));
        assert_eq!(dimension, "A2:A2");
        let mut rows: Vec<i32> = sheet.sheet_data.rows().collect();
        rows.sort();
        assert_eq!(rows, [2, 5, 7, 9]);
        let heights: Vec<(i32, f64)> = sheet.rows.iter().map(|r| (r.r, r.height)).collect();
        assert_eq!(heights, [(5, 409.5), (9, 15.0)]);
    }

    #[test]
    fn column_ranges_split_where_the_outline_changes() {
        let sheet = r#"<a><cols><col min="1" max="4" width="9" customWidth="1"/></cols></a>"#;
        let outline = BTreeMap::from([(2, level(1, false)), (3, level(1, false))]);
        assert_eq!(
            outline_columns(sheet, &outline, "8.43").unwrap(),
            r#"<a><cols><col min="1" max="1" width="9" customWidth="1"/><col min="2" max="3" width="9" customWidth="1" outlineLevel="1"/><col min="4" max="4" width="9" customWidth="1"/></cols></a>"#
        );
    }

    #[test]
    fn an_outlined_column_without_an_entry_gets_one_in_column_order() {
        let sheet = r#"<a><cols><col min="1" max="1" width="9" customWidth="1"/><col min="5" max="5" width="9" customWidth="1"/></cols><sheetData></sheetData></a>"#;
        let outline = BTreeMap::from([(3, level(1, false))]);
        assert_eq!(
            outline_columns(sheet, &outline, "8.43").unwrap(),
            r#"<a><cols><col min="1" max="1" width="9" customWidth="1"/><col min="3" max="3" width="8.43" outlineLevel="1"/><col min="5" max="5" width="9" customWidth="1"/></cols><sheetData></sheetData></a>"#
        );
    }

    #[test]
    fn a_sheet_without_columns_gets_them_for_its_outline() {
        let sheet = r#"<a><sheetData></sheetData></a>"#;
        let outline = BTreeMap::from([(3, level(1, false))]);
        assert_eq!(
            outline_columns(sheet, &outline, "8.43").unwrap(),
            r#"<a><cols><col min="3" max="3" width="8.43" outlineLevel="1"/></cols><sheetData></sheetData></a>"#
        );
    }
}
