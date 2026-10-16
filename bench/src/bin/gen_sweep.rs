#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use rust_xlsxwriter::{Format, Formula, Workbook};

const SEED_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

fn pseudo_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(SEED_MULTIPLIER)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn numbers(dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let mut state = 1;
    for row in 0..50_000u32 {
        for column in 0..20u16 {
            sheet.write_number(
                row,
                column,
                (pseudo_random(&mut state) % 1_000_000) as f64 / 100.0,
            )?;
        }
    }
    book.save(dir.join("numbers-dense.xlsx"))?;
    Ok(())
}

fn strings(dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    for row in 0..50_000u32 {
        for column in 0..20u16 {
            sheet.write_string(row, column, format!("text {row} {column}"))?;
        }
    }
    book.save(dir.join("strings-heavy.xlsx"))?;
    Ok(())
}

fn styled(dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    let bold = Format::new().set_bold();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let sheet = book.add_worksheet();
    for row in 0..50_000u32 {
        for column in 0..20u16 {
            if column % 2 == 0 {
                sheet.write_number_with_format(
                    row,
                    column,
                    f64::from(40_000 + row % 5_000),
                    &date,
                )?;
            } else {
                sheet.write_string_with_format(
                    row,
                    column,
                    format!("name {}", row % 500),
                    &bold,
                )?;
            }
        }
    }
    book.save(dir.join("styled-dense.xlsx"))?;
    Ok(())
}

fn formulas(dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    for row in 0..50_000u32 {
        sheet.write_number(row, 0, f64::from(row))?;
        sheet.write_number(row, 1, f64::from(row % 97))?;
        for column in 2..12u16 {
            let text = format!("=A{0}*{1}+B{0}", row + 1, column);
            sheet.write_formula(row, column, Formula::new(text).set_result("0"))?;
        }
    }
    book.save(dir.join("formulas-heavy.xlsx"))?;
    Ok(())
}

fn sparse(dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let mut state = 7;
    for row in 0..200_000u32 {
        for _ in 0..5 {
            let column = (pseudo_random(&mut state) % 200) as u16;
            sheet.write_number(row, column, f64::from(row))?;
        }
    }
    book.save(dir.join("sparse.xlsx"))?;
    Ok(())
}

fn many_sheets(dir: &Path) -> Result<()> {
    let mut book = Workbook::new();
    for index in 0..100u32 {
        let sheet = book.add_worksheet();
        for row in 0..10_000u32 {
            sheet.write_number(row, 0, f64::from(row + index))?;
            sheet.write_string(row, 1, format!("s{}", row % 300))?;
        }
    }
    book.save(dir.join("many-sheets.xlsx"))?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir] = args.as_slice() else {
        bail!("usage: gen_sweep <output dir>");
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    numbers(&dir)?;
    strings(&dir)?;
    styled(&dir)?;
    formulas(&dir)?;
    sparse(&dir)?;
    many_sheets(&dir)
}
