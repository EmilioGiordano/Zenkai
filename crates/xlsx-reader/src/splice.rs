// Copies a worksheet part from the file on disk while replacing some of its rows: the
// rows outside the given spans are copied byte for byte, the rows inside are dropped and
// the caller writes the new ones in their place. Anything the copy cannot vouch for, from
// a prefixed <sheetData> to a comment between rows, is an error, never a guess.

use std::collections::HashSet;
use std::io::{self, Read, Write};

use ironcalc::base::expressions::utils::{is_valid_column_number, is_valid_row};

use crate::sheet_data::{cell_position, parse_range};
use crate::stream::CHUNK_BYTES;

const MAIN_NAMESPACE: &[u8] = b"http://schemas.openxmlformats.org/spreadsheetml/2006/main";
// A tag is held whole to read its name and attributes. Excel's longest are a few
// kilobytes (a <selection> listing many ranges); past this the part is not copied.
const MAX_TAG_BYTES: usize = 1024 * 1024;
// Excel caps a formula at 8,192 characters; escaped, one can take up to six bytes.
const MAX_FORMULA_BYTES: usize = 8192 * 6;
// Shared formulas dropped with their rows, whose `si` kept rows must no longer use; at
// four bytes each, 256 KiB. Past this many the part is not copied.
const MAX_DROPPED_GROUPS: usize = 64 * 1024;
// Shared formula indices are remembered in a bitmap, 16 Mi bits = 2 MiB. Excel numbers
// them from 0, so a larger one is not what it writes.
const MAX_SHARED_INDEX: i32 = 16 * 1024 * 1024;
// Formulas from dropped rows whose range reaches kept rows, each holding its text of at
// most MAX_FORMULA_BYTES: 256 * 48 KiB = 12 MiB at worst. Past this many in one span the
// part is not copied.
const MAX_REACHING_ANCHORS: usize = 256;

// Rows `first` to `last`, both included, numbered from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSpan {
    pub first: u32,
    pub last: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorKind {
    Shared { si: i32 },
    // An array or data table formula over its range.
    Ranged,
}

// A formula written over a range from a dropped row, where the range reaches kept rows
// that still depend on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReachingAnchor {
    pub row: u32,
    pub column: u32,
    pub last_row: u32,
    pub last_column: u32,
    pub kind: AnchorKind,
    // Unescaped.
    pub text: String,
}

