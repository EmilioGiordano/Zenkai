#![forbid(unsafe_code)]

use crate::types::{Cell, Workbook};

// Peak committed memory per stored cell while opening, measured with the
// release instrument (committed MB): 2M plain cells peak 583 B/cell, styled
// and shared-string cells 639, short formulas (distinct and shared) 788-814,
// zenkai-pesado (2.93M cells, a third formulas) 665. Rounded down, so the
// estimate stays a lower bound on what the file needs.
pub const BYTES_PER_CELL: u64 = 600;
// Extra per formula cell over a value cell: the formula synthetics peak
// 205-231 B/cell above the value ones.
pub const BYTES_PER_FORMULA: u64 = 200;
// An IronCalc row entry costs about as much as one stored cell: a map node
// holding the row number plus the row map itself.
pub const BYTES_PER_ROW: u64 = 128;

pub const RAM_SHARE_PERCENT: u64 = 60;
pub const FALLBACK_BUDGET_MB: u64 = 4096;
pub const BUDGET_VARIABLE: &str = "ZENKAI_MEMORY_BUDGET_MB";
pub const BUDGET_REFUSAL_PREFIX: &str = "this file needs at least ";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenCost {
    pub cells: u64,
    pub formulas: u64,
    pub rows: u64,
    pub bytes: u64,
}

pub fn open_cost(book: &Workbook) -> OpenCost {
    let mut cost = OpenCost::default();
    for sheet in &book.worksheets {
        cost.rows += sheet.sheet_data.len() as u64;
        for columns in sheet.sheet_data.values() {
            for cell in columns.values() {
                cost.cells += 1;
                if matches!(cell, Cell::CellFormula { .. } | Cell::ArrayFormula { .. }) {
                    cost.formulas += 1;
                }
            }
        }
        for formula in &sheet.shared_formulas {
            cost.bytes += formula.len() as u64;
        }
    }
    for string in &book.shared_strings {
        cost.bytes += string.len() as u64;
    }
    cost
}

pub fn needed_bytes(cost: OpenCost) -> u64 {
    cost.cells
        .saturating_mul(BYTES_PER_CELL)
        .saturating_add(cost.formulas.saturating_mul(BYTES_PER_FORMULA))
        .saturating_add(cost.bytes)
        .saturating_add(cost.rows.saturating_mul(BYTES_PER_ROW))
}

pub fn describe_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        format!("{} MB", bytes / 1024 / 1024)
    }
}

pub fn refusal(cost: OpenCost, budget_bytes: u64) -> String {
    format!(
        "{BUDGET_REFUSAL_PREFIX}{} to open ({} cells, {} formulas), more than the {} supported; close other workbooks, or use a machine with more memory",
        describe_bytes(needed_bytes(cost)),
        cost.cells,
        cost.formulas,
        describe_bytes(budget_bytes),
    )
}

// Mirrors the streaming reader's production budget: an explicit override in
// MB, else sixty percent of physical RAM, else the app's fallback.
pub fn production_budget_bytes() -> u64 {
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
    budget_mb.unwrap_or(FALLBACK_BUDGET_MB) * 1024 * 1024
}

pub fn check_cost(cost: OpenCost, budget_bytes: u64) -> Result<(), String> {
    if needed_bytes(cost) <= budget_bytes {
        return Ok(());
    }
    Err(refusal(cost, budget_bytes))
}

pub fn check_with_budget(book: &Workbook, budget_bytes: u64) -> Result<(), String> {
    check_cost(open_cost(book), budget_bytes)
}

pub fn check_open_budget(book: &Workbook) -> Result<(), String> {
    check_with_budget(book, production_budget_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Model;

    #[test]
    fn counts_cells_and_formulas() {
        let mut model = Model::new_empty("budget", "en", "UTC", "en").unwrap();
        model.set_user_input(0, 1, 1, "42".to_string()).unwrap();
        model.set_user_input(0, 2, 1, "=A1*2".to_string()).unwrap();
        let cost = open_cost(&model.workbook);
        assert_eq!(cost.cells, 2);
        assert_eq!(cost.formulas, 1);
    }

    #[test]
    fn formula_cells_cost_more_than_value_cells() {
        let values = needed_bytes(OpenCost {
            cells: 1000,
            formulas: 0,
            rows: 10,
            bytes: 0,
        });
        let formulas = needed_bytes(OpenCost {
            cells: 1000,
            formulas: 1000,
            rows: 10,
            bytes: 0,
        });
        assert!(formulas > values);
        assert_eq!(formulas - values, 1000 * BYTES_PER_FORMULA);
    }

    #[test]
    fn the_refusal_names_what_the_file_needs() {
        let cost = OpenCost {
            cells: 1000,
            formulas: 1000,
            rows: 0,
            bytes: 0,
        };
        assert!(check_cost(cost, needed_bytes(cost)).is_ok());
        let error = check_cost(cost, needed_bytes(cost) - 1).unwrap_err();
        assert!(error.starts_with(BUDGET_REFUSAL_PREFIX));
        assert!(error.contains("1000 formulas"));
        assert!(error.contains("close other workbooks"));
    }
}
