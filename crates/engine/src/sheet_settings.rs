// IronCalc 0.8.3 reads past a sheet's tab colour, view zoom, default sizes, print
// settings and outline, and never writes them. They are read from the original sheet
// parts on open and written back by `sheet_patch` on save. Every element is rebuilt
// from validated values, never copied as text: the file is hostile input and IronCalc's
// output declares only the main and relationship namespaces.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use roxmltree::Node;

use crate::file::Unsupported;

const MAX_ROW: u32 = 1_048_576;
const MAX_COLUMN: u32 = 16_384;
const MAX_OUTLINE_LEVEL: u32 = 7;
// Excel allows 1026 manual page breaks in each direction.
const MAX_BREAKS: usize = 1_026;
// Excel caps each header and footer section at 255 characters.
const MAX_HEADER_CHARS: usize = 255;

#[derive(Clone, Copy)]
enum Value {
    Bool,
    Int(u32, u32),
    Real(f64, f64),
    Choice(&'static [&'static str]),
    Hex,
    Measure,
}

const TAB_COLOR: &[(&str, Value)] = &[
    ("auto", Value::Bool),
    ("indexed", Value::Int(0, 65)),
    ("rgb", Value::Hex),
    ("theme", Value::Int(0, 11)),
    ("tint", Value::Real(-1.0, 1.0)),
];
const OUTLINE_PROPERTIES: &[(&str, Value)] = &[
    ("applyStyles", Value::Bool),
    ("summaryBelow", Value::Bool),
    ("summaryRight", Value::Bool),
    ("showOutlineSymbols", Value::Bool),
];
const PAGE_SETUP_PROPERTIES: &[(&str, Value)] =
    &[("autoPageBreaks", Value::Bool), ("fitToPage", Value::Bool)];
// IronCalc writes workbookViewId, tabSelected and showGridLines itself; repeating one
// would make the element malformed.
const SHEET_VIEW: &[(&str, Value)] = &[
    ("showFormulas", Value::Bool),
    ("showRowColHeaders", Value::Bool),
    ("showZeros", Value::Bool),
    ("rightToLeft", Value::Bool),
    ("showRuler", Value::Bool),
    ("showOutlineSymbols", Value::Bool),
    ("showWhiteSpace", Value::Bool),
    (
        "view",
        Value::Choice(&["normal", "pageBreakPreview", "pageLayout"]),
    ),
    ("zoomScale", Value::Int(10, 400)),
    ("zoomScaleNormal", Value::Int(10, 400)),
    ("zoomScaleSheetLayoutView", Value::Int(10, 400)),
    ("zoomScalePageLayoutView", Value::Int(10, 400)),
];
const SHEET_FORMAT: &[(&str, Value)] = &[
    ("baseColWidth", Value::Int(0, 255)),
    ("defaultColWidth", Value::Real(0.0, 255.0)),
    ("defaultRowHeight", Value::Real(0.0, 409.5)),
    ("customHeight", Value::Bool),
    ("zeroHeight", Value::Bool),
    ("thickTop", Value::Bool),
    ("thickBottom", Value::Bool),
];
const PRINT_OPTIONS: &[(&str, Value)] = &[
    ("horizontalCentered", Value::Bool),
    ("verticalCentered", Value::Bool),
    ("headings", Value::Bool),
    ("gridLines", Value::Bool),
    ("gridLinesSet", Value::Bool),
];
const PAGE_MARGINS: &[(&str, Value)] = &[
    ("left", Value::Real(0.0, 100.0)),
    ("right", Value::Real(0.0, 100.0)),
    ("top", Value::Real(0.0, 100.0)),
    ("bottom", Value::Real(0.0, 100.0)),
    ("header", Value::Real(0.0, 100.0)),
    ("footer", Value::Real(0.0, 100.0)),
];
// r:id is left out on purpose: it points at a printer settings part IronCalc does not write.
const PAGE_SETUP: &[(&str, Value)] = &[
    ("paperSize", Value::Int(1, 118)),
    ("paperHeight", Value::Measure),
    ("paperWidth", Value::Measure),
    ("scale", Value::Int(10, 400)),
    ("firstPageNumber", Value::Int(0, 32_767)),
    ("fitToWidth", Value::Int(0, 32_767)),
    ("fitToHeight", Value::Int(0, 32_767)),
    (
        "pageOrder",
        Value::Choice(&["downThenOver", "overThenDown"]),
    ),
    (
        "orientation",
        Value::Choice(&["default", "portrait", "landscape"]),
    ),
    ("usePrinterDefaults", Value::Bool),
    ("blackAndWhite", Value::Bool),
    ("draft", Value::Bool),
    (
        "cellComments",
        Value::Choice(&["none", "asDisplayed", "atEnd"]),
    ),
    ("useFirstPageNumber", Value::Bool),
    (
        "errors",
        Value::Choice(&["displayed", "blank", "dash", "NA"]),
    ),
    ("horizontalDpi", Value::Int(1, 65_535)),
    ("verticalDpi", Value::Int(1, 65_535)),
    ("copies", Value::Int(1, 32_767)),
];
const HEADER_FOOTER: &[(&str, Value)] = &[
    ("differentOddEven", Value::Bool),
    ("differentFirst", Value::Bool),
    ("scaleWithDoc", Value::Bool),
    ("alignWithMargins", Value::Bool),
];
const HEADER_FOOTER_TEXTS: [&str; 6] = [
    "oddHeader",
    "oddFooter",
    "evenHeader",
    "evenFooter",
    "firstHeader",
    "firstFooter",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outline {
    pub level: u32,
    pub collapsed: bool,
}

impl Outline {
    pub fn attributes(self) -> String {
        let mut out = String::new();
        if self.level > 0 {
            out.push_str(&format!(" outlineLevel=\"{}\"", self.level));
        }
        if self.collapsed {
            out.push_str(" collapsed=\"1\"");
        }
        out
    }
}

// Settings that name rows or columns by index: they stay right only while no row or
// column is inserted or deleted before the last index they name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Indexed {
    pub outline: BTreeMap<u32, Outline>,
    pub breaks: String,
    break_count: usize,
    reach: u32,
}