fn unusable(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Markup {
    Start,
    Empty,
    End,
    // A comment or processing instruction.
    Other,
}

struct Input<R> {
    inner: R,
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

impl<R: Read> Input<R> {
    // At least `wanted` bytes, fewer only where the part ends.
    fn available(&mut self, wanted: usize) -> io::Result<&[u8]> {
        while self.end - self.start < wanted {
            if self.start > 0 {
                self.buf.copy_within(self.start..self.end, 0);
                self.end -= self.start;
                self.start = 0;
            }
            let read = self.inner.read(&mut self.buf[self.end..])?;
            if read == 0 {
                break;
            }
            self.end += read;
        }
        Ok(&self.buf[self.start..self.end])
    }

    fn consume(&mut self, count: usize) {
        self.start += count;
    }
}

// Whether `held` followed by `window` ends with `terminator`.
fn ends_across(held: &[u8], window: &[u8], terminator: &[u8]) -> bool {
    if window.len() >= terminator.len() {
        return window.ends_with(terminator);
    }
    let from_held = terminator.len() - window.len();
    held.len() >= from_held
        && held[held.len() - from_held..] == terminator[..from_held]
        && window == &terminator[from_held..]
}

struct Scanner<R> {
    input: Input<R>,
    tag: Vec<u8>,
}

impl<R: Read> Scanner<R> {
    // Character data up to the next '<', written to `out` when given. False at the end.
    fn text(&mut self, mut out: Option<&mut dyn Write>) -> io::Result<bool> {
        loop {
            let window = self.input.available(1)?;
            if window.is_empty() {
                return Ok(false);
            }
            let found = window.iter().position(|b| *b == b'<');
            let count = found.unwrap_or(window.len());
            if let Some(out) = out.as_deref_mut() {
                out.write_all(&window[..count])?;
            }
            self.input.consume(count);
            if found.is_some() {
                return Ok(true);
            }
        }
    }

    fn text_into(&mut self, held: &mut Vec<u8>) -> io::Result<()> {
        loop {
            let window = self.input.available(1)?;
            if window.is_empty() {
                return Err(unusable("the sheet ends inside a formula"));
            }
            let found = window.iter().position(|b| *b == b'<');
            let count = found.unwrap_or(window.len());
            if held.len() + count > MAX_FORMULA_BYTES {
                return Err(unusable("a formula is longer than Excel allows"));
            }
            held.extend_from_slice(&window[..count]);
            self.input.consume(count);
            if found.is_some() {
                return Ok(());
            }
        }
    }

    // Reads the markup that starts at the next byte, a '<', into `self.tag`.
    fn markup(&mut self) -> io::Result<Markup> {
        self.tag.clear();
        let head = self.input.available(4)?;
        // The terminator is searched after the opener, which it must not share bytes with:
        // `<!-->` and `<?>` stay open in an XML parser.
        let (kind, terminator, opener): (Markup, &[u8], usize) = if head.starts_with(b"<!--") {
            (Markup::Other, b"-->", 4)
        } else if head.starts_with(b"<?") {
            (Markup::Other, b"?>", 2)
        } else if head.starts_with(b"<!") {
            return Err(unusable("a DTD or CDATA section in a sheet"));
        } else if head.starts_with(b"</") {
            (Markup::End, b">", 0)
        } else if head.starts_with(b"<") {
            (Markup::Start, b">", 0)
        } else {
            return Err(unusable("the sheet ends where a tag was expected"));
        };
        // Only attribute values are quoted; a comment may hold a lone apostrophe.
        let honor_quotes = kind != Markup::Other;
        let mut quote: Option<u8> = None;
        loop {
            let window = self.input.available(1)?;
            if window.is_empty() {
                return Err(unusable("the sheet ends inside a tag"));
            }
            let mut finished = None;
            for (at, &byte) in window.iter().enumerate() {
                match quote {
                    Some(open) if open == byte => quote = None,
                    Some(_) => {}
                    None if honor_quotes && matches!(byte, b'"' | b'\'') => quote = Some(byte),
                    None if byte == b'>'
                        && self.tag.len() + at + 1 >= opener + terminator.len()
                        && ends_across(&self.tag, &window[..=at], terminator) =>
                    {
                        finished = Some(at + 1);
                        break;
                    }
                    None => {}
                }
            }
            let count = finished.unwrap_or(window.len());
            if self.tag.len() + count > MAX_TAG_BYTES {
                return Err(unusable("a tag is longer than a sheet holds"));
            }
            self.tag.extend_from_slice(&window[..count]);
            self.input.consume(count);
            if finished.is_some() {
                break;
            }
        }
        if kind == Markup::Start && self.tag.ends_with(b"/>") {
            return Ok(Markup::Empty);
        }
        Ok(kind)
    }

    fn rest(mut self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(&self.input.buf[self.input.start..self.input.end])?;
        io::copy(&mut self.input.inner, out)?;
        Ok(())
    }
}

fn tag_name(tag: &[u8]) -> &[u8] {
    let inner = tag
        .strip_prefix(b"</")
        .or_else(|| tag.strip_prefix(b"<"))
        .unwrap_or(tag);
    let end = inner
        .iter()
        .position(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'))
        .unwrap_or(inner.len());
    &inner[..end]
}

fn skip_space(tag: &[u8], mut at: usize) -> usize {
    while tag.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

// Where the raw value of attribute `name` lies in `tag`.
fn attribute_span(tag: &[u8], name: &[u8]) -> Option<(usize, usize)> {
    let opener = if tag.starts_with(b"</") { 2 } else { 1 };
    let mut at = opener + tag_name(tag).len();
    loop {
        at = skip_space(tag, at);
        let key_start = at;
        while tag
            .get(at)
            .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'/' | b'>'))
        {
            at += 1;
        }
        if at == key_start {
            return None;
        }
        let key = &tag[key_start..at];
        at = skip_space(tag, at);
        if tag.get(at) != Some(&b'=') {
            return None;
        }
        at = skip_space(tag, at + 1);
        let quote = *tag.get(at).filter(|b| matches!(b, b'"' | b'\''))?;
        let value_start = at + 1;
        let value_end = value_start + tag.get(value_start..)?.iter().position(|b| *b == quote)?;
        if key == name {
            return Some((value_start, value_end));
        }
        at = value_end + 1;
    }
}

// The value of a plain attribute, a reference or a number; one holding an entity is not
// what Excel writes there.
fn attribute<'t>(tag: &'t [u8], name: &[u8]) -> io::Result<Option<&'t str>> {
    let Some((start, end)) = attribute_span(tag, name) else {
        return Ok(None);
    };
    let value = std::str::from_utf8(&tag[start..end]).map_err(unusable)?;
    if value.contains('&') {
        return Err(unusable("an escaped reference in a sheet"));
    }
    Ok(Some(value))
}

