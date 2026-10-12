use ironcalc::base::types::Cell;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::ReadError;
use crate::sheet_data::{SheetCells, Slot, remap_strings, scan_rows};

// Below this the rows are read on one thread; above it, in pieces of about this size.
const PIECE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct ScannedSheet<'a> {
    pub(crate) stub: Option<Vec<u8>>,
    pub(crate) cells: SheetCells<'a>,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn unsupported(what: &str) -> ReadError {
    ReadError::Unsupported(what.to_string())
}

struct SheetDataSpan {
    start: usize,
    content: (usize, usize),
    end: usize,
    name: Vec<u8>,
}

pub(crate) fn scan_worksheet<'a>(
    xml: &'a str,
    sheet_name: &str,
) -> Result<ScannedSheet<'a>, ReadError> {
    check_characters(xml.as_bytes())?;
    let (span, prefixes) = find_sheet_data(xml)?;
    let bytes = xml.as_bytes();
    let (Some(before), Some(content), Some(after)) = (
        bytes.get(..span.start),
        xml.get(span.content.0..span.content.1),
        bytes.get(span.end..),
    ) else {
        return Err(unreadable("sheetData out of place"));
    };
    if content.contains('\r') {
        return Err(unsupported("a carriage return inside sheetData"));
    }
    let stub = (span.start != span.end).then(|| [before, b"<", &span.name, b"/>", after].concat());
    let row_tag = match span.name.iter().position(|b| *b == b':') {
        Some(colon) => [b"<", &span.name[..=colon], b"row"].concat(),
        None => b"<row".to_vec(),
    };
    let cells = scan_content(content, &row_tag, &prefixes, sheet_name)?;
    Ok(ScannedSheet { stub, cells })
}

// The one <sheetData> child of the root, as IronCalc's importer finds it, and the
// namespace prefixes in scope for it.
fn find_sheet_data(xml: &str) -> Result<(SheetDataSpan, Vec<Vec<u8>>), ReadError> {
    let mut reader = Reader::from_str(xml);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut prefixes = Vec::new();
    let mut depth = 0usize;
    let mut found: Option<SheetDataSpan> = None;
    loop {
        let before = position(&reader);
        match reader.read_event().map_err(unreadable)? {
            Event::Start(start) if depth == 1 && start.local_name().as_ref() == b"sheetData" => {
                if found.is_some() {
                    return Err(unsupported("two sheetData elements"));
                }
                declare_prefixes(&start, &mut prefixes)?;
                let content_start = position(&reader);
                let name = start.name().as_ref().to_vec();
                let closing = [b"</", name.as_slice()].concat();
                let tail = xml.as_bytes().get(content_start..).unwrap_or_default();
                let content_end = tail
                    .windows(closing.len())
                    .rposition(|w| w == closing.as_slice())
                    .map(|at| content_start + at)
                    .ok_or_else(|| unreadable("sheetData is not closed"))?;
                let end = close_sheet_data(xml, content_end)?;
                found = Some(SheetDataSpan {
                    start: before,
                    content: (content_start, content_end),
                    end,
                    name,
                });
                reader = Reader::from_str(xml.get(end..).unwrap_or_default());
                reader.config_mut().allow_unmatched_ends = true;
            }
            Event::Start(start) => {
                if depth == 0 {
                    declare_prefixes(&start, &mut prefixes)?;
                }
                depth += 1;
            }
            Event::Empty(start) if depth == 1 && start.local_name().as_ref() == b"sheetData" => {
                if found.is_some() {
                    return Err(unsupported("two sheetData elements"));
                }
                found = Some(SheetDataSpan {
                    start: before,
                    content: (before, before),
                    end: before,
                    name: Vec::new(),
                });
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    let span = found.ok_or_else(|| unreadable("a worksheet has no sheetData"))?;
    Ok((span, prefixes))
}

// The position just past the end tag of <sheetData> that starts at `at`.
fn close_sheet_data(xml: &str, at: usize) -> Result<usize, ReadError> {
    let mut reader = Reader::from_str(xml.get(at..).unwrap_or_default());
    reader.config_mut().allow_unmatched_ends = true;
    match reader.read_event().map_err(unreadable)? {
        Event::End(end) if end.local_name().as_ref() == b"sheetData" => Ok(at + position(&reader)),
        _ => Err(unreadable("sheetData is not closed")),
    }
}

fn declare_prefixes(start: &BytesStart<'_>, prefixes: &mut Vec<Vec<u8>>) -> Result<(), ReadError> {
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute.map_err(unreadable)?;
        if let Some(quick_xml::name::PrefixDeclaration::Named(prefix)) =
            attribute.key.as_namespace_binding()
        {
            prefixes.push(prefix.to_vec());
        }
    }
    Ok(())
}

fn position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX)
}

