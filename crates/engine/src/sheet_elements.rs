// IronCalc reads past a sheet's filter, validation, protection and hyperlinks, and past
// the elements that point into its relationships part (drawings, comments, tables), and
// never writes them. A save that rewrites a sheet into the file it was read from writes
// them back, and keeps the relationships part beside it. Each element is rebuilt from
// the parsed original, never copied as text: the file is hostile input, and IronCalc's
// root declares only the main and relationship namespaces, so attributes of any other
// namespace (such as xr:uid) and extension lists are left out.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::sync::Arc;

use roxmltree::Node;

use crate::file::Unsupported;
use crate::preflight::element_kind;
use crate::saved::Remains;

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
// Bound to the xml prefix in every document, so xml:space needs no declaration.
const XML: &str = "http://www.w3.org/XML/1998/namespace";
const MAX_ROW: u32 = 1_048_576;
const MAX_COLUMN: u32 = 16_384;
// Excel allows 65,530 hyperlinks on a sheet; at about 250 bytes each they are the largest
// element a sheet carries. A sheet whose elements are larger keeps none of them.
pub const MAX_ELEMENT_BYTES: usize = 16 * 1024 * 1024;
// Four sheets at the cap above. The workbook holds these bytes while it is open and copies
// them into each save, so they stay bounded whatever the sheet count.
pub const MAX_WORKBOOK_ELEMENT_BYTES: usize = 4 * MAX_ELEMENT_BYTES;

// Where an element goes among those IronCalc writes after the cell data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    // Right after </sheetData>, before <mergeCells>.
    AfterCellData,
    // After <conditionalFormatting>, before the print settings.
    BeforePrint,
    // After the page breaks, before <extLst>.
    AfterBreaks,
}

// The children of CT_Worksheet carried, in the order the schema requires.
const CARRIED: [(&str, Place); 16] = [
    ("sheetCalcPr", Place::AfterCellData),
    ("sheetProtection", Place::AfterCellData),
    ("protectedRanges", Place::AfterCellData),
    ("autoFilter", Place::AfterCellData),
    ("sortState", Place::AfterCellData),
    ("dataValidations", Place::BeforePrint),
    ("hyperlinks", Place::BeforePrint),
    ("ignoredErrors", Place::AfterBreaks),
    ("drawing", Place::AfterBreaks),
    ("legacyDrawing", Place::AfterBreaks),
    ("legacyDrawingHF", Place::AfterBreaks),
    ("drawingHF", Place::AfterBreaks),
    ("picture", Place::AfterBreaks),
    ("oleObjects", Place::AfterBreaks),
    ("controls", Place::AfterBreaks),
    ("tableParts", Place::AfterBreaks),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Columns,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relationships {
    Kept,
    Dropped,
}

// Which inserted or deleted rows and columns leave an element right; Zenkai cannot shift
// the ranges it names, as Excel would.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Reach {
    // The last row and column its ranges name.
    Cells { rows: u32, columns: u32 },
    // Its formulas or link locations may name cells on any sheet.
    Workbook,
}

impl Reach {
    const NONE: Reach = Reach::Cells {
        rows: 0,
        columns: 0,
    };
    const SHEET: Reach = Reach::Cells {
        rows: u32::MAX,
        columns: u32::MAX,
    };

    fn and(self, other: Reach) -> Reach {
        match (self, other) {
            (
                Reach::Cells { rows, columns },
                Reach::Cells {
                    rows: other_rows,
                    columns: other_columns,
                },
            ) => Reach::Cells {
                rows: rows.max(other_rows),
                columns: columns.max(other_columns),
            },
            _ => Reach::Workbook,
        }
    }

    // A reference that is not a plain cell, such as a whole column, reaches the whole sheet.
    fn including(self, ranges: &str) -> Reach {
        ranges
            .split_ascii_whitespace()
            .flat_map(|range| range.split(':'))
            .map(|cell| match cell_position(cell) {
                Some((rows, columns)) => Reach::Cells { rows, columns },
                None => Reach::SHEET,
            })
            .fold(self, Reach::and)
    }

