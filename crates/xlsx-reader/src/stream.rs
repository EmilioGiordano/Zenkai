use std::cell::Cell;
use std::io::{Read, Seek};
use std::num::NonZeroU64;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::ReadError;
use crate::limits::{self, CellBudget, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, Route, Usage};
use crate::nesting::{NestingCheck, is_xml_part};

pub(crate) const CHUNK_BYTES: usize = 64 * 1024;

const CHECKED_ELEMENTS: [&[u8]; 19] = [
    b"f",
    b"formula",
    b"formula1",
    b"formula2",
    b"definedName",
    b"hyperlinks",
    b"dataValidations",
    b"autoFilter",
    b"sheetProtection",
    b"customSheetViews",
    b"customWorkbookViews",
    b"scenarios",
    b"cellWatches",
    b"sparklineGroups",
    b"conditionalFormattings",
    b"slicerList",
    b"timelineRefs",
    b"workbookProtection",
    b"rPh",
];

// XML forbids entities or whitespace between '<' and an element name, so a part with
// no '<name' or '<prefix:name' start tag for these names cannot contain them; the
// full parse is skipped for it. Text that only looks like a tag merely costs a parse.
pub fn has_checked_element(bytes: &[u8]) -> bool {
    start_tag_names(bytes).any(is_checked)
}

fn is_checked(local: &[u8]) -> bool {
    CHECKED_ELEMENTS.contains(&local)
}

const LONGEST_CHECKED: usize = {
    let mut longest = 0;
    let mut index = 0;
    while index < CHECKED_ELEMENTS.len() {
        if CHECKED_ELEMENTS[index].len() > longest {
            longest = CHECKED_ELEMENTS[index].len();
        }
        index += 1;
    }
    longest
};

