use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use zenkai_engine::{Engine, Workbook, open_xlsx, save_xlsx_atomic, xlsx_bytes};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

use crate::scale_memory as mem;

const SHEET: SheetId = SheetId(0);

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn emit(key: &str, value: f64) {
    println!("{key}={value:.1}");
}

pub fn run(args: &[&str]) -> Result<()> {
    match args {
        ["open", file] => open_stage(Path::new(file)),
        ["steady", file] => steady_stage(Path::new(file)),
        ["fill", "--rows", rows] => fill_stage(rows.parse().map_err(|e| anyhow!("bad rows: {e}"))?),
        ["edit-save", file, "--out", out] => edit_save_stage(Path::new(file), Path::new(out)),
        ["save", file, "--out", out] => save_stage(Path::new(file), Path::new(out)),
        _ => bail!(
            "usage: scale-stage open <file> | steady <file> | fill --rows <n> | \
             edit-save <file> --out <file> | save <file> --out <file>"
        ),
    }
}

fn open_workbook(path: &Path) -> Result<(Workbook, f64)> {
    let start = Instant::now();
    let opened = open_xlsx(path)?;
    let open_ms = ms(start);
    mem::check_cap("open")?;
    Ok((opened.workbook, open_ms))
}

fn open_stage(path: &Path) -> Result<()> {
    let (_, open_ms) = open_workbook(path)?;
    emit("open_ms", open_ms);
    emit("peak_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    Ok(())
}

fn steady_stage(path: &Path) -> Result<()> {
    // The workbook stays alive while the rest reading is taken; measuring
    // after the drop would report allocator residue instead of rest memory.
    let (_workbook, open_ms) = open_workbook(path)?;
    emit("open_ms", open_ms);
    emit("steady_commit_mb", mem::to_mb(mem::current_commit_bytes()?));
    emit("peak_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    Ok(())
}

// A formula column filled down N rows in a fresh workbook, through the app's fill path.
fn fill_stage(rows: u32) -> Result<()> {
    if rows == 0 || u64::from(rows) > 1_000_000 {
        bail!("fill rows must be between 1 and 1000000");
    }
    let mut workbook = Workbook::new_empty()?;
    let from = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(0));
    let range = Range::new(
        from,
        CellPos::new(RowIdx::clamped(i64::from(rows) - 1), ColIdx::clamped(0)),
    );
    let start = Instant::now();
    workbook.fill_with(SHEET, from, "=B1*2", range)?;
    let fill_ms = ms(start);
    mem::check_cap("fill")?;
    emit("fill_ms", fill_ms);
    emit("fill_rows", f64::from(rows));
    emit("peak_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    Ok(())
}

fn edit_save_stage(path: &Path, out: &Path) -> Result<()> {
    let (mut workbook, open_ms) = open_workbook(path)?;
    let at = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(0));
    let start = Instant::now();
    workbook.set_input(SHEET, at, "1")?;
    let edit_ms = ms(start);
    let save_ms = save_to(workbook, out)?;
    mem::check_cap("edit-save")?;
    emit("open_ms", open_ms);
    emit("edit_ms", edit_ms);
    emit("save_ms", save_ms);
    emit("edit_save_ms", edit_ms + save_ms);
    emit("peak_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    Ok(())
}

fn save_stage(path: &Path, out: &Path) -> Result<()> {
    let (workbook, open_ms) = open_workbook(path)?;
    let save_ms = save_to(workbook, out)?;
    mem::check_cap("save")?;
    emit("open_ms", open_ms);
    emit("save_ms", save_ms);
    emit("peak_commit_mb", mem::to_mb(mem::peak_commit_bytes()?));
    Ok(())
}

// The corpus file is never touched: bytes go to a temp path through the atomic save.
fn save_to(workbook: Workbook, out: &Path) -> Result<f64> {
    let start = Instant::now();
    let bytes = xlsx_bytes(&workbook)?;
    save_xlsx_atomic(&bytes, out).with_context(|| format!("saving {}", out.display()))?;
    Ok(ms(start))
}

pub fn out_path(workdir: &Path, case: &str, stage: &str) -> PathBuf {
    workdir.join(format!("{case}.{stage}.xlsx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_stage_is_an_error() {
        assert!(run(&["wipe", "everything"]).is_err());
        assert!(run(&["fill", "--rows", "many"]).is_err());
        assert!(run(&["fill", "--rows", "0"]).is_err());
    }

    #[test]
    fn save_outputs_stay_beside_their_case_and_stage() {
        let workdir = Path::new("/tmp/work");
        assert_eq!(
            out_path(workdir, "a.xlsx", "save"),
            Path::new("/tmp/work/a.xlsx.save.xlsx")
        );
    }
}
