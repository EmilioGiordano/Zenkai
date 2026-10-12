use std::io::{Read, Seek};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ReadError;
use crate::limits::{self, CellBudget, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES};

const CHUNK_BYTES: usize = 64 * 1024;

pub(crate) struct Budgets {
    total: AtomicU64,
    cells: CellBudget,
    used_cells: AtomicU64,
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
        }
    }

    pub(crate) fn check_cells(&self, additional: u64) -> Result<(), ReadError> {
        let cells = self
            .used_cells
            .fetch_add(additional, Ordering::Relaxed)
            .saturating_add(additional);
        self.cells.check(cells)
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

    pub(crate) fn stream<R: Read>(&self, inner: R, compressed: u64, part: &str) -> Guard<'_, R> {
        Guard {
            inner,
            budgets: self,
            compressed,
            expanded: 0,
            part: part.to_string(),
        }
    }
}

pub(crate) struct Guard<'b, R> {
    inner: R,
    budgets: &'b Budgets,
    compressed: u64,
    expanded: u64,
    part: String,
}

impl<R: Read> Guard<'_, R> {
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
        limits::check_ratio(&self.part, self.compressed, self.expanded)?;
        Ok(read as usize)
    }
}

pub(crate) fn read_part<R: Read>(
    entry: R,
    compressed: u64,
    part: &str,
    budgets: &Budgets,
) -> Result<Vec<u8>, ReadError> {
    let mut guard = budgets.stream(entry, compressed, part);
    let mut bytes = Vec::with_capacity(CHUNK_BYTES);
    let mut chunk = [0u8; CHUNK_BYTES];
    loop {
        match guard.read_chunk(&mut chunk)? {
            0 => return Ok(bytes),
            read => bytes.extend_from_slice(&chunk[..read]),
        }
    }
}

// The IronCalc fallback is never a way past the limits: every entry streams once
// through the budgets, so a lied declared size cannot smuggle a part past the caps.
pub(crate) fn check_fallback_caps<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<(), ReadError> {
    let budgets = Budgets::production();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let name = entry.name().to_string();
        let compressed = entry.compressed_size();
        let mut guard = budgets.stream(entry, compressed, &name);
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

    #[test]
    fn declared_sizes_only_size_the_first_allocation() {
        let data = vec![7u8; 100];
        let part = read_part(data.as_slice(), u64::MAX, "xl/workbook.xml", &budgets()).unwrap();
        assert_eq!(part.len(), 100);
    }

    #[test]
    fn the_bytes_really_read_are_counted_across_entries() {
        let budgets = budgets();
        let first = read_part([b'a'; 600].as_slice(), 600, "a", &budgets).unwrap();
        assert_eq!(first.len(), 600);
        budgets.total.store(100, Ordering::Relaxed);
        assert!(matches!(
            read_part([b'b'; 600].as_slice(), 600, "b", &budgets),
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