// Where the name that starts `rest`, right after a '<', ends; None when `rest` ends first.
fn name_end(rest: &[u8]) -> Option<usize> {
    rest.iter()
        .position(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/' | b'<'))
}

fn local_part(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

// The local name after every '<', read without parsing: an end tag gives an empty name and a
// tag inside a comment counts too.
pub fn start_tag_names(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split(|b| *b == b'<').skip(1).map(|rest| {
        let name = &rest[..name_end(rest).unwrap_or(rest.len())];
        local_part(name)
    })
}

// Watches streamed bytes for the elements above, so parts without any are never inflated
// whole. A tag name a chunk ends in is carried into the next chunk however long its prefix
// is: only its local part after the last ':' is kept, and of that no more than a checked
// name could take, so the state stays a few bytes.
pub(crate) struct Prefilter {
    // The local part read so far of the name the last chunk ended in.
    unfinished: Option<Vec<u8>>,
    hit: bool,
}

impl Prefilter {
    pub(crate) fn new() -> Prefilter {
        Prefilter {
            unfinished: None,
            hit: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.hit {
            return;
        }
        let mut rest = chunk;
        if let Some(mut local) = self.unfinished.take() {
            let end = name_end(rest);
            keep_local(&mut local, &rest[..end.unwrap_or(rest.len())]);
            let Some(end) = end else {
                self.unfinished = Some(local);
                return;
            };
            if is_checked(&local) {
                self.hit = true;
                return;
            }
            rest = &rest[end..];
        }
        while let Some(at) = rest.iter().position(|b| *b == b'<') {
            rest = &rest[at + 1..];
            let Some(end) = name_end(rest) else {
                let mut local = Vec::with_capacity(LONGEST_CHECKED + 1);
                keep_local(&mut local, rest);
                self.unfinished = Some(local);
                return;
            };
            if is_checked(local_part(&rest[..end])) {
                self.hit = true;
                return;
            }
        }
    }

    // A name the bytes end in counts, as `has_checked_element` counts it.
    pub(crate) fn hit(&self) -> bool {
        self.hit || self.unfinished.as_deref().is_some_and(is_checked)
    }
}

fn keep_local(local: &mut Vec<u8>, piece: &[u8]) {
    let piece = match piece.iter().rposition(|b| *b == b':') {
        Some(colon) => {
            local.clear();
            &piece[colon + 1..]
        }
        None => piece,
    };
    let room = (LONGEST_CHECKED + 1).saturating_sub(local.len());
    local.extend_from_slice(&piece[..piece.len().min(room)]);
}

// Counts worksheet cells, formulas and rows while their bytes stream past, so
// the IronCalc fallback can charge them to the memory budget without inflating
// the part. A tag counts when its local name after the last `:` is `c`, `f` or
// `row`, the elements the readers match by local name; text and attribute values
// cannot hold a raw `<`, so only comments and character data could fake a tag,
// and both are skipped. Anything else that looks like a tag over-counts, which
// fails closed.
pub(crate) struct CellCounter {
    cells: u64,
    formulas: u64,
    rows: u64,
    comment: bool,
    cdata: bool,
    tag: Option<TagName>,
    held: Vec<u8>,
}

// The local name of the open tag being scanned. A prefix of any length only
// resets the few bytes compared, so a tag split across chunks still counts
// without holding the whole tag.
#[derive(Default)]
struct TagName {
    local: [u8; 3],
    len: u8,
    oversized: bool,
}

enum TagEnd {
    Cell,
    Formula,
    Row,
    Other,
}

impl TagName {
    fn push(&mut self, byte: u8) -> Option<TagEnd> {
        if byte == b':' {
            self.len = 0;
            self.oversized = false;
            None
        } else if is_delimiter(byte) {
            Some(if self.oversized {
                TagEnd::Other
            } else if self.len == 1 && self.local[0] == b'c' {
                TagEnd::Cell
            } else if self.len == 1 && self.local[0] == b'f' {
                TagEnd::Formula
            } else if self.len == 3 && self.local == *b"row" {
                TagEnd::Row
            } else {
                TagEnd::Other
            })
        } else if byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'-' | b'.')
            || !byte.is_ascii()
        {
            if !self.oversized {
                if self.len < 3 {
                    self.local[usize::from(self.len)] = byte;
                    self.len += 1;
                } else {
                    self.oversized = true;
                }
            }
            None
        } else {
            Some(TagEnd::Other)
        }
    }
}

// The longest opener this distinguishes (`<![CDATA[`); a shorter tail is held
// for the next chunk instead of judged half-seen.
const COUNTER_LOOKAHEAD: usize = 9;

impl CellCounter {
    pub(crate) fn new() -> CellCounter {
        CellCounter {
            cells: 0,
            formulas: 0,
            rows: 0,
            comment: false,
            cdata: false,
            tag: None,
            held: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.held.extend_from_slice(chunk);
        self.process(false);
    }

    pub(crate) fn finish(&mut self) {
        self.process(true);
        self.tag = None;
        self.held.clear();
    }

    pub(crate) fn usage(&self) -> Usage {
        Usage {
            cells: self.cells,
            formulas: self.formulas,
            shapes: 0,
            bytes: 0,
            rows: self.rows,
        }
    }

    fn process(&mut self, last: bool) {
        let mut i = 0;
        while i < self.held.len() {
            if self.comment {
                match find_terminator(&self.held[i..], b"-->") {
                    Some(end) => {
                        self.comment = false;
                        i += end + 3;
                    }
                    None if last => i = self.held.len(),
                    None => {
                        let keep = (self.held.len() - i).min(2);
                        i = self.held.len() - keep;
                        break;
                    }
                }
                continue;
            }
            if self.cdata {
                match find_terminator(&self.held[i..], b"]]>") {
                    Some(end) => {
                        self.cdata = false;
                        i += end + 3;
                    }
                    None if last => i = self.held.len(),
                    None => {
                        let keep = (self.held.len() - i).min(2);
                        i = self.held.len() - keep;
                        break;
                    }
                }
                continue;
            }
            if self.tag.is_some() {
                let byte = self.held[i];
                if byte == b'<' {
                    self.tag = None;
                    continue;
                }
                if let Some(tag) = self.tag.as_mut() {
                    match tag.push(byte) {
                        None => {}
                        Some(TagEnd::Cell) => {
                            self.cells += 1;
                            self.tag = None;
                        }
                        Some(TagEnd::Formula) => {
                            self.formulas += 1;
                            self.tag = None;
                        }
                        Some(TagEnd::Row) => {
                            self.rows += 1;
                            self.tag = None;
                        }
                        Some(TagEnd::Other) => self.tag = None,
                    }
                }
                i += 1;
                continue;
            }
            if self.held[i] != b'<' {
                i += 1;
                continue;
            }
            let rest = &self.held[i..];
            if !last && rest.len() < COUNTER_LOOKAHEAD {
                break;
            }
            if rest.starts_with(b"<!--") {
                self.comment = true;
                i += 4;
            } else if rest.starts_with(b"<![CDATA[") {
                self.cdata = true;
                i += 9;
            } else if rest.starts_with(b"</") {
                i += 2;
            } else {
                self.tag = Some(TagName::default());
                i += 1;
            }
        }
        self.held.drain(..i);
    }
}

fn find_terminator(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>')
}

// Counts the bytes really pulled from the archive underneath the zip reader, so the
// compression ratio is measured, never taken from the forgeable headers. Read-ahead
// inside the decoder can attribute a few kilobytes to the wrong entry; against the
// 64 MB gate that never decides a refusal.
#[derive(Clone)]
pub(crate) struct CompressedMeter(Rc<Cell<u64>>);

impl CompressedMeter {
    pub(crate) fn new() -> CompressedMeter {
        CompressedMeter(Rc::new(Cell::new(0)))
    }

    pub(crate) fn wrap<R>(&self, inner: R) -> Counted<R> {
        Counted {
            inner,
            count: self.0.clone(),
        }
    }

    pub(crate) fn snapshot(&self) -> u64 {
        self.0.get()
    }

    pub(crate) fn used_since(&self, start: u64) -> u64 {
        self.snapshot().saturating_sub(start)
    }
}

pub(crate) struct Counted<R> {
    inner: R,
    count: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.count.set(self.count.get().saturating_add(read as u64));
        Ok(read)
    }
}

impl<R: Seek> Seek for Counted<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

pub(crate) struct Budgets {
    total: AtomicU64,
    cells: CellBudget,
    used_cells: AtomicU64,
    used_formulas: AtomicU64,
    used_shapes: AtomicU64,
    used_bytes: AtomicU64,
    used_rows: AtomicU64,
    // The worksheet bytes the archive declares and the bytes read from them so far,
    // only to report what a refused file would have needed.
    sheet_declared: AtomicU64,
    sheet_read: AtomicU64,
    entry_cap: u64,
    // Set once the open has failed, so worksheets still streaming on other threads stop.
    cancelled: AtomicBool,
    // Raised by the caller to abandon the read; checked wherever `cancelled` is.
    stop: Arc<AtomicBool>,
}

impl Budgets {
    pub(crate) fn production() -> Budgets {
        Self::with_cells(CellBudget::production())
    }

    pub(crate) fn with_cells(cells: CellBudget) -> Budgets {
        Self::with_limits(cells, MAX_ENTRY_BYTES)
    }

    #[cfg(test)]
    pub(crate) fn with_entry_cap(cells: CellBudget, entry_cap: u64) -> Budgets {
        Self::with_limits(cells, entry_cap)
    }

    fn with_limits(cells: CellBudget, entry_cap: u64) -> Budgets {
        Budgets {
            total: AtomicU64::new(MAX_TOTAL_BYTES),
            cells,
            used_cells: AtomicU64::new(0),
            used_formulas: AtomicU64::new(0),
            used_shapes: AtomicU64::new(0),
            used_bytes: AtomicU64::new(0),
            used_rows: AtomicU64::new(0),
            sheet_declared: AtomicU64::new(0),
            sheet_read: AtomicU64::new(0),
            entry_cap,
            cancelled: AtomicBool::new(false),
            stop: Arc::default(),
        }
    }

    pub(crate) fn with_stop(mut self, stop: Arc<AtomicBool>) -> Budgets {
        self.stop = stop;
        self
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed)
    }

    pub(crate) fn expect_sheet_bytes(&self, declared: u64) {
        self.sheet_declared
            .store(declared.min(MAX_TOTAL_BYTES), Ordering::Relaxed);
    }

    pub(crate) fn check_usage(&self, additional: Usage) -> Result<(), ReadError> {
        self.charge_usage(additional, Route::Streaming)
    }

    // The fallback walk counts cells without keeping them, but IronCalc's importer
    // will hold them far less compactly than the streaming reader.
    pub(crate) fn check_importer_usage(&self, additional: Usage) -> Result<(), ReadError> {
        self.charge_usage(additional, Route::Importer)
    }

    fn charge_usage(&self, additional: Usage, route: Route) -> Result<(), ReadError> {
        let cells = self
            .used_cells
            .fetch_add(additional.cells, Ordering::Relaxed)
            .saturating_add(additional.cells);
        let formulas = self
            .used_formulas
            .fetch_add(additional.formulas, Ordering::Relaxed)
            .saturating_add(additional.formulas);
        let shapes = self
            .used_shapes
            .fetch_add(additional.shapes, Ordering::Relaxed)
            .saturating_add(additional.shapes);
        let bytes = self
            .used_bytes
            .fetch_add(additional.bytes, Ordering::Relaxed)
            .saturating_add(additional.bytes);
        let rows = self
            .used_rows
            .fetch_add(additional.rows, Ordering::Relaxed)
            .saturating_add(additional.rows);
        let declared = self.sheet_declared.load(Ordering::Relaxed);
        let read = self.sheet_read.load(Ordering::Relaxed);
        self.cells.check_route(
            Usage {
                cells,
                formulas,
                shapes,
                bytes,
                rows,
            },
            route,
            (declared > 0).then_some((declared, read)),
        )
    }

    fn charge(&self, bytes: u64) -> Result<(), ReadError> {
        self.total
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(bytes)
            })
            .map(|_| ())
            .map_err(|_| {
                ReadError::Unsafe(
                    "the workbook expands past the size the reader allows".to_string(),
                )
            })
    }

    // A part a save just wrote is charged as reading it back would charge it. Its
    // compression ratio was checked as it was written.
    pub(crate) fn admit_written(&self, part: &str, expanded: u64) -> Result<(), ReadError> {
        self.charge(expanded)?;
        if expanded > self.entry_cap {
            return Err(past_entry_cap(part, self.entry_cap));
        }
        Ok(())
    }

    pub(crate) fn admit_written_worksheet(&self, expanded: u64) -> Result<(), ReadError> {
        self.charge(expanded)
    }

    pub(crate) fn stream<R: Read>(
        &self,
        inner: R,
        meter: &CompressedMeter,
        part: &str,
    ) -> Guard<'_, R> {
        self.guard(inner, meter, part, true, true)
    }

    pub(crate) fn stream_reread<R: Read>(
        &self,
        inner: R,
        meter: &CompressedMeter,
        part: &str,
    ) -> Guard<'_, R> {
        self.guard(inner, meter, part, false, true)
    }

    // Worksheets read row by row skip the per-part cap: their cells charge the
    // memory budget as they stream past, while the ratio and the total still
    // bound them. Every other part streams through `stream` instead.
    pub(crate) fn stream_worksheet<R: Read>(
        &self,
        inner: R,
        meter: &CompressedMeter,
        part: &str,
    ) -> Guard<'_, R> {
        self.guard(inner, meter, part, true, false)
    }

    fn guard<R: Read>(
        &self,
        inner: R,
        meter: &CompressedMeter,
        part: &str,
        charge_total: bool,
        cap_entry: bool,
    ) -> Guard<'_, R> {
        Guard {
            inner,
            budgets: self,
            meter: meter.clone(),
            start: meter.snapshot(),
            charge_total,
            cap_entry,
            expanded: 0,
            part: part.to_string(),
        }
    }
}

