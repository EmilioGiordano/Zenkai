use std::io::Read;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::ReadError;
use crate::limits::Usage;
use crate::sheet_data::{SheetCells, scan_rows_into};
use crate::stream::{Budgets, CharacterCheck, CompressedMeter, Guard};

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
    meter: &CompressedMeter,
    part: &str,
    sheet_name: &str,
    budgets: &Budgets,
) -> Result<StreamedSheet, ReadError> {
    let mut pump = Pump::new(entry, meter, part, budgets);
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
            let body = &pump.head[pump.pos..pump.pos + end];
            let span = &body[blank_prefix_len(body)..];
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
    head_scan: ForwardScan,
    pos: usize,
    eof: bool,
}

impl<'b, R: Read> Pump<'b, R> {
    fn new(entry: R, meter: &CompressedMeter, part: &str, budgets: &'b Budgets) -> Self {
        Pump {
            guard: budgets.stream_worksheet(entry, meter, part),
            check: CharacterCheck::new(),
            chunk: [0u8; CHUNK_BYTES],
            head: Vec::new(),
            head_scan: ForwardScan::new(),
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
            match scan_head(&mut self.head_scan, &self.head)? {
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
    // position. A row longer than the cap goes to IronCalc instead of growing the
    // window without bound, and the row scan resumes where the last fill stopped
    // instead of re-parsing from the read position.
    fn peek(&mut self) -> Result<(Next, usize), ReadError> {
        let mut row_scan = ForwardScan::new();
        loop {
            if self.window().len() > ROW_CAP_BYTES {
                return Err(unsupported("a row larger than supported"));
            }
            match classify(self.window()) {
                Ok(Some((Next::Row, _))) => match scan_row_end(&mut row_scan, self.window())? {
                    Some(end) => return Ok((Next::Row, end)),
                    None if !self.eof => self.fill()?,
                    None => return Err(unreadable("sheetData is not closed")),
                },
                Ok(Some(found)) => return Ok(found),
                Ok(None) | Err(_) if !self.eof => self.fill()?,
                Ok(None) => return Err(unreadable("sheetData is not closed")),
                Err(error) => return Err(error),
            }
        }
    }

    // The bytes from the read position on, kept whole for the stub and the
    // settings; capped far below streamed parts and charged to the memory
    // budget, like every other whole copy.
    fn rest(&mut self) -> Result<Vec<u8>, ReadError> {
        let mut rest = self.head[self.pos..].to_vec();
        self.guard.whole_fits(rest.len() as u64)?;
        self.head.clear();
        self.pos = 0;
        loop {
            match self.guard.read_chunk(&mut self.chunk)? {
                0 => {
                    self.guard.check_whole(rest.len() as u64)?;
                    return Ok(rest);
                }
                read => {
                    self.check.push(&self.chunk[..read])?;
                    rest.extend_from_slice(&self.chunk[..read]);
                    self.guard.whole_fits(rest.len() as u64)?;
                }
            }
        }
    }
}

fn assembled(
    prefix: Vec<u8>,
    qname: Vec<u8>,
    rowtags: Vec<u8>,
    mut tail: Vec<u8>,
    driver: Driver,
) -> StreamedSheet {
    let mut stub = Vec::with_capacity(prefix.len() + qname.len() + 3 + tail.len());
    stub.extend_from_slice(&prefix);
    stub.extend_from_slice(b"<");
    stub.extend_from_slice(&qname);
    stub.extend_from_slice(b"/>");
    stub.extend_from_slice(&tail);
    let mut head = Vec::with_capacity(prefix.len() + rowtags.len() + qname.len() * 2 + 8);
    head.extend_from_slice(&prefix);
    head.extend_from_slice(b"<");
    head.extend_from_slice(&qname);
    head.extend_from_slice(b">");
    head.extend_from_slice(&rowtags);
    head.extend_from_slice(b"</");
    head.extend_from_slice(&qname);
    head.extend_from_slice(b">");
    // The tail buffer is reused for the settings instead of copying the tail a
    // third time; the stub above holds the only other copy.
    tail.splice(0..0, head);
    StreamedSheet {
        stub,
        settings: tail,
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
    let rowtags_bytes = rowtags_from_unit(rowtags, cut, unit);
    let text = std::str::from_utf8(unit).map_err(unreadable)?;
    let before = driver.merged.totals();
    let spills = scan_rows_into(
        &mut driver.merged,
        text,
        driver.prefixes,
        driver.sheet_name,
    )?;
    // An array formula spills into later rows; when its range runs past the rows
    // read so far a later piece would miss the cover, so the file goes to IronCalc.
    if spills {
        return Err(unsupported("an array formula spilling past one piece"));
    }
    let after = driver.merged.totals();
    let usage = Usage {
        cells: after.cells.saturating_sub(before.cells),
        // One event per formula cell: plain and array formulas convert, shared
        // anchors convert once, shared children point at their anchor.
        formulas: after.formulas.saturating_sub(before.formulas),
        bytes: after
            .bytes
            .saturating_sub(before.bytes)
            .saturating_add(rowtags_bytes),
        rows: after.rows.saturating_sub(before.rows),
    };
    driver.budgets.check_usage(usage)
}

// The outline row tags of one unit, found by the same pattern the pieces cut by;
// the strict parse later still sees every byte.
fn rowtags_from_unit(rowtags: &mut Vec<u8>, cut: &[u8], unit: &[u8]) -> u64 {
    let mut from = 0;
    let mut added = 0u64;
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
        added = added.saturating_add(append_settings_tag(rowtags, &unit[from..next]));
        from = next;
    }
    added
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
    meter: &CompressedMeter,
    part: &str,
    budgets: &Budgets,
) -> Result<Vec<u8>, ReadError> {
    let mut guard = budgets.stream_worksheet(entry, meter, part);
    let mut check = CharacterCheck::new();
    let mut head: Vec<u8> = Vec::new();
    let mut head_scan = ForwardScan::new();
    let mut chunk = [0u8; CHUNK_BYTES];
    let (tag, content) = loop {
        match scan_head(&mut head_scan, &head)? {
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
    // One chunk from the entry, refusing a row past the cap on the same pass so
    // the window never grows without bound while an event stays incomplete.
    let mut fill = |window: &mut Vec<u8>, eof: &mut bool, from: usize| -> Result<(), ReadError> {
        match guard.read_chunk(&mut chunk)? {
            0 => *eof = true,
            read => {
                check.push(&chunk[..read])?;
                window.extend_from_slice(&chunk[..read]);
            }
        }
        if window[from..].len() > ROW_CAP_BYTES {
            return Err(unsupported("a row larger than supported"));
        }
        Ok(())
    };
    loop {
        while pos == window.len() && !eof {
            fill(&mut window, &mut eof, pos)?;
        }
        if pos == window.len() {
            return Err(unreadable("sheetData is not closed"));
        }
        if window[pos..].len() > ROW_CAP_BYTES {
            return Err(unsupported("a row larger than supported"));
        }
        // Each scan resumes across refills, so a huge row is read once. A scan that
        // has found its event must not be asked again: its offset may already lie
        // past the tag it found, and a second call would report a later tag.
        let mut event_scan = ForwardScan::new();
        let found = loop {
            match scan_event(&mut event_scan, &window[pos..]) {
                Ok(Some(found)) => break found,
                Ok(None) | Err(_) if !eof => fill(&mut window, &mut eof, pos)?,
                Ok(None) => return Err(unreadable("sheetData is not closed")),
                Err(error) => return Err(error),
            }
        };
        let (next, end) = match found {
            (Next::Row, _) => {
                let mut row_scan = ForwardScan::new();
                loop {
                    match scan_row_end(&mut row_scan, &window[pos..])? {
                        Some(end) => break (Next::Row, end),
                        None if !eof => fill(&mut window, &mut eof, pos)?,
                        None => return Err(unreadable("sheetData is not closed")),
                    }
                }
            }
            found => found,
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
            let row = &window[pos..pos + end];
            let added = append_settings_tag(&mut rowtags, &row[blank_prefix_len(row)..]);
            if added > 0 {
                budgets.check_usage(Usage {
                    cells: 0,
                    formulas: 0,
                    bytes: added,
                    rows: 0,
                })?;
            }
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
            0 => {
                guard.check_whole(rest.len() as u64)?;
                return Ok(rest);
            }
            read => {
                check.push(&chunk[..read])?;
                rest.extend_from_slice(&chunk[..read]);
                guard.whole_fits(rest.len() as u64)?;
            }
        }
    }
}

struct Driver<'d> {
    sheet_name: &'d str,
    prefixes: &'d [Vec<u8>],
    budgets: &'d Budgets,
    merged: SheetCells,
}

// The row start tags the settings reader needs: rows with an outline level travel
// along, and the first cell tag follows a row without a number so the outline keeps
// it. Anything else from the rows never reaches the settings.
fn append_settings_tag(rowtags: &mut Vec<u8>, row: &[u8]) -> u64 {
    let tag = if row.ends_with(b"/>") {
        row
    } else {
        let Some(len) = tag_end(row) else {
            return 0;
        };
        &row[..len]
    };
    if !has_outline_marker(tag) {
        return 0;
    }
    rowtags.extend_from_slice(tag);
    let mut added = tag.len() as u64;
    if !row_numbered(tag)
        && let Some(cell) = first_cell_tag(&row[tag.len()..])
    {
        rowtags.extend_from_slice(cell);
        added += cell.len() as u64;
    }
    added
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
    BytesStart::from_content(text, name_len)
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

#[derive(Debug)]
enum Next {
    Row,
    SheetDataEnd,
    Other,
}

// The first event starting at the window with its end offset, skipping blank text
// between rows; None when the window ends mid-event and more bytes may complete it.
// A leading end tag is read by hand: a fresh reader with end-name checking rejects
// it for having no open tag, and the strict parse later still sees every byte. Only
// one event is ever read, so a refill re-parses at most one tag; row content is left
// to the resumable scan below.
fn classify(window: &[u8]) -> Result<Option<(Next, usize)>, ReadError> {
    let skipped = blank_prefix_len(window);
    let window = &window[skipped..];
    if window.is_empty() {
        return Ok(None);
    }
    if window.starts_with(b"</") {
        let Some((name, end)) = end_tag(window) else {
            return Ok(None);
        };
        let local = name.rsplit(|b| *b == b':').next().unwrap_or(&name);
        if local == b"sheetData" {
            return Ok(Some((Next::SheetDataEnd, skipped + end)));
        }
        return Ok(Some((Next::Other, skipped + end)));
    }
    let mut reader = Reader::from_reader(window);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    match reader.read_event().map_err(unreadable)? {
        Event::Start(start) if start.local_name().as_ref() == b"row" => {
            Ok(tag_end(window).map(|end| (Next::Row, skipped + end)))
        }
        Event::Empty(start) if start.local_name().as_ref() == b"row" => {
            Ok(Some((Next::Row, skipped + position(&reader))))
        }
        Event::End(end) if end.local_name().as_ref() == b"sheetData" => {
            Ok(Some((Next::SheetDataEnd, skipped + position(&reader))))
        }
        Event::Eof => Ok(None),
        _ => Ok(Some((Next::Other, skipped + position(&reader)))),
    }
}

#[derive(Debug)]
enum Head {
    Incomplete,
    Empty,
    // The start of the <sheetData> tag and the start of its content.
    Found(usize, usize),
}

// A forward-only scan over bytes arriving in chunks. The scan remembers how far is
// settled, so a refill continues from there instead of re-parsing from byte zero.
// Cuts the scan finds are always re-checked by the strict parse later, so on
// malformed input it may cut anywhere; it must only never hang nor grow.
struct ForwardScan {
    depth: usize,
    offset: usize,
    mode: ScanMode,
    tag_start: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    Markup,
    Comment,
    CData,
    Tag { quote: u8, closing: bool },
    Doctype { brackets: u32, quote: u8 },
    Pi,
}

impl ForwardScan {
    fn new() -> ForwardScan {
        ForwardScan {
            depth: 0,
            offset: 0,
            mode: ScanMode::Markup,
            tag_start: 0,
        }
    }
}

#[derive(Debug)]
enum ScanGoal {
    Head,
    Row,
    Event,
}

enum ScanEvent {
    NeedMore,
    Head(Head),
    RowDone(usize),
    Event(Next, usize),
}

// One step over the buffer from the settled offset. Outside comments, CDATA,
// processing instructions and quoted attribute values a literal `<` always starts
// markup, so depth counting stays exact on well-formed input.
fn advance(scan: &mut ForwardScan, buf: &[u8], goal: ScanGoal) -> Result<ScanEvent, ReadError> {
    loop {
        match scan.mode {
            ScanMode::Markup => {
                let Some(rel) = buf[scan.offset..].iter().position(|byte| *byte == b'<') else {
                    scan.offset = buf.len();
                    return Ok(ScanEvent::NeedMore);
                };
                let at = scan.offset + rel;
                let rest = &buf[at..];
                // A `<` that ends the buffer may still become an end tag, which must
                // not be counted as a start tag once the next chunk arrives.
                if rest.len() == 1 {
                    scan.offset = at;
                    return Ok(ScanEvent::NeedMore);
                }
                if rest.starts_with(b"<!--") {
                    scan.mode = ScanMode::Comment;
                    scan.offset = at + 4;
                } else if rest.starts_with(b"<![CDATA[") {
                    scan.mode = ScanMode::CData;
                    scan.offset = at + 9;
                } else if rest.starts_with(b"<!DOCTYPE") || rest.starts_with(b"<!doctype") {
                    scan.mode = ScanMode::Doctype {
                        brackets: 0,
                        quote: 0,
                    };
                    scan.offset = at + 1;
                } else if rest.starts_with(b"<?") {
                    scan.mode = ScanMode::Pi;
                    scan.offset = at + 2;
                } else if rest.starts_with(b"<!") {
                    if rest.len() < 9 && is_opener_prefix(rest) {
                        scan.offset = at;
                        return Ok(ScanEvent::NeedMore);
                    }
                    return Err(unreadable("a worksheet part is malformed"));
                } else if rest.starts_with(b"</") {
                    scan.tag_start = at;
                    match tag_content(rest) {
                        TagContent::Complete { end } => {
                            if let Some(event) = close_end_tag(scan, buf, at, end, &goal)? {
                                return Ok(event);
                            }
                        }
                        TagContent::BrokenAt(rel) => scan.offset = at + rel,
                        TagContent::Incomplete { quote } => {
                            scan.mode = ScanMode::Tag {
                                quote,
                                closing: true,
                            };
                            scan.offset = buf.len();
                            return Ok(ScanEvent::NeedMore);
                        }
                    }
                } else {
                    scan.tag_start = at;
                    match tag_content(rest) {
                        TagContent::Complete { end } => {
                            if let Some(event) = goal_event(scan, buf, at, end, &goal)? {
                                return Ok(event);
                            }
                            close_tag(scan, buf, at, end);
                        }
                        TagContent::BrokenAt(rel) => scan.offset = at + rel,
                        TagContent::Incomplete { quote } => {
                            scan.mode = ScanMode::Tag {
                                quote,
                                closing: false,
                            };
                            scan.offset = buf.len();
                            return Ok(ScanEvent::NeedMore);
                        }
                    }
                }
            }
            ScanMode::Comment => {
                let from = scan.offset.saturating_sub(2);
                let Some(rel) = find_bytes(&buf[from..], b"-->") else {
                    scan.offset = buf.len().saturating_sub(2).max(scan.offset);
                    return Ok(ScanEvent::NeedMore);
                };
                scan.mode = ScanMode::Markup;
                scan.offset = from + rel + 3;
                if matches!(goal, ScanGoal::Event) {
                    return Ok(ScanEvent::Event(Next::Other, scan.offset));
                }
            }
            ScanMode::CData => {
                let from = scan.offset.saturating_sub(2);
                let Some(rel) = find_bytes(&buf[from..], b"]]>") else {
                    scan.offset = buf.len().saturating_sub(2).max(scan.offset);
                    return Ok(ScanEvent::NeedMore);
                };
                scan.mode = ScanMode::Markup;
                scan.offset = from + rel + 3;
                if matches!(goal, ScanGoal::Event) {
                    return Ok(ScanEvent::Event(Next::Other, scan.offset));
                }
            }
            ScanMode::Tag { quote, closing } => {
                let mut quote = quote;
                let mut index = scan.offset;
                let mut broken = None;
                while index < buf.len() {
                    match buf[index] {
                        b'<' if quote == 0 => {
                            broken = Some(index);
                            break;
                        }
                        b'>' if quote == 0 => break,
                        q @ (b'"' | b'\'') if quote == 0 => quote = q,
                        q if q == quote => quote = 0,
                        _ => {}
                    }
                    index += 1;
                }
                if let Some(at) = broken {
                    scan.mode = ScanMode::Markup;
                    scan.offset = at;
                    continue;
                }
                if index >= buf.len() {
                    scan.mode = ScanMode::Tag { quote, closing };
                    scan.offset = buf.len();
                    return Ok(ScanEvent::NeedMore);
                }
                scan.mode = ScanMode::Markup;
                let at = scan.tag_start;
                if closing {
                    if let Some(event) = close_end_tag(scan, buf, at, index + 1 - at, &goal)? {
                        return Ok(event);
                    }
                    continue;
                }
                if let Some(event) = goal_event(scan, buf, at, index + 1 - at, &goal)? {
                    return Ok(event);
                }
                close_tag(scan, buf, at, index + 1 - at);
                scan.offset = index + 1;
            }
            ScanMode::Doctype {
                mut brackets,
                mut quote,
            } => {
                let mut index = scan.offset;
                let mut done = None;
                while index < buf.len() {
                    let byte = buf[index];
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
                        done = Some(index + 1);
                        break;
                    }
                    index += 1;
                }
                let Some(end) = done else {
                    scan.mode = ScanMode::Doctype { brackets, quote };
                    scan.offset = buf.len();
                    return Ok(ScanEvent::NeedMore);
                };
                scan.mode = ScanMode::Markup;
                scan.offset = end;
                if matches!(goal, ScanGoal::Event) {
                    return Ok(ScanEvent::Event(Next::Other, scan.offset));
                }
            }
            ScanMode::Pi => {
                let from = scan.offset.saturating_sub(1);
                let Some(rel) = find_bytes(&buf[from..], b"?>") else {
                    scan.offset = buf.len().saturating_sub(1).max(scan.offset);
                    return Ok(ScanEvent::NeedMore);
                };
                scan.mode = ScanMode::Markup;
                scan.offset = from + rel + 2;
                if matches!(goal, ScanGoal::Event) {
                    return Ok(ScanEvent::Event(Next::Other, scan.offset));
                }
            }
        }
    }
}

enum TagContent {
    Complete { end: usize },
    BrokenAt(usize),
    Incomplete { quote: u8 },
}

// The start tag at the slice start: its length when an unquoted `>` closes it, the
// offset of a bare `<` inside it, or the quote left open when the slice runs out.
fn tag_content(tag: &[u8]) -> TagContent {
    let mut quote = 0u8;
    let mut index = 1;
    while index < tag.len() {
        match tag[index] {
            b'<' if quote == 0 => return TagContent::BrokenAt(index),
            b'>' if quote == 0 => return TagContent::Complete { end: index + 1 },
            q @ (b'"' | b'\'') if quote == 0 => quote = q,
            q if q == quote => quote = 0,
            _ => {}
        }
        index += 1;
    }
    TagContent::Incomplete { quote }
}

fn close_tag(scan: &mut ForwardScan, buf: &[u8], at: usize, end: usize) {
    if buf[at + end - 2] != b'/' {
        scan.depth += 1;
    }
}

fn close_end_tag(
    scan: &mut ForwardScan,
    buf: &[u8],
    at: usize,
    end: usize,
    goal: &ScanGoal,
) -> Result<Option<ScanEvent>, ReadError> {
    // An end tag is read with the same leniency as the single-event parse: the
    // name runs to the first whitespace or `>`, anything else up to the `>`.
    if matches!(goal, ScanGoal::Event) {
        let Some((name, end)) = end_tag(&buf[at..]) else {
            return Err(unreadable("a worksheet part is malformed"));
        };
        let local = name.rsplit(|b| *b == b':').next().unwrap_or(&name);
        if local == b"sheetData" {
            return Ok(Some(ScanEvent::Event(Next::SheetDataEnd, at + end)));
        }
        return Ok(Some(ScanEvent::Event(Next::Other, at + end)));
    }
    if matches!(goal, ScanGoal::Row) && scan.depth == 1 {
        return Ok(Some(ScanEvent::RowDone(at + end)));
    }
    scan.depth = scan.depth.saturating_sub(1);
    scan.offset = at + end;
    Ok(None)
}

fn goal_event(
    scan: &mut ForwardScan,
    buf: &[u8],
    at: usize,
    end: usize,
    goal: &ScanGoal,
) -> Result<Option<ScanEvent>, ReadError> {
    let empty = buf[at + end - 2] == b'/';
    match goal {
        // One event, the way the single-event parse reads it: a row start is a
        // row, anything else complete is skipped over as another event.
        ScanGoal::Event => {
            let Some(name) = tag_name(&buf[at + 1..at + end]) else {
                return Err(unreadable("a worksheet part is malformed"));
            };
            let local = name.rsplit(|b| *b == b':').next().unwrap_or(name);
            if local == b"row" {
                return Ok(Some(ScanEvent::Event(Next::Row, at + end)));
            }
            return Ok(Some(ScanEvent::Event(Next::Other, at + end)));
        }
        ScanGoal::Head => {
            let name = tag_name(&buf[at + 1..at + end]);
            if scan.depth == 1
                && let Some(name) = name
                && local_is(name, b"sheetData")
            {
                if empty {
                    return Ok(Some(ScanEvent::Head(Head::Empty)));
                }
                return Ok(Some(ScanEvent::Head(Head::Found(at, at + end))));
            }
        }
        ScanGoal::Row => {
            if empty && scan.depth == 0 {
                return Ok(Some(ScanEvent::RowDone(at + end)));
            }
        }
    }
    scan.offset = at + end;
    Ok(None)
}

// The byte offset where <sheetData> content starts, found without parsing.
fn scan_head(scan: &mut ForwardScan, head: &[u8]) -> Result<Head, ReadError> {
    match advance(scan, head, ScanGoal::Head)? {
        ScanEvent::Head(found) => Ok(found),
        ScanEvent::NeedMore => Ok(Head::Incomplete),
        ScanEvent::RowDone(_) | ScanEvent::Event(..) => {
            Err(unreadable("a worksheet part is malformed"))
        }
    }
}

// The end of the <row> starting the buffer; None when more bytes may complete it.
fn scan_row_end(scan: &mut ForwardScan, buf: &[u8]) -> Result<Option<usize>, ReadError> {
    match advance(scan, buf, ScanGoal::Row)? {
        ScanEvent::RowDone(end) => Ok(Some(end)),
        ScanEvent::NeedMore => Ok(None),
        ScanEvent::Head(_) | ScanEvent::Event(..) => {
            Err(unreadable("a worksheet part is malformed"))
        }
    }
}

// The first event starting at the buffer with its end offset; None when more
// bytes may complete it. The scan resumes where the last fill stopped instead
// of re-parsing from the buffer start.
fn scan_event(scan: &mut ForwardScan, buf: &[u8]) -> Result<Option<(Next, usize)>, ReadError> {
    match advance(scan, buf, ScanGoal::Event)? {
        ScanEvent::Event(next, end) => Ok(Some((next, end))),
        ScanEvent::NeedMore => Ok(None),
        ScanEvent::Head(_) | ScanEvent::RowDone(_) => {
            Err(unreadable("a worksheet part is malformed"))
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// Blank XML text at the slice start, such as line breaks between rows.
fn blank_prefix_len(buf: &[u8]) -> usize {
    buf.iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .count()
}

// Whether the truncated slice could still grow into a comment, CDATA or doctype
// opener once the next chunk arrives.
fn is_opener_prefix(rest: &[u8]) -> bool {
    [
        b"<!--".as_slice(),
        b"<![CDATA[".as_slice(),
        b"<!DOCTYPE".as_slice(),
        b"<!doctype".as_slice(),
    ]
    .iter()
    .any(|opener| opener.starts_with(rest))
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
        stream_with(xml, &budgets()).unwrap()
    }

    #[test]
    fn pieces_scanned_into_one_sheet_share_their_strings() {
        let mut cells = SheetCells::default();
        for row in 1..=2 {
            let xml = format!(
                r#"<row r="{row}"><c r="A{row}" t="inlineStr"><is><t>same</t></is></c></row>"#
            );
            scan_rows_into(&mut cells, &xml, &[Vec::new()], "Sheet1").unwrap();
        }
        assert_eq!(cells.strings.len(), 1);
        assert_eq!(cells.data.row_count(), 2);
        assert_eq!(cells.string_cells.len(), 2);
    }

    #[test]
    fn rows_out_of_order_across_pieces_are_left_to_ironcalc() {
        let mut cells = SheetCells::default();
        let piece = |row: u32| format!(r#"<row r="{row}"><c r="A{row}"><v>1</v></c></row>"#);
        scan_rows_into(&mut cells, &piece(5), &[Vec::new()], "Sheet1").unwrap();
        let error = scan_rows_into(&mut cells, &piece(5), &[Vec::new()], "Sheet1").unwrap_err();
        assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
    }

    fn stream_with(xml: &str, budgets: &Budgets) -> Result<StreamedSheet, ReadError> {
        let meter = CompressedMeter::new();
        stream_worksheet(
            meter.wrap(xml.as_bytes()),
            &meter,
            "xl/worksheets/sheet1.xml",
            "Sheet1",
            budgets,
        )
    }

    #[test]
    fn an_unfinished_end_tag_resumes_instead_of_restarting() {
        let mut scan = ForwardScan::new();
        let first = [b"</".as_slice(), &[b'a'; 1024]].concat();
        assert!(matches!(
            advance(&mut scan, &first, ScanGoal::Row),
            Ok(ScanEvent::NeedMore)
        ));
        assert_eq!(scan.offset, first.len());
        let mut grown = first.clone();
        grown.extend_from_slice(&[b'a'; 1024]);
        assert!(matches!(
            advance(&mut scan, &grown, ScanGoal::Row),
            Ok(ScanEvent::NeedMore)
        ));
        assert_eq!(scan.offset, grown.len());
    }

    #[test]
    fn a_huge_unfinished_end_tag_does_not_hang_the_scan() {
        let mut scan = ForwardScan::new();
        let mut buf = b"</".to_vec();
        let started = std::time::Instant::now();
        for _ in 0..1024 {
            buf.extend_from_slice(&[b'a'; 65536]);
            assert!(matches!(
                advance(&mut scan, &buf, ScanGoal::Row),
                Ok(ScanEvent::NeedMore)
            ));
        }
        assert_eq!(scan.offset, buf.len());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_leading_end_tag_is_read_without_a_parser_stack() {
        assert!(matches!(
            classify(b"</sheetData>").unwrap(),
            Some((Next::SheetDataEnd, 12))
        ));
        assert!(matches!(
            classify(b"</x:sheetData >").unwrap(),
            Some((Next::SheetDataEnd, 15))
        ));
        assert!(matches!(
            classify(b"</other>").unwrap(),
            Some((Next::Other, 8))
        ));
        assert!(classify(b"</sheetData").unwrap().is_none());
        assert!(matches!(
            classify(b"  \r\n <row r=\"1\"/>").unwrap(),
            Some((Next::Row, 17))
        ));
    }

    #[test]
    fn the_head_scan_resumes_instead_of_restarting() {
        let part = format!(r#"{HEAD}<row r="1"><c r="A1"><v>1</v></c></row>{TAIL}"#);
        let bytes = part.as_bytes();
        let mut scan = ForwardScan::new();
        let cut = HEAD.len() / 2;
        assert!(matches!(
            scan_head(&mut scan, &bytes[..cut]),
            Ok(Head::Incomplete)
        ));
        assert!(scan.offset > 0);
        assert!(matches!(scan_head(&mut scan, bytes), Ok(Head::Found(_, _))));
    }

    #[test]
    fn the_row_scan_resumes_instead_of_restarting() {
        let row = format!(
            r#"<row r="1"><c r="A1"><v>{}</v></c></row>"#,
            "7".repeat(1000)
        );
        let bytes = row.as_bytes();
        let mut scan = ForwardScan::new();
        let cut = bytes.len() / 2;
        assert!(matches!(scan_row_end(&mut scan, &bytes[..cut]), Ok(None)));
        assert!(scan.offset > 0);
        assert!(matches!(
            scan_row_end(&mut scan, bytes),
            Ok(Some(end)) if end == bytes.len()
        ));
    }

    #[test]
    fn an_unclosed_first_row_is_an_error_not_a_hang() {
        for xml in [
            format!(r#"{HEAD}<row r="1"><c r="A1"><v>1</v></c>{TAIL}"#),
            format!(r#"{HEAD}<row r="1"><c r="A1"><v>1</v></c>"#),
        ] {
            let started = std::time::Instant::now();
            let error = stream_with(&xml, &budgets())
                .err()
                .expect("an unclosed row must fail");
            assert!(
                started.elapsed() < std::time::Duration::from_secs(30),
                "took {started:?}"
            );
            let _ = error;
        }
    }

    #[test]
    fn a_first_row_past_the_cap_goes_to_ironcalc() {
        let xml = format!(
            r#"{HEAD}<row r="1"><c r="A1"><v>{}</v>"#,
            "7".repeat(17 * 1024 * 1024)
        );
        let started = std::time::Instant::now();
        let error = stream_with(&xml, &budgets())
            .err()
            .expect("a row past the cap must go to IronCalc");
        assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "took {started:?}"
        );
    }

    #[test]
    fn a_huge_configuration_is_refused_without_rescanning() {
        let pad = " ".repeat(64 * 1024 * 1024);
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{pad}<sheetData></sheetData></worksheet>"#
        );
        let started = std::time::Instant::now();
        let error = stream_with(&xml, &budgets())
            .err()
            .expect("a 64 MB configuration must fail");
        assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "took {started:?}"
        );
    }

    #[test]
    fn an_unclosed_comment_is_an_error_not_a_hang() {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><!--{}"#,
            " ".repeat(30 * 1024 * 1024)
        );
        let started = std::time::Instant::now();
        let error = stream_with(&xml, &budgets())
            .err()
            .expect("an unclosed comment must fail");
        assert!(matches!(error, ReadError::Unreadable(_)), "{error}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "took {started:?}"
        );
    }

    fn config_with(xml: &str, budgets: &Budgets) -> Result<Vec<u8>, ReadError> {
        let meter = CompressedMeter::new();
        stream_config(
            meter.wrap(xml.as_bytes()),
            &meter,
            "xl/worksheets/sheet1.xml",
            budgets,
        )
    }

    #[test]
    fn a_row_number_is_read_past_the_tag_name() {
        assert!(row_numbered(br#"<row r="2" spans="1:4">"#));
        assert!(row_numbered(br#"<row r="2"/>"#));
        assert!(!row_numbered(br#"<row spans="1:4" outlineLevel="1">"#));
    }

    // rust_xlsxwriter writes every row on one line, so tags fall across chunk ends.
    fn one_line_row_tag(n: u32, extra: &str) -> String {
        format!(r#"<row r="{n}" spans="1:4"{extra}>"#)
    }

    fn one_line_rows(rows: u32, extra: &str) -> String {
        (2..rows + 2)
            .map(|n| {
                format!(
                    r#"{}<c r="A{n}"><v>{n}</v></c><c r="B{n}" t="s"><v>{}</v></c><c r="C{n}"><f>A{n}*2</f><v>{}</v></c><c r="D{n}"><v>0.25</v></c></row>"#,
                    one_line_row_tag(n, extra),
                    n % 200,
                    n * 2
                )
            })
            .collect()
    }

    #[test]
    fn a_config_is_read_wherever_the_chunks_cut_one_line_rows() {
        const ROWS: u32 = 1_500;
        let tail = r#"</sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#;
        for extra in ["", r#" outlineLevel="1""#] {
            let rows = one_line_rows(ROWS, extra);
            let kept: String = match extra {
                "" => String::new(),
                _ => (2..ROWS + 2).map(|n| one_line_row_tag(n, extra)).collect(),
            };
            let row_len = rows.find("</row>").unwrap() + "</row>".len();
            assert!(rows.len() > 3 * CHUNK_BYTES);
            for pad in 0..row_len {
                let head = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews{}><sheetView workbookViewId="0"/></sheetViews><sheetData>"#,
                    " ".repeat(pad)
                );
                let config = config_with(&format!("{head}{rows}{tail}"), &budgets())
                    .unwrap_or_else(|error| panic!("padding {pad}: {error}"));
                assert!(
                    String::from_utf8(config).unwrap() == format!("{head}{kept}{tail}"),
                    "padding {pad}, extra {extra:?}"
                );
            }
        }
    }

    #[test]
    fn a_row_tag_cut_by_a_chunk_in_a_row_longer_than_a_chunk_keeps_its_outline() {
        let tag = |n: u32| format!(r#"<row r="{n}" outlineLevel="1">"#);
        let text = "x".repeat(100_000);
        let row = |n: u32| {
            format!(
                r#"{}<c r="A{n}" t="inlineStr"><is><t>{text}</t></is></c></row>"#,
                tag(n)
            )
        };
        let head = |pad: usize| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews{}><sheetView workbookViewId="0"/></sheetViews><sheetData>"#,
                " ".repeat(pad)
            )
        };
        let second_tag_at = 2 * CHUNK_BYTES - 10;
        let pad = second_tag_at - head(0).len() - row(1).len();
        let rows: String = (1..=3).map(row).collect();
        let xml = format!("{}{rows}{TAIL}", head(pad));
        assert_eq!(xml.find(&tag(2)), Some(second_tag_at));
        let config = String::from_utf8(config_with(&xml, &budgets()).unwrap()).unwrap();
        let kept: String = (1..=3).map(tag).collect();
        assert!(
            config == format!("{}{kept}{TAIL}", head(pad)),
            "{}",
            &config[pad..]
        );
    }

    #[test]
    fn a_lone_angle_bracket_at_the_buffer_end_waits_for_the_next_chunk() {
        let row = br#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
        let mut scan = ForwardScan::new();
        let cut = row.iter().rposition(|b| *b == b'<').unwrap() + 1;
        assert!(matches!(scan_row_end(&mut scan, &row[..cut]), Ok(None)));
        assert!(matches!(
            scan_row_end(&mut scan, row),
            Ok(Some(end)) if end == row.len()
        ));
    }

    #[test]
    fn a_huge_unterminated_comment_after_sheet_data_is_capped() {
        let xml = format!(r#"{HEAD}<!--{}"#, " ".repeat(20 * 1024 * 1024));
        let started = std::time::Instant::now();
        let error =
            config_with(&xml, &budgets()).expect_err("a 20 MB unterminated comment must fail");
        assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_comment_before_sheet_data_still_opens() {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><!-- note --><sheetData><row r="1"><c r="A1"><v>7</v></c></row>{TAIL}"#
        );
        let streamed = stream(&xml);
        assert_eq!(streamed.cells.data.row_count(), 1);
    }

    #[test]
    fn rows_flow_into_cells_and_config_into_the_stub() {
        let xml = format!(
            r#"{HEAD}<row r="1"><c r="A1"><v>7</v></c><c r="B1"><f>A1*6</f></c></row>{TAIL}"#
        );
        let streamed = stream(&xml);
        assert_eq!(streamed.cells.data.row_count(), 1);
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
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(100));
        let error = stream_with(&xml, &budgets)
            .err()
            .expect("ten cells with huge strings must trip the budget");
        assert!(error.to_string().contains("needs at least"), "{error}");
    }

    #[test]
    fn a_row_past_the_sheet_end_is_an_error_not_an_overflow() {
        let xml = format!(
            r#"{HEAD}<row r="2147483647"><c r="A1"><f t="array" ref="A1:A2">1</f></c></row>{TAIL}"#
        );
        let error = stream_with(&xml, &budgets())
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
        let error = stream_with(&xml, &budgets)
            .err()
            .expect("a growing sheet must stop");
        let message = error.to_string();
        assert!(message.contains("needs at least"), "{message}");
    }

    #[test]
    fn outline_row_tags_count_against_the_budget() {
        let junk = "x".repeat(2048);
        let mut rows = String::new();
        for n in 1..=200 {
            rows.push_str(&format!(
                r#"<row r="{n}" outlineLevel="1" junk="{junk}"><c r="A{n}"><v>1</v></c></row>"#
            ));
        }
        let xml = format!(r#"{HEAD}{rows}{TAIL}"#);
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(500));
        let error = stream_with(&xml, &budgets)
            .err()
            .expect("outline tags with kilobytes of attributes must trip the budget");
        assert!(error.to_string().contains("needs at least"), "{error}");
    }

    #[test]
    fn outline_row_tags_in_the_config_count_against_the_budget() {
        let junk = "x".repeat(2048);
        let mut rows = String::new();
        for n in 1..=200 {
            rows.push_str(&format!(
                r#"<row r="{n}" outlineLevel="1" junk="{junk}"><c r="A{n}"><v>1</v></c></row>"#
            ));
        }
        let xml = format!(r#"{HEAD}{rows}{TAIL}"#);
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(500));
        let error = config_with(&xml, &budgets).expect_err("outline tags must trip the budget");
        assert!(error.to_string().contains("needs at least"), "{error}");
    }

    #[test]
    fn a_streamed_sheet_past_the_per_part_cap_hits_the_cell_budget() {
        let mut rows = String::new();
        for n in 1..=2000 {
            rows.push_str(&format!(r#"<row r="{n}"><c r="A{n}"><v>1</v></c></row>"#));
        }
        let xml = format!(r#"{HEAD}{rows}{TAIL}"#);
        assert!(xml.len() as u64 > 1024);
        let budgets = Budgets::with_entry_cap(CellBudget::max_cells_for_test(1000), 1024);
        let error = stream_with(&xml, &budgets)
            .err()
            .expect("a sheet over budget must stop");
        let message = error.to_string();
        assert!(message.contains("needs at least"), "{message}");
    }
}