// roxmltree, which IronCalc parses with, rejects characters XML forbids and normalizes
// line breaks; quick-xml does neither, so such a part is left to IronCalc. Excel ends the
// XML declaration with CRLF, so carriage returns are refused only inside sheetData.
fn check_characters(bytes: &[u8]) -> Result<(), ReadError> {
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r') {
            return Err(unsupported("a control character"));
        }
        if byte == 0xEF
            && bytes.get(index + 1) == Some(&0xBF)
            && matches!(bytes.get(index + 2), Some(0xBE | 0xBF))
        {
            return Err(unsupported("a noncharacter"));
        }
        index += 1;
    }
    Ok(())
}

// Large sheets are cut before row start tags and the pieces read in parallel. A cut inside
// a comment, CDATA or a tag leaves a piece that does not parse, so the file goes to
// IronCalc. An array formula spills into later rows, so its sheet is read in one piece.
fn scan_content<'a>(
    content: &'a str,
    row_tag: &[u8],
    prefixes: &[Vec<u8>],
    sheet_name: &str,
) -> Result<SheetCells<'a>, ReadError> {
    let pieces = cut_before_rows(content, row_tag);
    if pieces.len() < 2 {
        return scan_rows(content, prefixes, sheet_name);
    }
    let scanned = crate::in_parallel(pieces.len(), |piece| {
        scan_rows(pieces[piece], prefixes, sheet_name)
    })?;
    if scanned.iter().any(|piece| piece.has_arrays) {
        return scan_rows(content, prefixes, sheet_name);
    }
    join(scanned)
}

fn cut_before_rows<'a>(content: &'a str, row_tag: &[u8]) -> Vec<&'a str> {
    let bytes = content.as_bytes();
    let mut pieces = Vec::new();
    let mut start = 0;
    while bytes.len() - start > PIECE_BYTES {
        let from = start + PIECE_BYTES;
        let Some(cut) = bytes[from..]
            .windows(row_tag.len() + 1)
            .position(|w| {
                w.starts_with(row_tag)
                    && matches!(w.last(), Some(b' ' | b'>' | b'/' | b'\t' | b'\n'))
            })
            .map(|at| from + at)
        else {
            break;
        };
        let Some(piece) = content.get(start..cut) else {
            break;
        };
        pieces.push(piece);
        start = cut;
    }
    if let Some(last) = content.get(start..) {
        pieces.push(last);
    }
    pieces
}

// Puts the pieces back in document order: rows, strings numbered by first appearance,
// formula events and jobs renumbered after the pieces before them.
fn join(pieces: Vec<SheetCells<'_>>) -> Result<SheetCells<'_>, ReadError> {
    let mut joined = SheetCells::default();
    let mut string_index: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
    for piece in pieces {
        if let (Some((last, _)), Some((first, _))) = (joined.data.last(), piece.data.first())
            && last >= first
        {
            return Err(unsupported("rows out of order"));
        }
        let row_offset = joined.data.len();
        let event_offset = i32::try_from(joined.events.len()).map_err(unreadable)?;
        let job_offset = joined.jobs.len();
        let mut strings = Vec::with_capacity(piece.strings.len());
        for text in piece.strings {
            let next = i32::try_from(joined.strings.len()).map_err(unreadable)?;
            let index = *string_index.entry(text.clone()).or_insert_with(|| {
                joined.strings.push(text);
                next
            });
            strings.push(index);
        }
        let mut data = piece.data;
        remap_strings(&mut data, &piece.string_cells, &strings)?;
        for (_, row) in &mut data {
            for (_, cell) in row {
                if let Cell::CellFormula { f, .. } | Cell::ArrayFormula { f, .. } = cell {
                    *f += event_offset;
                }
            }
        }
        joined
            .string_cells
            .extend(piece.string_cells.into_iter().map(|slot| Slot {
                row: slot.row + row_offset,
                cell: slot.cell,
            }));
        joined.data.extend(data);
        joined.rows.extend(piece.rows);
        joined.events.extend(
            piece
                .events
                .into_iter()
                .map(|event| event.after_jobs(job_offset)),
        );
        joined.jobs.extend(piece.jobs);
    }
    Ok(joined)
}
