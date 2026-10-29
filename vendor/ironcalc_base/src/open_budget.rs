#![forbid(unsafe_code)]

use crate::types::{Cell, Workbook};

// Streaming-route costs, fitted on 2026-10-14 to `measure_open --app` (release, allocator
// peak; committed memory is 3-6% above it) over generated matrix shapes and real files:
// about 29.5 B per cell, 199 B per row, about 540 B per distinct string at the peak (125 B
// at rest; the rest is transient assembly) plus its 16 B of text. Formulas were refitted on
// 2026-10-29, once the reader keeps one job per formula shape, to the committed peaks (debug
// build) of 20k to 200k rows of repeated lookups and of 200k rows of repeated or distinct
// arithmetic: about 120-140 B per formula cell over the cell, and per distinct shape its
// key, parsed tree and text. Each constant is the fit with a margin so every measured
// shape is charged at least 1.2x its committed peak.
pub const BYTES_PER_CELL: u64 = 70;
// Extra per formula cell over a value cell.
pub const BYTES_PER_FORMULA: u64 = 220;
// Extra per distinct formula shape over its text: its key while reading and its parsed
// tree in the model. With the formula cell it is charged at least what a cell of distinct
// text measured before shapes were kept once (390-470 B).
pub const BYTES_PER_SHAPE: u64 = 650;
// Extra per distinct string over its text.
pub const BYTES_PER_UNIQUE_STRING: u64 = 650;
pub const BYTES_PER_ROW: u64 = 256;
// The same costs when the file goes through IronCalc's importer, which holds the whole
// workbook as a DOM and then converts it: measured before the streaming reader built
// cells in place (583-814 B per cell). Charged by the fallback walk only.
pub const IMPORTER_BYTES_PER_CELL: u64 = 520;
pub const IMPORTER_BYTES_PER_FORMULA: u64 = 200;
pub const IMPORTER_BYTES_PER_ROW: u64 = 128;
// IronCalc's importer parses each worksheet into a DOM holding about sixteen
// times its bytes: an 827 MB sheet peaks past 14 GB committed while parsing.
pub const DOM_BYTES_PER_XML_BYTE: u64 = 16;
// The shared string table inflates about twice: its text plus the string list.
pub const SST_BYTES_PER_XML_BYTE: u64 = 2;

pub const RAM_SHARE_PERCENT: u64 = 60;
pub const FALLBACK_BUDGET_MB: u64 = 4096;
pub const BUDGET_VARIABLE: &str = "ZENKAI_MEMORY_BUDGET_MB";
pub const BUDGET_REFUSAL_PREFIX: &str = "this file needs about ";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenCost {
    pub cells: u64,
    pub formulas: u64,
    pub shapes: u64,
    pub rows: u64,
    pub bytes: u64,
}

pub fn open_cost(book: &Workbook) -> OpenCost {
    let mut cost = OpenCost::default();
    for sheet in &book.worksheets {
        cost.rows += sheet.sheet_data.row_count() as u64;
        for (_, _, cell) in sheet.sheet_data.iter() {
            cost.cells += 1;
            if matches!(cell, Cell::CellFormula { .. } | Cell::ArrayFormula { .. }) {
                cost.formulas += 1;
            }
        }
        cost.shapes += sheet.shared_formulas.len() as u64;
        for formula in &sheet.shared_formulas {
            cost.bytes += formula.len() as u64;
        }
    }
    for string in &book.shared_strings {
        cost.bytes += string.len() as u64 + BYTES_PER_UNIQUE_STRING;
    }
    cost
}

pub fn needed_bytes(cost: OpenCost) -> u64 {
    cost.cells
        .saturating_mul(BYTES_PER_CELL)
        .saturating_add(cost.formulas.saturating_mul(BYTES_PER_FORMULA))
        .saturating_add(cost.shapes.saturating_mul(BYTES_PER_SHAPE))
        .saturating_add(cost.bytes)
        .saturating_add(cost.rows.saturating_mul(BYTES_PER_ROW))
}