pub(crate) struct Guard<'b, R> {
    inner: R,
    budgets: &'b Budgets,
    meter: CompressedMeter,
    start: u64,
    charge_total: bool,
    cap_entry: bool,
    expanded: u64,
    part: String,
}

fn past_entry_cap(part: &str, cap: u64) -> ReadError {
    ReadError::Unsafe(format!(
        "part {part} expands past the {} per-part limit",
        limits::describe_bytes(cap),
    ))
}

fn whole_too_big(part: &str) -> ReadError {
    ReadError::Unsafe(format!(
        "part {part} is larger than the {} kept whole",
        limits::describe_bytes(limits::MAX_WHOLE_PART_BYTES),
    ))
}

impl<R: Read> Guard<'_, R> {
    pub(crate) fn whole_fits(&self, bytes: u64) -> Result<(), ReadError> {
        if bytes > limits::MAX_WHOLE_PART_BYTES {
            return Err(whole_too_big(&self.part));
        }
        Ok(())
    }

    pub(crate) fn check_whole(&self, bytes: u64) -> Result<(), ReadError> {
        self.whole_fits(bytes)?;
        self.charge_whole(bytes)
    }

    fn charge_whole(&self, bytes: u64) -> Result<(), ReadError> {
        self.budgets.check_usage(Usage {
            cells: 0,
            formulas: 0,
            shapes: 0,
            bytes,
            rows: 0,
        })
    }

    fn compressed_used(&self) -> u64 {
        self.meter.used_since(self.start)
    }

    pub(crate) fn read_chunk(&mut self, buf: &mut [u8]) -> Result<usize, ReadError> {
        if self.budgets.cancelled() {
            return Err(ReadError::Unreadable(
                "the file is no longer being read".to_string(),
            ));
        }
        let read = self
            .inner
            .read(buf)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        if read == 0 {
            return Ok(0);
        }
        let read = read as u64;
        if self.charge_total {
            self.budgets.charge(read)?;
        }
        self.expanded = self.expanded.saturating_add(read);
        if !self.cap_entry {
            self.budgets.sheet_read.fetch_add(read, Ordering::Relaxed);
        }
        if self.cap_entry && self.expanded > self.budgets.entry_cap {
            return Err(past_entry_cap(&self.part, self.budgets.entry_cap));
        }
        limits::check_ratio(&self.part, self.compressed_used(), self.expanded)?;
        Ok(read as usize)
    }
}

