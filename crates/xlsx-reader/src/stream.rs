use std::cell::Cell;
use std::io::{Read, Seek};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ReadError;
use crate::limits::{self, CellBudget, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, Route, Usage};

pub(crate) const CHUNK_BYTES: usize = 64 * 1024;

const CHECKED_ELEMENTS: [&[u8]; 9] = [
    b"f",
    b"formula",
    b"formula1",
    b"formula2",
    b"definedName",
    b"hyperlinks",
    b"dataValidations",
    b"autoFilter",
    b"sheetProtection",
];

// XML forbids entities or whitespace between '<' and an element name, so a part with
// no '<name' or '<prefix:name' start tag for these names cannot contain them; the
// full parse is skipped for it. Text that only looks like a tag merely costs a parse.
pub fn has_checked_element(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some(at) = rest.iter().position(|b| *b == b'<') {
        rest = &rest[at + 1..];
        let name_end = rest
            .iter()
            .position(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/' | b'<'))
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        let local = name.rsplit(|b| *b == b':').next().unwrap_or(name);
        if CHECKED_ELEMENTS.contains(&local) {
            return true;
        }
    }
    false
}

// Watches streamed bytes for the elements above, so parts without any are never
// inflated whole. A tag start split across chunks is still seen: every push also
// scans the tail of the previous one. Only a tag whose name alone is longer than
// the overlap escapes, and such a prefix cannot name a checked element in practice.
pub(crate) struct Prefilter {
    overlap: Vec<u8>,
    hit: bool,
}

const PREFILTER_OVERLAP: usize = 64;

impl Prefilter {
    pub(crate) fn new() -> Prefilter {
        Prefilter {
            overlap: Vec::new(),
            hit: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.hit || chunk.is_empty() {
            return;
        }
        let mut window = Vec::with_capacity(self.overlap.len() + chunk.len());
        window.extend_from_slice(&self.overlap);
        window.extend_from_slice(chunk);
        if has_checked_element(&window) {
            self.hit = true;
            return;
        }
        let keep = window.len().min(PREFILTER_OVERLAP);
        self.overlap = window[window.len() - keep..].to_vec();
    }

    pub(crate) fn hit(&self) -> bool {
        self.hit
    }
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
    used_bytes: AtomicU64,
    used_rows: AtomicU64,
    // The worksheet bytes the archive declares and the bytes read from them so far,
    // only to report what a refused file would have needed.
    sheet_declared: AtomicU64,
    sheet_read: AtomicU64,
    entry_cap: u64,
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
            used_bytes: AtomicU64::new(0),
            used_rows: AtomicU64::new(0),
            sheet_declared: AtomicU64::new(0),
            sheet_read: AtomicU64::new(0),
            entry_cap,
        }
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
            bytes,
            rows: 0,
        })
    }

    fn compressed_used(&self) -> u64 {
        self.meter.used_since(self.start)
    }

    pub(crate) fn read_chunk(&mut self, buf: &mut [u8]) -> Result<usize, ReadError> {
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
            return Err(ReadError::Unsafe(format!(
                "part {} expands past the {} per-part limit",
                self.part,
                limits::describe_bytes(self.budgets.entry_cap),
            )));
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
    )
}

#[derive(Clone, Copy)]
enum WholeLimit {
    KeptWhole,
    // A worksheet IronCalc reads: IronCalc holds it whole anyway, and the DOM charge
    // already bounded it before this read, so the cap for other parts kept whole
    // would only refuse large valid sheets. The per-part cap still applies.
    DomCharged,
}

fn read_part_with<R: Read>(
    mut guard: Guard<'_, R>,
    part: &str,
    limit: WholeLimit,
) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::with_capacity(CHUNK_BYTES);
    let mut chunk = [0u8; CHUNK_BYTES];
    loop {
        match guard.read_chunk(&mut chunk)? {
            0 => {
                limits::check_ratio(part, guard.compressed_used(), guard.expanded)?;
                match limit {
                    WholeLimit::KeptWhole => guard.check_whole(bytes.len() as u64)?,
                    WholeLimit::DomCharged => guard.charge_whole(bytes.len() as u64)?,
                }
                return Ok(bytes);
            }
            read => {
                bytes.extend_from_slice(&chunk[..read]);
                if let WholeLimit::KeptWhole = limit {
                    guard.whole_fits(bytes.len() as u64)?;
                }
            }
        }
    }
}

// Streams one part through the budgets and inflates it whole only when it may hold
// something the preflight checks; anything else is decided by entry name and needs
// no bytes. A re-read part is measured again but not charged twice to the total.
// With cell counting the cells and rows stream past a counter on the same pass,
// so the caller can charge them without reading the part again.
pub(crate) fn read_part_if_checked<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    meter: &CompressedMeter,
    budgets: &Budgets,
    count_cells: bool,
) -> Result<(String, Vec<u8>, Usage), ReadError> {
    let (name, usage) = {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let name = entry.name().to_string();
        let mut guard = budgets.stream(entry, meter, &name);
        let mut filter = Prefilter::new();
        let mut counter = CellCounter::new();
        let mut chunk = [0u8; CHUNK_BYTES];
        let mut inflated = 0u64;
        loop {
            match guard.read_chunk(&mut chunk)? {
                0 => break,
                read => {
                    inflated += read as u64;
                    filter.push(&chunk[..read]);
                    if count_cells {
                        counter.push(&chunk[..read]);
                    }
                }
            }
        }
        counter.finish();
        if count_cells {
            budgets.cells.check_dom_part(&name, inflated)?;
        }
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
        read_part_with(budgets.stream_reread(entry, meter, &name), &name, limit)?,
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
            read_part_if_checked(&mut archive, 0, &meter, &budgets, false).unwrap();
        assert_eq!(name, "xl/styles.xml");
        assert!(clean.is_empty(), "a clean part needs no bytes");
        let (name, checked, _) =
            read_part_if_checked(&mut archive, 1, &meter, &budgets, false).unwrap();
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
        read_part_if_checked(&mut archive, 0, &meter, budgets, count_cells)
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
