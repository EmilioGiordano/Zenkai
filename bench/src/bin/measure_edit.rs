#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use zenkai_engine::{
    EditProfile, Engine, EngineError, RecalcMode, XlsxReader, open_xlsx_with, run_with_engine_stack,
};
use zenkai_types::CellPos;

const USAGE: &str =
    "usage: measure_edit <file.xlsx> <sheet name or index> <cell A1> <text> [--repeat N]";

struct Request {
    path: PathBuf,
    sheet: String,
    cell: CellPos,
    text: String,
    repeat: u32,
}

struct Edit {
    profile: EditProfile,
    total: Duration,
}

struct Report {
    sheet_index: u32,
    open: Duration,
    edits: Vec<Edit>,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn parse_request(args: &[String]) -> Result<Request> {
    let (positional, repeat) = match args {
        [file, sheet, cell, text] => ([file, sheet, cell, text], 1),
        [file, sheet, cell, text, flag, count] if flag == "--repeat" => {
            let count: u32 = count.parse().map_err(|_| anyhow!(USAGE))?;
            ([file, sheet, cell, text], count)
        }
        _ => bail!(USAGE),
    };
    let [file, sheet, cell, text] = positional;
    let cell = CellPos::parse_a1(cell).ok_or_else(|| anyhow!("the cell is not an A1 address"))?;
    if repeat == 0 {
        bail!("--repeat must be at least 1");
    }
    Ok(Request {
        path: PathBuf::from(file),
        sheet: sheet.clone(),
        cell,
        text: text.clone(),
        repeat,
    })
}

// The app's own open path, then the app's own edit path (`Workbook::set_input`) with the
// phases timed apart. Nothing the file holds is read out, so private files can be measured.
fn measure(request: &Request) -> Result<Report, EngineError> {
    let started = Instant::now();
    let opened = open_xlsx_with(&request.path, XlsxReader::Fast)?;
    let open = started.elapsed();
    let mut workbook = opened.workbook;
    let sheets = workbook.sheets();
    let by_index = request
        .sheet
        .parse::<usize>()
        .ok()
        .and_then(|index| sheets.get(index));
    let sheet = by_index
        .or_else(|| sheets.iter().find(|sheet| sheet.name == request.sheet))
        .map(|sheet| sheet.id)
        .ok_or_else(|| EngineError::Rejected("no sheet has that name or index".to_string()))?;
    let mut edits = Vec::new();
    for _ in 0..request.repeat {
        let started = Instant::now();
        let profile = workbook.set_input_profiled(sheet, request.cell, &request.text)?;
        edits.push(Edit {
            profile,
            total: started.elapsed(),
        });
    }
    Ok(Report {
        sheet_index: sheet.0,
        open,
        edits,
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = parse_request(&args)?;
    if !request.path.is_file() {
        bail!("not a file: {}", request.path.display());
    }
    // The engine's error texts can quote what the file holds, so only the kind is shown.
    let report = run_with_engine_stack(|| measure(&request)).map_err(|error| {
        anyhow!(
            "the measurement failed: {}",
            match error {
                EngineError::Rejected(_) => "rejected",
                EngineError::TooLarge(_) => "too large",
                _ => "could not open or edit the file",
            }
        )
    })?;
    println!(
        "sheet_index={} cell={} text_len={} repeat={}",
        report.sheet_index,
        request.cell,
        request.text.chars().count(),
        request.repeat
    );
    println!("open_ms={:.0}", ms(report.open));
    for (run, edit) in (1..).zip(&report.edits) {
        let mode = match edit.profile.mode {
            RecalcMode::Full => "full",
            RecalcMode::Incremental => "incremental",
        };
        println!(
            "run={run} edit_ms={:.1} recalc_mode={mode} recalc_ms={:.1} warm_used_areas_ms={:.1} total_ms={:.1}",
            ms(edit.profile.edit),
            ms(edit.profile.recalculation),
            ms(edit.profile.warm_used_areas),
            ms(edit.total)
        );
    }
    Ok(())
}