fn to_row(row: i32) -> io::Result<u32> {
    if !is_valid_row(row) {
        return Err(unusable("a row outside the sheet"));
    }
    u32::try_from(row).map_err(unusable)
}

fn to_column(column: i32) -> io::Result<u32> {
    if !is_valid_column_number(column) {
        return Err(unusable("a column outside the sheet"));
    }
    u32::try_from(column).map_err(unusable)
}

// What an <f> start tag in a dropped row says about a range it covers.
struct RangedFormula {
    kind: AnchorKind,
    first: (u32, u32),
    last: (u32, u32),
}

fn ranged_formula(tag: &[u8]) -> io::Result<Option<RangedFormula>> {
    let Some(range) = attribute(tag, b"ref")? else {
        return Ok(None);
    };
    let kind = match attribute(tag, b"t")? {
        Some("shared") => {
            let si = attribute(tag, b"si")?
                .ok_or_else(|| unusable("a shared formula without si"))?
                .parse::<i32>()
                .map_err(unusable)?;
            AnchorKind::Shared { si }
        }
        Some("array" | "dataTable") => AnchorKind::Ranged,
        _ => return Ok(None),
    };
    let (row1, column1, row2, column2) =
        parse_range(range).ok_or_else(|| unusable("a formula range that does not read"))?;
    let first = (to_row(row1)?, to_column(column1)?);
    let last = (to_row(row2)?, to_column(column2)?);
    if last.0 < first.0 || last.1 < first.1 {
        return Err(unusable("a formula range that runs backwards"));
    }
    Ok(Some(RangedFormula { kind, first, last }))
}

// The `si` of every shared formula anchor read so far, in rows kept and dropped alike.
#[derive(Default)]
struct SeenAnchors {
    words: Vec<u64>,
}

impl SeenAnchors {
    fn slot(si: i32) -> io::Result<(usize, u64)> {
        if !(0..MAX_SHARED_INDEX).contains(&si) {
            return Err(unusable("a shared formula index outside what Excel writes"));
        }
        let bit = si as usize;
        Ok((bit / 64, 1 << (bit % 64)))
    }

    fn insert(&mut self, si: i32) -> io::Result<()> {
        let (word, mask) = Self::slot(si)?;
        if self.words.len() <= word {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= mask;
        Ok(())
    }

    fn contains(&self, si: i32) -> io::Result<bool> {
        let (word, mask) = Self::slot(si)?;
        Ok(self.words.get(word).is_some_and(|bits| bits & mask != 0))
    }
}

struct Splice<'s, F> {
    spans: &'s [RowSpan],
    next: usize,
    reaching: Vec<ReachingAnchor>,
    seen_anchors: SeenAnchors,
    dropped_groups: HashSet<i32>,
    last_row: u32,
    write_span: F,
}

