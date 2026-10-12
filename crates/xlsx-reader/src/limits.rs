use std::io::{Read, Seek};

use zenkai_types::Range;

use crate::ReadError;

// No longer enforced when opening: declared sizes are hints, and the real bytes are
// counted as they are read. Only the IronCalc fallback still requires every part to
// fit here, so a huge file never reaches the reader without limits.
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
// A part expanding past this ratio is a zip bomb. Repetitive Excel files stay far
// below it, so the ratio never needs tuning; a legitimate file that trips the gate
// moves the threshold below, never the ratio.
pub const MAX_RATIO: u64 = 150;
// Below this many decompressed bytes a part passes on the cell and total budgets
// alone; the ratio check only runs past it.
pub const RATIO_GATE_BYTES: u64 = 64 * 1024 * 1024;
// Steady model growth per stored cell, measured by opening the MID corpus file:
// 339 MB for 2,928,620 cells. Opening stops when the cells read so far times this
// would pass 60 % of physical RAM committed.
pub const BYTES_PER_CELL: u64 = 122;
pub const RAM_SHARE_PERCENT: u64 = 60;
// Used only when the machine's memory cannot be read, mirroring the app's budget.
pub const FALLBACK_BUDGET_MB: u64 = 4096;
pub const BUDGET_VARIABLE: &str = "ZENKAI_MEMORY_BUDGET_MB";
// Excel's own limit; it also bounds how deep any formula's syntax tree can get.
pub const MAX_FORMULA_CHARS: usize = 8_192;
pub const MAX_FORMULA_DEPTH: usize = 256;
pub const MAX_FORMULA_AREA: u64 = 1_000_000;
// Together the array formulas of one sheet may claim this many cells: each claimed cell costs a
// map entry (about 40 bytes), so 4 million is roughly 160 MB.
pub const MAX_ARRAY_CELLS_PER_SHEET: u64 = 4_000_000;

// Declared sizes can lie, so this only counts entries. Per-part and total sizes are
// enforced on the bytes really decompressed, as they are read.
pub fn check_archive<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Result<(), ReadError> {
    if archive.len() > MAX_ENTRIES {
        return Err(ReadError::Unsafe(format!(
            "{} entries in the archive",
            archive.len()
        )));
    }
    Ok(())
}

pub fn megabytes(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}

pub fn describe_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        format!("{} MB", megabytes(bytes))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CellBudget {
    max_cells: u64,
}

impl CellBudget {
    pub fn production() -> CellBudget {
        let override_mb = std::env::var(BUDGET_VARIABLE)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .filter(|megabytes| *megabytes > 0);
        let budget_mb = override_mb.or_else(|| {
            let mut system = sysinfo::System::new();
            system.refresh_memory();
            let total_mb = system.total_memory() / 1024 / 1024;
            (total_mb > 0).then(|| total_mb * RAM_SHARE_PERCENT / 100)
        });
        CellBudget::from_budget_mb(budget_mb.unwrap_or(FALLBACK_BUDGET_MB))
    }

    pub fn from_budget_mb(megabytes: u64) -> CellBudget {
        CellBudget {
            max_cells: megabytes * 1024 * 1024 / BYTES_PER_CELL,
        }
    }

    pub fn max_cells_for_test(max_cells: u64) -> CellBudget {
        CellBudget { max_cells }
    }

    pub fn max_cells(self) -> u64 {
        self.max_cells
    }

    pub fn check(self, cells: u64) -> Result<(), ReadError> {
        if cells <= self.max_cells {
            return Ok(());
        }
        Err(ReadError::Unsafe(format!(
            "this file needs about {} to open ({} cells), more than the {} supported",
            describe_bytes(cells.saturating_mul(BYTES_PER_CELL)),
            cells,
            describe_bytes(self.max_cells.saturating_mul(BYTES_PER_CELL)),
        )))
    }
}

// What IronCalc's reader and its fallback still require, on the sizes the archive
// declares: the streaming reader never enforces this when opening.
pub fn check_fallback_caps_bytes(bytes: &[u8]) -> Result<(), ReadError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    check_fallback_caps(&mut archive)
}

pub fn check_fallback_caps<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<(), ReadError> {
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(ReadError::Unsafe(format!(
                "part {} expands to {} MB",
                entry.name(),
                megabytes(entry.size())
            )));
        }
        total = total.saturating_add(entry.size());
    }
    if total > MAX_TOTAL_BYTES {
        return Err(ReadError::Unsafe(format!(
            "the workbook expands to {} MB",
            megabytes(total)
        )));
    }
    Ok(())
}

pub fn check_ratio(part: &str, compressed: u64, expanded: u64) -> Result<(), ReadError> {
    if expanded > RATIO_GATE_BYTES && expanded > compressed.saturating_mul(MAX_RATIO) {
        return Err(ReadError::Unsafe(format!(
            "part {part} expands past the {MAX_RATIO}:1 compression limit ({} from {} compressed)",
            describe_bytes(expanded),
            describe_bytes(compressed),
        )));
    }
    Ok(())
}

pub fn check_formula(formula: &str, area: Option<&str>) -> Result<(), String> {
    if let Some(range) = area.and_then(Range::parse_a1)
        && range.cell_count() > MAX_FORMULA_AREA
    {
        return Err(format!(
            "a formula covers {} cells, more than the {MAX_FORMULA_AREA} supported",
            range.cell_count()
        ));
    }
    check_formula_text(formula)
}

pub fn check_formula_text(formula: &str) -> Result<(), String> {
    let chars = formula.chars().count();
    if chars > MAX_FORMULA_CHARS {
        return Err(format!(
            "a formula has {chars} characters, more than the {MAX_FORMULA_CHARS} Excel allows"
        ));
    }
    let depth = max_depth(formula);
    if depth > MAX_FORMULA_DEPTH {
        return Err(format!(
            "a formula is nested {depth} levels deep, more than the {MAX_FORMULA_DEPTH} supported"
        ));
    }
    Ok(())
}

fn max_depth(formula: &str) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    for c in formula.chars() {
        match c {
            '(' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_parts_pass_on_the_other_budgets_alone() {
        assert!(check_ratio("xl/worksheets/sheet1.xml", 1, RATIO_GATE_BYTES).is_ok());
        assert!(check_ratio("xl/worksheets/sheet1.xml", 0, RATIO_GATE_BYTES).is_ok());
    }

    #[test]
    fn extreme_ratios_past_the_gate_are_refused() {
        let expanded = RATIO_GATE_BYTES + 1;
        assert!(check_ratio("xl/sharedStrings.xml", expanded / MAX_RATIO + 1, expanded).is_ok());
        let error =
            check_ratio("xl/sharedStrings.xml", expanded / MAX_RATIO - 1, expanded).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)));
        assert!(error.to_string().contains("150"), "{error}");
    }

    #[test]
    fn the_cell_budget_names_the_memory_the_file_needs() {
        let budget = CellBudget::max_cells_for_test(10_000);
        assert!(budget.check(10_000).is_ok());
        let error = budget.check(10_001).unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)));
        let message = error.to_string();
        assert!(message.contains("needs about"), "{message}");
        assert!(message.contains("10001 cells"), "{message}");
    }

    #[test]
    fn sixty_percent_of_a_16gb_machine_covers_the_big_file() {
        assert!(CellBudget::from_budget_mb(9_752).max_cells() > 40_000_000);
    }
}
