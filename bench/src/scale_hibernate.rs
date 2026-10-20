use std::path::Path;
use std::time::Instant;

use anyhow::{Result, bail};
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

use crate::scale_memory as mem;

const SHEET: SheetId = SheetId(0);
const MAX_ROWS: u32 = 1_000_000;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn emit(key: &str, value: f64) {
    println!("{key}={value:.1}");
}

// A workbook of `rows` formulas with one undo step, put to rest in `dump` and woken again:
// the time of each, the size of the dump and the committed memory at each point.
pub fn run(rows: u32, dump: &Path) -> Result<()> {
    if rows == 0 || rows > MAX_ROWS {
        bail!("hibernate rows must be between 1 and {MAX_ROWS}");
    }
    let mut workbook = Workbook::new_empty()?;
    let from = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(0));
    let range = Range::new(
        from,
        CellPos::new(RowIdx::clamped(i64::from(rows) - 1), ColIdx::clamped(0)),
    );
    workbook.fill_with(SHEET, from, "=B1*2", range)?;
    mem::check_cap("hibernate-fill")?;
    emit("loaded_commit_mb", mem::to_mb(mem::current_commit_bytes()?));

    let start = Instant::now();
    let kept = workbook.hibernate(dump)?;
    emit("hibernate_ms", ms(start));
    emit("peak_through_hibernate_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    emit("dump_mb", mem::to_mb(std::fs::metadata(dump)?.len()));

    drop(workbook);
    emit("rested_commit_mb", mem::to_mb(mem::current_commit_bytes()?));

    let start = Instant::now();
    let mut woken = Workbook::wake(dump, &kept)?;
    emit("wake_ms", ms(start));
    emit("woken_commit_mb", mem::to_mb(mem::current_commit_bytes()?));
    emit("peak_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    mem::check_cap("hibernate-wake")?;

    // The history came back with the cells: one undo empties what the fill wrote.
    woken.undo()?;
    if !woken.cell(SHEET, from).text.is_empty() {
        bail!("the woken workbook did not undo the fill");
    }
    std::fs::remove_file(dump)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_outside_the_limit_are_refused() {
        assert!(run(0, Path::new("unused")).is_err());
        assert!(run(MAX_ROWS + 1, Path::new("unused")).is_err());
    }
}
