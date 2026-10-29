#![forbid(unsafe_code)]

use crate::types::{Cell, FormulaValue, SpillValue, Workbook};
use crate::Model;

// Streaming-route costs, fitted on 2026-10-14 to `measure_open --app` (release, allocator
// peak; committed memory is 3-6% above it) over generated matrix shapes and real files:
// about 29.5 B per cell, 199 B per row, about 540 B per distinct string at the peak (125 B
// at rest; the rest is transient assembly) plus its 16 B of text. Formulas were refitted on
// 2026-10-29, once the reader keeps one job per formula shape, to the same release
// measurement of 20k to 1M rows of repeated lookups, 200k rows of repeated or distinct
// arithmetic and 200k and 1M rows of a formula returning 200 characters. Each constant is
// the lowest that charges every measured shape at least 1.2x its committed peak.
pub const BYTES_PER_CELL: u64 = 70;
// Extra per formula cell over a value cell, its evaluated result included: the formula
// returning 200 characters sets it, the lookups alone would need 110-140 B.
pub const BYTES_PER_FORMULA: u64 = 240;
// The result text BYTES_PER_FORMULA already pays for: the formula it was fitted on returns
// 200 characters. Text past it is charged while the first evaluation runs.
pub const TEXT_BYTES_PER_FORMULA: u64 = 200;
// Extra per distinct formula shape over its text: its key while reading and its parsed
// tree in the model, fitted on 628,902 distinct shapes in 859,659 formula cells.
pub const BYTES_PER_SHAPE: u64 = 450;
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

// The estimate and budget a workbook passed when its model was built, kept until its first
// evaluation, which is the only one that can still refuse it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenAllowance {
    cost: OpenCost,
    budget_bytes: u64,
}

impl OpenAllowance {
    pub(crate) fn checked(book: &Workbook, budget_bytes: u64) -> Result<OpenAllowance, String> {
        let cost = open_cost(book);
        check_cost(cost, budget_bytes)?;
        Ok(OpenAllowance { cost, budget_bytes })
    }

    // Text past what each formula already pays for is charged 1.2 times, the margin of
    // every other cost, so the headroom the estimate leaves buys 5/6 of its bytes in text.
    fn result_text_bytes(self) -> u64 {
        let headroom = self.budget_bytes.saturating_sub(needed_bytes(self.cost));
        self.cost
            .formulas
            .saturating_mul(TEXT_BYTES_PER_FORMULA)
            .saturating_add(headroom.saturating_mul(5) / 6)
    }

    // The evaluation stops at the first result past the budget, so all that is known of
    // the need is that it is above the allowance.
    fn refusal(self) -> String {
        refusal(
            self.cost,
            self.budget_bytes.saturating_add(1),
            self.budget_bytes,
        )
    }
}

// What result text the first evaluation of an opened workbook may still add, which the
// estimate cannot know before evaluating. Charged by growth over what each cell held, so
// the cached values a file carries are not charged again. Unbounded in every other
// evaluation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ResultText {
    #[default]
    Unbounded,
    Left(u64),
    Spent,
}

impl ResultText {
    pub(crate) fn replace(&mut self, old: Option<&Cell>, new: &Cell) {
        let ResultText::Left(left) = self else {
            return;
        };
        let old = old.map_or(0, text_len);
        let new = text_len(new);
        if new <= old {
            *left = left.saturating_add(old - new);
        } else if new - old <= *left {
            *left -= new - old;
        } else {
            *self = ResultText::Spent;
        }
    }

    pub(crate) fn is_spent(self) -> bool {
        self == ResultText::Spent
    }
}

fn text_len(cell: &Cell) -> u64 {
    let len = match cell {
        Cell::CellFormula {
            v: FormulaValue::Text(text),
            ..
        } => text.len(),
        Cell::ArrayFormula(array) => match &array.v {
            FormulaValue::Text(text) => text.len(),
            _ => 0,
        },
        Cell::SpillCell(spill) => match &spill.v {
            SpillValue::Text(text) => text.len(),
            _ => 0,
        },
        _ => 0,
    };
    len as u64
}