pub(crate) fn read_part<R: Read>(
    entry: R,
    meter: &CompressedMeter,
    part: &str,
    budgets: &Budgets,
) -> Result<Vec<u8>, ReadError> {
    read_part_with(
        budgets.stream(entry, meter, part),
        part,
        WholeLimit::KeptWhole,
        true,
    )
}

pub(crate) fn read_part_to_peek<R: Read>(
    entry: R,
    meter: &CompressedMeter,
    part: &str,
    budgets: &Budgets,
) -> Result<Vec<u8>, ReadError> {
    read_part_with(
        budgets.stream_reread(entry, meter, part),
        part,
        WholeLimit::Peek,
        true,
    )
}

#[derive(Clone, Copy)]
enum WholeLimit {
    KeptWhole,
    // A worksheet IronCalc reads: IronCalc holds it whole anyway, and the DOM charge
    // already bounded it before this read, so the cap for other parts kept whole
    // would only refuse large valid sheets. The per-part cap still applies.
    DomCharged,
    // A part the walk reads again later: this read only has to fit, so the charge for
    // its bytes is made once, by that later read.
    Peek,
}

// `nest_checked` is false for a part another pass over it has already counted.
fn read_part_with<R: Read>(
    mut guard: Guard<'_, R>,
    part: &str,
    limit: WholeLimit,
    nest_checked: bool,
) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::with_capacity(CHUNK_BYTES);
    let mut chunk = [0u8; CHUNK_BYTES];
    let mut nesting = (nest_checked && is_xml_part(part)).then(|| NestingCheck::new(part));
    loop {
        match guard.read_chunk(&mut chunk)? {
            0 => {
                limits::check_ratio(part, guard.compressed_used(), guard.expanded)?;
                match limit {
                    WholeLimit::KeptWhole => guard.check_whole(bytes.len() as u64)?,
                    WholeLimit::DomCharged => guard.charge_whole(bytes.len() as u64)?,
                    WholeLimit::Peek => guard.whole_fits(bytes.len() as u64)?,
                }
                return Ok(bytes);
            }
            read => {
                if let Some(nesting) = &mut nesting {
                    nesting.push(&chunk[..read])?;
                }
                bytes.extend_from_slice(&chunk[..read]);
                if !matches!(limit, WholeLimit::DomCharged) {
                    guard.whole_fits(bytes.len() as u64)?;
                }
            }
        }
    }
}

