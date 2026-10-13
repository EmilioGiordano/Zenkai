#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use peak_alloc::PeakAlloc;
use zenkai_engine::{Engine, Workbook, open_xlsx, run_with_engine_stack, write_atomic, xlsx_bytes};
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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (source, target, cell) = match args.as_slice() {
        [source, target] => (source, target, "A1"),
        [source, target, cell] => (source, target, cell.as_str()),
        _ => bail!("usage: measure_save <file.xlsx> <saved.xlsx> [edited cell, A1 by default]"),
    };
    let source = PathBuf::from(source);
    let target = PathBuf::from(target);
    if target.exists() {
        bail!("{} exists; pick a new path", target.display());
    }
    let pos = CellPos::parse_a1(cell).ok_or_else(|| anyhow!("{cell} is not a cell"))?;
    let sheet = SheetId(0);

    begin("open");
    let start = Instant::now();
    let mut workbook: Workbook = open_xlsx(&source)
        .with_context(|| format!("opening {}", source.display()))?
        .workbook;
    done("open", start, "");

    begin("edit");
    let start = Instant::now();
    let before = workbook.input(sheet, pos);
    workbook.set_input(sheet, pos, &flip_number(&before))?;
    done("edit", start, &format!("cell={cell}"));

    begin("serialize");
    let start = Instant::now();
    let bytes = xlsx_bytes(&workbook)?;
    done(
        "serialize",
        start,
        &format!("zip_mb={:.1}", bytes.len() as f64 / 1024.0 / 1024.0),
    );

    begin("write");
    let start = Instant::now();
    write_atomic(&target, &bytes, |_| Ok(()))?;
    done("write", start, "");
    drop(bytes);

    begin("verify");
    let start = Instant::now();
    let written = std::fs::read(&target)?;
    run_with_engine_stack(|| Workbook::from_xlsx_bytes(&written, "verify"))?;
    done("verify", start, "");
    Ok(())
}