pub fn importer_needed_bytes(cost: OpenCost) -> u64 {
    cost.cells
        .saturating_mul(IMPORTER_BYTES_PER_CELL)
        .saturating_add(cost.formulas.saturating_mul(IMPORTER_BYTES_PER_FORMULA))
        .saturating_add(cost.bytes)
        .saturating_add(cost.rows.saturating_mul(IMPORTER_BYTES_PER_ROW))
}

pub fn describe_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        format!("{} MB", bytes / 1024 / 1024)
    }
}

// A need is shown rounded up and the allowance rounded down, so a refusal never reads
// "needs 9.5 GB, allows 9.5 GB".
fn describe_need(bytes: u64) -> String {
    const GIGABYTE: u128 = 1024 * 1024 * 1024;
    if u128::from(bytes) >= GIGABYTE {
        let tenths = (u128::from(bytes) * 10).div_ceil(GIGABYTE);
        format!("{}.{} GB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", bytes.div_ceil(1024 * 1024))
    }
}

fn describe_allowance(bytes: u64) -> String {
    const GIGABYTE: u128 = 1024 * 1024 * 1024;
    if u128::from(bytes) >= GIGABYTE {
        let tenths = u128::from(bytes) * 10 / GIGABYTE;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", bytes / 1024 / 1024)
    }
}

// `needed` is the estimate for `cost` on the route the caller reads the file by.
pub fn refusal(cost: OpenCost, needed: u64, budget_bytes: u64) -> String {
    format!(
        "{BUDGET_REFUSAL_PREFIX}{} to open ({} cells, {} formulas), this computer allows {} for it; close other workbooks, or use a machine with more memory",
        describe_need(needed),
        cost.cells,
        cost.formulas,
        describe_allowance(budget_bytes),
    )
}