// Streams one part through the budgets and inflates it whole only when it may hold
// something the preflight checks; anything else is decided by entry name and needs
// no bytes. A re-read part is measured again but not charged twice to the total.
// Every part here ends up in the stub IronCalc DOM-parses, prefilter hit or not, so
// the whole-part cap and the DOM charge apply to the bytes really inflated. The
// shared strings take the DOM factor too: no measurement backs a lower one.
// With cell counting the cells and rows stream past a counter on the same pass,
// so the caller can charge them without reading the part again. The DOM charge is
// multiplied by `dom_loads`, the times the importer parses the part.
pub(crate) fn read_part_if_checked<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    meter: &CompressedMeter,
    budgets: &Budgets,
    count_cells: bool,
    dom_loads: NonZeroU64,
) -> Result<(String, Vec<u8>, Usage), ReadError> {
    let (name, usage) = {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let name = entry.name().to_string();
        let mut guard = budgets.stream(entry, meter, &name);
        let mut filter = Prefilter::new();
        let mut counter = CellCounter::new();
        let mut nesting = is_xml_part(&name).then(|| NestingCheck::new(&name));
        let mut chunk = [0u8; CHUNK_BYTES];
        let mut inflated = 0u64;
        loop {
            match guard.read_chunk(&mut chunk)? {
                0 => break,
                read => {
                    inflated += read as u64;
                    if !count_cells {
                        guard.whole_fits(inflated)?;
                    }
                    if let Some(nesting) = &mut nesting {
                        nesting.push(&chunk[..read])?;
                    }
                    filter.push(&chunk[..read]);
                    if count_cells {
                        counter.push(&chunk[..read]);
                    }
                }
            }
        }
        counter.finish();
        budgets
            .cells
            .check_dom_part(&name, inflated.saturating_mul(dom_loads.get()))?;
        let usage = counter.usage();
        if !filter.hit() {
            return Ok((name, Vec::new(), usage));
        }
        (name, usage)
    };
    let limit = match count_cells {
        true => WholeLimit::DomCharged,
        false => WholeLimit::KeptWhole,
    };
    let entry = archive
        .by_name(&name)
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    Ok((
        name.clone(),
        read_part_with(
            budgets.stream_reread(entry, meter, &name),
            &name,
            limit,
            false,
        )?,
        usage,
    ))
}

// XML forbids control characters and noncharacters anywhere, so they are checked on
// every chunk as it streams; a trailing EF or EF BF is held back because the
// three-byte noncharacter may span two chunks.
pub(crate) struct CharacterCheck {
    held: Vec<u8>,
}

impl CharacterCheck {
    pub(crate) fn new() -> Self {
        CharacterCheck { held: Vec::new() }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), ReadError> {
        for &byte in bytes {
            if byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r') {
                return Err(ReadError::Unsupported("a control character".to_string()));
            }
        }
        let mut scan = std::mem::take(&mut self.held);
        scan.extend_from_slice(bytes);
        if scan
            .windows(3)
            .any(|w| w == [0xEF, 0xBF, 0xBE] || w == [0xEF, 0xBF, 0xBF])
        {
            return Err(ReadError::Unsupported("a noncharacter".to_string()));
        }
        let hold = if scan.ends_with(&[0xEF, 0xBF]) {
            2
        } else if scan.ends_with(&[0xEF]) {
            1
        } else {
            0
        };
        self.held = scan[scan.len() - hold..].to_vec();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets() -> Budgets {
        Budgets::with_cells(CellBudget::max_cells_for_test(u64::MAX))
    }

    fn read(bytes: &[u8], part: &str, budgets: &Budgets) -> Result<Vec<u8>, ReadError> {
        let meter = CompressedMeter::new();
        read_part(meter.wrap(bytes), &meter, part, budgets)
    }

