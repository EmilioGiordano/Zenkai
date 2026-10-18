use std::io::{self, Read, Seek};

use crate::ReadError;
use crate::package::read_entry;
use crate::stream::{Budgets, CompressedMeter};

// IronCalc's importer reads the styles from this path whatever the relationships say.
pub const STYLES_PART: &str = "xl/styles.xml";

// Whether new items can be added to the lists a save appends to without rewriting the
// rest of the part: an unprefixed styleSheet root, and each of those lists once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StylesShape {
    Appendable,
    Opaque,
}

// The elements of the styles part a cell's s="" and a rule's dxfId="" count into. A sheet
// written by the engine points into the copied part only when the model holds as many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleCounts {
    pub num_fmts: usize,
    pub fonts: usize,
    pub fills: usize,
    pub borders: usize,
    pub cell_style_xfs: usize,
    pub cell_xfs: usize,
    pub cell_styles: usize,
    pub dxfs: usize,
    pub shape: StylesShape,
}

// The elements to add at the end of each list, written as the part writes them. `after`
// is what the part holds once they are in.
#[derive(Clone, Debug)]
pub struct StyleAppendix {
    pub after: StyleCounts,
    pub num_fmts: String,
    pub fonts: String,
    pub fills: String,
    pub borders: String,
    pub cell_xfs: String,
}

pub(crate) fn style_counts<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
    budgets: &Budgets,
) -> Result<StyleCounts, ReadError> {
    let unreadable = |e: &dyn std::fmt::Display| ReadError::Unreadable(e.to_string());
    let entry = archive.by_name(STYLES_PART).map_err(|e| unreadable(&e))?;
    let bytes = read_entry(entry, meter, budgets)?;
    let text = std::str::from_utf8(&bytes).map_err(|e| unreadable(&e))?;
    counts_of(text)
}

const LISTS: [(&str, &str); 8] = [
    ("numFmts", "numFmt"),
    ("fonts", "font"),
    ("fills", "fill"),
    ("borders", "border"),
    ("cellStyleXfs", "xf"),
    ("cellXfs", "xf"),
    ("cellStyles", "cellStyle"),
    ("dxfs", "dxf"),
];

fn lists<'a, 'i>(
    root: roxmltree::Node<'a, 'i>,
    name: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    root.children().filter(move |node| node.has_tag_name(name))
}

fn counts_of(text: &str) -> Result<StyleCounts, ReadError> {
    let document =
        roxmltree::Document::parse(text).map_err(|e| ReadError::Unreadable(e.to_string()))?;
    let root = document.root_element();
    let count = |(list, item): (&'static str, &str)| {
        lists(root, list).next().map_or(0, |node| {
            node.children().filter(|c| c.has_tag_name(item)).count()
        })
    };
    let unprefixed = |node: roxmltree::Node, name: &str| {
        text[node.range()]
            .strip_prefix('<')
            .and_then(|rest| rest.strip_prefix(name))
            .is_some_and(|rest| {
                rest.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace())
            })
    };
    let once = |name: &'static str| lists(root, name).count() == 1;
    let appendable = unprefixed(root, "styleSheet")
        && ["fonts", "fills", "borders", "cellXfs"]
            .iter()
            .all(|&name| once(name) && lists(root, name).all(|node| unprefixed(node, name)))
        && lists(root, "numFmts").count() <= 1
        && lists(root, "numFmts").all(|node| unprefixed(node, "numFmts"));
    let [
        num_fmts,
        fonts,
        fills,
        borders,
        cell_style_xfs,
        cell_xfs,
        cell_styles,
        dxfs,
    ] = LISTS.map(count);
    Ok(StyleCounts {
        num_fmts,
        fonts,
        fills,
        borders,
        cell_style_xfs,
        cell_xfs,
        cell_styles,
        dxfs,
        shape: match appendable {
            true => StylesShape::Appendable,
            false => StylesShape::Opaque,
        },
    })
}

// The end of the start tag at the beginning of `element`, past any '>' inside a quoted
// attribute value.
fn start_tag_end(element: &str) -> Option<usize> {
    let mut quote = None;
    for (at, character) in element.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), found) if open == found => quote = None,
            (None, '>') => return Some(at + 1),
            _ => {}
        }
    }
    None
}