impl Indexed {
    fn add_break(&mut self, element: String, index: u32) {
        self.breaks.push_str(&element);
        self.break_count += 1;
        self.reach = self.reach.max(index.saturating_add(1));
    }

    fn add_outline(&mut self, index: u32, outline: Outline) {
        if outline.level > 0 || outline.collapsed {
            self.outline.insert(index, outline);
            self.reach = self.reach.max(index);
        }
    }

    pub fn max_level(&self) -> u32 {
        self.outline.values().map(|o| o.level).max().unwrap_or(0)
    }

    pub fn breaks_element(&self, name: &str) -> String {
        if self.break_count == 0 {
            return String::new();
        }
        format!(
            "<{name} count=\"{0}\" manualBreakCount=\"{0}\">{1}</{name}>",
            self.break_count, self.breaks
        )
    }

    fn lost(&self) -> Vec<Unsupported> {
        let mut lost = Vec::new();
        if !self.outline.is_empty() {
            lost.push(Unsupported::Outline);
        }
        if self.break_count > 0 {
            lost.push(Unsupported::PageBreaks);
        }
        lost
    }

    // `from` is the 1-based first row or column that moved.
    fn moved_from(&mut self, from: u32) -> Vec<Unsupported> {
        if from > self.reach {
            return Vec::new();
        }
        let lost = self.lost();
        *self = Indexed::default();
        lost
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetSettings {
    pub properties: String,
    pub view: String,
    pub format: Option<String>,
    pub print: String,
    pub rows: Indexed,
    pub columns: Indexed,
}

impl SheetSettings {
    pub fn is_empty(&self) -> bool {
        *self == SheetSettings::default()
    }

    pub fn format_element(&self) -> String {
        let (rows, columns) = (self.rows.max_level(), self.columns.max_level());
        if self.format.is_none() && rows == 0 && columns == 0 {
            return String::new();
        }
        let mut attributes = self.format.clone().unwrap_or_default();
        if !attributes.contains(" defaultRowHeight=") {
            attributes.push_str(" defaultRowHeight=\"15\"");
        }
        if rows > 0 {
            attributes.push_str(&format!(" outlineLevelRow=\"{rows}\""));
        }
        if columns > 0 {
            attributes.push_str(&format!(" outlineLevelCol=\"{columns}\""));
        }
        format!("<sheetFormatPr{attributes}/>")
    }

    // Elements after the cell data, in the order CT_Worksheet requires.
    pub fn trailing_elements(&self) -> String {
        format!(
            "{}{}{}",
            self.print,
            self.rows.breaks_element("rowBreaks"),
            self.columns.breaks_element("colBreaks")
        )
    }
}

// The settings carried for every sheet, by IronCalc's sheet id so they follow a sheet
// that is renamed or moved.
#[derive(Debug, Default)]
pub struct Carried {
    sheets: HashMap<u32, SheetSettings>,
    dropped: BTreeSet<Unsupported>,
}

impl Carried {
    pub fn new(sheets: HashMap<u32, SheetSettings>) -> Carried {
        Carried {
            sheets,
            dropped: BTreeSet::new(),
        }
    }

    pub fn get(&self, sheet_id: u32) -> Option<&SheetSettings> {
        self.sheets.get(&sheet_id)
    }

    pub fn rows_moved(&mut self, sheet_id: u32, from: u32) {
        if let Some(settings) = self.sheets.get_mut(&sheet_id) {
            self.dropped.extend(settings.rows.moved_from(from));
        }
    }

    pub fn columns_moved(&mut self, sheet_id: u32, from: u32) {
        if let Some(settings) = self.sheets.get_mut(&sheet_id) {
            self.dropped.extend(settings.columns.moved_from(from));
        }
    }

    pub fn dropped(&self) -> Vec<Unsupported> {
        self.dropped.iter().copied().collect()
    }
}

fn valid(kind: Value, raw: &str) -> Option<String> {
    let raw = raw.trim();
    match kind {
        Value::Bool => match raw {
            "1" | "true" => Some("1".to_string()),
            "0" | "false" => Some("0".to_string()),
            _ => None,
        },
        Value::Int(min, max) => raw
            .parse::<u32>()
            .ok()
            .filter(|n| (min..=max).contains(n))
            .map(|n| n.to_string()),
        Value::Real(min, max) => raw
            .parse::<f64>()
            .ok()
            .filter(|n| (min..=max).contains(n))
            .map(|n| n.to_string()),
        Value::Choice(options) => options
            .iter()
            .find(|option| **option == raw)
            .map(ToString::to_string),
        Value::Hex => (matches!(raw.len(), 6 | 8) && raw.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| raw.to_string()),
        Value::Measure => is_measure(raw).then(|| raw.to_string()),
    }
}

// ST_PositiveUniversalMeasure, such as "297mm" or "8.5in".
fn is_measure(raw: &str) -> bool {
    let Some(number) = ["mm", "cm", "in", "pt", "pc", "pi"]
        .iter()
        .find_map(|unit| raw.strip_suffix(unit))
    else {
        return false;
    };
    let mut parts = number.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or("0");
    number.len() <= 16
        && !whole.is_empty()
        && !fraction.is_empty()
        && whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
}

fn attributes(node: Node, spec: &[(&str, Value)]) -> String {
    spec.iter()
        .filter_map(|(name, kind)| {
            let value = valid(*kind, node.attribute(*name)?)?;
            Some(format!(" {name}=\"{value}\""))
        })
        .collect()
}

fn element(node: Node, name: &str, spec: &[(&str, Value)]) -> String {
    format!("<{name}{}/>", attributes(node, spec))
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn read(part: &str) -> SheetSettings {
    let part = part.strip_prefix('\u{feff}').unwrap_or(part);
    let (head, data, tail) = split_at_cell_data(part);
    let outside = format!("{head}{tail}");
    let document = match roxmltree::Document::parse(&outside) {
        Ok(document) => document,
        Err(error) => {
            tracing::debug!(%error, "sheet settings not readable; the engine reads cells only");
            return SheetSettings::default();
        }
    };
    let sheet = document.root_element();
    if sheet.tag_name().name() != "worksheet" {
        return SheetSettings::default();
    }
    let mut settings = SheetSettings {
        properties: properties(sheet),
        view: child(sheet, "sheetViews")
            .and_then(|views| child(views, "sheetView"))
            .map(|view| attributes(view, SHEET_VIEW))
            .unwrap_or_default(),
        format: child(sheet, "sheetFormatPr").map(|format| attributes(format, SHEET_FORMAT)),
        print: print(sheet),
        ..SheetSettings::default()
    };
    read_column_outline(sheet, &mut settings.columns);
    read_breaks(sheet, "rowBreaks", MAX_ROW, &mut settings.rows);
    read_breaks(sheet, "colBreaks", MAX_COLUMN, &mut settings.columns);
    if data.contains("outlineLevel") || data.contains("collapsed") {
        read_row_outline(data, &mut settings.rows);
    }
    settings
}

fn properties(sheet: Node) -> String {
    let Some(properties) = child(sheet, "sheetPr") else {
        return String::new();
    };
    let children = [
        ("tabColor", TAB_COLOR),
        ("outlinePr", OUTLINE_PROPERTIES),
        ("pageSetUpPr", PAGE_SETUP_PROPERTIES),
    ];
    let inner: String = children
        .iter()
        .filter_map(|(name, spec)| Some(element(child(properties, name)?, name, spec)))
        .collect();
    if inner.is_empty() {
        return String::new();
    }
    format!("<sheetPr>{inner}</sheetPr>")
}

fn print(sheet: Node) -> String {
    let mut out = String::new();
    if let Some(options) = child(sheet, "printOptions") {
        out.push_str(&element(options, "printOptions", PRINT_OPTIONS));
    }
    if let Some(margins) = child(sheet, "pageMargins") {
        let attributes = attributes(margins, PAGE_MARGINS);
        // All six margins are required; a partial set would make the file invalid.
        if attributes.matches('=').count() == PAGE_MARGINS.len() {
            out.push_str(&format!("<pageMargins{attributes}/>"));
        } else {
            tracing::warn!("page margins with missing or invalid values are not kept");
        }
    }
    if let Some(setup) = child(sheet, "pageSetup") {
        out.push_str(&element(setup, "pageSetup", PAGE_SETUP));
    }
    if let Some(header_footer) = child(sheet, "headerFooter") {
        out.push_str(&header_footer_element(header_footer));
    }
    out
}

fn header_footer_element(node: Node) -> String {
    let mut out = format!("<headerFooter{}>", attributes(node, HEADER_FOOTER));
    for name in HEADER_FOOTER_TEXTS {
        let Some(text) = child(node, name).and_then(|n| n.text()) else {
            continue;
        };
        if text.chars().count() > MAX_HEADER_CHARS {
            tracing::warn!(
                section = name,
                "header or footer longer than Excel allows is not kept"
            );
            continue;
        }
        out.push_str(&format!("<{name}>{}</{name}>", escape_text(text)));
    }
    out.push_str("</headerFooter>");
    out
}

fn read_breaks(sheet: Node, name: &str, last: u32, indexed: &mut Indexed) {
    let Some(breaks) = child(sheet, name) else {
        return;
    };
    let spec = [
        ("id", Value::Int(0, last)),
        ("min", Value::Int(0, MAX_ROW)),
        ("max", Value::Int(0, MAX_ROW)),
        ("man", Value::Bool),
        ("pt", Value::Bool),
    ];
    for brk in breaks
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "brk")
        .take(MAX_BREAKS)
    {
        let id = brk
            .attribute("id")
            .and_then(|raw| raw.trim().parse::<u32>().ok())
            .filter(|id| *id <= last);
        if let Some(id) = id {
            indexed.add_break(element(brk, "brk", &spec), id);
        }
    }
}

fn outline_of(level: Option<&str>, collapsed: Option<&str>) -> Outline {
    Outline {
        level: level
            .and_then(|raw| valid(Value::Int(0, MAX_OUTLINE_LEVEL), raw))
            .and_then(|level| level.parse().ok())
            .unwrap_or(0),
        collapsed: collapsed.and_then(|raw| valid(Value::Bool, raw)).as_deref() == Some("1"),
    }
}

// IronCalc reads columns only when the sheet has exactly one <cols>; outline levels are
// kept on the same terms, so each one lands on a column IronCalc writes back.
fn read_column_outline(sheet: Node, columns: &mut Indexed) {
    let lists: Vec<Node> = sheet
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "cols")
        .collect();
    let [list] = lists.as_slice() else {
        return;
    };
    for col in list.children().filter(Node::is_element) {
        let bound = |name: &str| {
            col.attribute(name)
                .and_then(|raw| valid(Value::Int(1, MAX_COLUMN), raw))
                .and_then(|n| n.parse::<u32>().ok())
        };
        let (Some(min), Some(max)) = (bound("min"), bound("max")) else {
            continue;
        };
        let outline = outline_of(col.attribute("outlineLevel"), col.attribute("collapsed"));
        for index in min..=max {
            columns.add_outline(index, outline);
        }
    }
}

// The cell data can be most of a large file, so it is never parsed as a tree: only the
// row start tags are read, and only when the sheet mentions an outline at all.
fn read_row_outline(data: &str, rows: &mut Indexed) {
    let mut unnumbered: Option<Outline> = None;
    let mut rest = data;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let name_end = tag
            .find(|c: char| c.is_ascii_whitespace() || c == '/')
            .unwrap_or(tag.len());
        let name = tag[..name_end].rsplit(':').next().unwrap_or_default();
        let attributes = &tag[name_end..];
        match name {
            "row" => {
                let outline = outline_of(
                    attribute(attributes, "outlineLevel"),
                    attribute(attributes, "collapsed"),
                );
                // A row without r takes its number from its first cell, as in IronCalc.
                unnumbered = None;
                match attribute(attributes, "r").and_then(|r| r.parse::<u32>().ok()) {
                    Some(row) if (1..=MAX_ROW).contains(&row) => rows.add_outline(row, outline),
                    Some(_) => {}
                    None => unnumbered = Some(outline),
                }
            }
            "c" => {
                if let Some(outline) = unnumbered.take()
                    && let Some(row) = attribute(attributes, "r").and_then(cell_row)
                {
                    rows.add_outline(row, outline);
                }
            }
            _ => {}
        }
    }
}

