use std::cell::Cell;
use std::io::{Read, Seek};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ReadError;
use crate::limits::{self, CellBudget, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, Usage};

const CHUNK_BYTES: usize = 64 * 1024;

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
    used_bytes: AtomicU64,
    used_rows: AtomicU64,
    entry_cap: u64,
}

impl Budgets {
    pub(crate) fn production() -> Budgets {
        Self::with_cells(CellBudget::production())
    }

    pub(crate) fn with_cells(cells: CellBudget) -> Budgets {
        Budgets {
            total: AtomicU64::new(MAX_TOTAL_BYTES),
            cells,
            used_cells: AtomicU64::new(0),
            used_bytes: AtomicU64::new(0),
            used_rows: AtomicU64::new(0),
            entry_cap: MAX_ENTRY_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_entry_cap(cells: CellBudget, entry_cap: u64) -> Budgets {
        Budgets {
            total: AtomicU64::new(MAX_TOTAL_BYTES),
            cells,
            used_cells: AtomicU64::new(0),
            used_bytes: AtomicU64::new(0),
            used_rows: AtomicU64::new(0),
            entry_cap,
        }
    }

    pub(crate) fn check_usage(&self, additional: Usage) -> Result<(), ReadError> {
        let cells = self
            .used_cells
            .fetch_add(additional.cells, Ordering::Relaxed)
            .saturating_add(additional.cells);
        let bytes = self
            .used_bytes
            .fetch_add(additional.bytes, Ordering::Relaxed)
            .saturating_add(additional.bytes);
        let rows = self
            .used_rows
            .fetch_add(additional.rows, Ordering::Relaxed)
            .saturating_add(additional.rows);
        self.cells.check(Usage { cells, bytes, rows })
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
        Guard {
            inner,
            budgets: self,
            meter: meter.clone(),
            start: meter.snapshot(),
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
    expanded: u64,
    part: String,
}

impl<R: Read> Guard<'_, R> {
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
        self.budgets.charge(read)?;
        self.expanded = self.expanded.saturating_add(read);
        if self.expanded > self.budgets.entry_cap {
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
    let mut guard = budgets.stream(entry, meter, part);
    let mut bytes = Vec::with_capacity(CHUNK_BYTES);
    let mut chunk = [0u8; CHUNK_BYTES];
    loop {
        match guard.read_chunk(&mut chunk)? {
            0 => {
                limits::check_ratio(part, guard.compressed_used(), guard.expanded)?;
                return Ok(bytes);
            }
            read => bytes.extend_from_slice(&chunk[..read]),
        }
    }
}

// The IronCalc fallback is never a way past the limits: every entry streams once
// through the budgets, so a lied declared size cannot smuggle a part past the caps.
pub(crate) fn check_fallback_caps<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    meter: &CompressedMeter,
) -> Result<(), ReadError> {
    let budgets = Budgets::production();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let name = entry.name().to_string();
        let mut guard = budgets.stream(entry, meter, &name);
        let mut expanded = 0u64;
        let mut chunk = [0u8; CHUNK_BYTES];
        loop {
            match guard.read_chunk(&mut chunk)? {
                0 => break,
                read => {
                    expanded = expanded.saturating_add(read as u64);
                    if expanded > MAX_ENTRY_BYTES {
                        return Err(ReadError::Unsafe(format!(
                            "part {name} expands to {} MB",
                            limits::megabytes(expanded)
                        )));
                    }
                }
            }
        }
    }
    Ok(())
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
