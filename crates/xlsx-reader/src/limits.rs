use std::io::{Read, Seek};

use ironcalc::base::expressions::utils::{
    is_valid_column_number, is_valid_row, parse_reference_a1,
};
use zenkai_types::Range;

use crate::ReadError;

// Worksheets the streaming reader streams skip this cap: their cells charge the
// memory budget as they are read. Every other part, and every part the IronCalc
// fallback walks, still has to fit, so a huge file never reaches that reader.
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
// A real 216 MB workbook holds parts totalling about 1.6 GB, so the total stays
// far above any single part while still bounding a flood of mid-size parts.
pub const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
// Parts inflated whole for a DOM parse or copied whole expand several times over
// in memory, so they are capped far below streamed parts and charged to the
// memory budget cells share.
pub const MAX_WHOLE_PART_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
// A part expanding past this ratio is a zip bomb. Repetitive Excel files stay far
// below it, so the ratio never needs tuning; a legitimate file that trips the gate
// moves the threshold below, never the ratio.
pub const MAX_RATIO: u64 = 150;
// Below this many decompressed bytes a part passes on the cell and total budgets
// alone; the ratio check only runs past it.
pub const RATIO_GATE_BYTES: u64 = 64 * 1024 * 1024;
// The peak open costs live in ironcalc_base::open_budget, measured with the
// release instrument and shared with Model::from_workbook's guard, so the
// read refuses files the model build could not hold.
pub use ironcalc::base::open_budget::BYTES_PER_ROW as ROW_BYTES;
pub use ironcalc::base::open_budget::{
    BUDGET_VARIABLE, BYTES_PER_CELL, BYTES_PER_FORMULA, FALLBACK_BUDGET_MB, RAM_SHARE_PERCENT,
};
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

pub use ironcalc::base::open_budget::describe_bytes;

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

    #[cfg(test)]
    pub fn max_cells_for_test(max_cells: u64) -> CellBudget {
        CellBudget { max_cells }
    }

    pub fn max_cells(self) -> u64 {
        self.max_cells
    }

    pub fn check(self, usage: Usage) -> Result<(), ReadError> {
        let needed = usage.needed_bytes();
        let supported = self.max_cells.saturating_mul(BYTES_PER_CELL);
        if needed <= supported {
            return Ok(());
        }
        Err(ReadError::Unsafe(ironcalc::base::open_budget::refusal(
            usage.cost(),
            supported,
        )))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub cells: u64,
    pub formulas: u64,
    pub bytes: u64,
    pub rows: u64,
}

impl Usage {
    pub fn needed_bytes(self) -> u64 {
        ironcalc::base::open_budget::needed_bytes(self.cost())
    }

    fn cost(self) -> ironcalc::base::open_budget::OpenCost {
        ironcalc::base::open_budget::OpenCost {
            cells: self.cells,
            formulas: self.formulas,
            rows: self.rows,
            bytes: self.bytes,
        }
    }
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

pub fn check_formula(formula: &str, area: Option<&str>, shared: bool) -> Result<(), String> {
    if let Some(range) = area {
        // A shared formula ref may cover a whole column, so it is only checked to
        // stay inside the sheet; every other formula keeps the area limit.
        if shared {
            check_range_bounds(range)?;
        } else if let Some(parsed) = Range::parse_a1(range)
            && parsed.cell_count() > MAX_FORMULA_AREA
        {
            return Err(format!(
                "a formula covers {} cells, more than the {MAX_FORMULA_AREA} supported",
                parsed.cell_count()
            ));
        }
    }
    check_formula_text(formula)
}

fn check_range_bounds(range: &str) -> Result<(), String> {
    let invalid = || format!("invalid range {range}");
    let (left, right) = match range.split_once(':') {
        Some(pair) => pair,
        None => (range, range),
    };
    for end in [left.trim(), right.trim()] {
        let cell = parse_reference_a1(end).ok_or_else(invalid)?;
        if !is_valid_row(cell.row) || !is_valid_column_number(cell.column) {
            return Err(invalid());
        }
    }
    Ok(())
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
        assert!(
            budget
                .check(Usage {
                    cells: 10_000,
                    formulas: 0,
                    bytes: 0,
                    rows: 0
                })
                .is_ok()
        );
        let error = budget
            .check(Usage {
                cells: 10_001,
                formulas: 0,
                bytes: 0,
                rows: 0,
            })
            .unwrap_err();
        assert!(matches!(error, ReadError::Unsafe(_)));
        let message = error.to_string();
        assert!(message.contains("needs at least"), "{message}");
        assert!(message.contains("10001 cells"), "{message}");
        assert!(message.contains("close other workbooks"), "{message}");
    }

    #[test]
    fn formula_cells_trip_the_budget_before_value_cells() {
        let budget = CellBudget::max_cells_for_test(1000);
        assert!(
            budget
                .check(Usage {
                    cells: 1000,
                    formulas: 0,
                    bytes: 0,
                    rows: 0,
                })
                .is_ok()
        );
        assert!(
            budget
                .check(Usage {
                    cells: 1000,
                    formulas: 1000,
                    bytes: 0,
                    rows: 0,
                })
                .is_err()
        );
    }

    #[test]
    fn string_formula_and_row_bytes_count_against_the_budget() {
        let budget = CellBudget::max_cells_for_test(100);
        let supported = 100 * BYTES_PER_CELL;
        assert!(
            budget
                .check(Usage {
                    cells: 99,
                    formulas: 0,
                    bytes: supported - 99 * BYTES_PER_CELL,
                    rows: 0,
                })
                .is_ok()
        );
        assert!(
            budget
                .check(Usage {
                    cells: 99,
                    formulas: 0,
                    bytes: supported - 99 * BYTES_PER_CELL + 1,
                    rows: 0,
                })
                .is_err()
        );
        assert!(
            budget
                .check(Usage {
                    cells: 0,
                    formulas: 0,
                    bytes: 0,
                    rows: supported / ROW_BYTES,
                })
                .is_ok()
        );
        assert!(
            budget
                .check(Usage {
                    cells: 0,
                    formulas: 0,
                    bytes: 0,
                    rows: supported / ROW_BYTES + 1,
                })
                .is_err()
        );
    }

    #[test]
    fn sixty_percent_of_a_16gb_machine_covers_seventeen_million_cells() {
        assert!(CellBudget::from_budget_mb(9_752).max_cells() > 17_000_000);
    }

    #[test]
    fn a_shared_formula_may_cover_a_whole_column() {
        assert!(check_formula("B1*2", Some("A1:A1048576"), true).is_ok());
        assert!(check_formula("B1*2", Some("A1:A1048577"), true).is_err());
        assert!(check_formula("B1*2", Some("A1:XFE1048576"), true).is_err());
        assert!(check_formula("1", Some("A1:A1048576"), false).is_err());
    }
}