    // `from` is the 1-based first row or column that moved on the element's sheet.
    fn survives(self, axis: Axis, from: u32) -> bool {
        match (self, axis) {
            (Reach::Workbook, _) => false,
            (Reach::Cells { rows, .. }, Axis::Rows) => from > rows,
            (Reach::Cells { columns, .. }, Axis::Columns) => from > columns,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Element {
    // Position in CARRIED, which is the schema order.
    order: usize,
    name: &'static str,
    place: Place,
    // Shared, so a save takes the elements without copying their text.
    xml: Arc<str>,
    // Of `xml`, taken once when read.
    digest: u64,
    reach: Reach,
}

// Every save and open hashes a sheet's elements into its print; the text, up to the byte
// cap, is hashed once when read, and its digest stands for it.
impl Hash for Element {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.order, self.place, self.digest, self.reach).hash(state);
    }
}

fn digest(xml: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    xml.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
enum Linked {
    // The relationships part goes with the rewritten sheet, and these elements point
    // into it.
    Kept(Vec<Element>),
    // The part and every element pointing into it are left out; the parts it pointed at
    // stay in the file, unreferenced.
    #[default]
    Dropped,
    // The file's relationships hold nothing for this sheet, so there is nothing for a move
    // on another sheet to drop and the sheet is left as it is.
    Nothing,
}

// What is left of the element bytes a workbook may hold, spent sheet by sheet as they are
// read.
pub struct ElementBudget {
    left: usize,
}

impl ElementBudget {
    pub fn workbook() -> ElementBudget {
        ElementBudget {
            left: MAX_WORKBOOK_ELEMENT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Elements {
    linked: Linked,
    standalone: Vec<Element>,
}

impl Elements {
    // `sheet` is the root of the sheet part as `sheet_settings::read` parsed it.
    pub fn read(sheet: Node, budget: &mut ElementBudget) -> Elements {
        if sheet.tag_name().namespace() != Some(MAIN) {
            return Elements::default();
        }
        let cap = MAX_ELEMENT_BYTES.min(budget.left);
        let mut linked = Some(Vec::new());
        let mut standalone = Vec::new();
        let mut bytes = 0;
        let extended = extended_names(sheet);
        for (order, (name, place)) in CARRIED.into_iter().enumerate() {
            let Some(node) = sheet
                .children()
                .find(|n| n.is_element() && n.tag_name().name() == name && in_main(*n))
            else {
                continue;
            };
            // The rebuilt text is never built for an element whose text in the file is
            // already past the cap.
            if bytes + node.range().len() > cap {
                return Elements::over(budget);
            }
            let points = points_into_relationships(node);
            // Excel writes part of some elements, such as list validations naming another
            // sheet, into the sheet's extension list, which is not kept: keeping the rest
            // would hide the loss.
            let rebuilt = match extended.contains(&name) {
                true => None,
                false => match rebuild(node, cap - bytes) {
                    Ok(xml) => Some(xml),
                    Err(Unbuildable::Foreign) => None,
                    Err(Unbuildable::TooLarge) => return Elements::over(budget),
                },
            };
            let Some(xml) = rebuilt else {
                tracing::debug!(
                    element = name,
                    "a sheet element that cannot be rebuilt is not kept"
                );
                // Every element pointing into the part is written back, or none is.
                if points {
                    linked = None;
                }
                continue;
            };
            bytes += xml.len();
            let element = Element {
                order,
                name,
                place,
                digest: digest(&xml),
                xml: xml.into(),
                reach: reach(node),
            };
            match (&mut linked, points) {
                (Some(linked), true) => linked.push(element),
                (None, true) => {}
                (_, false) => standalone.push(element),
            }
        }
        budget.left -= bytes;
        Elements {
            linked: linked.map_or(Linked::Dropped, Linked::Kept),
            standalone,
        }
    }

    // A sheet past its own cap keeps none of its elements; one past what the workbook has
    // left also leaves nothing for the sheets read after it.
    fn over(budget: &mut ElementBudget) -> Elements {
        tracing::warn!("sheet elements larger than supported are not kept");
        if budget.left < MAX_ELEMENT_BYTES {
            budget.left = 0;
        }
        Elements::default()
    }

    fn all(&self) -> impl Iterator<Item = &Element> {
        let linked = match &self.linked {
            Linked::Kept(linked) => linked.as_slice(),
            Linked::Dropped | Linked::Nothing => &[],
        };
        linked.iter().chain(&self.standalone)
    }

    // Writes the elements that go at `place`, in schema order.
    pub fn write_at(&self, out: &mut impl Write, place: Place) -> io::Result<()> {
        let mut placed: Vec<&Element> = self.all().filter(|e| e.place == place).collect();
        placed.sort_by_key(|e| e.order);
        placed
            .iter()
            .try_for_each(|e| out.write_all(e.xml.as_bytes()))
    }

    pub fn kinds(&self) -> BTreeSet<Unsupported> {
        self.all().filter_map(|e| element_kind(e.name)).collect()
    }

    pub fn relationships(&self) -> Relationships {
        match self.linked {
            Linked::Kept(_) | Linked::Nothing => Relationships::Kept,
            Linked::Dropped => Relationships::Dropped,
        }
    }

    // What a save may write into a file that holds `remains` of the sheet.
    pub(crate) fn left_in(&self, remains: Remains) -> Elements {
        match remains {
            Remains::All => self.clone(),
            Remains::Unlinked => Elements {
                linked: Linked::Dropped,
                standalone: self.standalone.clone(),
            },
            Remains::Nothing => Elements::default(),
        }
    }

    // The file's relationships hold nothing for this sheet, as the package was read.
    pub fn nothing_linked(&mut self) {
        if self.linked == Linked::Kept(Vec::new()) {
            self.linked = Linked::Nothing;
        }
    }

    // Rows or columns from `from` (1-based) on were inserted or deleted on this sheet. The
    // parts the relationships point at (drawing anchors, table ranges, pivot locations,
    // comment cells) name rows and columns Zenkai cannot shift, so they all go.
    pub fn moved(&mut self, axis: Axis, from: u32) {
        self.linked = Linked::Dropped;
        self.standalone.retain(|e| e.reach.survives(axis, from));
    }

    // Rows or columns were inserted or deleted on another sheet. The parts the
    // relationships point at (chart series, table formulas, pivot tables) may name cells
    // there and are never read, so they all go, even those that name none.
    pub fn moved_elsewhere(&mut self) {
        if self.linked != Linked::Nothing {
            self.linked = Linked::Dropped;
        }
        self.standalone.retain(|e| e.reach != Reach::Workbook);
    }
}

fn in_main(node: Node) -> bool {
    node.tag_name().namespace() == Some(MAIN)
}

// The names of the elements in the sheet's own extension list.
fn extended_names<'a>(sheet: Node<'a, '_>) -> Vec<&'a str> {
    sheet
        .children()
        .filter(|n| n.is_element() && in_main(*n) && n.tag_name().name() == "extLst")
        .flat_map(|list| list.descendants())
        .filter(Node::is_element)
        .map(|n| n.tag_name().name())
        .collect()
}

fn points_into_relationships(node: Node) -> bool {
    node.descendants().any(|n| {
        n.attributes().any(|a| {
            a.namespace()
                .is_some_and(|ns| ns.ends_with("/relationships"))
        })
    })
}

fn reach(node: Node) -> Reach {
    let mut reach = Reach::NONE;
    for n in node.descendants().filter(|n| n.is_element() && in_main(*n)) {
        let named = match n.tag_name().name() {
            "formula" | "formula1" | "formula2" => {
                Some(n.text().unwrap_or_default()).filter(|text| !is_literal(text))
            }
            "hyperlink" => n
                .attribute("location")
                .filter(|location| !location.is_empty()),
            _ => None,
        };
        if let Some(named) = named {
            reach = reach.and(match is_plain_range(named) {
                true => Reach::SHEET,
                false => Reach::Workbook,
            });
        }
        for ranges in ["ref", "sqref"].into_iter().filter_map(|a| n.attribute(a)) {
            reach = reach.including(ranges);
        }
    }
    reach
}

// Only a plain cell or range is known to name cells of its own sheet alone; a defined
// name, a table column or a function may name cells on any sheet.
fn is_plain_range(text: &str) -> bool {
    let text = text.trim();
    let range = text.strip_prefix('=').unwrap_or(text);
    range.split(':').count() <= 2 && range.split(':').all(|cell| cell_position(cell).is_some())
}

// A number or a quoted text names no cell.
fn is_literal(formula: &str) -> bool {
    let formula = formula.trim();
    if formula.parse::<f64>().is_ok_and(f64::is_finite) {
        return true;
    }
    formula.len() >= 2
        && formula.starts_with('"')
        && formula.ends_with('"')
        && !formula[1..formula.len() - 1]
            .replace("\"\"", "")
            .contains('"')
}

// The 1-based row and column of a cell reference such as "B7" or "$B$7".
pub(crate) fn cell_position(reference: &str) -> Option<(u32, u32)> {
    let plain: String = reference.chars().filter(|c| *c != '$').collect();
    let split = plain.find(|c: char| !c.is_ascii_alphabetic())?;
    let (letters, digits) = plain.split_at(split);
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let column = letters.bytes().fold(0u32, |column, letter| {
        column * 26 + u32::from(letter.to_ascii_uppercase() - b'A') + 1
    });
    let row = digits.parse::<u32>().ok()?;
    ((1..=MAX_ROW).contains(&row) && (1..=MAX_COLUMN).contains(&column)).then_some((row, column))
}

fn text_entity(c: char) -> Option<&'static str> {
    match c {
        '&' => Some("&amp;"),
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '\r' => Some("&#13;"),
        _ => None,
    }
}

fn attribute_entity(c: char) -> Option<&'static str> {
    match c {
        '"' => Some("&quot;"),
        '\t' => Some("&#9;"),
        '\n' => Some("&#10;"),
        c => text_entity(c),
    }
}

enum Unbuildable {
    // An element of another namespace outside an extension list, or a relationship
    // attribute of another namespace: written back, it would not mean the same.
    Foreign,
    // Rebuilt, it would pass the bytes the sheet has left.
    TooLarge,
}

// Checked per character: one character escapes to up to six bytes, so a check after the
// whole text could build six times the limit.
fn push_escaped(
    out: &mut String,
    text: &str,
    entity: fn(char) -> Option<&'static str>,
    limit: usize,
) -> Result<(), Unbuildable> {
    for c in text.chars() {
        match entity(c) {
            Some(escaped) => out.push_str(escaped),
            None => out.push(c),
        }
        if out.len() > limit {
            return Err(Unbuildable::TooLarge);
        }
    }
    Ok(())
}

fn has_content(node: Node) -> bool {
    node.children().any(|n| n.is_element() || n.is_text())
}

enum Visit<'a, 'input> {
    Open(Node<'a, 'input>),
    Close(Node<'a, 'input>),
}

// Walks the tree with its own stack instead of recursion, however deep the file nests it,
// and stops once the text passes `limit`.
fn rebuild(node: Node, limit: usize) -> Result<String, Unbuildable> {
    let mut xml = String::new();
    let mut pending = vec![Visit::Open(node)];
    while let Some(visit) = pending.pop() {
        match visit {
            Visit::Open(n) if n.is_text() => {
                push_escaped(&mut xml, n.text().unwrap_or_default(), text_entity, limit)?;
            }
            Visit::Open(n) if n.is_element() => {
                if !in_main(n) {
                    return Err(Unbuildable::Foreign);
                }
                let name = n.tag_name().name();
                if name == "extLst" {
                    continue;
                }
                xml.push('<');
                xml.push_str(name);
                for attribute in n.attributes() {
                    let prefix = match attribute.namespace() {
                        None => "",
                        Some(RELATIONSHIPS) => "r:",
                        Some(XML) => "xml:",
                        Some(other) if other.ends_with("/relationships") => {
                            return Err(Unbuildable::Foreign);
                        }
                        Some(_) => continue,
                    };
                    xml.push(' ');
                    xml.push_str(prefix);
                    xml.push_str(attribute.name());
                    xml.push_str("=\"");
                    push_escaped(&mut xml, attribute.value(), attribute_entity, limit)?;
                    xml.push('"');
                }
                if has_content(n) {
                    xml.push('>');
                    pending.push(Visit::Close(n));
                    let mut child = n.last_child();
                    while let Some(next) = child {
                        pending.push(Visit::Open(next));
                        child = next.prev_sibling();
                    }
                } else {
                    xml.push_str("/>");
                }
            }
            Visit::Open(_) => {}
            Visit::Close(n) => {
                xml.push_str("</");
                xml.push_str(n.tag_name().name());
                xml.push('>');
            }
        }
        if xml.len() > limit {
            return Err(Unbuildable::TooLarge);
        }
    }
    Ok(xml)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::sync::Arc;

    use super::{
        Axis, ElementBudget, Elements, MAX_ELEMENT_BYTES, MAX_WORKBOOK_ELEMENT_BYTES, Place,
        Relationships, Unbuildable, attribute_entity, cell_position, is_literal, push_escaped,
    };
    use crate::file::Unsupported;
    use crate::saved::Remains;

    const ROOT: &str = concat!(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
        r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
        r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
        r#"xmlns:xr="http://schemas.microsoft.com/office/spreadsheetml/2014/revision" "#,
        r#"mc:Ignorable="xr">"#
    );

    fn parsed(sheet: &str) -> Elements {
        let document = roxmltree::Document::parse(sheet).unwrap();
        Elements::read(document.root_element(), &mut ElementBudget::workbook())
    }

    fn elements(body: &str) -> Elements {
        parsed(&format!("{ROOT}{body}</worksheet>"))
    }

    fn elements_within(body: &str, budget: &mut ElementBudget) -> Elements {
        let sheet = format!("{ROOT}{body}</worksheet>");
        let document = roxmltree::Document::parse(&sheet).unwrap();
        Elements::read(document.root_element(), budget)
    }

    fn at(elements: &Elements, place: Place) -> String {
        let mut out = Vec::new();
        elements.write_at(&mut out, place).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn written(elements: &Elements) -> String {
        [Place::AfterCellData, Place::BeforePrint, Place::AfterBreaks]
            .into_iter()
            .map(|place| at(elements, place))
            .collect()
    }

    #[test]
    fn foreign_attributes_and_extension_lists_are_left_out() {
        let filter = elements(concat!(
            r#"<autoFilter ref="A1:C9" xr:uid="{0}"><filterColumn colId="0"><filters>"#,
            r#"<filter val="a&amp;b"/></filters></filterColumn><extLst><ext uri="u">"#,
            r#"<x14:f xmlns:x14="urn:x14"/></ext></extLst></autoFilter>"#
        ));
        assert_eq!(
            at(&filter, Place::AfterCellData),
            concat!(
                r#"<autoFilter ref="A1:C9"><filterColumn colId="0"><filters>"#,
                r#"<filter val="a&amp;b"/></filters></filterColumn></autoFilter>"#
            )
        );
        assert_eq!(filter.kinds(), BTreeSet::from([Unsupported::AutoFilter]));
    }

    #[test]
    fn an_element_excel_extends_in_the_extension_list_is_not_kept() {
        let validations = elements(concat!(
            r#"<dataValidations count="1"><dataValidation sqref="A1"/></dataValidations>"#,
            r#"<autoFilter ref="A1:B2"/><extLst><ext uri="{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}" "#,
            r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
            r#"<x14:dataValidations count="1"/></ext></extLst>"#
        ));
        assert_eq!(
            validations.kinds(),
            BTreeSet::from([Unsupported::AutoFilter])
        );
    }

    #[test]
    fn prefixes_in_the_file_are_written_as_the_root_declares_them() {
        let drawing = parsed(concat!(
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
            r#"xmlns:rel="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
            r#"<x:drawing rel:id="rId1"/></x:worksheet>"#
        ));
        assert_eq!(
            at(&drawing, Place::AfterBreaks),
            r#"<drawing r:id="rId1"/>"#
        );
        assert_eq!(drawing.relationships(), Relationships::Kept);
    }

    #[test]
    fn an_element_holding_another_namespace_is_not_kept() {
        let links = elements(concat!(
            r#"<hyperlinks><hyperlink ref="A1" location="S!A1"/>"#,
            r#"<f:other xmlns:f="urn:f"/></hyperlinks>"#
        ));
        assert_eq!(written(&links), "");
        assert!(links.kinds().is_empty());
        let strict = parsed(concat!(
            r#"<worksheet xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main">"#,
            r#"<autoFilter ref="A1:B2"/></worksheet>"#
        ));
        assert_eq!(strict, Elements::default());
    }

    #[test]
    fn every_element_pointing_into_the_relationships_is_kept_or_none_is() {
        let mixed = elements(concat!(
            r#"<autoFilter ref="A1:B2"/><drawing r:id="rId1"/>"#,
            r#"<legacyDrawing xmlns:s="http://purl.oclc.org/ooxml/officeDocument/relationships" "#,
            r#"s:id="rId2"/>"#
        ));
        assert_eq!(mixed.relationships(), Relationships::Dropped);
        assert_eq!(written(&mixed), r#"<autoFilter ref="A1:B2"/>"#);
        let links = elements(concat!(
            r#"<hyperlinks><hyperlink ref="A1" r:id="rId1"/></hyperlinks>"#,
            r#"<drawing r:id="rId2"/>"#
        ));
        assert_eq!(links.relationships(), Relationships::Kept);
        assert_eq!(links.left_in(Remains::Unlinked), Elements::default());
        assert_eq!(links.left_in(Remains::All), links);
        // A sheet keeps its relationships with no element pointing into them: pivot tables
        // and comments are reached through the part alone.
        let filter = elements(r#"<autoFilter ref="A1:B2"/>"#);
        assert_eq!(filter.relationships(), Relationships::Kept);
        let unlinked = filter.left_in(Remains::Unlinked);
        assert_eq!(unlinked.relationships(), Relationships::Dropped);
        assert_eq!(written(&unlinked), written(&filter));
        assert_eq!(filter.left_in(Remains::Nothing), Elements::default());
    }

    #[test]
    fn elements_come_back_in_schema_order_whatever_the_file_order() {
        let sheet = elements(concat!(
            r#"<tableParts count="1"><tablePart r:id="rId3"/></tableParts>"#,
            r#"<hyperlinks><hyperlink ref="B2" location="A1"/></hyperlinks>"#,
            r#"<autoFilter ref="A1:B2"/><sheetProtection sheet="1"/><drawing r:id="rId1"/>"#,
            r#"<dataValidations count="1"><dataValidation sqref="C3"/></dataValidations>"#
        ));
        assert_eq!(
            at(&sheet, Place::AfterCellData),
            r#"<sheetProtection sheet="1"/><autoFilter ref="A1:B2"/>"#
        );
        assert_eq!(
            at(&sheet, Place::BeforePrint),
            concat!(
                r#"<dataValidations count="1"><dataValidation sqref="C3"/></dataValidations>"#,
                r#"<hyperlinks><hyperlink ref="B2" location="A1"/></hyperlinks>"#
            )
        );
        assert_eq!(
            at(&sheet, Place::AfterBreaks),
            r#"<drawing r:id="rId1"/><tableParts count="1"><tablePart r:id="rId3"/></tableParts>"#
        );
    }

    #[test]
    fn white_space_handling_is_kept() {
        let validation = concat!(
            r#"<dataValidations count="1"><dataValidation sqref="A1" xml:space="preserve">"#,
            r#"<formula1>" a "</formula1></dataValidation></dataValidations>"#
        );
        assert_eq!(at(&elements(validation), Place::BeforePrint), validation);
    }

    #[test]
    fn text_and_attributes_are_escaped_back() {
        let validation = concat!(
            r#"<dataValidations count="1"><dataValidation type="list" sqref="A1" "#,
            r#"prompt="say &quot;hi&quot;&#9;&#10;&lt;&amp;"><formula1>"a&lt;b&gt;&amp;"</formula1>"#,
            r#"</dataValidation></dataValidations>"#
        );
        assert_eq!(at(&elements(validation), Place::BeforePrint), validation);
    }

    #[test]
    fn a_move_keeps_only_the_elements_it_cannot_shift() {
        let mut filter = elements(r#"<autoFilter ref="A1:C10"/><sheetProtection sheet="1"/>"#);
        filter.moved(Axis::Rows, 11);
        filter.moved(Axis::Columns, 4);
        assert_eq!(filter.kinds().len(), 2);
        filter.moved(Axis::Rows, 10);
        assert_eq!(
            filter.kinds(),
            BTreeSet::from([Unsupported::SheetProtection])
        );

        let mut drawing = elements(r#"<drawing r:id="rId1"/>"#);
        drawing.moved(Axis::Rows, 1_000_000);
        assert_eq!(drawing.relationships(), Relationships::Dropped);

        let validation = |formula: &str| {
            elements(&format!(
                r#"<dataValidations count="1"><dataValidation sqref="B2"><formula1>{formula}</formula1></dataValidation></dataValidations>"#
            ))
        };
        let mut literal = validation(r#""yes""#);
        literal.moved(Axis::Rows, 3);
        literal.moved_elsewhere();
        assert_eq!(
            literal.kinds(),
            BTreeSet::from([Unsupported::DataValidation])
        );
        let mut same_sheet = validation("$A$1:$A$5");
        same_sheet.moved_elsewhere();
        assert_eq!(same_sheet.kinds().len(), 1);
        same_sheet.moved(Axis::Rows, 1_000_000);
        assert!(same_sheet.kinds().is_empty());
        let mut other_sheet = validation("Lists!$A$1:$A$5");
        other_sheet.moved_elsewhere();
        assert!(other_sheet.kinds().is_empty());
    }

    #[test]
    fn a_link_location_reaches_the_sheet_it_names() {
        let link = |location: &str| {
            elements(&format!(
                r#"<hyperlinks><hyperlink ref="B2" location="{location}"/></hyperlinks>"#
            ))
        };
        let mut other_sheet = link("Other!A5");
        other_sheet.moved_elsewhere();
        assert!(other_sheet.kinds().is_empty());
        let mut same_sheet = link("A5");
        same_sheet.moved_elsewhere();
        assert_eq!(same_sheet.kinds().len(), 1);
        same_sheet.moved(Axis::Rows, 1_000_000);
        assert!(same_sheet.kinds().is_empty());
    }

    #[test]
    fn a_move_on_another_sheet_drops_the_relationships() {
        for linked in [
            r#"<drawing r:id="rId1"/>"#,
            r#"<hyperlinks><hyperlink ref="B2" r:id="rId1"/></hyperlinks>"#,
            r#"<tableParts count="1"><tablePart r:id="rId1"/></tableParts>"#,
        ] {
            let mut sheet = elements(&format!(r#"{linked}<sheetProtection sheet="1"/>"#));
            assert_eq!(sheet.relationships(), Relationships::Kept);
            sheet.moved_elsewhere();
            assert_eq!(sheet.relationships(), Relationships::Dropped, "{linked}");
            assert_eq!(
                sheet.kinds(),
                BTreeSet::from([Unsupported::SheetProtection]),
                "{linked}"
            );
        }
        // Pivot tables are reached through the relationships part alone.
        let mut unpointed = elements(r#"<sheetProtection sheet="1"/>"#);
        assert_eq!(unpointed.relationships(), Relationships::Kept);
        unpointed.moved_elsewhere();
        assert_eq!(unpointed.relationships(), Relationships::Dropped);
        assert_eq!(unpointed.kinds().len(), 1);
    }

    #[test]
    fn a_sheet_the_file_links_nothing_to_is_left_alone_by_a_move_elsewhere() {
        let mut sheet = elements(r#"<sheetProtection sheet="1"/>"#);
        sheet.nothing_linked();
        let before = sheet.clone();
        sheet.moved_elsewhere();
        assert_eq!(sheet, before);
        assert_eq!(sheet.relationships(), Relationships::Kept);
        sheet.moved(Axis::Rows, 1);
        assert_eq!(sheet.relationships(), Relationships::Dropped);
        // An element pointing into the relationships part means the sheet does link.
        let mut linked = elements(r#"<drawing r:id="rId1"/>"#);
        linked.nothing_linked();
        linked.moved_elsewhere();
        assert_eq!(linked.relationships(), Relationships::Dropped);
    }

    #[test]
    fn only_a_plain_cell_or_range_reaches_its_own_sheet_alone() {
        let validation = |formula: &str| {
            elements(&format!(
                r#"<dataValidations count="1"><dataValidation sqref="B2"><formula1>{formula}</formula1></dataValidation></dataValidations>"#
            ))
        };
        for own in ["A5", "$A$1:$A$5", " =B2 "] {
            let mut kept = validation(own);
            kept.moved_elsewhere();
            assert_eq!(kept.kinds().len(), 1, "{own}");
        }
        for named in [
            "MyName",
            "Table1[Col]",
            "OFFSET(A1,0,0)",
            "A1,B2",
            "A1:B2:C3",
            "TRUE",
            "A:A",
            "",
        ] {
            let mut dropped = validation(named);
            dropped.moved_elsewhere();
            assert!(dropped.kinds().is_empty(), "{named}");
        }
        let mut named_link =
            elements(r#"<hyperlinks><hyperlink ref="B2" location="Target"/></hyperlinks>"#);
        named_link.moved_elsewhere();
        assert!(named_link.kinds().is_empty());
    }

    #[test]
    fn a_save_takes_the_elements_without_copying_their_text() {
        let sheet = elements(r#"<autoFilter ref="A1:B2"/><drawing r:id="rId1"/>"#);
        let taken = sheet.left_in(Remains::All);
        let pairs: Vec<_> = taken.all().zip(sheet.all()).collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|(a, b)| Arc::ptr_eq(&a.xml, &b.xml)));
    }

    fn hashed(elements: &Elements) -> u64 {
        let mut hasher = DefaultHasher::new();
        elements.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn the_hash_follows_the_text_and_what_a_move_drops() {
        let filter = r#"<autoFilter ref="A1:B2"/>"#;
        assert_eq!(hashed(&elements(filter)), hashed(&elements(filter)));
        let other = r#"<autoFilter ref="A1:B3"/>"#;
        assert_ne!(hashed(&elements(filter)), hashed(&elements(other)));
        let mut moved = elements(filter);
        moved.moved(Axis::Rows, 1);
        assert_ne!(hashed(&moved), hashed(&elements(filter)));
    }

    #[test]
    fn whole_columns_reach_every_row() {
        let mut columns = elements(r#"<autoFilter ref="A:C"/>"#);
        columns.moved(Axis::Rows, 1_048_576);
        assert!(columns.kinds().is_empty());
    }

    #[test]
    fn elements_up_to_the_byte_budget_are_kept() {
        let frame = r#"<hyperlinks><hyperlink ref="A1" location=""/></hyperlinks>"#.len();
        let links = |length: usize| {
            elements(&format!(
                r#"<hyperlinks><hyperlink ref="A1" location="{}"/></hyperlinks>"#,
                "X".repeat(length)
            ))
        };
        let at_budget = links(MAX_ELEMENT_BYTES - frame);
        assert_eq!(at(&at_budget, Place::BeforePrint).len(), MAX_ELEMENT_BYTES);
        assert_eq!(links(MAX_ELEMENT_BYTES - frame + 1), Elements::default());
    }

    #[test]
    fn sheets_past_the_workbook_budget_keep_nothing() {
        assert_eq!(ElementBudget::workbook().left, MAX_WORKBOOK_ELEMENT_BYTES);
        let filter = r#"<autoFilter ref="A1:B2"/>"#;
        let calc = "<sheetCalcPr/>";
        let mut budget = ElementBudget {
            left: 2 * filter.len(),
        };
        assert_eq!(written(&elements_within(filter, &mut budget)), filter);
        assert_eq!(written(&elements_within(filter, &mut budget)), filter);
        assert_eq!(budget.left, 0);
        assert_eq!(elements_within(calc, &mut budget), Elements::default());

        let mut budget = ElementBudget {
            left: filter.len() + calc.len(),
        };
        assert_eq!(written(&elements_within(filter, &mut budget)), filter);
        let both = format!(r#"{filter}<sheetProtection sheet="1"/>"#);
        assert_eq!(elements_within(&both, &mut budget), Elements::default());
        assert_eq!(budget.left, 0);
        assert_eq!(elements_within(calc, &mut budget), Elements::default());
    }

    #[test]
    fn an_element_past_the_budget_in_the_file_is_not_rebuilt() {
        let filter = r#"<autoFilter ref="A1:B2"/>"#;
        let tagged = r#"<autoFilter ref="A1:B2" xr:uid="{00000000-0000-0000-0000-000000000001}"/>"#;
        let mut budget = ElementBudget { left: filter.len() };
        assert_eq!(elements_within(tagged, &mut budget), Elements::default());
        let mut budget = ElementBudget { left: filter.len() };
        assert_eq!(written(&elements_within(filter, &mut budget)), filter);
    }

    #[test]
    fn escaping_stops_just_past_the_limit() {
        let quotes = "\"".repeat(1_000);
        let mut out = String::new();
        assert!(matches!(
            push_escaped(&mut out, &quotes, attribute_entity, 60),
            Err(Unbuildable::TooLarge)
        ));
        assert!(out.len() <= 60 + "&quot;".len(), "{}", out.len());
        let mut out = String::new();
        assert!(push_escaped(&mut out, "\"\"", attribute_entity, 12).is_ok());
        assert_eq!(out, "&quot;&quot;");
    }

    #[test]
    fn an_element_whose_escaped_text_passes_the_budget_is_not_kept() {
        let quoted = format!(r#"<sheetProtection algorithmName='{}'/>"#, "\"".repeat(100));
        let mut budget = ElementBudget { left: quoted.len() };
        assert_eq!(elements_within(&quoted, &mut budget), Elements::default());
        assert_eq!(budget.left, 0);
    }

    #[test]
    fn cell_positions_stay_on_the_grid() {
        assert_eq!(cell_position("B7"), Some((7, 2)));
        assert_eq!(cell_position("$XFD$1048576"), Some((1_048_576, 16_384)));
        for outside in ["XFE1", "A0", "A1048577", "A", "7", "AAAA1", "A1B", ""] {
            assert_eq!(cell_position(outside), None, "{outside}");
        }
    }

    #[test]
    fn only_numbers_and_quoted_texts_are_literal() {
        for literal in ["3", " -1.5e3 ", r#""a""b""#, r#""""#] {
            assert!(is_literal(literal), "{literal}");
        }
        for formula in ["A1", r#""a"b""#, r#"""#, "inf", "NaN", r#""a"&B1"#] {
            assert!(!is_literal(formula), "{formula}");
        }
    }
}