impl<F> Splice<'_, F> {
    fn covered(&self, first: u32, last: u32) -> bool {
        let mut row = first;
        for span in self.spans {
            if span.last < row {
                continue;
            }
            if span.first > row {
                return false;
            }
            if span.last >= last {
                return true;
            }
            row = span.last + 1;
        }
        false
    }

    // Writes every span that ends before `row`.
    fn flush_before<W: Write>(&mut self, row: u32, out: &mut W) -> io::Result<()>
    where
        F: FnMut(&RowSpan, &[ReachingAnchor], &mut W) -> io::Result<()>,
    {
        let spans = self.spans;
        while let Some(span) = spans.get(self.next).filter(|span| span.last < row) {
            (self.write_span)(span, &self.reaching, out)?;
            self.reaching.clear();
            self.next += 1;
        }
        Ok(())
    }

    fn flush_all<W: Write>(&mut self, out: &mut W) -> io::Result<()>
    where
        F: FnMut(&RowSpan, &[ReachingAnchor], &mut W) -> io::Result<()>,
    {
        let spans = self.spans;
        for span in &spans[self.next..] {
            (self.write_span)(span, &self.reaching, out)?;
            self.reaching.clear();
        }
        self.next = spans.len();
        Ok(())
    }

    fn dropping(&self, row: u32) -> bool {
        self.spans
            .get(self.next)
            .is_some_and(|span| span.first <= row)
    }

    // Returns whether the formula's end tag was read too.
    fn dropped_formula<R: Read>(
        &mut self,
        scan: &mut Scanner<R>,
        cell: Option<(u32, u32)>,
        empty: bool,
    ) -> io::Result<bool> {
        let Some(RangedFormula { kind, first, last }) = ranged_formula(&scan.tag)? else {
            return Ok(false);
        };
        if let AnchorKind::Shared { si } = kind {
            self.seen_anchors.insert(si)?;
        }
        if self.covered(first.0, last.0) {
            if let AnchorKind::Shared { si } = kind {
                if self.dropped_groups.len() >= MAX_DROPPED_GROUPS {
                    return Err(unusable("too many shared formulas in the rows written again"));
                }
                self.dropped_groups.insert(si);
            }
            return Ok(false);
        }
        if cell != Some(first) {
            return Err(unusable("a formula range that does not start at its cell"));
        }
        if self.reaching.len() >= MAX_REACHING_ANCHORS {
            return Err(unusable("too many formulas reach the rows kept"));
        }
        let mut text = Vec::new();
        if !empty {
            scan.text_into(&mut text)?;
            if scan.markup()? != Markup::End || tag_name(&scan.tag) != b"f" {
                return Err(unusable("a formula holds more than its text"));
            }
        }
        let text = std::str::from_utf8(&text).map_err(unusable)?;
        let text = quick_xml::escape::unescape(text).map_err(unusable)?;
        self.reaching.push(ReachingAnchor {
            row: first.0,
            column: first.1,
            last_row: last.0,
            last_column: last.1,
            kind,
            text: text.into_owned(),
        });
        Ok(!empty)
    }

    fn drop_row<R: Read>(&mut self, scan: &mut Scanner<R>) -> io::Result<()> {
        let mut depth = 0usize;
        let mut cell = None;
        loop {
            if !scan.text(None)? {
                return Err(unusable("the sheet ends inside a row"));
            }
            let kind = scan.markup()?;
            let name = tag_name(&scan.tag);
            let (is_cell, is_formula, is_row) = (name == b"c", name == b"f", name == b"row");
            match kind {
                Markup::Other => return Err(unusable("a comment inside a row")),
                Markup::End if depth == 0 && is_row => return Ok(()),
                Markup::End if depth == 0 => return Err(unusable("a row is not closed")),
                Markup::End => depth -= 1,
                Markup::Start | Markup::Empty => {
                    if is_cell {
                        cell = match attribute(&scan.tag, b"r")?.and_then(cell_position) {
                            Some((row, column)) => Some((to_row(row)?, to_column(column)?)),
                            None => None,
                        };
                    }
                    let closed = is_formula
                        && self.dropped_formula(scan, cell, kind == Markup::Empty)?;
                    if kind == Markup::Start && !closed {
                        depth += 1;
                    }
                }
            }
        }
    }

    fn copy_row<R: Read, W: Write>(
        &mut self,
        scan: &mut Scanner<R>,
        out: &mut W,
    ) -> io::Result<()> {
        let mut depth = 0usize;
        loop {
            if !scan.text(Some(&mut *out as &mut dyn Write))? {
                return Err(unusable("the sheet ends inside a row"));
            }
            let kind = scan.markup()?;
            match kind {
                Markup::Other => return Err(unusable("a comment inside a row")),
                Markup::End if depth == 0 => {
                    if tag_name(&scan.tag) != b"row" {
                        return Err(unusable("a row is not closed"));
                    }
                    out.write_all(&scan.tag)?;
                    return Ok(());
                }
                Markup::End => {
                    out.write_all(&scan.tag)?;
                    depth -= 1;
                }
                Markup::Start | Markup::Empty => {
                    if tag_name(&scan.tag) == b"f" {
                        self.check_shared(&scan.tag)?;
                    }
                    out.write_all(&scan.tag)?;
                    if kind == Markup::Start {
                        depth += 1;
                    }
                }
            }
        }
    }

    // A kept cell must not take a shared formula whose anchor was dropped or has not been
    // read yet: readers resolve a follower by si alone, so a later anchor could be one
    // that the written rows replace.
    fn check_shared(&mut self, tag: &[u8]) -> io::Result<()> {
        if attribute(tag, b"t")? != Some("shared") {
            return Ok(());
        }
        let Some(si) = attribute(tag, b"si")? else {
            return Ok(());
        };
        let si = si.parse::<i32>().map_err(unusable)?;
        if attribute(tag, b"ref")?.is_some() {
            return self.seen_anchors.insert(si);
        }
        if !self.seen_anchors.contains(si)? {
            return Err(unusable(
                "a kept cell takes a shared formula whose anchor was not read before it",
            ));
        }
        if self.dropped_groups.contains(&si) {
            return Err(unusable(
                "a kept cell takes a shared formula that is written again",
            ));
        }
        Ok(())
    }

    fn row_number(&mut self, tag: &[u8]) -> io::Result<u32> {
        let row = attribute(tag, b"r")?
            .ok_or_else(|| unusable("a row without its number"))?
            .parse::<i32>()
            .map_err(unusable)?;
        let row = to_row(row)?;
        if row <= self.last_row {
            return Err(unusable("rows out of order"));
        }
        self.last_row = row;
        Ok(row)
    }

    fn rows<R: Read, W: Write>(&mut self, scan: &mut Scanner<R>, out: &mut W) -> io::Result<()>
    where
        F: FnMut(&RowSpan, &[ReachingAnchor], &mut W) -> io::Result<()>,
    {
        loop {
            if !scan.text(Some(&mut *out as &mut dyn Write))? {
                return Err(unusable("the sheet data is not closed"));
            }
            let kind = scan.markup()?;
            let name = tag_name(&scan.tag);
            let (is_row, is_sheet_data) = (name == b"row", name == b"sheetData");
            match kind {
                Markup::Other => return Err(unusable("a comment between rows")),
                Markup::End if is_sheet_data => {
                    self.flush_all(out)?;
                    out.write_all(&scan.tag)?;
                    return Ok(());
                }
                Markup::End => return Err(unusable("an unexpected end tag between rows")),
                Markup::Start | Markup::Empty if is_row => {
                    let row = self.row_number(&scan.tag)?;
                    self.flush_before(row, out)?;
                    if self.dropping(row) {
                        if kind == Markup::Start {
                            self.drop_row(scan)?;
                        }
                    } else {
                        out.write_all(&scan.tag)?;
                        if kind == Markup::Start {
                            self.copy_row(scan, out)?;
                        }
                    }
                }
                Markup::Start | Markup::Empty => {
                    return Err(unusable("an element other than a row in the sheet data"));
                }
            }
        }
    }
}

