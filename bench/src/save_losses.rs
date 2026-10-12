use std::collections::BTreeSet;

use zenkai_engine::Unsupported;

// A loss and the warnings that tell the user about it before saving; a loss with no
// warning is a silent loss.
pub struct Loss {
    pub label: String,
    warned_by: &'static [Unsupported],
    not_content: bool,
}

impl Loss {
    fn new(label: &str, warned_by: &'static [Unsupported]) -> Loss {
        Loss {
            label: label.to_string(),
            warned_by,
            not_content: false,
        }
    }

    pub fn is_warned(&self, warned: &[Unsupported]) -> bool {
        self.not_content || self.warned_by.iter().any(|kind| warned.contains(kind))
    }
}

// Elements that describe the application that wrote the file rather than its content.
// workbookPr's one content flag, date1904, is checked as an attribute below.
const NOT_CONTENT: [(&str, &str); 2] = [
    ("fileVersion", "writer version (not content)"),
    ("workbookPr", "workbook flags (not content)"),
];

const DRAWN: &[Unsupported] = &[
    Unsupported::Images,
    Unsupported::Charts,
    Unsupported::Comments,
];

const BY_PART: [(&str, &str, &[Unsupported]); 8] = [
    ("xl/charts/", "charts", &[Unsupported::Charts]),
    ("xl/drawings/", "drawings", DRAWN),
    ("xl/media/", "images", &[Unsupported::Images]),
    ("xl/comments", "comments", &[Unsupported::Comments]),
    ("xl/tables/", "tables", &[Unsupported::Tables]),
    (
        "xl/pivottables/",
        "pivot tables",
        &[Unsupported::PivotTables],
    ),
    ("xl/vbaproject", "macros", &[Unsupported::Macros]),
    (
        "xl/externallinks/",
        "external links",
        &[Unsupported::ExternalLinks],
    ),
];

// Top-level elements of the workbook and worksheet parts, by local name.
const BY_ELEMENT: [(&str, &str, &[Unsupported]); 23] = [
    ("sheetPr", "sheet properties", &[]),
    ("sheetFormatPr", "default sizes", &[]),
    ("cols", "column widths", &[]),
    ("conditionalFormatting", "conditional formatting", &[]),
    (
        "dataValidations",
        "data validation",
        &[Unsupported::DataValidation],
    ),
    ("hyperlinks", "hyperlinks", &[Unsupported::Hyperlinks]),
    ("definedNames", "defined names", &[]),
    ("mergeCells", "merged cells", &[]),
    ("autoFilter", "autofilter", &[Unsupported::AutoFilter]),
    (
        "sheetProtection",
        "sheet protection",
        &[Unsupported::SheetProtection],
    ),
    ("printOptions", "print options", &[]),
    ("pageMargins", "page margins", &[]),
    ("pageSetup", "page setup", &[]),
    ("headerFooter", "header/footer", &[]),
    ("rowBreaks", "page breaks", &[Unsupported::PageBreaks]),
    ("colBreaks", "page breaks", &[Unsupported::PageBreaks]),
    ("drawing", "drawings", DRAWN),
    ("legacyDrawing", "drawings", DRAWN),
    ("legacyDrawingHF", "header images", DRAWN),
    ("tableParts", "tables", &[Unsupported::Tables]),
    ("extLst", "extensions", &[]),
    ("calcPr", "calculation settings", &[]),
    ("bookViews", "window settings", &[]),
];

// Attributes whose loss matters even when their element is written.
const BY_ATTRIBUTE: [(&str, &str, &[Unsupported]); 12] = [
    ("<tabColor", "tab colour", &[]),
    ("zoomScale=", "zoom", &[]),
    ("<pane ", "frozen panes", &[]),
    ("showGridLines=\"0\"", "hidden gridlines", &[]),
    ("customHeight=\"1\"", "row heights", &[]),
    ("hidden=\"1\"", "hidden rows/columns", &[]),
    (
        "outlineLevel=",
        "row and column grouping",
        &[Unsupported::Outline],
    ),
    (
        "collapsed=",
        "row and column grouping",
        &[Unsupported::Outline],
    ),
    ("rightToLeft=\"1\"", "right-to-left", &[]),
    ("date1904=\"1\"", "1904 date system", &[]),
    ("iterate=\"1\"", "iterative calculation", &[]),
    ("calcMode=\"manual\"", "manual calculation", &[]),
];

