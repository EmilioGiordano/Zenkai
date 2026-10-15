#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Result, bail};
use rust_xlsxwriter::{Formula, Workbook, Worksheet};

const COLUMNS: u16 = 8;
const SMALL_ROWS: u32 = 1_000;
const SHEETS: u32 = 16;
const ROW_COUNTS: [u32; 3] = [100_000, 200_000, 400_000];
const DISTINCT_TEXTS: u32 = 200;

#[derive(Clone, Copy)]
enum Shape {
    OneSheet,
    BigPlusSmall,
    Even,
    DistinctFormulas,
    // Five formulas a row, about a million at 200,000 rows.
    FormulaHeavy,
}

impl Shape {
    const ALL: [Shape; 5] = [
        Shape::OneSheet,
        Shape::BigPlusSmall,
        Shape::Even,
        Shape::DistinctFormulas,
        Shape::FormulaHeavy,
    ];

    fn name(self) -> &'static str {
        match self {
            Shape::OneSheet => "one-sheet",
            Shape::BigPlusSmall => "big-plus-small",
            Shape::Even => "even",
            Shape::DistinctFormulas => "distinct-formulas",
            Shape::FormulaHeavy => "formula-heavy",
        }
    }

    fn sheet_rows(self, rows: u32) -> Vec<u32> {
        match self {
            Shape::OneSheet | Shape::DistinctFormulas | Shape::FormulaHeavy => vec![rows],
            Shape::BigPlusSmall => std::iter::once(rows)
                .chain(std::iter::repeat_n(SMALL_ROWS, SHEETS as usize - 1))
                .collect(),
            Shape::Even => vec![rows / SHEETS; SHEETS as usize],
        }
    }

    // IronCalc keeps one parsed formula per distinct R1C1 text, so `=A2*C2` down a column
    // is one formula and a constant that differs per row makes one per row.
    fn formula(self, row: u32) -> String {
        let line = row + 1;
        match self {
            Shape::DistinctFormulas => format!("=A{line}*C{line}+{row}"),
            Shape::OneSheet | Shape::BigPlusSmall | Shape::Even | Shape::FormulaHeavy => {
                format!("=A{line}*C{line}")
            }
        }
    }
}

// Numbers, repeated shared strings and a formula per row with its cached result, the mix
// a save has to write back.
fn fill(sheet: &mut Worksheet, shape: Shape, rows: u32) -> Result<()> {
    for row in 1..=rows {
        let id = f64::from(row);
        let price = f64::from(row % 977) / 4.0;
        sheet.write_number(row, 0, id)?;
        sheet.write_string(row, 1, format!("item {}", row % DISTINCT_TEXTS))?;
        sheet.write_number(row, 2, price)?;
        let result = match shape {
            Shape::DistinctFormulas => id * price + id,
            Shape::OneSheet | Shape::BigPlusSmall | Shape::Even | Shape::FormulaHeavy => {
                id * price
            }
        };
        sheet.write_formula(
            row,
            3,
            Formula::new(shape.formula(row)).set_result(result.to_string()),
        )?;
        for column in 4..COLUMNS {
            let line = row + 1;
            let factor = f64::from(column);
            match shape {
                Shape::FormulaHeavy => sheet.write_formula(
                    row,
                    column,
                    Formula::new(format!("=A{line}*{column}+C{line}"))
                        .set_result((id * factor + price).to_string()),
                )?,
                Shape::OneSheet | Shape::BigPlusSmall | Shape::Even | Shape::DistinctFormulas => {
                    sheet.write_number(row, column, f64::from(row % (u32::from(column) * 31)))?
                }
            };
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir] = args.as_slice() else {
        bail!("usage: make_save_matrix <output dir>");
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    for shape in Shape::ALL {
        for rows in ROW_COUNTS {
            let mut book = Workbook::new();
            for sheet_rows in shape.sheet_rows(rows) {
                fill(book.add_worksheet(), shape, sheet_rows)?;
            }
            let path = dir.join(format!("{}-{rows}.xlsx", shape.name()));
            book.save(&path)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
