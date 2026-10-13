#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{PoisonError, RwLock, RwLockWriteGuard, mpsc};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use peak_alloc::PeakAlloc;
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, SheetId};

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

// PeakPagedMemorySize64 is the Win32 PeakPagefileUsage: peak committed private bytes
// over the whole process, so a stage only shows here when it beats every earlier one.
fn peak_commit_mb() -> f64 {
    let script = format!(
        "(Get-Process -Id {}).PeakPagedMemorySize64",
        std::process::id()
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .map_or(-1.0, |bytes| bytes / 1024.0 / 1024.0)
}

fn begin(stage: &str) {
    println!("stage={stage} begin");
    PEAK.reset_peak_usage();
}

// heap_peak_mb is the allocator peak inside the stage alone, the figure that shows what
// one save costs on top of the open workbook.
fn done(stage: &str, start: Instant, extra: &str) {
    println!(
        "stage={stage} ms={:.0} heap_now_mb={:.0} heap_peak_mb={:.0} peak_commit_mb={:.0} {extra}",
        ms(start),
        PEAK.current_usage_as_mb(),
        PEAK.peak_usage_as_mb(),
        peak_commit_mb()
    );
}

fn flip_number(current: &str) -> String {
    match current.parse::<f64>() {
        Ok(number) => (number + 1.0).to_string(),
        Err(_) => "1".to_string(),
    }
}

fn write_lock(shared: &RwLock<Workbook>) -> RwLockWriteGuard<'_, Workbook> {
    shared.write().unwrap_or_else(PoisonError::into_inner)
}

fn flip(workbook: &mut Workbook, sheet: SheetId, pos: CellPos) -> Result<()> {
    let before = workbook.input(sheet, pos);
    workbook.set_input(sheet, pos, &flip_number(&before))?;
    Ok(())
}

// Saves as the app does: the snapshot under the read lock, the write after it. An edit
// asked for once the save holds the lock reports how long it waited, which is what a user
// typing during a save feels.
fn edit_and_save(
    shared: &RwLock<Workbook>,
    sheet: SheetId,
    pos: CellPos,
    target: &Path,
) -> Result<()> {
    begin("edit");
    let start = Instant::now();
    flip(&mut write_lock(shared), sheet, pos)?;
    done("edit", start, "");

    begin("save");
    let (locked, lock_taken) = mpsc::channel();
    let start = Instant::now();
    let (locked_ms, edit_wait_ms) = std::thread::scope(|scope| -> Result<(f64, f64)> {
        let saver = scope.spawn(|| -> Result<f64> {
            let guard = shared.read().unwrap_or_else(PoisonError::into_inner);
            locked.send(())?;
            let snapshot = guard.save_snapshot(target, SaveTarget::Document)?;
            drop(guard);
            let locked_ms = ms(start);
            save_xlsx(snapshot)?;
            Ok(locked_ms)
        });
        lock_taken.recv()?;
        let asked = Instant::now();
        flip(&mut write_lock(shared), sheet, pos)?;
        let edit_wait_ms = ms(asked);
        let locked_ms = saver
            .join()
            .map_err(|_| anyhow!("the save thread panicked"))??;
        Ok((locked_ms, edit_wait_ms))
    })?;
    let zip_mb = std::fs::metadata(target)?.len() as f64 / 1024.0 / 1024.0;
    done(
        "save",
        start,
        &format!(
            "locked_ms={locked_ms:.0} queued_edit_wait_ms={edit_wait_ms:.0} zip_mb={zip_mb:.1}"
        ),
    );
    Ok(())
}

// The second save copies from the file the first one wrote, as saving a document again does.
// The first snapshot, of the workbook as opened, costs what the fingerprint taken at open
// costs: every sheet hashed and nothing copied.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: measure_save <file.xlsx> <saved.xlsx> [edited cell, A1] [sheet index, 0]";
    let (source, target, cell, sheet) = match args.as_slice() {
        [source, target] => (source, target, "A1", "0"),
        [source, target, cell] => (source, target, cell.as_str(), "0"),
        [source, target, cell, sheet] => (source, target, cell.as_str(), sheet.as_str()),
        _ => bail!(usage),
    };
    let source = PathBuf::from(source);
    let target = PathBuf::from(target);
    if target.exists() {
        bail!("{} exists; pick a new path", target.display());
    }
    let pos = CellPos::parse_a1(cell).ok_or_else(|| anyhow!("{cell} is not a cell"))?;
    let sheet = SheetId(
        sheet
            .parse()
            .with_context(|| format!("{sheet} is not a sheet index"))?,
    );

    begin("open");
    let start = Instant::now();
    let workbook: Workbook = open_xlsx(&source)
        .with_context(|| format!("opening {}", source.display()))?
        .workbook;
    done("open", start, "");

    begin("unedited_snapshot");
    let start = Instant::now();
    drop(workbook.save_snapshot(&target, SaveTarget::Document)?);
    done("unedited_snapshot", start, "");

    let shared = RwLock::new(workbook);
    println!("save=first sheet={} cell={cell}", sheet.0);
    edit_and_save(&shared, sheet, pos, &target)?;
    println!("save=again sheet={} cell={cell}", sheet.0);
    edit_and_save(&shared, sheet, pos, &target)?;
    Ok(())
}