fn cell_row(reference: &str) -> Option<u32> {
    let digits = reference.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    digits
        .parse::<u32>()
        .ok()
        .filter(|row| (1..=MAX_ROW).contains(row))
}

// The value of an unprefixed attribute inside a start tag's text.
pub fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    for (at, _) in tag.match_indices(name) {
        if !tag[..at].ends_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let rest = tag[at + name.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        let value = &rest[1..];
        return value.find(quote).map(|end| &value[..end]);
    }
    None
}

// Splits a sheet part into what comes before <sheetData>, the cell data, and what comes
// after it. A part without cell data is all head.
fn split_at_cell_data(part: &str) -> (&str, &str, &str) {
    let Some((start, open_end)) = cell_data_start(part) else {
        return (part, "", "");
    };
    if part[..open_end].ends_with("/>") {
        return (&part[..start], "", &part[open_end..]);
    }
    let Some((close, close_end)) = cell_data_end(part, open_end) else {
        return (part, "", "");
    };
    (&part[..start], &part[open_end..close], &part[close_end..])
}

fn cell_data_start(part: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    while let Some(at) = part[offset..].find('<') {
        let start = offset + at;
        let end = start + part[start..].find('>')? + 1;
        let tag = &part[start + 1..end - 1];
        let name = tag
            .split(|c: char| c.is_ascii_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        if name.rsplit(':').next() == Some("sheetData") && !name.starts_with('/') {
            return Some((start, end));
        }
        offset = end;
    }
    None
}

fn cell_data_end(part: &str, from: usize) -> Option<(usize, usize)> {
    let at = part[from..].rfind("sheetData>")? + from;
    let before = &part[from..at];
    let close = if before.ends_with("</") {
        at - 2
    } else {
        let prefix_start = before.rfind("</")?;
        let prefix = &before[prefix_start + 2..];
        let valid_prefix = prefix.len() > 1
            && prefix.ends_with(':')
            && !prefix.contains(|c: char| c.is_ascii_whitespace() || c == '<' || c == '>');
        if !valid_prefix {
            return None;
        }
        from + prefix_start
    };
    Some((close, at + "sheetData>".len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

    fn sheet(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><worksheet xmlns="{MAIN}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{inner}</worksheet>"#
        )
    }

    #[test]
    fn page_setup_drops_the_printer_settings_link_and_bad_values() {
        let part = sheet(
            r#"<sheetData/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" orientation="landscape" scale="5000" r:id="rId1"/>"#,
        );
        let settings = read(&part);
        assert_eq!(
            settings.print,
            r#"<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" orientation="landscape"/>"#
        );
    }

    #[test]
    fn prefixed_parts_are_rebuilt_without_the_prefix() {
        let part = format!(
            r#"<x:worksheet xmlns:x="{MAIN}"><x:sheetPr><x:tabColor rgb="FFFF0000"/></x:sheetPr><x:sheetData><x:row r="2" outlineLevel="1"><x:c r="A2"/></x:row></x:sheetData><x:headerFooter><x:oddHeader>&amp;CA &lt; B</x:oddHeader></x:headerFooter></x:worksheet>"#
        );
        let settings = read(&part);
        assert_eq!(
            settings.properties,
            r#"<sheetPr><tabColor rgb="FFFF0000"/></sheetPr>"#
        );
        assert_eq!(
            settings.print,
            "<headerFooter><oddHeader>&amp;CA &lt; B</oddHeader></headerFooter>"
        );
        assert_eq!(settings.rows.outline.get(&2).map(|o| o.level), Some(1));
    }

    #[test]
    fn row_outline_reads_numbered_and_unnumbered_rows() {
        let data = r#"<row r="3" spans="1:2" outlineLevel="2" collapsed="true"/><row outlineLevel="1"><c r="B7"/></row><row r="9" outlineLevel="99"/>"#;
        let mut rows = Indexed::default();
        read_row_outline(data, &mut rows);
        let levels: Vec<_> = rows
            .outline
            .iter()
            .map(|(r, o)| (*r, o.level, o.collapsed))
            .collect();
        assert_eq!(levels, [(3, 2, true), (7, 1, false)]);
        assert_eq!(rows.reach, 7);
    }

    #[test]
    fn moving_rows_before_the_outline_drops_it_and_reports_it() {
        let mut settings = SheetSettings::default();
        settings.rows.add_outline(
            5,
            Outline {
                level: 1,
                collapsed: false,
            },
        );
        let mut carried = Carried::new(HashMap::from([(1, settings)]));
        carried.rows_moved(1, 6);
        assert!(carried.dropped().is_empty());
        carried.columns_moved(1, 1);
        assert!(carried.dropped().is_empty());
        carried.rows_moved(1, 5);
        assert_eq!(carried.dropped(), [Unsupported::Outline]);
        assert!(carried.get(1).is_some_and(|s| s.rows.outline.is_empty()));
    }

    #[test]
    fn cell_data_split_survives_lookalike_text() {
        let part = sheet(
            r#"<sheetPr codeName="sheetData"/><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>sheetData&gt;</t></is></c></row></sheetData><headerFooter><oddHeader>sheetData&gt;</oddHeader></headerFooter>"#,
        );
        let (head, data, tail) = split_at_cell_data(&part);
        assert!(
            head.ends_with(r#"<sheetPr codeName="sheetData"/>"#),
            "{head}"
        );
        assert!(
            data.starts_with("<row") && data.ends_with("</row>"),
            "{data}"
        );
        assert!(tail.starts_with("<headerFooter>"), "{tail}");
    }

    #[test]
    fn measures_and_hex_colours_are_validated() {
        assert!(is_measure("297mm") && is_measure("8.5in"));
        assert!(!is_measure("mm") && !is_measure("1.mm") && !is_measure("-3mm"));
        assert_eq!(valid(Value::Hex, "FF00FF00"), Some("FF00FF00".to_string()));
        assert_eq!(valid(Value::Hex, "\"/><x"), None);
        assert_eq!(valid(Value::Real(0.0, 1.0), "NaN"), None);
    }

    #[test]
    fn attribute_matches_whole_names_only() {
        let tag = r#" spans="1:2" xr="9" r='4' customHeight = "1""#;
        assert_eq!(attribute(tag, "r"), Some("4"));
        assert_eq!(attribute(tag, "customHeight"), Some("1"));
        assert_eq!(attribute(tag, "Height"), None);
    }
}
