#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

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
            Shape::OneSheet | Shape::BigPlusSmall | Shape::Even | Shape::FormulaHeavy => id * price,
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

// Whether the big sheets' first formula column reads the edited cell, Inputs!A1, so a
// one-cell edit changes them too.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Linked {
    No,
    EveryRow,
    // One row in LINKED_ROW_STEP.
    FewRows,
}

const LINKED_ROW_STEP: u32 = 10_000;
const DISTINCT_STRINGS: u32 = 50_000;

// One file of the sweep: a small first sheet, Inputs, holds the edited cell next to
// `big_sheets` sheets of `rows` rows. Each big row has an id in A, then `strings` text
// columns, then `formulas` formula columns, then numbers, `COLUMNS` in all.
struct Sweep {
    name: &'static str,
    inputs: bool,
    big_sheets: u32,
    rows: u32,
    strings: u16,
    formulas: u16,
    linked: Linked,
}

impl Sweep {
    const BASE: Sweep = Sweep {
        name: "base",
        inputs: true,
        big_sheets: 1,
        rows: 100_000,
        strings: 2,
        formulas: 2,
        linked: Linked::No,
    };

    // One property away from BASE at a time.
    const ALL: [Sweep; 11] = [
        Sweep::BASE,
        Sweep {
            name: "rows-400k",
            rows: 400_000,
            ..Sweep::BASE
        },
        Sweep {
            name: "sheets-10",
            big_sheets: 10,
            rows: 30_000,
            ..Sweep::BASE
        },
        Sweep {
            name: "sheets-30",
            big_sheets: 30,
            rows: 10_000,
            ..Sweep::BASE
        },
        Sweep {
            name: "edit-in-big-sheet",
            inputs: false,
            ..Sweep::BASE
        },
        Sweep {
            name: "formulas-0",
            formulas: 0,
            ..Sweep::BASE
        },
        Sweep {
            name: "formulas-7",
            strings: 0,
            formulas: 7,
            ..Sweep::BASE
        },
        Sweep {
            name: "strings-0",
            strings: 0,
            ..Sweep::BASE
        },
        Sweep {
            name: "strings-5",
            strings: 5,
            ..Sweep::BASE
        },
        Sweep {
            name: "linked-every-row",
            linked: Linked::EveryRow,
            ..Sweep::BASE
        },
        Sweep {
            name: "linked-few-rows",
            linked: Linked::FewRows,
            ..Sweep::BASE
        },
    ];

    fn links(&self, row: u32) -> bool {
        match self.linked {
            Linked::No => false,
            Linked::EveryRow => true,
            Linked::FewRows => row.is_multiple_of(LINKED_ROW_STEP),
        }
    }

    fn fill(&self, sheet: &mut Worksheet) -> Result<()> {
        for row in 1..=self.rows {
            let line = row + 1;
            let id = f64::from(row);
            sheet.write_number(row, 0, id)?;
            for column in 1..COLUMNS {
                let factor = f64::from(column);
                if column <= self.strings {
                    let text = (row * u32::from(column)) % DISTINCT_STRINGS;
                    sheet.write_string(row, column, format!("text {text}"))?;
                } else if column <= self.strings + self.formulas {
                    let formula = if column == self.strings + 1 && self.links(row) {
                        Formula::new(format!("=A{line}*Inputs!$A$1")).set_result(id.to_string())
                    } else {
                        Formula::new(format!("=A{line}*{column}"))
                            .set_result((id * factor).to_string())
                    };
                    sheet.write_formula(row, column, formula)?;
                } else {
                    sheet.write_number(row, column, f64::from(row % (u32::from(column) * 31)))?;
                }
            }
        }
        Ok(())
    }

    fn write(&self, dir: &Path) -> Result<()> {
        let mut book = Workbook::new();
        if self.inputs {
            let inputs = book.add_worksheet().set_name("Inputs")?;
            for row in 0..100 {
                inputs.write_number(row, 0, 1.0)?;
            }
        }
        for _ in 0..self.big_sheets {
            self.fill(book.add_worksheet())?;
        }
        let path = dir.join(format!("sweep-{}.xlsx", self.name));
        book.save(&path)?;
        println!("wrote {}", path.display());
        Ok(())
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dir, sweep) = match args.as_slice() {
        [dir] => (dir, false),
        [dir, mode] if mode == "sweep" => (dir, true),
        _ => bail!("usage: make_save_matrix <output dir> [sweep]"),
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    if sweep {
        for file in &Sweep::ALL {
            file.write(&dir)?;
        }
        return Ok(());
    }
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