enum SheetData {
    Open,
    Empty,
}

// Copies everything before <sheetData> with the dimension replaced. The rows written in
// place are unprefixed, so the root must make the main namespace the default one.
fn head<R: Read>(
    scan: &mut Scanner<R>,
    out: &mut dyn Write,
    dimension: &str,
) -> io::Result<SheetData> {
    let mut depth = 0usize;
    loop {
        if !scan.text(Some(&mut *out))? {
            return Err(unusable("the sheet has no sheet data"));
        }
        let kind = scan.markup()?;
        let name = tag_name(&scan.tag);
        let (is_root, is_sheet_data, is_dimension) = (
            name == b"worksheet",
            name == b"sheetData",
            name == b"dimension",
        );
        let prefixed_data = name.ends_with(b":sheetData");
        match kind {
            Markup::Other => out.write_all(&scan.tag)?,
            Markup::End if depth <= 1 => return Err(unusable("the sheet has no sheet data")),
            Markup::End => {
                out.write_all(&scan.tag)?;
                depth -= 1;
            }
            Markup::Start | Markup::Empty if depth == 0 => {
                let main = attribute_span(&scan.tag, b"xmlns")
                    .is_some_and(|(start, end)| &scan.tag[start..end] == MAIN_NAMESPACE);
                if !is_root || !main || kind == Markup::Empty {
                    return Err(unusable("the sheet's root is not a plain worksheet"));
                }
                out.write_all(&scan.tag)?;
                depth = 1;
            }
            Markup::Start | Markup::Empty if is_sheet_data => {
                let plain = matches!(scan.tag.as_slice(), b"<sheetData>" | b"<sheetData/>");
                if depth != 1 || !plain {
                    return Err(unusable("the sheet data is not where a worksheet keeps it"));
                }
                return Ok(match kind {
                    Markup::Start => SheetData::Open,
                    _ => SheetData::Empty,
                });
            }
            Markup::Start | Markup::Empty if prefixed_data => {
                return Err(unusable("prefixed sheet data"));
            }
            Markup::Start | Markup::Empty => {
                match attribute_span(&scan.tag, b"ref").filter(|_| is_dimension && depth == 1) {
                    Some((start, end)) => {
                        out.write_all(&scan.tag[..start])?;
                        out.write_all(dimension.as_bytes())?;
                        out.write_all(&scan.tag[end..])?;
                    }
                    None => out.write_all(&scan.tag)?,
                }
                if kind == Markup::Start {
                    depth += 1;
                }
            }
        }
    }
}

