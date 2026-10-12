use std::io::Read;

use ironcalc::base::types::Cell;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::ReadError;
use crate::limits::Usage;
use crate::sheet_data::{SheetCells, Slot, remap_strings, scan_rows};
use crate::stream::{Budgets, CharacterCheck, Guard};

// Rows are parsed in pieces of about this size, the same size the buffered reader
// used; a piece is dropped once its cells are merged.
const PIECE_BYTES: usize = 4 * 1024 * 1024;
// Past a piece the reader keeps this many bytes back so the last row start still
// has its boundary character in the window.
const CUT_MARGIN: usize = 256 * 1024;
// Past this many bytes without a row start the row is larger than any sheet holds,
// so the file goes to IronCalc instead of growing the window without bound.
const ROW_CAP_BYTES: usize = 16 * 1024 * 1024;
// The configuration before <sheetData> is small; past this the file goes to IronCalc.
const HEAD_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const CHUNK_BYTES: usize = 64 * 1024;

pub(crate) struct StreamedSheet {
    pub(crate) stub: Vec<u8>,
    pub(crate) settings: Vec<u8>,
    pub(crate) cells: SheetCells,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn unsupported(what: &str) -> ReadError {
    ReadError::Unsupported(what.to_string())
}

// One pass over the worksheet: the configuration flows into the stub byte for byte,
// rows are parsed in pieces cut before row starts and dropped after merging, and only
// the model cells, strings and formula texts stay in memory.
pub(crate) fn stream_worksheet<R: Read>(
    entry: R,
    compressed: u64,
    part: &str,
    sheet_name: &str,
    budgets: &Budgets,
) -> Result<StreamedSheet, ReadError> {
    let mut pump = Pump::new(entry, compressed, part, budgets);
    let Some((tag, content)) = pump.sheet_content()? else {
        let whole = pump.rest()?;
        return Ok(StreamedSheet {
            settings: whole.clone(),
            stub: whole,
            cells: SheetCells::default(),
        });
    };
    let (qname, prefixes) = sheet_tags(&pump.head[..content])?;
    let prefix = pump.head[..tag].to_vec();
    pump.head.drain(..content);
    pump.pos = 0;
    let mut driver = Driver {
        sheet_name,
        prefixes: &prefixes,
        budgets,
        merged: SheetCells::default(),
        strings: std::collections::HashMap::new(),
    };
    let mut rowtags: Vec<u8> = Vec::new();
    let cut = match pump.peek()? {
        (Next::SheetDataEnd, end) => {
            pump.pos += end;
            return Ok(assembled(prefix, qname, rowtags, pump.rest()?, driver));
        }
        (Next::Other, _) => {
            return Err(unsupported("something other than a row in sheetData"));
        }
        (Next::Row, end) => {
            let span = &pump.head[pump.pos..pump.pos + end];
            if span.contains(&b'\r') {
                return Err(unsupported("a carriage return inside sheetData"));
            }
            let Some(pattern) = row_cut_pattern(span) else {
                return Err(unsupported("something other than a row in sheetData"));
            };
            pattern
        }
    };
    loop {
        while !pump.eof && pump.window().len() < PIECE_BYTES + CUT_MARGIN {
            pump.fill()?;
        }
        if pump.window().is_empty() {
            return Err(unreadable("sheetData is not closed"));
        }
        if let Some((rel, end)) = find_end_tag(pump.window()) {
            let start = pump.pos;
            parse_unit(
                &mut driver,
                &mut rowtags,
                &cut,
                &pump.head[start..start + rel],
            )?;
            pump.pos = start + end;
            let tail = pump.rest()?;
            return Ok(assembled(prefix, qname, rowtags, tail, driver));
        }
        match find_cut(pump.window(), &cut) {
            Some(at) if at > 0 => {
                let start = pump.pos;
                parse_unit(
                    &mut driver,
                    &mut rowtags,
                    &cut,
                    &pump.head[start..start + at],
                )?;
                pump.pos = start + at;
                if pump.pos > PIECE_BYTES {
                    let pos = pump.pos;
                    pump.head.drain(..pos);
                    pump.pos = 0;
                }
            }
            _ if pump.eof => return Err(unreadable("sheetData is not closed")),
            _ if pump.window().len() >= ROW_CAP_BYTES => {
                return Err(unsupported("a row larger than supported"));
            }
            _ => pump.fill()?,
        }
    }
}

// The decompressed bytes read so far with the read position, refilled through the
// budgets; parsed bytes drain once a piece is merged so the window stays flat.
struct Pump<'b, R: Read> {
    guard: Guard<'b, R>,
    check: CharacterCheck,
    chunk: [u8; CHUNK_BYTES],
    head: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl<'b, R: Read> Pump<'b, R> {
    fn new(entry: R, compressed: u64, part: &str, budgets: &'b Budgets) -> Self {
        Pump {
            guard: budgets.stream(entry, compressed, part),
            check: CharacterCheck::new(),
            chunk: [0u8; CHUNK_BYTES],
            head: Vec::new(),
            pos: 0,
            eof: false,
        }
    }

    fn window(&self) -> &[u8] {
        &self.head[self.pos..]
    }

    fn fill(&mut self) -> Result<(), ReadError> {
        match self.guard.read_chunk(&mut self.chunk)? {
            0 => self.eof = true,
            read => {
                self.check.push(&self.chunk[..read])?;
                self.head.extend_from_slice(&self.chunk[..read]);
            }
        }
        Ok(())
    }

    // The start of the <sheetData> tag and the start of its content; None when the
    // configuration holds no rows.
    fn sheet_content(&mut self) -> Result<Option<(usize, usize)>, ReadError> {
        loop {
            match scan_head(&self.head)? {
                Head::Incomplete => {
                    if self.head.len() >= HEAD_LIMIT_BYTES {
                        return Err(unsupported(
                            "a worksheet with configuration larger than supported",
                        ));
                    }
                    self.fill()?;
                    if self.eof {
                        return Err(unreadable("a worksheet has no sheetData"));
                    }
                }
                Head::Empty => return Ok(None),
                Head::Found(tag, content) => return Ok(Some((tag, content))),
            }
        }
    }

    // The next row, end tag or other event with its end offset from the read
    // position; a tag truncated by the window reads more bytes and tries again,
    // and the budgets bound how far a malformed file gets this way.
    fn peek(&mut self) -> Result<(Next, usize), ReadError> {
        loop {
            match peek_event(self.window()) {
                Ok(Some(found)) => return Ok(found),
                Ok(None) | Err(_) if !self.eof => self.fill()?,
                Ok(None) => return Err(unreadable("sheetData is not closed")),
                Err(error) => return Err(error),
            }
        }
    }

    fn rest(&mut self) -> Result<Vec<u8>, ReadError> {
        let mut rest = self.head[self.pos..].to_vec();
        self.head.clear();
        self.pos = 0;
        loop {
            match self.guard.read_chunk(&mut self.chunk)? {
                0 => return Ok(rest),
                read => {
                    self.check.push(&self.chunk[..read])?;
                    rest.extend_from_slice(&self.chunk[..read]);
                }
            }
        }
    }
}

fn assembled(
    prefix: Vec<u8>,
    qname: Vec<u8>,
    rowtags: Vec<u8>,
    tail: Vec<u8>,
    driver: Driver,
) -> StreamedSheet {
    let mut stub = Vec::with_capacity(prefix.len() + qname.len() + 3 + tail.len());
    stub.extend_from_slice(&prefix);
    stub.extend_from_slice(b"<");
    stub.extend_from_slice(&qname);
    stub.extend_from_slice(b"/>");
    stub.extend_from_slice(&tail);
    let mut settings =
        Vec::with_capacity(prefix.len() + rowtags.len() + tail.len() + qname.len() * 2 + 8);
    settings.extend_from_slice(&prefix);
    settings.extend_from_slice(b"<");
    settings.extend_from_slice(&qname);
    settings.extend_from_slice(b">");
    settings.extend_from_slice(&rowtags);
    settings.extend_from_slice(b"</");
    settings.extend_from_slice(&qname);
    settings.extend_from_slice(b">");
    settings.extend_from_slice(&tail);
    StreamedSheet {
        stub,
        settings,
        cells: driver.merged,
    }
}

// One unit holds complete rows: outline tags travel to the settings, and the
// strict parse reads every byte.
fn parse_unit(
    driver: &mut Driver,
    rowtags: &mut Vec<u8>,
    cut: &[u8],
    unit: &[u8],
) -> Result<(), ReadError> {
    if unit.is_empty() {
        return Ok(());
    }
    if unit.contains(&b'\r') {
        return Err(unsupported("a carriage return inside sheetData"));
    }
    rowtags_from_unit(rowtags, cut, unit);
    let text = std::str::from_utf8(unit).map_err(unreadable)?;
    let scanned = scan_rows(text, driver.prefixes, driver.sheet_name)?;
    // An array formula spills into later rows; when its range runs past the rows
    // read so far a later piece would miss the cover, so the file goes to IronCalc.
    if spill_past_piece(&scanned) {
        return Err(unsupported("an array formula spilling past one piece"));
    }
    let usage = Usage {
        cells: scanned.data.iter().map(|(_, row)| row.len() as u64).sum(),
        bytes: scanned
            .strings
            .iter()
            .map(|text| text.len() as u64)
            .sum::<u64>()
            .saturating_add(
                scanned
                    .jobs
                    .iter()
                    .map(|job| job.text.len() as u64)
                    .sum::<u64>(),
            ),
        rows: scanned.data.len() as u64,
    };
    driver.budgets.check_usage(usage)?;
    merge(&mut driver.merged, &mut driver.strings, scanned)?;
    Ok(())
}

// The outline row tags of one unit, found by the same pattern the pieces cut by;
// the strict parse later still sees every byte.
fn rowtags_from_unit(rowtags: &mut Vec<u8>, cut: &[u8], unit: &[u8]) {
    let mut from = 0;
    while from < unit.len() {
        let mut next = unit.len();
        let mut at = from + 1;
        while at + cut.len() <= unit.len() {
            let Some(rel) = unit[at..].windows(cut.len()).position(|w| w == cut) else {
                break;
            };
            at += rel;
            if matches!(
                unit.get(at + cut.len()),
                Some(b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>')
            ) {
                next = at;
                break;
            }
            at += 1;
        }
        append_settings_tag(rowtags, &unit[from..next]);
        from = next;
    }
}

// The bytes every row start begins with, read off the first row: the tag name may
// carry a namespace prefix, which the strict parse validates later.
fn row_cut_pattern(row: &[u8]) -> Option<Vec<u8>> {
    let tag = if row.ends_with(b"/>") {
        row
    } else {
        &row[..tag_end(row)?]
    };
    let name = tag_name(&tag[1..])?;
    let mut pattern = Vec::with_capacity(name.len() + 1);
    pattern.push(b'<');
    pattern.extend_from_slice(name);
    Some(pattern)
}

// The last row start in the buffer, so the piece before it holds complete rows;
// a start cut off by the buffer end is left for the next fill.
fn find_cut(buf: &[u8], pattern: &[u8]) -> Option<usize> {
    let mut end = buf.len();
    while let Some(at) = buf[..end].iter().rposition(|byte| *byte == b'<') {
        if buf[at..].starts_with(pattern)
            && matches!(
                buf.get(at + pattern.len()),
                Some(b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>')
            )
        {
            return Some(at);
        }
        if at == 0 {
            break;
        }
        end = at;
    }
    None
}

// The first worksheet-data end tag with the end of it; None when no complete end
// tag is in sight yet. Anything unusual is left for the strict parse.
fn find_end_tag(buf: &[u8]) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(rel) = buf[from..].iter().position(|byte| *byte == b'<') {
        let at = from + rel;
        if buf[at..].starts_with(b"</") {
            let mut index = at + 2;
            while index < buf.len() && !buf[index].is_ascii_whitespace() && buf[index] != b'>' {
                index += 1;
            }
            let name = buf.get(at + 2..index)?;
            if local_is(name, b"sheetData") {
                let mut quote = 0u8;
                let mut end = index;
                while end < buf.len() {
                    let byte = buf[end];
                    if quote != 0 {
                        if byte == quote {
                            quote = 0;
                        }
                    } else if byte == b'"' || byte == b'\'' {
                        quote = byte;
                    } else if byte == b'<' {
                        break;
                    } else if byte == b'>' {
                        return Some((at, end + 1));
                    }
                    end += 1;
                }
                return None;
            }
        }
        from = at + 1;
    }
    None
}

// The configuration of one worksheet without its rows, for a reader that parses the
// rows itself. Row bytes are counted against the budgets as they stream past, then
// dropped through a sliding window, so memory stays flat whatever the sheet holds.
pub(crate) fn stream_config<R: Read>(
    entry: R,
    compressed: u64,
    part: &str,
    budgets: &Budgets,
) -> Result<Vec<u8>, ReadError> {
    let mut guard = budgets.stream(entry, compressed, part);
    let mut check = CharacterCheck::new();
    let mut head: Vec<u8> = Vec::new();
    let mut chunk = [0u8; CHUNK_BYTES];
    let (tag, content) = loop {
        match scan_head(&head)? {
            Head::Incomplete => {
                if head.len() >= HEAD_LIMIT_BYTES {
                    return Err(unsupported(
                        "a worksheet with configuration larger than supported",
                    ));
                }
                match guard.read_chunk(&mut chunk)? {
                    0 => return Err(unreadable("a worksheet has no sheetData")),
                    read => {
                        check.push(&chunk[..read])?;
                        head.extend_from_slice(&chunk[..read]);
                    }
                }
            }
            Head::Empty => {
                let rest = stream_rest(&mut guard, &mut check, &mut chunk)?;
                head.extend(rest);
                return Ok(head);
            }
            Head::Found(tag, content) => break (tag, content),
        }
    };
    let (qname, _) = sheet_tags(&head[..content])?;
    let mut window: Vec<u8> = head.split_off(tag);
    let mut rowtags: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut eof = false;
    loop {
        while pos == window.len() && !eof {
            match guard.read_chunk(&mut chunk)? {
                0 => eof = true,
                read => {
                    check.push(&chunk[..read])?;
                    window.extend_from_slice(&chunk[..read]);
                }
            }
        }
        if pos == window.len() {
            return Err(unreadable("sheetData is not closed"));
        }
        let (next, end) = loop {
            match peek_event(&window[pos..]) {
                Ok(Some(found)) => break found,
                Ok(None) | Err(_) if !eof => match guard.read_chunk(&mut chunk)? {
                    0 => eof = true,
                    read => {
                        check.push(&chunk[..read])?;
                        window.extend_from_slice(&chunk[..read]);
                    }
                },
                Ok(None) => return Err(unreadable("sheetData is not closed")),
                Err(error) => return Err(error),
            }
        };
        if matches!(next, Next::SheetDataEnd) {
            let mut config = head;
            config.extend_from_slice(b"<");
            config.extend_from_slice(&qname);
            config.extend_from_slice(b">");
            config.extend_from_slice(&rowtags);
            config.extend_from_slice(b"</");
            config.extend_from_slice(&qname);
            config.extend_from_slice(b">");
            config.extend_from_slice(&window[pos + end..]);
            config.extend(stream_rest(&mut guard, &mut check, &mut chunk)?);
            return Ok(config);
        }
        if matches!(next, Next::Row) {
            append_settings_tag(&mut rowtags, &window[pos..pos + end]);
        }
        pos += end;
        if pos > PIECE_BYTES {
            window.drain(..pos);
            pos = 0;
        }
    }
}

fn stream_rest<R: Read>(
    guard: &mut crate::stream::Guard<R>,
    check: &mut CharacterCheck,
    chunk: &mut [u8; CHUNK_BYTES],
) -> Result<Vec<u8>, ReadError> {
    let mut rest = Vec::new();
    loop {
        match guard.read_chunk(chunk)? {
            0 => return Ok(rest),
            read => {
                check.push(&chunk[..read])?;
                rest.extend_from_slice(&chunk[..read]);
            }
        }
    }
}

struct Driver<'d> {
    sheet_name: &'d str,
    prefixes: &'d [Vec<u8>],
    budgets: &'d Budgets,
    merged: SheetCells,
    strings: std::collections::HashMap<String, i32>,
}

// The row start tags the settings reader needs: rows with an outline level travel
// along, and the first cell tag follows a row without a number so the outline keeps
// it. Anything else from the rows never reaches the settings.
fn append_settings_tag(rowtags: &mut Vec<u8>, row: &[u8]) {
    let tag = if row.ends_with(b"/>") {
        row
    } else {
        let Some(len) = tag_end(row) else {
            return;
        };
        &row[..len]
    };
    if !has_outline_marker(tag) {
        return;
    }
    rowtags.extend_from_slice(tag);
    if !row_numbered(tag)
        && let Some(cell) = first_cell_tag(&row[tag.len()..])
    {
        rowtags.extend_from_slice(cell);
    }
}

fn has_outline_marker(tag: &[u8]) -> bool {
    tag.windows(b"outlineLevel".len())
        .any(|w| w == b"outlineLevel")
        || tag.windows(b"collapsed".len()).any(|w| w == b"collapsed")
}

fn row_numbered(tag: &[u8]) -> bool {
    let mut inner = tag.strip_prefix(b"<").unwrap_or(tag);
    inner = inner.strip_suffix(b">").unwrap_or(inner);
    inner = inner.strip_suffix(b"/").unwrap_or(inner);
    let name_len = inner
        .iter()
        .position(|b| b.is_ascii_whitespace())
        .unwrap_or(inner.len());
    let Ok(text) = std::str::from_utf8(inner) else {
        return false;
    };
    BytesStart::from_content(&text[..name_len], name_len)
        .attributes()
        .with_checks(true)
        .filter_map(Result::ok)
        .any(|attribute| attribute.key.as_ref() == b"r")
}

fn first_cell_tag(after: &[u8]) -> Option<&[u8]> {
    let mut rest = after;
    while let Some(at) = rest.windows(2).position(|w| w == b"<c") {
        match rest.get(at + 2) {
            Some(b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>') => {
                let span = &rest[at..];
                return tag_end(span).map(|len| &span[..len]);
            }
            _ => rest = &rest[at + 1..],
        }
    }
    None
}

// Whether an array formula in the piece claims rows past its last row: a later piece
// would read those cells without knowing their anchor, so only IronCalc's
// whole-sheet read stays correct there. Rows and array ranges are validated against
// the sheet limits when read, so the addition below cannot overflow.
fn spill_past_piece(piece: &SheetCells) -> bool {
    let Some((last, _)) = piece.data.last() else {
        return false;
    };
    piece.data.iter().any(|(row, cells)| {
        cells.iter().any(|(_, cell)| {
            matches!(cell, Cell::ArrayFormula { r: (_, height), .. } if row + height - 1 > *last)
        })
    })
}

// Puts the pieces back in document order: rows, strings numbered by first appearance,
// formula events and jobs renumbered after the pieces before them.
fn merge(
    merged: &mut SheetCells,
    strings: &mut std::collections::HashMap<String, i32>,
    piece: SheetCells,
) -> Result<(), ReadError> {
    if let (Some((last, _)), Some((first, _))) = (merged.data.last(), piece.data.first())
        && last >= first
    {
        return Err(unsupported("rows out of order"));
    }
    let row_offset = merged.data.len();
    let event_offset = i32::try_from(merged.events.len()).map_err(unreadable)?;
    let job_offset = merged.jobs.len();
    let mut table = Vec::with_capacity(piece.strings.len());
    for text in piece.strings {
        let next = i32::try_from(merged.strings.len()).map_err(unreadable)?;
        let index = *strings.entry(text.clone()).or_insert_with(|| {
            merged.strings.push(text);
            next
        });
        table.push(index);
    }
    let mut data = piece.data;
    remap_strings(&mut data, &piece.string_cells, &table)?;
    for (_, row) in &mut data {
        for (_, cell) in row {
            if let Cell::CellFormula { f, .. } | Cell::ArrayFormula { f, .. } = cell {
                *f += event_offset;
            }
        }
    }
    merged
        .string_cells
        .extend(piece.string_cells.into_iter().map(|slot| Slot {
            row: slot.row + row_offset,
            cell: slot.cell,
        }));
    merged.data.extend(data);
    merged.rows.extend(piece.rows);
    merged.events.extend(
        piece
            .events
            .into_iter()
            .map(|event| event.after_jobs(job_offset)),
    );
    merged.jobs.extend(piece.jobs);
    Ok(())
}

#[derive(Debug)]
enum Next {
    Row,
    SheetDataEnd,
    Other,
}

// The next event starting at the window, with its end offset from the window start;
// None when the window ends mid-event and more bytes may complete it. A leading end
// tag is read by hand: a fresh reader with end-name checking rejects it for having
// no open tag, and the strict parse later still sees every byte.
fn peek_event(window: &[u8]) -> Result<Option<(Next, usize)>, ReadError> {
    if window.starts_with(b"</") {
        let Some((name, end)) = end_tag(window) else {
            return Ok(None);
        };
        let local = name.rsplit(|b| *b == b':').next().unwrap_or(&name);
        if local == b"sheetData" {
            return Ok(Some((Next::SheetDataEnd, end)));
        }
        return Ok(Some((Next::Other, end)));
    }
    let mut reader = Reader::from_reader(window);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    match reader.read_event().map_err(unreadable)? {
        Event::Start(start) if start.local_name().as_ref() == b"row" => {
            Ok(row_end(window)?.map(|end| (Next::Row, end)))
        }
        Event::Empty(start) if start.local_name().as_ref() == b"row" => {
            Ok(Some((Next::Row, position(&reader))))
        }
        Event::End(end) if end.local_name().as_ref() == b"sheetData" => {
            Ok(Some((Next::SheetDataEnd, position(&reader))))
        }
        Event::Eof => Ok(None),
        _ => Ok(Some((Next::Other, position(&reader)))),
    }
}

// The end of the <row> starting the window; None when more bytes may complete it.
fn row_end(window: &[u8]) -> Result<Option<usize>, ReadError> {
    let mut reader = Reader::from_reader(window);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut depth = 0usize;
    loop {
        match reader.read_event().map_err(unreadable)? {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => return Ok(Some(position(&reader))),
            Event::End(_) if depth == 1 => return Ok(Some(position(&reader))),
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}

#[derive(Debug)]
enum Head {
    Incomplete,
    Empty,
    // The start of the <sheetData> tag and the start of its content.
    Found(usize, usize),
}

// The byte offset where <sheetData> content starts, found without parsing: outside
// comments, CDATA, processing instructions and quoted attribute values a literal
// `<` always starts markup, so depth counting stays exact on well-formed input.
// Anything else fails later in the strict parse and leaves the file to IronCalc.
fn scan_head(head: &[u8]) -> Result<Head, ReadError> {
    let mut index = 0;
    let mut depth = 0usize;
    while index < head.len() {
        if head[index] != b'<' {
            index += 1;
            continue;
        }
        let rest = &head[index..];
        if rest.starts_with(b"<!--") {
            let Some(end) = find_bytes(&head[index + 4..], b"-->") else {
                return Ok(Head::Incomplete);
            };
            index += 4 + end + 3;
        } else if rest.starts_with(b"<![CDATA[") {
            let Some(end) = find_bytes(&head[index + 9..], b"]]>") else {
                return Ok(Head::Incomplete);
            };
            index += 9 + end + 3;
        } else if rest.starts_with(b"<!DOCTYPE") || rest.starts_with(b"<!doctype") {
            let Some(end) = doctype_end(&head[index..]) else {
                return Ok(Head::Incomplete);
            };
            index += end;
        } else if rest.starts_with(b"<?") {
            let Some(end) = find_bytes(&head[index + 2..], b"?>") else {
                return Ok(Head::Incomplete);
            };
            index += 2 + end + 2;
        } else if rest.starts_with(b"<!") {
            return Err(unreadable("a worksheet part is malformed"));
        } else if rest.starts_with(b"</") {
            let Some(tag) = tag_end(&head[index..]) else {
                return Ok(Head::Incomplete);
            };
            depth = depth.saturating_sub(1);
            index += tag;
        } else {
            let Some(tag) = tag_end(&head[index..]) else {
                return Ok(Head::Incomplete);
            };
            let empty = head[index + tag - 2] == b'/';
            let name = tag_name(&head[index + 1..index + tag]);
            match name {
                Some(name) if depth == 1 && local_is(name, b"sheetData") => {
                    if empty {
                        return Ok(Head::Empty);
                    }
                    return Ok(Head::Found(index, index + tag));
                }
                _ => {
                    if !empty {
                        depth += 1;
                    }
                    index += tag;
                }
            }
        }
    }
    Ok(Head::Incomplete)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// Past a DOCTYPE, whose internal subset may hold `>` inside brackets.
fn doctype_end(head: &[u8]) -> Option<usize> {
    let mut brackets = 0u32;
    let mut quote = 0u8;
    let mut index = 0;
    while index < head.len() {
        let byte = head[index];
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
        } else if matches!(byte, b'"' | b'\'') {
            quote = byte;
        } else if byte == b'[' {
            brackets += 1;
        } else if byte == b']' {
            brackets = brackets.saturating_sub(1);
        } else if byte == b'>' && brackets == 0 {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

// Past a start or end tag starting at the slice, through its unquoted `>`; a `<`
// inside means malformed input.
fn tag_end(head: &[u8]) -> Option<usize> {
    let mut quote = 0u8;
    let mut index = 1;
    while index < head.len() {
        match head[index] {
            b'<' => return None,
            b'>' if quote == 0 => return Some(index + 1),
            q @ (b'"' | b'\'') if quote == 0 => quote = q,
            q if q == quote => quote = 0,
            _ => {}
        }
        index += 1;
    }
    None
}

fn tag_name(tag: &[u8]) -> Option<&[u8]> {
    let end = tag
        .iter()
        .position(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'))
        .unwrap_or(tag.len());
    let name = &tag[..end];
    (!name.is_empty()).then_some(name)
}

fn local_is(name: &[u8], local: &[u8]) -> bool {
    name.rsplit(|b| *b == b':').next() == Some(local)
}

// The qualified name and end offset of the end tag starting the window; None when no
// closing `>` is in sight yet. Anything unusual is left for the strict parse.
fn end_tag(window: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut index = 2;
    while index < window.len() && !window[index].is_ascii_whitespace() && window[index] != b'>' {
        index += 1;
    }
    let name = window.get(2..index)?.to_vec();
    while index < window.len() && window[index] != b'>' {
        index += 1;
    }
    (index < window.len()).then(|| (name, index + 1))
}

// The qualified <sheetData> name for the stub and the namespace prefixes its rows
// may use, read from the configuration with the same strictness as the row parser.
fn sheet_tags(head: &[u8]) -> Result<(Vec<u8>, Vec<Vec<u8>>), ReadError> {
    let mut reader = Reader::from_reader(head);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut prefixes = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event().map_err(unreadable)? {
            Event::Start(start) => {
                if depth == 0 {
                    declare_prefixes(&start, &mut prefixes)?;
                }
                if depth == 1 && start.local_name().as_ref() == b"sheetData" {
                    declare_prefixes(&start, &mut prefixes)?;
                    return Ok((start.name().as_ref().to_vec(), prefixes));
                }
                depth += 1;
            }
            Event::Empty(start) => {
                if depth == 1 && start.local_name().as_ref() == b"sheetData" {
                    return Err(unreadable("sheetData out of place"));
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Err(unreadable("a worksheet has no sheetData")),
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::CellBudget;

    const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetData>"#;
    const TAIL: &str = "</sheetData></worksheet>";

    fn budgets() -> Budgets {
        Budgets::with_cells(CellBudget::max_cells_for_test(u64::MAX))
    }

    fn stream(xml: &str) -> StreamedSheet {
        stream_worksheet(
            xml.as_bytes(),
            0,
            "xl/worksheets/sheet1.xml",
            "Sheet1",
            &budgets(),
        )
        .unwrap()
    }

    #[test]
    fn a_leading_end_tag_is_read_without_a_parser_stack() {
        assert!(matches!(
            peek_event(b"</sheetData>").unwrap(),
            Some((Next::SheetDataEnd, 12))
        ));
        assert!(matches!(
            peek_event(b"</x:sheetData >").unwrap(),
            Some((Next::SheetDataEnd, 15))
        ));
        assert!(matches!(
            peek_event(b"</other>").unwrap(),
            Some((Next::Other, 8))
        ));
        assert!(peek_event(b"</sheetData").unwrap().is_none());
    }

    #[test]
    fn rows_flow_into_cells_and_config_into_the_stub() {
        let xml = format!(
            r#"{HEAD}<row r="1"><c r="A1"><v>7</v></c><c r="B1"><f>A1*6</f></c></row>{TAIL}"#
        );
        let streamed = stream(&xml);
        assert_eq!(streamed.cells.data.len(), 1);
        assert_eq!(streamed.cells.jobs.len(), 1);
        let stub = String::from_utf8(streamed.stub).unwrap();
        assert!(stub.contains("<sheetView"), "{stub}");
        assert!(stub.contains("<sheetData/>"), "{stub}");
        assert!(!stub.contains("<row"), "{stub}");
    }

    #[test]
    fn an_empty_sheet_keeps_its_part_untouched() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetData/><mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells></worksheet>"#;
        let streamed = stream(xml);
        assert!(streamed.cells.data.is_empty());
        assert_eq!(String::from_utf8(streamed.stub).unwrap(), xml);
    }

    #[test]
    fn empty_content_empties_the_stub() {
        let xml = format!(r#"{HEAD}{TAIL}"#);
        let streamed = stream(&xml);
        assert!(streamed.cells.data.is_empty());
        let stub = String::from_utf8(streamed.stub).unwrap();
        assert!(stub.contains("<sheetData/>"), "{stub}");
        assert!(!stub.contains("<row"), "{stub}");
    }

    #[test]
    fn string_bytes_count_against_the_budget_while_cells_fit() {
        let filler = "x".repeat(20_000);
        let mut rows = String::new();
        for n in 1..=10 {
            rows.push_str(&format!(
                r#"<row r="{n}"><c r="A{n}" t="inlineStr"><is><t>{n}{filler}</t></is></c></row>"#
            ));
        }
        let xml = format!(r#"{HEAD}{rows}{TAIL}"#);
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(1000));
        let error = stream_worksheet(
            xml.as_bytes(),
            0,
            "xl/worksheets/sheet1.xml",
            "Sheet1",
            &budgets,
        )
        .err()
        .expect("ten cells with huge strings must trip the budget");
        assert!(error.to_string().contains("needs at least"), "{error}");
    }

    #[test]
    fn a_row_past_the_sheet_end_is_an_error_not_an_overflow() {
        let xml = format!(
            r#"{HEAD}<row r="2147483647"><c r="A1"><f t="array" ref="A1:A2">1</f></c></row>{TAIL}"#
        );
        let error = stream_worksheet(
            xml.as_bytes(),
            0,
            "xl/worksheets/sheet1.xml",
            "Sheet1",
            &budgets(),
        )
        .err()
        .expect("an out-of-range row must not reach the model");
        assert!(matches!(error, ReadError::Unreadable(_)), "{error}");
    }

    #[test]
    fn the_cell_budget_stops_a_growing_sheet() {
        let mut rows = String::new();
        for n in 1..=2000 {
            rows.push_str(&format!(r#"<row r="{n}"><c r="A{n}"><v>1</v></c></row>"#));
        }
        let xml = format!(r#"{HEAD}{rows}{TAIL}"#);
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(1000));
        let error = stream_worksheet(
            xml.as_bytes(),
            0,
            "xl/worksheets/sheet1.xml",
            "Sheet1",
            &budgets,
        )
        .err()
        .expect("a growing sheet must stop");
        let message = error.to_string();
        assert!(message.contains("needs at least"), "{message}");
    }
}
