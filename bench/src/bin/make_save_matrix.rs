#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Result, bail};
use rust_xlsxwriter::{Formula, Workbook, Worksheet};

const COLUMNS: u16 = 8;
const SMALL_ROWS: u32 = 1_000;
const SHEETS: u32 = 16;
const ROW_COUNTS: [u32; 2] = [100_000, 400_000];
const DISTINCT_TEXTS: u32 = 200;

#[derive(Clone, Copy)]
enum Shape {
    OneSheet,
    BigPlusSmall,
    Even,
}

impl Shape {
    const ALL: [Shape; 3] = [Shape::OneSheet, Shape::BigPlusSmall, Shape::Even];

    fn name(self) -> &'static str {
        match self {
            Shape::OneSheet => "one-sheet",
            Shape::BigPlusSmall => "big-plus-small",
            Shape::Even => "even",
        }
    }

    fn sheet_rows(self, rows: u32) -> Vec<u32> {
        match self {
            Shape::OneSheet => vec![rows],
            Shape::BigPlusSmall => std::iter::once(rows)
                .chain(std::iter::repeat_n(SMALL_ROWS, SHEETS as usize - 1))
                .collect(),
            Shape::Even => vec![rows / SHEETS; SHEETS as usize],
        }
    }
}

// Numbers, repeated shared strings and a formula per row with its cached result, the mix
// a save has to write back.
fn fill(sheet: &mut Worksheet, rows: u32) -> Result<()> {
    for row in 1..=rows {
        let id = f64::from(row);
        let price = f64::from(row % 977) / 4.0;
        sheet.write_number(row, 0, id)?;
        sheet.write_string(row, 1, format!("item {}", row % DISTINCT_TEXTS))?;
        sheet.write_number(row, 2, price)?;
        let line = row + 1;
        sheet.write_formula(
            row,
            3,
            Formula::new(format!("=A{line}*C{line}")).set_result((id * price).to_string()),
        )?;
        for column in 4..COLUMNS {
            sheet.write_number(row, column, f64::from(row % (u32::from(column) * 31)))?;
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
                fill(book.add_worksheet(), sheet_rows)?;
            }
            let path = dir.join(format!("{}-{rows}.xlsx", shape.name()));
            book.save(&path)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
