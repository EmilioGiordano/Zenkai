use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust_xlsxwriter::{Color, DocProperties, Format, FormatAlign, FormatBorder, Formula, Workbook};

const SEED: u64 = 0x005E_ED2E_4CA1;
const WORDS: [&str; 8] = [
    "alpha", "beta", "gamma", "delta", "ñandú", "über", "東京", "zeta",
];
const CHAIN_LEN: u32 = 10_000;
const DATA_ROWS: u32 = 10_000;
const MAIN_ROWS: u32 = 20_000;
const CATEGORIES: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    Values,
    SimpleFormulas,
    HeavyFormulas,
    Chain,
    Formatting,
}

impl Fixture {
    pub const ALL: [Fixture; 5] = [
        Fixture::Values,
        Fixture::SimpleFormulas,
        Fixture::HeavyFormulas,
        Fixture::Chain,
        Fixture::Formatting,
    ];

    pub fn file_name(self) -> &'static str {
        match self {
            Fixture::Values => "1-values.xlsx",
            Fixture::SimpleFormulas => "2-simple-formulas.xlsx",
            Fixture::HeavyFormulas => "3-heavy-formulas.xlsx",
            Fixture::Chain => "4-chain.xlsx",
            Fixture::Formatting => "5-formatting.xlsx",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fixture::Values => "1 values 100k x 20",
            Fixture::SimpleFormulas => "2 simple formulas 50k",
            Fixture::HeavyFormulas => "3 lookups 20k vs 10k",
            Fixture::Chain => "4 chain 10k",
            Fixture::Formatting => "5 formatting 10k",
        }
    }

    pub fn parse(name: &str) -> Option<Fixture> {
        Fixture::ALL.into_iter().find(|f| f.file_name() == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expected {
    Number(f64),
    Text(String),
}

pub struct ExpectedCell {
    pub sheet: u32,
    pub row: u32,
    pub col: u16,
    pub value: Expected,
}

pub struct EditProbe {
    pub sheet: u32,
    pub row: u32,
    pub col: u16,
    pub input: &'static str,
    pub probe_row: u32,
    pub probe_col: u16,
    pub probe_expected: f64,
}

pub fn edit_probe(fixture: Fixture) -> Option<EditProbe> {
    match fixture {
        Fixture::Chain => Some(EditProbe {
            sheet: 0,
            row: 0,
            col: 0,
            input: "2",
            probe_row: CHAIN_LEN - 1,
            probe_col: 0,
            probe_expected: f64::from(CHAIN_LEN) + 1.0,
        }),
        Fixture::SimpleFormulas => Some(EditProbe {
            sheet: 0,
            row: 0,
            col: 1,
            input: "1000",
            probe_row: 0,
            probe_col: 2,
            probe_expected: 1001.0,
        }),
        Fixture::HeavyFormulas => Some(EditProbe {
            sheet: 1,
            row: 0,
            col: 2,
            input: "cat1",
            probe_row: 0,
            probe_col: 5,
            probe_expected: f64::NAN,
        }),
        Fixture::Values | Fixture::Formatting => None,
    }
}

fn rng() -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(SEED)
}

pub fn generate(fixture: Fixture, dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    // logisheets 1.16.1 panics on the empty <dc:creator> rust_xlsxwriter writes by default.
    book.set_properties(&DocProperties::new().set_author("Zenkai bench"));
    match fixture {
        Fixture::Values => write_values(&mut book)?,
        Fixture::SimpleFormulas => write_simple(&mut book)?,
        Fixture::HeavyFormulas => write_heavy(&mut book)?,
        Fixture::Chain => write_chain(&mut book)?,
        Fixture::Formatting => write_formatting(&mut book)?,
    }
    book.save(dir.join(fixture.file_name()))?;
    Ok(())
}

pub fn expected(fixture: Fixture) -> Vec<ExpectedCell> {
    match fixture {
        Fixture::SimpleFormulas => expected_simple(),
        Fixture::HeavyFormulas => expected_heavy(),
        Fixture::Chain => (0..CHAIN_LEN)
            .map(|row| ExpectedCell {
                sheet: 0,
                row,
                col: 0,
                value: Expected::Number(f64::from(row) + 1.0),
            })
            .collect(),
        Fixture::Values | Fixture::Formatting => Vec::new(),
    }
}

// Formulas carry an empty cached value so engines that trust cached results
// (as Excel does without fullCalcOnLoad) still compute them on load.
fn formula(text: String) -> Formula {
    Formula::new(text).set_result("")
}

fn write_values(book: &mut Workbook) -> Result<()> {
    let sheet = book.add_worksheet();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let mut rng = rng();
    for row in 0..100_000u32 {
        for col in 0..20u16 {
            match col % 3 {
                0 => {
                    let n: f64 = rng.random_range(-10_000.0..10_000.0);
                    sheet.write_number(row, col, (n * 100.0).round() / 100.0)?;
                }
                1 => {
                    let word = WORDS[rng.random_range(0..WORDS.len())];
                    sheet.write_string(row, col, format!("{word} {row}"))?;
                }
                _ => {
                    let serial = f64::from(rng.random_range(40_000u32..46_000));
                    sheet.write_number_with_format(row, col, serial, &date)?;
                }
            }
        }
    }
    Ok(())
}

fn simple_inputs() -> Vec<(f64, f64)> {
    let mut rng = rng();
    (0..50_000u32)
        .map(|r| (f64::from(r + 1), f64::from(rng.random_range(1u32..1000))))
        .collect()
}

fn write_simple(book: &mut Workbook) -> Result<()> {
    let sheet = book.add_worksheet();
    for (row, (a, b)) in (0u32..).zip(simple_inputs()) {
        let r = row + 1;
        sheet.write_number(row, 0, a)?;
        sheet.write_number(row, 1, b)?;
        sheet.write_formula(row, 2, formula(format!("=A{r}+B{r}")))?;
        sheet.write_formula(row, 3, formula(format!("=SUM(A{r}:C{r})")))?;
        sheet.write_formula(row, 4, formula(format!("=IF(B{r}>500,\"hi\",\"lo\")")))?;
        sheet.write_formula(row, 5, formula(format!("=B{r}*2-A{r}/4")))?;
    }
    Ok(())
}

fn expected_simple() -> Vec<ExpectedCell> {
    let mut out = Vec::new();
    for (row, (a, b)) in (0u32..).zip(simple_inputs()) {
        let cell = |col, value| ExpectedCell {
            sheet: 0,
            row,
            col,
            value,
        };
        let label = if b > 500.0 { "hi" } else { "lo" };
        out.push(cell(2, Expected::Number(a + b)));
        out.push(cell(3, Expected::Number(2.0 * (a + b))));
        out.push(cell(4, Expected::Text(label.to_string())));
        out.push(cell(5, Expected::Number(b * 2.0 - a / 4.0)));
    }
    out
}

struct HeavyInputs {
    data: Vec<(String, f64, String)>,
    main: Vec<(String, String)>,
}

fn heavy_inputs() -> HeavyInputs {
    let mut rng = rng();
    let data = (0..DATA_ROWS)
        .map(|i| {
            let value = f64::from(rng.random_range(1u32..10_000));
            (format!("K{i}"), value, format!("cat{}", i % CATEGORIES))
        })
        .collect();
    let main = (0..MAIN_ROWS)
        .map(|_| {
            let key = format!("K{}", rng.random_range(0..DATA_ROWS + DATA_ROWS / 5));
            let category = format!("cat{}", rng.random_range(0..CATEGORIES));
            (key, category)
        })
        .collect();
    HeavyInputs { data, main }
}

fn write_heavy(book: &mut Workbook) -> Result<()> {
    let inputs = heavy_inputs();
    let n = DATA_ROWS;
    let main = book.add_worksheet().set_name("Main")?;
    for (row, (key, category)) in (0u32..).zip(&inputs.main) {
        let r = row + 1;
        main.write_string(row, 0, key)?;
        main.write_string(row, 1, category)?;
        main.write_formula(
            row,
            2,
            formula(format!(
                "=IFERROR(VLOOKUP(A{r},Data!$A$1:$B${n},2,FALSE),-1)"
            )),
        )?;
        main.write_formula(
            row,
            3,
            formula(format!(
                "=XLOOKUP(A{r},Data!$A$1:$A${n},Data!$B$1:$B${n},-1)"
            )),
        )?;
        main.write_formula(
            row,
            4,
            formula(format!("=SUMIFS(Data!$B$1:$B${n},Data!$C$1:$C${n},B{r})")),
        )?;
        main.write_formula(row, 5, formula(format!("=COUNTIFS(Data!$C$1:$C${n},B{r})")))?;
    }
    let data = book.add_worksheet().set_name("Data")?;
    for (row, (key, value, category)) in (0u32..).zip(&inputs.data) {
        data.write_string(row, 0, key)?;
        data.write_number(row, 1, *value)?;
        data.write_string(row, 2, category)?;
    }
    Ok(())
}

fn expected_heavy() -> Vec<ExpectedCell> {
    let inputs = heavy_inputs();
    let by_key: HashMap<&str, f64> = inputs
        .data
        .iter()
        .map(|(k, v, _)| (k.as_str(), *v))
        .collect();
    let mut totals: HashMap<&str, (f64, f64)> = HashMap::new();
    for (_, value, category) in &inputs.data {
        let entry = totals.entry(category.as_str()).or_default();
        entry.0 += value;
        entry.1 += 1.0;
    }
    let mut out = Vec::new();
    for (row, (key, category)) in (0u32..).zip(&inputs.main) {
        let cell = |col, value| ExpectedCell {
            sheet: 0,
            row,
            col,
            value,
        };
        let lookup = by_key.get(key.as_str()).copied().unwrap_or(-1.0);
        let (sum, count) = totals.get(category.as_str()).copied().unwrap_or_default();
        out.push(cell(2, Expected::Number(lookup)));
        out.push(cell(3, Expected::Number(lookup)));
        out.push(cell(4, Expected::Number(sum)));
        out.push(cell(5, Expected::Number(count)));
    }
    out
}

fn write_chain(book: &mut Workbook) -> Result<()> {
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 1.0)?;
    for row in 1..CHAIN_LEN {
        sheet.write_formula(row, 0, formula(format!("=A{row}+1")))?;
    }
    Ok(())
}