impl Model<'_> {
    /// Evaluates a model built by [Model::from_workbook] for the first time, and refuses it
    /// with the open budget's message when the text of its results does not fit the
    /// budget it was built within. A refused model is left half evaluated: drop it. A
    /// later call, or one on a model built otherwise, evaluates without the check.
    pub fn evaluate_opened(&mut self) -> Result<(), String> {
        let Some(allowance) = self.open_allowance.take() else {
            self.evaluate_indexed();
            return Ok(());
        };
        self.result_text = ResultText::Left(allowance.result_text_bytes());
        self.evaluate_indexed();
        let spent = self.result_text.is_spent();
        self.result_text = ResultText::Unbounded;
        if spent {
            return Err(allowance.refusal());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellValue;

    const ROWS: i32 = 100;
    const TEXT_LEN: u64 = 1000;

    // A column of formulas returning TEXT_LEN characters each, without the values a file
    // would cache when `cached` is false, as openpyxl writes them.
    fn long_text_book(cached: bool) -> Workbook {
        let mut model = Model::new_empty("budget", "en", "UTC", "en").unwrap();
        for row in 1..=ROWS {
            model
                .set_user_input(0, row, 1, format!("=REPT(\"x\",{TEXT_LEN})&{row}"))
                .unwrap();
        }
        model.evaluate();
        let mut book = model.workbook;
        if !cached {
            for row in 1..=ROWS {
                let cell = book.worksheets[0].sheet_data.get_mut(row, 1).unwrap();
                let Cell::CellFormula { v, .. } = cell else {
                    panic!("{cell:?}");
                };
                *v = FormulaValue::Unevaluated;
            }
        }
        book
    }

    // The least budget the text of `book`'s results fits in, with every other cost.
    fn fitting_budget(book: &Workbook, text_bytes: u64) -> u64 {
        let cost = open_cost(book);
        let past_allowance = text_bytes - cost.formulas * TEXT_BYTES_PER_FORMULA;
        needed_bytes(cost) + (past_allowance * 6).div_ceil(5)
    }

    fn results_text_bytes() -> u64 {
        (1..=ROWS as u64)
            .map(|row| TEXT_LEN + row.to_string().len() as u64)
            .sum()
    }

    fn values(model: &Model) -> Vec<CellValue> {
        (1..=ROWS)
            .map(|row| model.get_cell_value_by_index(0, row, 1).unwrap())
            .collect()
    }

    #[test]
    fn result_text_past_the_budget_refuses_the_open() {
        let book = long_text_book(false);
        let budget = fitting_budget(&book, results_text_bytes()) - 1;
        let mut model = Model::from_workbook_within(book, "en", budget).unwrap();
        let error = model.evaluate_opened().unwrap_err();
        assert!(error.starts_with(BUDGET_REFUSAL_PREFIX), "{error}");
        assert!(error.contains("100 formulas"), "{error}");
    }

    #[test]
    fn result_text_just_within_the_budget_opens_with_every_value() {
        let book = long_text_book(false);
        let mut expected = Model::from_workbook_within(book.clone(), "en", u64::MAX).unwrap();
        expected.evaluate();
        let budget = fitting_budget(&book, results_text_bytes());
        let mut model = Model::from_workbook_within(book, "en", budget).unwrap();
        model.evaluate_opened().unwrap();
        assert_eq!(values(&model), values(&expected));
        assert_eq!(
            model.get_cell_value_by_index(0, 7, 1).unwrap(),
            CellValue::String(format!("{}7", "x".repeat(TEXT_LEN as usize)))
        );
    }

    #[test]
    fn cached_result_text_is_not_charged_again() {
        let book = long_text_book(true);
        let budget = needed_bytes(open_cost(&book));
        let mut model = Model::from_workbook_within(book, "en", budget).unwrap();
        model.evaluate_opened().unwrap();
        assert_eq!(model.result_text, ResultText::Unbounded);
    }

    #[test]
    fn evaluations_after_the_first_are_not_charged() {
        let book = long_text_book(false);
        let budget = fitting_budget(&book, results_text_bytes());
        let mut model = Model::from_workbook_within(book, "en", budget).unwrap();
        model.evaluate_opened().unwrap();
        // More text than the whole first evaluation was allowed.
        for row in 1..=4 {
            model
                .set_user_input(0, row, 2, "=REPT(\"y\",30000)".to_string())
                .unwrap();
        }
        model.evaluate_opened().unwrap();
        assert_eq!(
            model.get_cell_value_by_index(0, 4, 2).unwrap(),
            CellValue::String("y".repeat(30000))
        );
    }

    #[test]
    fn cached_spilled_text_is_not_charged_again() {
        let mut model = Model::new_empty("budget", "en", "UTC", "en").unwrap();
        model
            .set_user_input(0, 1, 1, "=REPT(\"z\",1000)&SEQUENCE(50)".to_string())
            .unwrap();
        model.evaluate();
        assert_eq!(
            model.get_cell_value_by_index(0, 50, 1).unwrap(),
            CellValue::String(format!("{}50", "z".repeat(1000)))
        );
        let book = model.workbook;
        let budget = needed_bytes(open_cost(&book));
        let mut model = Model::from_workbook_within(book, "en", budget).unwrap();
        model.evaluate_opened().unwrap();
    }

    #[test]
    fn spilled_text_past_the_budget_refuses_the_open() {
        let mut model = Model::new_empty("budget", "en", "UTC", "en").unwrap();
        model
            .set_user_input(0, 1, 1, "=REPT(\"z\",1000)&SEQUENCE(50)".to_string())
            .unwrap();
        model.evaluate();
        let mut book = model.workbook;
        for row in 2..=50 {
            book.worksheets[0].sheet_data.remove(row, 1);
        }
        let budget = needed_bytes(open_cost(&book));
        let mut model = Model::from_workbook_within(book, "en", budget).unwrap();
        let error = model.evaluate_opened().unwrap_err();
        assert!(error.starts_with(BUDGET_REFUSAL_PREFIX), "{error}");
    }

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
    // kept once: it measured 3,782 MiB committed at the peak, which this charge covers
    // 1.2 times within the allowance of a 16 GB machine.
    #[test]
    fn a_million_rows_of_repeated_lookups_fit_sixty_percent_of_sixteen_gigabytes() {
        let cost = OpenCost {
            cells: 29_312_838,
            formulas: 21_000_143,
            shapes: 163,
            rows: 1_002_334,
            bytes: 213_068 * (60 + BYTES_PER_UNIQUE_STRING),
        };
        let allowance = 17_043_439_616 * RAM_SHARE_PERCENT / 100;
        assert!(needed_bytes(cost) <= allowance, "{}", needed_bytes(cost));
        assert!(needed_bytes(cost) * 10 >= 3_782 * 1024 * 1024 * 12);
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
        // MiB, release build); the lookup sheets read their strings at 60 B of text each.
        let measured = [
            (
                10_000_000,
                0,
                0,
                1_000_000,
                8_000_000 * (16 + BYTES_PER_UNIQUE_STRING),
                4_628,
            ),
            (3_000_000, 859_659, 628_902, 200_000, 628_902 * 20, 605),
            (3_000_000, 859_071, 13, 200_000, 13 * 20, 206),
            (10_485_760, 0, 0, 1_048_576, 0, 527),
            (
                599_997,
                420_144,
                163,
                22_334,
                9_735 * (60 + BYTES_PER_UNIQUE_STRING),
                89,
            ),
            (
                1_478_987,
                1_050_144,
                163,
                52_334,
                16_869 * (60 + BYTES_PER_UNIQUE_STRING),
                233,
            ),
            (
                5_873_740,
                4_200_144,
                163,
                202_334,
                50_057 * (60 + BYTES_PER_UNIQUE_STRING),
                916,
            ),
            (
                11_733_601,
                8_400_144,
                163,
                402_334,
                92_439 * (60 + BYTES_PER_UNIQUE_STRING),
                1_808,
            ),
            (
                29_312_838,
                21_000_143,
                163,
                1_002_334,
                213_068 * (60 + BYTES_PER_UNIQUE_STRING),
                3_782,
            ),
            (400_000, 200_000, 1, 200_000, 20, 94),
            (2_000_000, 1_000_000, 1, 1_000_000, 20, 504),
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
