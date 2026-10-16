#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use peak_alloc::PeakAlloc;
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

const SHEET: SheetId = SheetId(0);
const CHUNK_ROWS: u32 = 20_000;

#[derive(Clone, Copy)]
enum Fill {
    Shared,
    Text,
}

fn pos(row: u32, col: u16) -> CellPos {
    CellPos::new(
        RowIdx::clamped(i64::from(row)),
        ColIdx::clamped(i64::from(col)),
    )
}

fn begin(stage: &str) {
    println!("stage={stage} begin");
    PEAK.reset_peak_usage();
}

fn done(stage: &str, start: Instant, extra: &str) {
    println!(
        "stage={stage} ms={:.0} heap_now_mb={:.0} heap_peak_mb={:.0} {extra}",
        start.elapsed().as_secs_f64() * 1000.0,
        PEAK.current_usage_as_mb(),
        PEAK.peak_usage_as_mb(),
    );
}

fn numbers(workbook: &mut Workbook, rows: u32) -> Result<()> {
    let mut first = 0;
    while first < rows {
        let count = CHUNK_ROWS.min(rows - first);
        let block: Vec<Vec<String>> = (first..first + count)
            .map(|row| vec![row.to_string(), (row % 977).to_string()])
            .collect();
        workbook.set_inputs(SHEET, pos(first, 0), &block)?;
        first += count;
    }
    Ok(())
}

// Ctrl+D over `rows` rows (`shared`), or Ctrl+Enter, which fills as text, over the same
// cells (`text`); the two differ by what the formula cost per cell.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: measure_fill <rows> <shared|text> <saved.xlsx>";
    let [rows, mode, target] = args.as_slice() else {
        bail!(usage);
    };
    let rows: u32 = rows.parse().context("rows")?;
    let mode = match mode.as_str() {
        "shared" => Fill::Shared,
        "text" => Fill::Text,
        _ => bail!(usage),
    };
    let target = PathBuf::from(target);
    if target.exists() {
        bail!("{} exists; pick a new path", target.display());
    }

    let mut workbook = Workbook::new_empty()?;
    begin("data");
    let start = Instant::now();
    numbers(&mut workbook, rows)?;
    done("data", start, "");

    let formula = "=A1*2+B1";
    let column = Range::new(pos(0, 2), pos(rows - 1, 2));
    begin("fill");
    let start = Instant::now();
    match mode {
        Fill::Shared => {
            workbook.set_input(SHEET, pos(0, 2), formula)?;
            workbook.fill(SHEET, column, true)?;
        }
        Fill::Text => workbook.fill_with(SHEET, pos(0, 2), formula, column)?,
    }
    done("fill", start, "");
    let last = workbook.input(SHEET, pos(rows - 1, 2));
    println!("last={last}");

    begin("undo");
    let start = Instant::now();
    workbook.undo()?;
    done("undo", start, "");
    begin("redo");
    let start = Instant::now();
    workbook.redo()?;
    done("redo", start, "");

    begin("save");
    let start = Instant::now();
    let snapshot = workbook.save_snapshot(&target, SaveTarget::Document)?;
    save_xlsx(snapshot)?;
    let zip_mb = std::fs::metadata(&target)?.len() as f64 / 1024.0 / 1024.0;
    done("save", start, &format!("zip_mb={zip_mb:.1}"));

    begin("reopen");
    let start = Instant::now();
    let reopened = open_xlsx(&target)?.workbook;
    done("reopen", start, "");
    let same = (0..rows)
        .all(|row| reopened.input(SHEET, pos(row, 2)) == workbook.input(SHEET, pos(row, 2)));
    println!("reopened_formulas_equal={same}");
    Ok(())
}