pub fn parts(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    (0..archive.len())
        .map(|i| {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_ascii_lowercase();
            let mut text = String::new();
            if name.ends_with(".xml") {
                std::io::Read::read_to_string(&mut entry, &mut text).map_err(|e| e.to_string())?;
            }
            Ok((name, text))
        })
        .collect()
}

// Local names of the root's children, read from the tags so a large sheet costs one scan.
fn top_level_elements(xml: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut depth = 0usize;
    let mut rest = xml;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if tag.starts_with('/') {
            depth = depth.saturating_sub(1);
            continue;
        }
        let name = tag
            .split(|c: char| c.is_ascii_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        let local = name.rsplit(':').next().unwrap_or_default();
        if depth == 1 {
            names.insert(local.to_string());
        }
        if !tag.ends_with('/') {
            depth += 1;
        }
    }
    names
}

fn is_layout_part(name: &str) -> bool {
    name == "xl/workbook.xml" || (name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
}

// What is present in the original and absent after Zenkai saves it: by part name, by
// top-level element of the workbook and worksheet parts, and by key attribute.
pub fn dropped(original: &[u8], saved: &[u8]) -> Result<Vec<Loss>, String> {
    let (before, after) = (parts(original)?, parts(saved)?);
    let has_name =
        |set: &[(String, String)], prefix: &str| set.iter().any(|(n, _)| n.starts_with(prefix));
    let elements = |set: &[(String, String)]| {
        set.iter()
            .filter(|(name, _)| is_layout_part(name))
            .flat_map(|(_, xml)| top_level_elements(xml))
            .collect::<BTreeSet<_>>()
    };
    let has_attribute = |set: &[(String, String)], marker: &str| {
        set.iter()
            .any(|(name, xml)| is_layout_part(name) && xml.contains(marker))
    };
    let mut lost: Vec<Loss> = BY_PART
        .iter()
        .filter(|(prefix, ..)| has_name(&before, prefix) && !has_name(&after, prefix))
        .map(|(_, label, warned_by)| Loss::new(label, warned_by))
        .collect();
    let (kept, had) = (elements(&after), elements(&before));
    for name in had.difference(&kept) {
        let known = BY_ELEMENT.iter().find(|(element, ..)| element == name);
        let not_content = NOT_CONTENT.iter().find(|(element, _)| element == name);
        lost.push(match (known, not_content) {
            (Some((_, label, warned_by)), _) => Loss::new(label, warned_by),
            (None, Some((_, label))) => Loss {
                not_content: true,
                ..Loss::new(label, &[])
            },
            (None, None) => Loss::new(&format!("<{name}>"), &[]),
        });
    }
    lost.extend(
        BY_ATTRIBUTE
            .iter()
            .filter(|(marker, ..)| has_attribute(&before, marker) && !has_attribute(&after, marker))
            .map(|(_, label, warned_by)| Loss::new(label, warned_by)),
    );
    let mut seen = BTreeSet::new();
    lost.retain(|loss| seen.insert(loss.label.clone()));
    Ok(lost)
}

#[cfg(test)]
mod tests {
    use super::top_level_elements;

    #[test]
    fn top_level_elements_skip_nested_ones() {
        let xml = r#"<?xml version="1.0"?><x:worksheet xmlns:x="m"><x:sheetPr><x:tabColor rgb="FF000000"/></x:sheetPr><x:sheetData><x:row r="1"/></x:sheetData><x:pageMargins left="1"/></x:worksheet>"#;
        let names: Vec<String> = top_level_elements(xml).into_iter().collect();
        assert_eq!(names, ["pageMargins", "sheetData", "sheetPr"]);
    }
}