// The start tag with its count attribute set, when it has one.
fn with_count(start_tag: &str, total: usize) -> String {
    for quote in ['"', '\''] {
        let marker = format!("count={quote}");
        let Some(at) = start_tag.find(&marker) else {
            continue;
        };
        let value = at + marker.len();
        let preceded = start_tag[..at].ends_with(|c: char| c.is_whitespace());
        if let (true, Some(length)) = (preceded, start_tag[value..].find(quote)) {
            return format!(
                "{}{total}{}",
                &start_tag[..value],
                &start_tag[value + length..]
            );
        }
    }
    start_tag.to_string()
}

struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

fn unusable(reason: &str) -> io::Error {
    io::Error::other(format!("the styles part cannot take new styles: {reason}"))
}

// `text`, the styles part as it was read with `expected`, with the appended elements
// before each list's end tag and the counts to match.
pub(crate) fn appended(
    text: &str,
    expected: StyleCounts,
    appendix: &StyleAppendix,
) -> io::Result<String> {
    let found = counts_of(text).map_err(|e| unusable(&e.to_string()))?;
    if found != expected || found.shape != StylesShape::Appendable {
        return Err(unusable("it is not the one it was read with"));
    }
    let document = roxmltree::Document::parse(text).map_err(|e| unusable(&e.to_string()))?;
    let root = document.root_element();
    let after = appendix.after;
    let additions = [
        ("numFmts", &appendix.num_fmts, after.num_fmts),
        ("fonts", &appendix.fonts, after.fonts),
        ("fills", &appendix.fills, after.fills),
        ("borders", &appendix.borders, after.borders),
        ("cellXfs", &appendix.cell_xfs, after.cell_xfs),
    ];
    let mut edits = Vec::new();
    for (name, items, total) in additions {
        if items.is_empty() {
            continue;
        }
        let Some(list) = lists(root, name).next() else {
            let fonts = lists(root, "fonts")
                .next()
                .ok_or_else(|| unusable("it has no fonts"))?;
            edits.push(Edit {
                start: fonts.range().start,
                end: fonts.range().start,
                replacement: format!("<{name} count=\"{total}\">{items}</{name}>"),
            });
            continue;
        };
        let range = list.range();
        let element = &text[range.clone()];
        let tag_end = start_tag_end(element).ok_or_else(|| unusable("a list is cut short"))?;
        let start_tag = &element[..tag_end];
        match start_tag.strip_suffix("/>") {
            Some(open) => edits.push(Edit {
                start: range.start,
                end: range.end,
                replacement: format!("{}>{items}</{name}>", with_count(open, total)),
            }),
            None => {
                let end_tag = element
                    .rfind("</")
                    .filter(|&at| at >= tag_end)
                    .ok_or_else(|| unusable("a list has no end tag"))?;
                edits.push(Edit {
                    start: range.start + end_tag,
                    end: range.start + end_tag,
                    replacement: items.clone(),
                });
                edits.push(Edit {
                    start: range.start,
                    end: range.start + tag_end,
                    replacement: with_count(start_tag, total),
                });
            }
        }
    }
    edits.sort_by_key(|edit| std::cmp::Reverse((edit.start, edit.end)));
    let mut result = text.to_string();
    for edit in edits {
        result.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PART: &str = r#"<?xml version="1.0"?><styleSheet xmlns="m"><numFmts count="1"><numFmt numFmtId="164" formatCode="0.0"/></numFmts><fonts count="1"><font/></fonts><fills count="2"><fill/><fill/></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="1"><xf a=">"/></cellXfs><cellStyles count="1"><cellStyle name="Normal"/></cellStyles><dxfs count="0"/></styleSheet>"#;

    fn appendix(counts: StyleCounts) -> StyleAppendix {
        StyleAppendix {
            after: StyleCounts {
                num_fmts: counts.num_fmts + 1,
                fonts: counts.fonts + 1,
                fills: counts.fills + 1,
                borders: counts.borders + 1,
                cell_xfs: counts.cell_xfs + 2,
                ..counts
            },
            num_fmts: "<numFmt numFmtId=\"165\" formatCode=\"0\"/>".to_string(),
            fonts: "<font><b/></font>".to_string(),
            fills: "<fill>x</fill>".to_string(),
            borders: "<border>y</border>".to_string(),
            cell_xfs: "<xf b=\"1\"/><xf b=\"2\"/>".to_string(),
        }
    }

    #[test]
    fn items_are_added_before_each_end_tag_and_the_counts_follow() {
        let counts = counts_of(PART).unwrap();
        assert_eq!(counts.shape, StylesShape::Appendable);
        assert_eq!((counts.fills, counts.cell_xfs, counts.dxfs), (2, 1, 0));
        let grown = appended(PART, counts, &appendix(counts)).unwrap();
        assert!(grown.contains(
            r#"<numFmts count="2"><numFmt numFmtId="164" formatCode="0.0"/><numFmt numFmtId="165" formatCode="0"/></numFmts>"#
        ));
        assert!(grown.contains(r#"<fonts count="2"><font/><font><b/></font></fonts>"#));
        assert!(grown.contains(r#"<fills count="3"><fill/><fill/><fill>x</fill></fills>"#));
        assert!(grown.contains(r#"<borders count="2"><border/><border>y</border></borders>"#));
        assert!(
            grown.contains(r#"<cellXfs count="3"><xf a=">"/><xf b="1"/><xf b="2"/></cellXfs>"#)
        );
        assert!(grown.contains(r#"<dxfs count="0"/></styleSheet>"#));
        let after = counts_of(&grown).unwrap();
        assert_eq!(after, appendix(counts).after);
    }

    #[test]
    fn missing_and_self_closing_lists_are_made() {
        let part = r#"<styleSheet xmlns="m"><fonts count="0"/><fills/><borders count='0'></borders><cellXfs count="0"/></styleSheet>"#;
        let counts = counts_of(part).unwrap();
        let grown = appended(part, counts, &appendix(counts)).unwrap();
        assert!(grown.starts_with(
            r#"<styleSheet xmlns="m"><numFmts count="1"><numFmt numFmtId="165" formatCode="0"/></numFmts><fonts count="1"><font><b/></font></fonts><fills>"#
        ));
        assert!(grown.contains(r#"<fills><fill>x</fill></fills>"#));
        assert!(grown.contains(r#"<borders count='1'><border>y</border></borders>"#));
        assert!(grown.contains(r#"<cellXfs count="2"><xf b="1"/><xf b="2"/></cellXfs>"#));
    }

    #[test]
    fn a_part_that_cannot_take_items_is_refused() {
        let counts = counts_of(PART).unwrap();
        let other = appendix(counts);
        let prefixed = PART
            .replace("<styleSheet xmlns=\"m\">", "<x:styleSheet xmlns:x=\"m\">")
            .replace("</styleSheet>", "</x:styleSheet>");
        assert_eq!(counts_of(&prefixed).unwrap().shape, StylesShape::Opaque);
        let twice = PART.replace("<dxfs count=\"0\"/>", "<fonts count=\"0\"/>");
        assert_eq!(counts_of(&twice).unwrap().shape, StylesShape::Opaque);
        let no_cell_xfs = PART.replace("cellXfs", "cellXfz");
        assert_eq!(counts_of(&no_cell_xfs).unwrap().shape, StylesShape::Opaque);
        assert!(appended(&prefixed, counts, &other).is_err());
        let changed = PART.replace("<font/>", "<font/><font/>");
        assert!(appended(&changed, counts, &other).is_err());
        assert!(appended("<styleSheet", counts, &other).is_err());
    }

    #[test]
    fn a_greater_than_sign_in_an_attribute_does_not_end_the_tag() {
        assert_eq!(start_tag_end(r#"<fonts a=">" count='2'>x"#), Some(23));
        assert_eq!(start_tag_end(r#"<fonts a=">"#), None);
        assert_eq!(
            with_count(r#"<fonts a="count=1" count="3">"#, 9),
            r#"<fonts a="count=1" count="9">"#
        );
        assert_eq!(with_count("<fonts>", 9), "<fonts>");
    }
}
