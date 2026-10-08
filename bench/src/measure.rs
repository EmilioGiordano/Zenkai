use std::fmt;
use std::time::Instant;

use crate::fixtures::{Expected, ExpectedCell, Fixture};

const TOLERANCE: f64 = 1e-9;

#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(String),
}

impl Seen {
    pub fn matches(&self, expected: &Expected) -> bool {
        match (self, expected) {
            (Seen::Number(a), Expected::Number(b)) => (a - b).abs() <= TOLERANCE * b.abs().max(1.0),
            (Seen::Text(a), Expected::Text(b)) => a == b,
            _ => false,
        }
    }

    pub fn same_as(&self, other: &Seen) -> bool {
        match (self, other) {
            (Seen::Number(a), Seen::Number(b)) => (a - b).abs() <= TOLERANCE * b.abs().max(1.0),
            _ => self == other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellSnapshot {
    pub value: Seen,
    pub formula: Option<String>,
}

pub struct SheetExtent {
    pub sheet: u32,
    pub rows: u32,
    pub cols: u16,
}

pub fn extents(fixture: Fixture) -> Vec<SheetExtent> {
    let one = |sheet, rows, cols| SheetExtent { sheet, rows, cols };
    match fixture {
        Fixture::Values => vec![one(0, 100_000, 20)],
        Fixture::SimpleFormulas => vec![one(0, 50_000, 6)],
        Fixture::HeavyFormulas => vec![one(0, 20_000, 6), one(1, 10_000, 3)],
        Fixture::Chain => vec![one(0, 10_000, 1)],
        Fixture::Formatting => vec![one(0, 10_000, 8)],
    }
}

#[derive(Default)]
pub struct Metrics {
    pub open_ms: f64,
    pub recalc_ms: Option<f64>,
    pub edit_ms: Option<f64>,
    pub edit_ok: Option<bool>,
    pub save_ms: f64,
    pub peak_mb: f64,
    pub idle_mb: f64,
    pub correct: Option<(usize, usize)>,
    pub roundtrip: (usize, usize),
    pub styles: Option<(usize, usize)>,
    pub merges: Option<(usize, usize)>,
}

impl fmt::Display for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let opt = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.1}"));
        let pair =
            |v: Option<(usize, usize)>| v.map_or("-".to_string(), |(a, b)| format!("{a}/{b}"));
        write!(
            f,
            "open_ms={:.1} recalc_ms={} edit_ms={} edit_ok={} save_ms={:.1} peak_mb={:.1} idle_mb={:.1} correct={} roundtrip={}/{} styles={} merges={}",
            self.open_ms,
            opt(self.recalc_ms),
            opt(self.edit_ms),
            self.edit_ok.map_or("-".to_string(), |v| v.to_string()),
            self.save_ms,
            self.peak_mb,
            self.idle_mb,
            pair(self.correct),
            self.roundtrip.0,
            self.roundtrip.1,
            pair(self.styles),
            pair(self.merges),
        )
    }
}

pub fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

pub fn count_correct(
    expected: &[ExpectedCell],
    read: impl Fn(&ExpectedCell) -> Seen,
) -> (usize, usize) {
    let ok = expected
        .iter()
        .filter(|cell| read(cell).matches(&cell.value))
        .count();
    (ok, expected.len())
}

pub fn count_same(
    fixture: Fixture,
    before: impl Fn(u32, u32, u16) -> CellSnapshot,
    after: impl Fn(u32, u32, u16) -> CellSnapshot,
) -> (usize, usize) {
    let mut same = 0;
    let mut total = 0;
    for extent in extents(fixture) {
        for row in 0..extent.rows {
            for col in 0..extent.cols {
                let a = before(extent.sheet, row, col);
                let b = after(extent.sheet, row, col);
                total += 1;
                if a.value.same_as(&b.value) && a.formula == b.formula {
                    same += 1;
                }
            }
        }
    }
    (same, total)
}

pub fn normalize_formula(formula: &str) -> String {
    formula
        .trim_start_matches('=')
        .replace('$', "")
        .to_uppercase()
}