fn formatting_formats() -> [Format; 8] {
    [
        Format::new().set_bold(),
        Format::new()
            .set_italic()
            .set_font_color(Color::RGB(0x1F4E79)),
        Format::new()
            .set_background_color(Color::RGB(0xFFF2CC))
            .set_border(FormatBorder::Thin),
        Format::new()
            .set_align(FormatAlign::Center)
            .set_num_format("0.00%"),
        Format::new()
            .set_num_format("#,##0.00")
            .set_align(FormatAlign::Right),
        Format::new()
            .set_num_format("dd/mm/yyyy")
            .set_font_name("Consolas"),
        Format::new().set_bold().set_italic().set_font_size(14),
        Format::new()
            .set_border(FormatBorder::Medium)
            .set_font_strikethrough(),
    ]
}

pub const FORMAT_ROWS: u32 = 10_000;
pub const FORMAT_COLS: u16 = 8;
pub const MERGE_EVERY: u32 = 50;

fn write_formatting(book: &mut Workbook) -> Result<()> {
    let sheet = book.add_worksheet();
    let formats = formatting_formats();
    let mut rng = rng();
    for col in 0..FORMAT_COLS {
        sheet.set_column_width(col, 8.0 + f64::from(col) * 2.5)?;
    }
    for row in 0..FORMAT_ROWS {
        if row.is_multiple_of(MERGE_EVERY) {
            sheet.merge_range(
                row,
                0,
                row,
                2,
                &format!("Section {}", row / MERGE_EVERY),
                &formats[0],
            )?;
            continue;
        }
        for col in 0..FORMAT_COLS {
            let format = &formats[(row as usize + col as usize) % formats.len()];
            if col % 2 == 0 {
                let n: f64 = rng.random_range(0.0..1000.0);
                sheet.write_number_with_format(row, col, (n * 100.0).round() / 100.0, format)?;
            } else {
                sheet.write_string_with_format(row, col, format!("r{row}c{col}"), format)?;
            }
        }
    }
    Ok(())
}

pub fn bold_expected(row: u32, col: u16) -> bool {
    if row.is_multiple_of(MERGE_EVERY) {
        return col <= 2;
    }
    matches!((row as usize + col as usize) % 8, 0 | 6)
}
