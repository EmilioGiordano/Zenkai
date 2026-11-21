// What each worksheet's relationships point at, followed one level into its drawings. A
// save that rewrites a sheet leaves its relationships behind, and with them what they
// point at, so this is what such a save drops beyond the sheet's own elements.

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read, Seek};

use crate::ReadError;
use crate::drawing_cells;
use crate::layout::names_in_order;
use crate::stream::{Budgets, CompressedMeter, Guard, read_part};

// Excel allows 65,530 hyperlinks on a sheet, each a relationship of about 250 bytes. A
// larger relationships part is not parsed, and its sheet counts as pointing at anything.
pub const MAX_RELATIONSHIPS_BYTES: usize = 16 * 1024 * 1024;
// Excel writes about a kilobyte per picture, shape or note, so some 16,000 of them. A larger
// drawing is not read, and its sheet counts as naming cells.
pub const MAX_DRAWING_BYTES: usize = 16 * 1024 * 1024;
// Four drawings at the cap above, so the drawings an open reads cost bounded time and memory
// whatever their count. Drawings past it are not read either.
pub const MAX_WORKBOOK_DRAWING_BYTES: usize = 4 * MAX_DRAWING_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SheetLink {
    Drawing,
    Chart,
    Image,
    PivotTable,
    Comments,
    Table,
    Hyperlink,
    // Not a part: a shape, control or link among the others names cells, which may be on
    // any sheet.
    CellReference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SheetLinks {
    Known(BTreeSet<SheetLink>),
    // The relationships could not be read or point at something not listed here, so the
    // sheet may point at anything.
    Unknown,
}

// The relationships part of `part`, beside it in a `_rels` folder.
pub fn relationships_part(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

// %XX escapes are decoded; a malformed one is kept as written.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = bytes
            .get(index + 1..index + 3)
            .filter(|_| bytes[index] == b'%')
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

// Part names are compared without case, as the package format does. Targets are read as
// leniently as any reader might, so an oddly spelled target cannot hide what it points at.
fn resolve(source: &str, target: &str) -> Option<String> {
    let without_fragment = target.split('#').next().unwrap_or_default();
    let target = percent_decoded(&without_fragment.replace('\\', "/"))?;
    if let Some(absolute) = target.strip_prefix('/') {
        return Some(absolute.to_ascii_lowercase());
    }
    let mut segments: Vec<&str> = source.split('/').collect();
    segments.pop();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            name => segments.push(name),
        }
    }
    Some(segments.join("/").to_ascii_lowercase())
}

// An entry name spelled as `resolve` spells targets, so two spellings of one part collide
// and count as a part stored twice.
fn normalized(name: &str) -> String {
    name.replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
        .to_ascii_lowercase()
}