pub fn refusal_for_part(part: &str, size: u64, factor: u64, budget_bytes: u64) -> String {
    format!(
        "{BUDGET_REFUSAL_PREFIX}{} to open (part {part} is {}, about {factor}x in memory), this computer allows {} for it; close other workbooks, or use a machine with more memory",
        describe_need(size.saturating_mul(factor)),
        describe_bytes(size),
        describe_allowance(budget_bytes),
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
    Err(refusal(cost, needed_bytes(cost), budget_bytes))
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
    fn a_formula_filled_down_is_one_shape() {
        let mut model = Model::new_empty("budget", "en", "UTC", "en").unwrap();
        for row in 1..=500 {
            model
                .set_user_input(0, row, 2, format!("=A{row}*2"))
                .unwrap();
            model
                .set_user_input(0, row, 3, format!("=A{row}+{row}"))
                .unwrap();
        }
        let cost = open_cost(&model.workbook);
        assert_eq!(cost.formulas, 1000);
        assert_eq!(cost.shapes, 501);
        let one_shape = OpenCost { shapes: 1, ..cost };
        assert_eq!(
            needed_bytes(cost) - needed_bytes(one_shape),
            500 * BYTES_PER_SHAPE
        );
    }

    // The lookup sheet of GitHub issue #212 at a million rows, refused before shapes were
    // kept once: it measured 3.7 GiB committed at the peak, under the 4.9 GiB projected
    // from 50k and 200k rows that this charge still covers 1.2 times.
    #[test]
    fn a_million_rows_of_repeated_lookups_fit_sixty_percent_of_sixteen_gigabytes() {
        let gibibyte = 1024 * 1024 * 1024;
        let cost = OpenCost {
            cells: 29_312_838,
            formulas: 21_000_143,
            shapes: 163,
            rows: 1_002_334,
            bytes: 213_068 * (60 + BYTES_PER_UNIQUE_STRING),
        };
        let allowance = 17_043_439_616 * RAM_SHARE_PERCENT / 100;
        assert!(needed_bytes(cost) <= allowance, "{}", needed_bytes(cost));
        assert!(needed_bytes(cost) * 100 >= 49 * gibibyte * 12);
    }

    #[test]
    fn formula_cells_cost_more_than_value_cells() {
        let values = needed_bytes(OpenCost {
            cells: 1000,
            formulas: 0,
            shapes: 0,
            rows: 10,
            bytes: 0,
        });
        let formulas = needed_bytes(OpenCost {
            cells: 1000,
            formulas: 1000,
            shapes: 0,
            rows: 10,
            bytes: 0,
        });
        assert!(formulas > values);
        assert_eq!(formulas - values, 1000 * BYTES_PER_FORMULA);
    }

    #[test]
    fn every_measured_shape_is_charged_at_least_one_and_a_fifth_of_its_peak() {
        let mebibyte = 1024 * 1024;
        // (cells, formulas, formula shapes, rows, text and string bytes, committed peak in
        // MiB); the lookup sheets read their strings at 60 B of text each.
        let measured = [
            (
                10_000_000,
                0,
                0,
                1_000_000,
                8_000_000 * (16 + BYTES_PER_UNIQUE_STRING),
                4_628,
            ),
            (3_000_000, 859_659, 628_902, 200_000, 628_902 * 20, 628),
            (3_000_000, 859_071, 13, 200_000, 13 * 20, 206),
            (10_485_760, 0, 0, 1_048_576, 0, 527),
            (
                599_997,
                420_144,
                163,
                22_334,
                9_735 * (60 + BYTES_PER_UNIQUE_STRING),
                114,
            ),
            (
                1_478_987,
                1_050_144,
                163,
                52_334,
                16_869 * (60 + BYTES_PER_UNIQUE_STRING),
                264,
            ),
            (
                5_873_740,
                4_200_144,
                163,
                202_334,
                50_057 * (60 + BYTES_PER_UNIQUE_STRING),
                1_021,
            ),
            (
                29_312_838,
                21_000_143,
                163,
                1_002_334,
                213_068 * (60 + BYTES_PER_UNIQUE_STRING),
                3_771,
            ),
        ];
        for (cells, formulas, shapes, rows, bytes, peak) in measured {
            let charged = needed_bytes(OpenCost {
                cells,
                formulas,
                shapes,
                rows,
                bytes,
            });
            assert!(
                charged * 10 >= peak * mebibyte * 12,
                "{cells} cells: {charged}"
            );
        }
    }

    #[test]
    fn the_importer_route_costs_more_than_the_streaming_route() {
        let cost = OpenCost {
            cells: 1000,
            formulas: 100,
            shapes: 0,
            rows: 10,
            bytes: 0,
        };
        assert!(importer_needed_bytes(cost) > needed_bytes(cost));
    }

    #[test]
    fn a_refusal_never_shows_the_need_at_or_below_the_allowance() {
        let gigabyte = 1024 * 1024 * 1024;
        let budget = 9 * gigabyte + gigabyte / 2 - 1;
        let cost = OpenCost {
            cells: 1,
            formulas: 0,
            shapes: 0,
            rows: 0,
            bytes: 0,
        };
        let error = refusal(cost, budget + 1, budget);
        assert!(error.contains("needs about 9.5 GB"), "{error}");
        assert!(error.contains("allows 9.4 GB"), "{error}");
        assert!(!error.contains("harm"), "{error}");
    }

    #[test]
    fn the_part_refusal_names_the_part() {
        let error = refusal_for_part("xl/worksheets/sheet2.xml", 827 << 20, 16, 9752 << 20);
        assert!(error.starts_with(BUDGET_REFUSAL_PREFIX));
        assert!(error.contains("xl/worksheets/sheet2.xml"), "{error}");
        assert!(error.contains("close other workbooks"), "{error}");
    }

    #[test]
    fn the_refusal_names_what_the_file_needs() {
        let cost = OpenCost {
            cells: 1000,
            formulas: 1000,
            shapes: 0,
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