// `spans` must be sorted and apart. `write_span` writes the rows of each span, once per
// span and in order, given the formulas of its dropped rows whose range reaches kept rows.
pub(crate) fn splice_rows<R: Read, W: Write>(
    part: R,
    out: &mut W,
    dimension: &str,
    spans: &[RowSpan],
    write_span: impl FnMut(&RowSpan, &[ReachingAnchor], &mut W) -> io::Result<()>,
) -> io::Result<()> {
    let sorted = spans.iter().all(|span| span.first <= span.last)
        && spans.windows(2).all(|pair| pair[0].last < pair[1].first);
    if !sorted {
        return Err(io::Error::other("row spans out of order"));
    }
    let mut scan = Scanner {
        input: Input {
            inner: part,
            buf: vec![0; CHUNK_BYTES],
            start: 0,
            end: 0,
        },
        tag: Vec::new(),
    };
    let form = head(&mut scan, &mut *out, dimension)?;
    let mut splice = Splice {
        spans,
        next: 0,
        reaching: Vec::new(),
        seen_anchors: SeenAnchors::default(),
        dropped_groups: HashSet::new(),
        last_row: 0,
        write_span,
    };
    out.write_all(b"<sheetData>")?;
    match form {
        SheetData::Open => splice.rows(&mut scan, out)?,
        SheetData::Empty => {
            splice.flush_all(out)?;
            out.write_all(b"</sheetData>")?;
        }
    }
    scan.rest(&mut *out)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const ROOT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:x14ac="http://schemas.microsoft.com/office/spreadsheetml/2009/9/ac">"#;

    fn sheet(rows: &str) -> String {
        format!(
            r#"{ROOT}<dimension ref="A1:B9"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetData>{rows}</sheetData><mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#
        )
    }

    // Reads one byte at a time, so every token straddles a refill.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some((first, rest)) = self.0.split_first() else {
                return Ok(0);
            };
            if buf.is_empty() {
                return Ok(0);
            }
            buf[0] = *first;
            self.0 = rest;
            Ok(1)
        }
    }

    type Calls = Vec<(RowSpan, Vec<ReachingAnchor>)>;

    fn spliced_with(
        part: impl Read,
        spans: &[RowSpan],
        rows: &dyn Fn(&RowSpan) -> String,
    ) -> io::Result<(String, Calls)> {
        let mut calls = Vec::new();
        let mut out = Vec::new();
        splice_rows(part, &mut out, "A1:C12", spans, |span, anchors, out: &mut Vec<u8>| {
            calls.push((*span, anchors.to_vec()));
            out.write_all(rows(span).as_bytes())
        })?;
        Ok((String::from_utf8(out).unwrap(), calls))
    }

    fn spliced(xml: &str, spans: &[RowSpan]) -> io::Result<(String, Calls)> {
        let rows = |span: &RowSpan| {
            (span.first..=span.last).fold(String::new(), |mut rows, row| {
                write!(rows, r#"<row r="{row}"><c r="A{row}"><v>0</v></c></row>"#).unwrap();
                rows
            })
        };
        let whole = spliced_with(xml.as_bytes(), spans, &rows)?;
        let trickled = spliced_with(Trickle(xml.as_bytes()), spans, &rows)?;
        assert_eq!(whole, trickled, "the result depends on the read sizes");
        Ok(whole)
    }

    fn span(first: u32, last: u32) -> RowSpan {
        RowSpan { first, last }
    }

    fn row(r: u32, cells: &str) -> String {
        format!(r#"<row r="{r}" spans="1:2" x14ac:dyDescent="0.25">{cells}</row>"#)
    }

    #[test]
    fn kept_rows_are_copied_byte_for_byte_and_spans_written_in_place() {
        let rows = [
            row(1, r#"<c r="A1" t="s"><v>0</v></c>"#),
            row(2, r#"<c r="A2"><v>2</v></c>"#),
            r#"<row r="3"/>"#.to_string(),
            row(5, r#"<c r="A5" t="str"><v>a &gt; b</v></c><c r="B5" s="1"/>"#),
            row(9, r#"<c r="A9"><v>9</v></c>"#),
        ];
        let xml = sheet(&rows.concat());
        let (out, calls) = spliced(&xml, &[span(2, 3), span(7, 7), span(12, 12)]).unwrap();
        let expected = sheet(&format!(
            r#"{}<row r="2"><c r="A2"><v>0</v></c></row><row r="3"><c r="A3"><v>0</v></c></row>{}<row r="7"><c r="A7"><v>0</v></c></row>{}<row r="12"><c r="A12"><v>0</v></c></row>"#,
            rows[0], rows[3], rows[4]
        ))
        .replace(r#"<dimension ref="A1:B9"/>"#, r#"<dimension ref="A1:C12"/>"#);
        assert_eq!(out, expected);
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|(_, anchors)| anchors.is_empty()));
    }

    #[test]
    fn an_empty_sheet_data_takes_the_written_rows() {
        let xml = format!(r#"{ROOT}<sheetData/></worksheet>"#);
        let (out, _) = spliced(&xml, &[span(4, 4)]).unwrap();
        assert_eq!(
            out,
            format!(
                r#"{ROOT}<sheetData><row r="4"><c r="A4"><v>0</v></c></row></sheetData></worksheet>"#
            )
        );
    }

    #[test]
    fn a_shared_formula_reaching_kept_rows_is_handed_over_unescaped() {
        let rows = [
            row(
                2,
                r#"<c r="B2"><f t="shared" ref="B2:C6" si="3">A2&amp;"&lt;"</f><v>1</v></c><c r="D2"><f t="shared" ref="D2:D3" si="4">A2</f><v>1</v></c>"#,
            ),
            row(3, r#"<c r="D3"><f t="shared" si="4"/><v>1</v></c>"#),
            row(4, r#"<c r="B4"><f t="shared" si="3"/><v>1</v></c>"#),
        ];
        let (out, calls) = spliced(&sheet(&rows.concat()), &[span(2, 3)]).unwrap();
        assert!(out.contains(&rows[2]));
        assert_eq!(
            calls[0].1,
            [ReachingAnchor {
                row: 2,
                column: 2,
                last_row: 6,
                last_column: 3,
                kind: AnchorKind::Shared { si: 3 },
                text: "A2&\"<\"".to_string(),
            }]
        );
    }

    #[test]
    fn a_kept_cell_taking_a_dropped_shared_formula_is_refused() {
        // The anchor's range says it ends in row 3, yet row 4 takes it: readers resolve
        // a shared formula by si alone.
        let rows = [
            row(2, r#"<c r="B2"><f t="shared" ref="B2:B3" si="0">A2</f><v>1</v></c>"#),
            row(4, r#"<c r="B4"><f t="shared" si="0"/><v>1</v></c>"#),
        ];
        let error = spliced(&sheet(&rows.concat()), &[span(2, 3)]).unwrap_err();
        assert!(error.to_string().contains("shared formula"), "{error}");
    }

    #[test]
    fn a_kept_follower_needs_its_anchor_read_before_it() {
        let anchor = row(1, r#"<c r="B1"><f t="shared" ref="B1:B3" si="0">A1</f><v>1</v></c>"#);
        let follower = row(2, r#"<c r="B2"><f t="shared" si="0"/><v>1</v></c>"#);
        let (out, _) = spliced(&sheet(&format!("{anchor}{follower}")), &[span(5, 5)]).unwrap();
        assert!(out.contains(&follower));
        let later = row(3, r#"<c r="B3"><f t="shared" ref="B3:B4" si="0">A3</f></c>"#);
        for rows in [
            follower.clone(),
            format!("{follower}{later}"),
            row(2, r#"<c r="B2"><f t="shared" si="1"/><v>1</v></c>"#),
        ] {
            let error = spliced(&sheet(&rows), &[span(5, 5)]).unwrap_err();
            assert!(error.to_string().contains("anchor"), "{error}");
        }
        let huge = row(1, r#"<c r="B1"><f t="shared" ref="B1:B3" si="99999999">A1</f></c>"#);
        assert!(spliced(&sheet(&huge), &[span(5, 5)]).is_err());
    }

    #[test]
    fn an_array_formula_reaching_kept_rows_is_handed_over() {
        let rows = [row(
            2,
            r#"<c r="B2"><f t="array" ref="B2:B5">A2:A5*2</f><v>1</v></c>"#,
        )];
        let (_, calls) = spliced(&sheet(&rows.concat()), &[span(2, 2)]).unwrap();
        assert_eq!(calls[0].1[0].kind, AnchorKind::Ranged);
        let (_, calls) = spliced(&sheet(&rows.concat()), &[span(2, 5)]).unwrap();
        assert!(calls[0].1.is_empty());
    }

    #[test]
    fn quoted_angle_brackets_and_comments_outside_the_rows_are_kept() {
        let xml = format!(
            r#"{ROOT}<!-- don't > stop --><sheetPr codeName="a>b"/><sheetData>{}</sheetData></worksheet>"#,
            row(1, r#"<c r="A1" t="inlineStr"><is><t>x</t></is></c>"#)
        );
        let (out, _) = spliced(&xml, &[]).unwrap();
        assert_eq!(out, xml);
    }

    #[test]
    fn a_comment_or_instruction_is_not_closed_by_its_own_opener() {
        let row_1 = r#"<row r="1"><c r="A1"><v>0</v></c></row>"#;
        for hidden in ["<!--> <sheetData/> -->", "<!---> <sheetData/> -->", "<?> <sheetData/> ?>"] {
            let xml = format!(r#"{ROOT}{hidden}<sheetData><row r="1"/></sheetData></worksheet>"#);
            let (out, _) = spliced(&xml, &[span(1, 1)]).unwrap();
            assert_eq!(
                out,
                format!("{ROOT}{hidden}<sheetData>{row_1}</sheetData></worksheet>")
            );
        }
        let closed = format!(r#"{ROOT}<?x?><!----><!-- a -- b ->--><sheetData/></worksheet>"#);
        let (out, _) = spliced(&closed, &[]).unwrap();
        assert_eq!(out, closed);
    }

    #[test]
    fn a_comment_ending_across_a_chunk_boundary_is_closed() {
        for ends_at in [CHUNK_BYTES - 1, CHUNK_BYTES, CHUNK_BYTES + 1] {
            let filler = "x".repeat(ends_at - ROOT.len() - 6);
            let xml = format!(r#"{ROOT}<!--{filler}--><sheetData/></worksheet>"#);
            let (out, _) = spliced(&xml, &[]).unwrap();
            assert_eq!(out, xml);
        }
    }

    #[test]
    fn a_part_the_copy_cannot_vouch_for_is_refused() {
        let cases = [
            sheet(&format!("{}{}", row(3, ""), row(2, ""))),
            sheet(r#"<row spans="1:2"><c r="A1"><v>1</v></c></row>"#),
            sheet(&format!("<!-- note -->{}", row(1, ""))),
            sheet(r#"<row r="1"><c r="A1"><v><![CDATA[<]]></v></c></row>"#),
            sheet(r#"<row r="2000000"/>"#),
            sheet(r#"<extLst/>"#),
            format!(r#"{ROOT}<x:sheetData xmlns:x="m"/></worksheet>"#),
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData/></x:worksheet>"#.to_string(),
            r#"<worksheet xmlns="urn:other"><sheetData/></worksheet>"#.to_string(),
            format!(r#"{ROOT}<sheetData>{}"#, row(1, "")),
            format!(r#"{ROOT}<sheetData><row r="1"><c r="A1"#),
            format!(r#"{ROOT}</worksheet>"#),
        ];
        for xml in cases {
            let refused = spliced_with(xml.as_bytes(), &[span(1, 1)], &|_| String::new());
            assert!(refused.is_err(), "{xml}");
        }
    }

    #[test]
    fn formulas_reaching_kept_rows_are_capped() {
        let reaching = |count: usize| {
            let rows: String = (1..=count)
                .map(|r| {
                    row(
                        r as u32,
                        &format!(r#"<c r="A{r}"><f t="array" ref="A{r}:A{}">1</f></c>"#, r + 5000),
                    )
                })
                .collect();
            let span = span(1, count as u32);
            spliced_with(sheet(&rows).as_bytes(), &[span], &|_| String::new())
        };
        assert!(reaching(MAX_REACHING_ANCHORS).is_ok());
        let error = reaching(MAX_REACHING_ANCHORS + 1).unwrap_err();
        assert!(error.to_string().contains("too many formulas"), "{error}");
    }

    #[test]
    fn hostile_sizes_are_refused_without_holding_them() {
        let long_tag = sheet(&format!(
            r#"<row r="1" x="{}"/>"#,
            "a".repeat(MAX_TAG_BYTES)
        ));
        assert!(spliced_with(long_tag.as_bytes(), &[], &|_| String::new()).is_err());
        let long_formula = sheet(&row(
            1,
            &format!(
                r#"<c r="A1"><f t="shared" ref="A1:A5" si="0">{}</f></c>"#,
                "1+".repeat(MAX_FORMULA_BYTES)
            ),
        ));
        assert!(spliced_with(long_formula.as_bytes(), &[span(1, 1)], &|_| String::new()).is_err());
        let unclosed_quote = sheet(r#"<row r="1" x="never>"#);
        assert!(spliced_with(unclosed_quote.as_bytes(), &[], &|_| String::new()).is_err());
    }

    #[test]
    fn spans_out_of_order_are_a_caller_error() {
        let xml = sheet("");
        assert!(spliced_with(xml.as_bytes(), &[span(3, 4), span(4, 5)], &|_| String::new()).is_err());
        assert!(spliced_with(xml.as_bytes(), &[span(5, 4)], &|_| String::new()).is_err());
    }
}
