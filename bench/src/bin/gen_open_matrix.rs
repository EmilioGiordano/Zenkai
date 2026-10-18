#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Result, bail};
use rust_xlsxwriter::{
    ConditionalFormat2ColorScale, ConditionalFormatCell, ConditionalFormatCellRule,
    ConditionalFormatFormula, Format, Formula, Workbook, Worksheet,
};

const SEED_MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const SEED_INCREMENT: u64 = 1_442_695_040_888_963_407;

struct Shape {
    sheets: u32,
    rows: u32,
    columns: u16,
    formula_percent: u64,
    string_percent: u64,
    distinct_formulas: bool,
    unique_strings: bool,
    styles: usize,
    conditional_rules: u32,
    cycle: bool,
    cycle_readers: u32,
    today: bool,
    spills: u32,
}

fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(SEED_MULTIPLIER)
        .wrapping_add(SEED_INCREMENT);
    *state >> 33
}

fn parse(args: &[String]) -> Result<Shape> {
    let mut shape = Shape {
        sheets: 1,
        rows: 100_000,
        columns: 10,
        formula_percent: 0,
        string_percent: 0,
        distinct_formulas: false,
        unique_strings: false,
        styles: 0,
        conditional_rules: 0,
        cycle: false,
        cycle_readers: 0,
        today: false,
        spills: 0,
    };
    for pair in args {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("expected key=value, got {pair}");
        };
        match key {
            "sheets" => shape.sheets = value.parse()?,
            "rows" => shape.rows = value.parse()?,
            "cols" => shape.columns = value.parse()?,
            "formulas" => shape.formula_percent = value.parse()?,
            "strings" => shape.string_percent = value.parse()?,
            "distinct" => shape.distinct_formulas = value == "1",
            "unique_strings" => shape.unique_strings = value == "1",
            "styles" => shape.styles = value.parse()?,
            "cf" => shape.conditional_rules = value.parse()?,
            "cycle" => shape.cycle = value == "1",
            "cycle_readers" => shape.cycle_readers = value.parse()?,
            "today" => shape.today = value == "1",
            "spills" => shape.spills = value.parse()?,
            _ => bail!("unknown key {key}"),
        }
    }
    if shape.columns < 3 {
        bail!("cols must be at least 3");
    }
    Ok(shape)
}

// What an edit has to deal with besides the data, all on the first sheet: rules over the
// whole data columns, two cells that read each other below the data (and optionally formulas
// that read them, and a TODAY cell), and dynamic arrays beside it.
fn add_extras(sheet: &mut Worksheet, shape: &Shape) -> Result<()> {
    let last_row = shape.rows.saturating_sub(1);
    for kind in 0..shape.conditional_rules {
        match kind % 3 {
            0 => sheet.add_conditional_format(
                0,
                0,
                last_row,
                0,
                &ConditionalFormatCell::new()
                    .set_rule(ConditionalFormatCellRule::GreaterThan(5000))
                    .set_format(Format::new().set_bold()),
            )?,
            1 => sheet.add_conditional_format(
                0,
                1,
                last_row,
                1,
                &ConditionalFormat2ColorScale::new(),
            )?,
            _ => sheet.add_conditional_format(
                0,
                0,
                last_row,
                0,
                &ConditionalFormatFormula::new()
                    .set_rule("=$B1>5000")
                    .set_format(Format::new().set_italic()),
            )?,
        };
    }
    if shape.cycle {
        let line = shape.rows + 1;
        sheet.write_formula(shape.rows, 0, Formula::new(format!("=B{line}+1")))?;
        sheet.write_formula(shape.rows, 1, Formula::new(format!("=A{line}+1")))?;
    }
    // Formulas that read the cycle, in a column of their own beyond the data, as a model
    // of a large closure of cells downstream of a cycle.
    for row in 0..shape.cycle_readers {
        let line = shape.rows + 1;
        sheet.write_formula(
            row,
            shape.columns + 2,
            Formula::new(format!("=$A${line}*0+{row}")),
        )?;
    }
    if shape.today {
        sheet.write_formula(shape.rows + 1, 0, Formula::new("=TODAY()"))?;
    }
    for spill in 0..shape.spills {
        sheet.write_dynamic_formula(spill * 10, shape.columns + 1, "=SEQUENCE(5)")?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, rest)) = args.split_first() else {
        bail!(
            "usage: gen_open_matrix <out.xlsx> [sheets=1 rows=100000 cols=10 formulas=0 strings=0 distinct=0 unique_strings=0 styles=0 cf=0 cycle=0 cycle_readers=0 today=0 spills=0]"
        );
    };
    let shape = parse(rest)?;
    let formats: Vec<Format> = (0..shape.styles)
        .map(|n| {
            Format::new()
                .set_num_format(format!("0.{}", "0".repeat(n % 6 + 1)).as_str())
                .set_font_size(8.0 + (n / 6) as f64)
        })
        .collect();
    let mut book = Workbook::new();
    let mut state = 7;
    for sheet_index in 0..shape.sheets {
        let sheet = book.add_worksheet();
        if sheet_index == 0 {
            add_extras(sheet, &shape)?;
        }
        for row in 0..shape.rows {
            for column in 0..shape.columns {
                let roll = next(&mut state) % 100;
                let format = formats.get((next(&mut state) as usize) % formats.len().max(1));
                let line = row + 1;
                if column >= 2 && roll < shape.formula_percent {
                    let text = if shape.distinct_formulas {
                        format!("=A{line}*{}+B{line}", next(&mut state) % 100_000)
                    } else {
                        format!("=A{line}*2+B{line}")
                    };
                    let formula = Formula::new(text);
                    match format {
                        Some(format) => {
                            sheet.write_formula_with_format(row, column, formula, format)?
                        }
                        None => sheet.write_formula(row, column, formula)?,
                    };
                } else if column >= 2 && roll >= 100 - shape.string_percent {
                    let text = if shape.unique_strings {
                        format!("item {sheet_index}-{row}-{column}")
                    } else {
                        format!("item {}", next(&mut state) % 5_000)
                    };
                    match format {
                        Some(format) => {
                            sheet.write_string_with_format(row, column, text, format)?
                        }
                        None => sheet.write_string(row, column, text)?,
                    };
                } else {
                    let value = (next(&mut state) % 1_000_000) as f64 / 100.0;
                    match format {
                        Some(format) => {
                            sheet.write_number_with_format(row, column, value, format)?
                        }
                        None => sheet.write_number(row, column, value)?,
                    };
                }
            }
        }
    }
    book.save(PathBuf::from(out))?;
    Ok(())
}