// A link to a place in a workbook names cells a move may shift: `#Sheet2!A1`, and, failing
// closed, any fragment or bracketed book outside a web or mail address, since
// `Book.xlsx#Sheet2!A1` and `[Book.xlsx]Sheet2!A1` may name this very file.
fn names_a_place(target: &str) -> bool {
    let target = target.trim();
    let web = ["http:", "https:", "mailto:"].iter().any(|scheme| {
        target
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    let fragment = target.contains('#') || target.to_ascii_lowercase().contains("%23");
    target.starts_with('#') || (!web && (fragment || target.starts_with('[')))
}

enum Kind {
    Link(SheetLink),
    // Nothing the user loses when a save leaves it behind.
    Harmless,
    Unlisted,
}

fn kind_of(kind: &str) -> Kind {
    match kind {
        "drawing" | "vmlDrawing" => Kind::Link(SheetLink::Drawing),
        "chart" | "chartEx" => Kind::Link(SheetLink::Chart),
        "image" => Kind::Link(SheetLink::Image),
        "pivotTable" => Kind::Link(SheetLink::PivotTable),
        "comments" | "threadedComment" => Kind::Link(SheetLink::Comments),
        "table" => Kind::Link(SheetLink::Table),
        "hyperlink" => Kind::Link(SheetLink::Hyperlink),
        "printerSettings" => Kind::Harmless,
        _ => Kind::Unlisted,
    }
}

type Found = Vec<(SheetLink, Option<String>)>;

// The links in a relationships part of `source`, each with the part it points at inside
// the package; None when the part cannot be read as relationships or links to anything
// not listed.
fn parse(bytes: &[u8], source: &str) -> Option<Found> {
    if bytes.len() > MAX_RELATIONSHIPS_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let document = roxmltree::Document::parse(text).ok()?;
    let mut found = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Relationship")
    {
        let kind = node.attribute("Type")?.rsplit('/').next()?;
        let link = match kind_of(kind) {
            Kind::Link(link) => link,
            Kind::Harmless => continue,
            Kind::Unlisted => return None,
        };
        let external = node
            .attribute("TargetMode")
            .is_some_and(|mode| mode.eq_ignore_ascii_case("External"));
        let target = node.attribute("Target");
        let part = match external {
            true => None,
            false => target.and_then(|target| resolve(source, target)),
        };
        found.push((link, part));
        if link == SheetLink::Hyperlink && target.is_some_and(names_a_place) {
            found.push((SheetLink::CellReference, None));
        }
    }
    Some(found)
}

// None when a drawing links to anything but what it shows.
fn drawing_links(found: Found) -> Option<BTreeSet<SheetLink>> {
    found
        .into_iter()
        .map(|(link, _)| match link {
            SheetLink::Chart
            | SheetLink::Image
            | SheetLink::Hyperlink
            | SheetLink::CellReference => Some(link),
            _ => None,
        })
        .collect()
}

struct Reader<'a, R> {
    archive: &'a mut zip::ZipArchive<R>,
    meter: &'a CompressedMeter,
    budgets: &'a Budgets,
    // Entry positions by lowercase name; None for a name stored more than once, since
    // readers disagree on which of the entries they take.
    names: HashMap<String, Option<usize>>,
    drawing_bytes_left: usize,
}

impl<'a, R: Read + Seek> Reader<'a, R> {
    fn new(
        archive: &'a mut zip::ZipArchive<R>,
        meter: &'a CompressedMeter,
        budgets: &'a Budgets,
    ) -> Result<Reader<'a, R>, ReadError> {
        let mut names: HashMap<String, Option<usize>> = HashMap::new();
        for (index, name) in names_in_order(archive)?.into_iter().enumerate() {
            names
                .entry(normalized(&name))
                .and_modify(|stored| *stored = None)
                .or_insert(Some(index));
        }
        Ok(Reader {
            archive,
            meter,
            budgets,
            names,
            drawing_bytes_left: MAX_WORKBOOK_DRAWING_BYTES,
        })
    }

    fn contains(&self, part: &str) -> bool {
        self.names.contains_key(part)
    }

    // A part without a relationships part points at nothing.
    fn relationships(&mut self, source: &str) -> Result<Option<Found>, ReadError> {
        let name = normalized(&relationships_part(source));
        let index = match self.names.get(&name) {
            None => return Ok(Some(Vec::new())),
            Some(None) => return Ok(None),
            Some(Some(index)) => *index,
        };
        let entry = self
            .archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        if entry.size() > MAX_RELATIONSHIPS_BYTES as u64 {
            return Ok(None);
        }
        // The declared size may lie; one byte past the cap is enough to refuse the part.
        let part = entry.name().to_string();
        let capped = entry.take(MAX_RELATIONSHIPS_BYTES as u64 + 1);
        let bytes = read_part(capped, self.meter, &part, self.budgets)?;
        Ok(parse(&bytes, source))
    }

    // A drawing stored twice, past the caps or unreadable may name cells.
    fn drawing_names_cells(&mut self, drawing: &str) -> Result<bool, ReadError> {
        let Some(&Some(index)) = self.names.get(drawing) else {
            return Ok(true);
        };
        let entry = match self.archive.by_index(index) {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!(%error, drawing, "a drawing that cannot be opened may name cells");
                return Ok(true);
            }
        };
        let cap = MAX_DRAWING_BYTES.min(self.drawing_bytes_left);
        if entry.size() > cap as u64 {
            return Ok(true);
        }
        let part = entry.name().to_string();
        // Nothing is kept once scanned, so the caps above bound the work instead of the
        // budgets that measure what the open keeps.
        let mut inflating = Inflating {
            guard: self
                .budgets
                .stream_reread(entry.take(cap as u64 + 1), self.meter, &part),
            read: 0,
        };
        let scanned = drawing_cells::names_cells(&mut inflating);
        self.drawing_bytes_left = self.drawing_bytes_left.saturating_sub(inflating.read);
        // The open walk never inflated the drawing, so one past a ratio or size check, or
        // damaged, counts as naming cells instead of stopping it; only a cancel stops it.
        match scanned {
            Ok(names) => Ok(names || inflating.read > cap),
            Err(_) if self.budgets.cancelled() => Err(ReadError::Cancelled),
            Err(error) => {
                tracing::debug!(%error, part, "a drawing that cannot be read may name cells");
                Ok(true)
            }
        }
    }
}