    #[test]
    fn whole_parts_past_the_smaller_cap_are_refused() {
        let budgets = budgets();
        let data = vec![7u8; 65 * 1024 * 1024];
        let error = read(&data, "xl/workbook.xml", &budgets).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("kept whole"), "{error}");
    }

    #[test]
    fn whole_part_bytes_count_against_the_memory_budget() {
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(0));
        let error = read(b"<workbook/>", "xl/workbook.xml", &budgets).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("needs about"), "{error}");
    }

    #[test]
    fn the_fallback_walk_is_charged_at_the_importer_cost() {
        let usage = Usage {
            cells: 300,
            formulas: 0,
            shapes: 0,
            bytes: 0,
            rows: 0,
        };
        let streaming = Budgets::with_cells(CellBudget::max_cells_for_test(1_000));
        assert!(streaming.check_usage(usage).is_ok());
        let importer = Budgets::with_cells(CellBudget::max_cells_for_test(1_000));
        let error = importer.check_importer_usage(usage).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert!(error.to_string().contains("needs about"), "{error}");
    }

    #[test]
    fn declared_worksheet_sizes_only_shape_the_refusal_message() {
        let usage = Usage {
            cells: 1_001,
            formulas: 0,
            shapes: 0,
            bytes: 0,
            rows: 0,
        };
        let budgets = Budgets::with_cells(CellBudget::max_cells_for_test(1_000));
        budgets.expect_sheet_bytes(u64::MAX);
        assert!(budgets.check_usage(usage).is_err());
        let roomy = Budgets::with_cells(CellBudget::max_cells_for_test(2_000));
        roomy.expect_sheet_bytes(u64::MAX);
        assert!(roomy.check_usage(usage).is_ok());
    }

    #[test]
    fn parts_past_the_per_part_cap_are_refused() {
        let budgets = Budgets::with_entry_cap(CellBudget::max_cells_for_test(u64::MAX), 1024);
        let data = vec![7u8; 2048];
        let error = read(&data, "xl/sharedStrings.xml", &budgets).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)));
        assert!(error.to_string().contains("per-part limit"), "{error}");
        let roomy = Budgets::with_cells(CellBudget::max_cells_for_test(u64::MAX));
        assert_eq!(
            read(&data, "xl/sharedStrings.xml", &roomy).unwrap().len(),
            2048
        );
    }

    #[test]
    fn written_parts_are_capped_like_parts_read_back() {
        let budgets = Budgets::with_entry_cap(CellBudget::max_cells_for_test(u64::MAX), 1024);
        budgets.admit_written("xl/styles.xml", 1024).unwrap();
        let error = budgets.admit_written("xl/styles.xml", 1025).unwrap_err();
        assert!(error.to_string().contains("per-part limit"), "{error}");
        budgets.admit_written_worksheet(1025).unwrap();
        let left = MAX_TOTAL_BYTES - 1024 - 1025 - 1025;
        budgets.admit_written_worksheet(left).unwrap();
        let error = budgets.admit_written_worksheet(1).unwrap_err();
        assert!(error.to_string().contains("expands past"), "{error}");
    }

    #[test]
    fn streamed_worksheets_skip_the_per_part_cap_but_keep_the_ratio() {
        let budgets = Budgets::with_entry_cap(CellBudget::max_cells_for_test(u64::MAX), 1024);
        let meter = CompressedMeter::new();
        let mut guard =
            budgets.stream_worksheet(std::io::repeat(7u8), &meter, "xl/worksheets/sheet1.xml");
        let mut chunk = [0u8; CHUNK_BYTES];
        let error = loop {
            match guard.read_chunk(&mut chunk) {
                Ok(_) => {}
                Err(error) => break error,
            }
        };
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        let message = error.to_string();
        assert!(!message.contains("per-part"), "{message}");
        assert!(message.contains("150"), "{message}");
    }

    #[test]
    fn the_prefilter_sees_tags_split_across_chunks() {
        let mut filter = Prefilter::new();
        filter.push(b"<worksheet><sheetData><row><c><");
        assert!(!filter.hit());
        filter.push(b"f>A1*2</f></c></row>");
        assert!(filter.hit());
    }

    fn filtered(bytes: &[u8], cuts: &[usize]) -> bool {
        let mut filter = Prefilter::new();
        let mut start = 0;
        for &cut in cuts.iter().chain([bytes.len()].iter()) {
            let cut = cut.clamp(start, bytes.len());
            filter.push(&bytes[start..cut]);
            start = cut;
        }
        filter.hit()
    }

    const TAG_START: usize = 17;

    fn tag(prefix_length: usize, name: &[u8]) -> Vec<u8> {
        let mut bytes = b"<root><x>text</x><".to_vec();
        if prefix_length > 0 {
            bytes.extend(vec![b'p'; prefix_length]);
            bytes.push(b':');
        }
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(b" a=\"1\"/></root>");
        bytes
    }

    // A namespace prefix longer than a fixed overlap between chunks would hide the name behind
    // it when a chunk boundary falls inside the tag.
    #[test]
    fn long_prefixes_across_chunk_boundaries_hide_no_checked_element() {
        for name in CHECKED_ELEMENTS {
            for prefix_length in [0, 1, 63, 64, 65, 200, CHUNK_BYTES + 3] {
                let bytes = tag(prefix_length, name);
                let colon = TAG_START + 1 + prefix_length;
                let mut cuts: Vec<usize> = (TAG_START..bytes.len())
                    .step_by(1 + prefix_length / 64)
                    .collect();
                cuts.extend(colon - 1..(colon + name.len() + 2).min(bytes.len()));
                for cut in cuts {
                    assert!(filtered(&bytes, &[cut]), "{prefix_length} at {cut}");
                }
                let chunks: Vec<usize> = (0..bytes.len()).step_by(CHUNK_BYTES / 3).collect();
                assert!(filtered(&bytes, &chunks), "{prefix_length}");
            }
        }
        let unchecked: [&[u8]; 3] = [b"fonts", b"formula11", b"xf"];
        for unchecked in unchecked {
            for prefix_length in [0, 70, 300] {
                let bytes = tag(prefix_length, unchecked);
                for cut in 0..bytes.len().min(400) {
                    assert!(!filtered(&bytes, &[cut]));
                }
            }
        }
        let parts: [&[u8]; 3] = [b"<", &[b'q'; 300], b":definedName>1</definedName>"];
        assert!(filtered(&parts.concat(), &[150]));
    }

    proptest::proptest! {
        #[test]
        fn any_split_of_a_prefixed_checked_tag_is_seen(
            name in 0..CHECKED_ELEMENTS.len(),
            prefix_length in 0usize..300,
            cuts in proptest::collection::vec(0usize..400, 0..6),
        ) {
            let mut cuts = cuts;
            cuts.sort_unstable();
            let bytes = tag(prefix_length, CHECKED_ELEMENTS[name]);
            proptest::prop_assert!(filtered(&bytes, &cuts));
            proptest::prop_assert_eq!(filtered(&bytes, &cuts), has_checked_element(&bytes));
        }
    }

    #[test]
    fn the_prefilter_ignores_lookalikes_and_clean_parts() {
        let mut filter = Prefilter::new();
        filter.push(b"<fonts><font><sz val=\"11\"/></font></fonts>");
        assert!(!filter.hit());
        let mut split = Prefilter::new();
        split.push(b"<defined");
        assert!(!split.hit());
        split.push(b"Name name=\"x\">1</definedName>");
        assert!(split.hit());
    }

    #[test]
    fn phonetic_runs_are_found_and_phonetic_settings_alone_are_not() {
        let mut settings = Prefilter::new();
        settings.push(b"<sst><si><t>x</t><phoneticPr fontId=\"0\"/></si></sst>");
        assert!(!settings.hit());
        let mut runs = Prefilter::new();
        runs.push(b"<sst><si><t>x</t><rPh sb=\"0\" eb=\"1\"><t>y</t></rPh></si></sst>");
        assert!(runs.hit());
    }

    #[test]
    fn tag_names_are_read_without_their_prefix() {
        let names: Vec<&[u8]> =
            start_tag_names(b"<?xml?><x:sst><x:si/><!-- <rPh --></x:sst>").collect();
        let expected: [&[u8]; 6] = [b"?xml?", b"sst", b"si", b"!--", b"rPh", b""];
        assert_eq!(names, expected);
    }

    #[test]
    fn the_cell_counter_sees_tags_split_across_chunks() {
        let mut counter = CellCounter::new();
        counter.push(b"<worksheet><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row><ro");
        counter.push(b"w r=\"2\"/><c/></sheetData>");
        counter.finish();
        let usage = counter.usage();
        assert_eq!((usage.cells, usage.rows), (2, 2));
    }

    #[test]
    fn the_cell_counter_counts_prefixed_rows_and_cells() {
        let mut counter = CellCounter::new();
        counter.push(b"<sheetData xmlns:x=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><x:row r=\"1\"><x:c r=\"A1\"/><x:r");
        counter.push(b"ow r=\"2\"/><x:cols><x:col min=\"1\"/></x:cols><c:v>not a cell</c:v></x:row><row r=\"3\"><c r=\"A3\"/></row></sheetData>");
        counter.finish();
        let usage = counter.usage();
        assert_eq!((usage.cells, usage.rows), (2, 3));
    }

    #[test]
    fn the_cell_counter_counts_plain_and_prefixed_formulas() {
        let mut counter = CellCounter::new();
        counter
            .push(b"<sheetData><row r=\"1\"><c r=\"A1\"><f>A1*2</f></c><x:c xmlns:x=\"urn:x\"><x:");
        counter.push(b"f>SUM(A1:A2)</x:f></x:c></row></sheetData><fonts><font/><formula1>x</formula1></fonts>");
        counter.finish();
        assert_eq!(counter.usage().formulas, 2);
    }

    #[test]
    fn the_cell_counter_skips_comments_cdata_and_lookalikes() {
        let mut counter = CellCounter::new();
        counter.push(b"<worksheet><!-- <c r=\"A1\"><row r=\"1\"> --><cols><col min=\"1\"/><conditionalFormatting><cfRule/></conditionalFormatting></cols>");
        counter.push(b"<sheetData><row r=\"1\"><c r=\"A1\"><f><![CDATA[<c><row]]></f></c></row></sheetData></worksheet>");
        counter.finish();
        let usage = counter.usage();
        assert_eq!((usage.cells, usage.rows), (1, 1));
    }

    #[test]
    fn parts_without_checked_elements_are_not_inflated() {
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::FileOptions::default();
        writer.start_file("xl/styles.xml", options).unwrap();
        writer
            .write_all(b"<styleSheet><fonts count=\"1\"/></styleSheet>")
            .unwrap();
        writer.start_file("xl/workbook.xml", options).unwrap();
        writer
            .write_all(b"<workbook><definedNames><definedName name=\"x\">1</definedName></definedNames></workbook>")
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(Cursor::new(bytes.as_slice()))).unwrap();
        let budgets = budgets();
        let (name, clean, _) =
            read_part_if_checked(&mut archive, 0, &meter, &budgets, false, NonZeroU64::MIN)
                .unwrap();
        assert_eq!(name, "xl/styles.xml");
        assert!(clean.is_empty(), "a clean part needs no bytes");
        let (name, checked, _) =
            read_part_if_checked(&mut archive, 1, &meter, &budgets, false, NonZeroU64::MIN)
                .unwrap();
        assert_eq!(name, "xl/workbook.xml");
        assert!(!checked.is_empty(), "a checked part is inflated");
    }

    fn stored_zip(name: &str, body: &[u8]) -> Vec<u8> {
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(name, options).unwrap();
        writer.write_all(body).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn read_checked(
        bytes: &[u8],
        budgets: &Budgets,
        count_cells: bool,
    ) -> Result<usize, ReadError> {
        let meter = CompressedMeter::new();
        let mut archive = zip::ZipArchive::new(meter.wrap(std::io::Cursor::new(bytes))).unwrap();
        read_part_if_checked(
            &mut archive,
            0,
            &meter,
            budgets,
            count_cells,
            NonZeroU64::MIN,
        )
        .map(|(_, part, _)| part.len())
    }

    #[test]
    fn a_worksheet_with_formulas_past_the_whole_cap_is_bounded_by_its_dom_charge() {
        let row = r#"<row r="1"><c r="A1"><f>B1*2</f><v>2</v></c></row>"#;
        let rows = row.repeat(limits::MAX_WHOLE_PART_BYTES as usize / row.len() + 1);
        let sheet = format!("<worksheet><sheetData>{rows}</sheetData></worksheet>");
        let bytes = stored_zip("xl/worksheets/sheet1.xml", sheet.as_bytes());
        assert_eq!(read_checked(&bytes, &budgets(), true).unwrap(), sheet.len());
        let error = read_checked(&bytes, &budgets(), false).unwrap_err();
        assert!(error.to_string().contains("kept whole"), "{error}");
        let small = Budgets::with_cells(CellBudget::max_cells_for_test(1_000));
        let error = read_checked(&bytes, &small, true).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        let capped = Budgets::with_entry_cap(CellBudget::max_cells_for_test(u64::MAX), 1024);
        let error = read_checked(&bytes, &capped, true).unwrap_err();
        assert!(error.to_string().contains("per-part limit"), "{error}");
    }

    #[test]
    fn a_part_without_checked_elements_is_still_capped_and_charged() {
        let body = vec![b'a'; limits::MAX_WHOLE_PART_BYTES as usize + 1];
        let bytes = stored_zip("xl/styles.xml", &body);
        let error = read_checked(&bytes, &budgets(), false).unwrap_err();
        assert!(error.to_string().contains("kept whole"), "{error}");
        let small = stored_zip("xl/styles.xml", &[b'a'; 1_000]);
        let tight = Budgets::with_cells(CellBudget::max_cells_for_test(1));
        let error = read_checked(&small, &tight, false).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)), "{error}");
        assert_eq!(read_checked(&small, &budgets(), false).unwrap(), 0);
    }

    #[test]
    fn the_bytes_really_read_are_counted_across_entries() {
        let budgets = budgets();
        let first = read(&[b'a'; 600], "a", &budgets).unwrap();
        assert_eq!(first.len(), 600);
        budgets.total.store(100, Ordering::Relaxed);
        assert!(matches!(
            read(&[b'b'; 600], "b", &budgets),
            Err(ReadError::Unsafe(_))
        ));
    }

    #[test]
    fn control_characters_and_noncharacters_are_refused() {
        let mut check = CharacterCheck::new();
        assert!(check.push(b"<a>ok</a>").is_ok());
        assert!(check.push(b"\x01").is_err());
        let mut check = CharacterCheck::new();
        assert!(check.push(b"<a>\xef").is_ok());
        assert!(check.push(b"\xbf\xbe</a>").is_err());
    }
}