// A drawing as it inflates, through the budgets' checks, counting the bytes it gives.
struct Inflating<'b, R> {
    guard: Guard<'b, R>,
    read: usize,
}

impl<R: Read> Read for Inflating<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.guard.read_chunk(buf).map_err(io::Error::other)?;
        self.read += read;
        Ok(read)
    }
}

// By position, as `worksheets`. Each drawing's relationships are read once, however many
// sheets point at it.
pub(crate) fn sheet_links<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
    worksheets: &[String],
) -> Result<Vec<SheetLinks>, ReadError> {
    let mut reader = Reader::new(archive, meter, budgets)?;
    let mut drawings: HashMap<String, Option<BTreeSet<SheetLink>>> = HashMap::new();
    let mut all = Vec::with_capacity(worksheets.len());
    'sheets: for sheet in worksheets {
        let Some(found) = reader.relationships(sheet)? else {
            all.push(SheetLinks::Unknown);
            continue;
        };
        let mut links = BTreeSet::new();
        for (link, target) in found {
            links.insert(link);
            if link != SheetLink::Drawing {
                continue;
            }
            // A drawing that is not in the package cannot be followed to what it shows.
            let Some(drawing) = target.filter(|drawing| reader.contains(drawing)) else {
                all.push(SheetLinks::Unknown);
                continue 'sheets;
            };
            let inner = match drawings.entry(drawing) {
                Entry::Occupied(read) => read.into_mut(),
                Entry::Vacant(unread) => {
                    let mut inner = reader.relationships(unread.key())?.and_then(drawing_links);
                    if let Some(inner) = &mut inner
                        && reader.drawing_names_cells(unread.key())?
                    {
                        inner.insert(SheetLink::CellReference);
                    }
                    unread.insert(inner)
                }
            };
            match inner {
                Some(inner) => links.extend(inner.iter().copied()),
                None => {
                    all.push(SheetLinks::Unknown);
                    continue 'sheets;
                }
            }
        }
        all.push(SheetLinks::Known(links));
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    const SHEET: &str = "xl/worksheets/sheet1.xml";
    const SHEET_RELS: &str = "xl/worksheets/_rels/sheet1.xml.rels";
    const DRAWING: &str = "xl/drawings/drawing1.xml";
    const DRAWING_RELS: &str = "xl/drawings/_rels/drawing1.xml.rels";

    fn rels(body: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#
        )
    }

    fn relationship(id: u32, kind: &str, target: &str) -> String {
        format!(
            r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        )
    }

    fn zipped(entries: &[(&str, String)], method: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::FileOptions::default().compression_method(method);
        for (name, body) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn links_of(entries: &[(&str, String)], sheets: &[&str]) -> Vec<SheetLinks> {
        links_in(zipped(entries, zip::CompressionMethod::Deflated), sheets)
    }

    fn links_in(bytes: Vec<u8>, sheets: &[&str]) -> Vec<SheetLinks> {
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes))).unwrap();
        let sheets: Vec<String> = sheets.iter().map(ToString::to_string).collect();
        sheet_links(&mut archive, &meter, &Budgets::production(), &sheets).unwrap()
    }

    fn known(links: &[SheetLink]) -> SheetLinks {
        SheetLinks::Known(links.iter().copied().collect())
    }

    #[test]
    fn a_sheet_without_relationships_points_at_nothing() {
        assert_eq!(links_of(&[(SHEET, String::new())], &[SHEET]), [known(&[])]);
    }

    #[test]
    fn drawings_are_followed_to_their_charts_and_pictures() {
        let sheet_rels = rels(&format!(
            "{}{}{}",
            relationship(1, "drawing", "../drawings/drawing1.xml"),
            relationship(2, "pivotTable", "../pivotTables/pivotTable1.xml"),
            relationship(3, "hyperlink", "https://example.com"),
        ))
        .replace(
            r#"Target="https://example.com""#,
            r#"Target="https://example.com" TargetMode="External""#,
        );
        let drawing_rels = rels(&format!(
            "{}{}",
            relationship(1, "chart", "../charts/chart1.xml"),
            relationship(2, "image", "../media/image1.png"),
        ));
        let entries = [
            (SHEET, String::new()),
            (SHEET_RELS, sheet_rels),
            (DRAWING, String::new()),
            (DRAWING_RELS, drawing_rels),
        ];
        assert_eq!(
            links_of(&entries, &[SHEET]),
            [known(&[
                SheetLink::Drawing,
                SheetLink::Chart,
                SheetLink::Image,
                SheetLink::PivotTable,
                SheetLink::Hyperlink,
            ])]
        );
    }

    #[test]
    fn each_sheet_has_only_its_own_links() {
        let second = "xl/worksheets/sheet2.xml";
        let entries = [
            (SHEET, String::new()),
            (second, String::new()),
            (
                "xl/worksheets/_rels/sheet2.xml.rels",
                rels(&relationship(1, "table", "../tables/table1.xml")),
            ),
        ];
        assert_eq!(
            links_of(&entries, &[SHEET, second]),
            [known(&[]), known(&[SheetLink::Table])]
        );
    }

    #[test]
    fn names_are_matched_without_case_and_absolute_targets_resolve() {
        let entries = [
            (SHEET, String::new()),
            (
                "XL/Worksheets/_RELS/Sheet1.xml.rels",
                rels(&relationship(1, "drawing", "/xl/Drawings/Drawing1.xml")),
            ),
            (DRAWING, String::new()),
            (
                DRAWING_RELS,
                rels(&relationship(1, "chart", "../charts/chart1.xml")),
            ),
        ];
        assert_eq!(
            links_of(&entries, &[SHEET]),
            [known(&[SheetLink::Drawing, SheetLink::Chart])]
        );
    }

    #[test]
    fn unreadable_relationships_leave_the_sheet_unknown() {
        let drawing = rels(&relationship(1, "drawing", "../drawings/drawing1.xml"));
        for (sheet_rels, drawing_rels) in [
            ("<Relationships".to_string(), None),
            (drawing.clone(), Some("not xml <".to_string())),
        ] {
            let mut entries = vec![
                (SHEET, String::new()),
                (SHEET_RELS, sheet_rels),
                (DRAWING, String::new()),
            ];
            if let Some(body) = drawing_rels {
                entries.push((DRAWING_RELS, body));
            }
            assert_eq!(links_of(&entries, &[SHEET]), [SheetLinks::Unknown]);
        }
    }

    #[test]
    fn a_relationships_part_past_the_cap_is_not_parsed() {
        let one = relationship(1, "table", "../tables/table1.xml");
        let fitting = rels(&one);
        let padding = MAX_RELATIONSHIPS_BYTES - fitting.len();
        let at_cap = rels(&format!("{one}{}", " ".repeat(padding)));
        assert_eq!(at_cap.len(), MAX_RELATIONSHIPS_BYTES);
        assert_eq!(
            parse(at_cap.as_bytes(), SHEET).map(|found| found.len()),
            Some(1)
        );
        let past_cap = rels(&format!("{one}{}", " ".repeat(padding + 1)));
        assert!(parse(past_cap.as_bytes(), SHEET).is_none());
        let entries = [
            (SHEET, String::new()),
            ("xl/worksheets/_rels/sheet1.xml.rels", past_cap),
        ];
        assert_eq!(links_of(&entries, &[SHEET]), [SheetLinks::Unknown]);
    }

    fn drawing_with(drawing_rels: &str) -> Vec<SheetLinks> {
        let entries = [
            (SHEET, String::new()),
            (
                SHEET_RELS,
                rels(&relationship(1, "drawing", "../drawings/drawing1.xml")),
            ),
            (DRAWING, String::new()),
            (DRAWING_RELS, rels(drawing_rels)),
        ];
        links_of(&entries, &[SHEET])
    }

    #[test]
    fn a_link_not_listed_leaves_the_sheet_unknown_unless_known_harmless() {
        let sheet_with = |body: &str| {
            let entries = [(SHEET, String::new()), (SHEET_RELS, rels(body))];
            links_of(&entries, &[SHEET])
        };
        let printer = relationship(1, "printerSettings", "../printerSettings/p1.bin");
        assert_eq!(sheet_with(&printer), [known(&[])]);
        for kind in [
            "oleObject",
            "ctrlProp",
            "slicer",
            "queryTable",
            "customProperty",
        ] {
            let unlisted = relationship(1, kind, "../other/part1.xml");
            assert_eq!(sheet_with(&unlisted), [SheetLinks::Unknown], "{kind}");
        }
        let link = relationship(1, "hyperlink", "https://example.com");
        assert_eq!(
            drawing_with(&link),
            [known(&[SheetLink::Drawing, SheetLink::Hyperlink])]
        );
        for kind in ["diagramData", "oleObject", "table"] {
            let unlisted = relationship(1, kind, "../other/part1.xml");
            assert_eq!(drawing_with(&unlisted), [SheetLinks::Unknown], "{kind}");
        }
    }

    fn with_drawing(drawing: String) -> [(&'static str, String); 3] {
        [
            (SHEET, String::new()),
            (
                SHEET_RELS,
                rels(&relationship(1, "drawing", "../drawings/drawing1.xml")),
            ),
            (DRAWING, drawing),
        ]
    }

    fn shape_linked_to(cell: &str) -> String {
        format!(r#"<xdr:wsDr xmlns:xdr="urn:xdr"><xdr:sp macro="" textlink="{cell}"/></xdr:wsDr>"#)
    }

    #[test]
    fn a_drawing_with_shape_text_showing_a_cell_names_cells() {
        assert_eq!(
            links_of(&with_drawing(shape_linked_to("")), &[SHEET]),
            [known(&[SheetLink::Drawing])]
        );
        for drawing in [shape_linked_to("Sheet3!$A$1"), "<xdr:sp".to_string()] {
            assert_eq!(
                links_of(&with_drawing(drawing.clone()), &[SHEET]),
                [known(&[SheetLink::Drawing, SheetLink::CellReference])],
                "{drawing}"
            );
        }
    }

    // Readers disagree on which of two entries of one name they take.
    #[test]
    fn a_drawing_stored_twice_names_cells() {
        for spelling in [
            "XL/Drawings/Drawing1.xml",
            "/xl/drawings/./drawing1.xml",
            "xl//drawings/drawing1.xml",
            "xl\\drawings\\drawing1.xml",
            "xl/drawings/drawing1.xml",
        ] {
            let mut entries = with_drawing(String::new()).to_vec();
            entries.push((spelling, shape_linked_to("A1")));
            assert_eq!(
                links_of(&entries, &[SHEET]),
                [known(&[SheetLink::Drawing, SheetLink::CellReference])],
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_drawing_up_to_the_cap_is_scanned_and_a_larger_one_names_cells() {
        let sized = |length: usize| {
            let (open, close) = (r#"<xdr:wsDr xmlns:xdr="urn:xdr">"#, "</xdr:wsDr>");
            let padding = " ".repeat(length - open.len() - close.len());
            format!("{open}{padding}{close}")
        };
        let links_with = |drawing: String| {
            let stored = zipped(&with_drawing(drawing), zip::CompressionMethod::Stored);
            links_in(stored, &[SHEET])
        };
        assert_eq!(
            links_with(sized(MAX_DRAWING_BYTES)),
            [known(&[SheetLink::Drawing])]
        );
        assert_eq!(
            links_with(sized(MAX_DRAWING_BYTES + 1)),
            [known(&[SheetLink::Drawing, SheetLink::CellReference])]
        );
    }

    const LOCAL_HEADER: [u8; 4] = 0x0403_4b50u32.to_le_bytes();
    const CENTRAL_HEADER: [u8; 4] = 0x0201_4b50u32.to_le_bytes();

    // The local and central headers of `name`, set to a compression method zip cannot read.
    fn with_method(mut bytes: Vec<u8>, name: &str, method: u16) -> Vec<u8> {
        let starts: Vec<usize> = bytes
            .windows(name.len())
            .enumerate()
            .filter(|(_, window)| *window == name.as_bytes())
            .map(|(at, _)| at)
            .collect();
        for start in starts {
            let (local, central) = (start.checked_sub(30), start.checked_sub(46));
            let field = match (local, central) {
                (Some(header), _) if bytes[header..].starts_with(&LOCAL_HEADER) => header + 8,
                (_, Some(header)) if bytes[header..].starts_with(&CENTRAL_HEADER) => header + 10,
                _ => continue,
            };
            bytes[field..field + 2].copy_from_slice(&method.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_drawing_zip_cannot_open_names_cells() {
        let stored = zipped(&with_drawing(String::new()), zip::CompressionMethod::Stored);
        let bzip2 = 12;
        assert_eq!(
            links_in(with_method(stored, DRAWING, bzip2), &[SHEET]),
            [known(&[SheetLink::Drawing, SheetLink::CellReference])]
        );
    }

    #[test]
    fn a_cancel_during_a_drawing_stops_the_links_pass() {
        let entries = with_drawing(String::new());
        let bytes = zipped(&entries, zip::CompressionMethod::Deflated);
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes))).unwrap();
        let budgets = Budgets::production();
        let mut reader = Reader::new(&mut archive, &meter, &budgets).unwrap();
        budgets.cancel();
        assert!(matches!(
            reader.drawing_names_cells(DRAWING),
            Err(ReadError::Cancelled)
        ));
    }

    // Drawings are not inflated on the way in, so one that fails here leaves the open going.
    #[test]
    fn a_drawing_that_cannot_be_inflated_names_cells() {
        let drawing = r#"<xdr:wsDr xmlns:xdr="urn:xdr"><!--intact--></xdr:wsDr>"#;
        let mut bytes = zipped(
            &with_drawing(drawing.to_string()),
            zip::CompressionMethod::Stored,
        );
        let at = bytes.windows(6).position(|w| w == b"intact").unwrap();
        bytes[at] = b'I';
        assert_eq!(
            links_in(bytes, &[SHEET]),
            [known(&[SheetLink::Drawing, SheetLink::CellReference])]
        );
    }

    #[test]
    fn drawings_past_the_workbook_budget_name_cells() {
        assert_eq!(MAX_WORKBOOK_DRAWING_BYTES, 4 * MAX_DRAWING_BYTES);
        let plain = r#"<xdr:wsDr xmlns:xdr="urn:xdr"/>"#;
        let second = "xl/drawings/drawing2.xml";
        let entries = [(DRAWING, plain.to_string()), (second, plain.to_string())];
        let bytes = zipped(&entries, zip::CompressionMethod::Deflated);
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes))).unwrap();
        let budgets = Budgets::production();
        let mut reader = Reader::new(&mut archive, &meter, &budgets).unwrap();
        reader.drawing_bytes_left = 2 * plain.len() - 1;
        assert!(!reader.drawing_names_cells(DRAWING).unwrap());
        assert_eq!(reader.drawing_bytes_left, plain.len() - 1);
        assert!(reader.drawing_names_cells(second).unwrap());
    }

    #[test]
    fn a_link_to_a_place_in_the_workbook_names_cells() {
        let external = |target: &str| {
            relationship(1, "hyperlink", target).replace("/>", r#" TargetMode="External"/>"#)
        };
        assert_eq!(
            drawing_with(&relationship(1, "hyperlink", "#Sheet2!A1")),
            [known(&[
                SheetLink::Drawing,
                SheetLink::Hyperlink,
                SheetLink::CellReference
            ])]
        );
        assert_eq!(
            drawing_with(&external("https://example.com/#top")),
            [known(&[SheetLink::Drawing, SheetLink::Hyperlink])]
        );
        for place in [
            " #'My Sheet'!B2",
            "Book.xlsx#Sheet2!A1",
            "[Book.xlsx]Sheet2!A1",
            "file:///C:/Books/Book.xlsx#Sheet2!A1",
            "Book.xlsx%23Sheet2!A1",
        ] {
            let sheet_level = [(SHEET, String::new()), (SHEET_RELS, rels(&external(place)))];
            assert_eq!(
                links_of(&sheet_level, &[SHEET]),
                [known(&[SheetLink::Hyperlink, SheetLink::CellReference])],
                "{place}"
            );
        }
        for web in [
            "HTTPS://example.com/#top",
            "mailto:a@example.com",
            "Book.xlsx",
        ] {
            let sheet_level = [(SHEET, String::new()), (SHEET_RELS, rels(&external(web)))];
            assert_eq!(
                links_of(&sheet_level, &[SHEET]),
                [known(&[SheetLink::Hyperlink])],
                "{web}"
            );
        }
    }

    #[test]
    fn a_drawing_missing_from_the_package_leaves_the_sheet_unknown() {
        for target in ["../drawings/drawing2.xml", "https://example.com/d.xml"] {
            let entries = [
                (SHEET, String::new()),
                (SHEET_RELS, rels(&relationship(1, "drawing", target))),
                (DRAWING, String::new()),
            ];
            assert_eq!(
                links_of(&entries, &[SHEET]),
                [SheetLinks::Unknown],
                "{target}"
            );
        }
    }

    #[test]
    fn oddly_spelled_drawing_targets_are_still_followed() {
        for target in [
            "..\\drawings\\drawing1.xml",
            "../drawings/drawing%31.xml",
            "../drawings/drawing1.xml#shape",
        ] {
            let entries = [
                (SHEET, String::new()),
                (SHEET_RELS, rels(&relationship(1, "drawing", target))),
                (DRAWING, String::new()),
                (
                    DRAWING_RELS,
                    rels(&relationship(1, "chart", "../charts/chart1.xml")),
                ),
            ];
            assert_eq!(
                links_of(&entries, &[SHEET]),
                [known(&[SheetLink::Drawing, SheetLink::Chart])],
                "{target}"
            );
        }
    }

    // Readers disagree on which of two entries of one name they take.
    #[test]
    fn relationships_stored_twice_leave_the_sheet_unknown() {
        for second in [SHEET_RELS, "XL/Worksheets/_rels/SHEET1.xml.rels"] {
            let entries = [
                (SHEET, String::new()),
                (SHEET_RELS, rels("")),
                (second, rels(&relationship(1, "pivotTable", "../p/p1.xml"))),
            ];
            assert_eq!(
                links_of(&entries, &[SHEET]),
                [SheetLinks::Unknown],
                "{second}"
            );
        }
    }

    #[test]
    fn targets_resolve_against_the_source_folder() {
        assert_eq!(
            resolve(SHEET, "../drawings/./drawing1.xml").as_deref(),
            Some("xl/drawings/drawing1.xml")
        );
        assert_eq!(resolve(SHEET, "../../../outside.xml"), None);
        assert_eq!(
            resolve(SHEET, "..\\Drawings\\a%20b.xml#x").as_deref(),
            Some("xl/drawings/a b.xml")
        );
        assert_eq!(
            resolve(SHEET, "%2E%2E/%zz.xml").as_deref(),
            Some("xl/%zz.xml")
        );
        assert_eq!(resolve(SHEET, "../%FF.xml"), None);
        assert_eq!(
            relationships_part(SHEET),
            "xl/worksheets/_rels/sheet1.xml.rels"
        );
    }
}
