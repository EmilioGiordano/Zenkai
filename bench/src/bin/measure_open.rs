#![forbid(unsafe_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use ironcalc::base::expressions::types::Area;
use ironcalc::base::types::Cell;
use ironcalc::base::{Model, UserModel};
use peak_alloc::PeakAlloc;
use zenkai_engine::{Engine, Workbook, XlsxReader, open_xlsx_with, read_xlsx_bytes};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

const LOCALE: &str = "en";
const TIMEZONE: &str = "UTC";

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn powershell_number(script: &str) -> Result<u64> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("running powershell")?;
    if !output.status.success() {
        bail!("powershell failed");
    }
    String::from_utf8(output.stdout)
        .context("non UTF-8 powershell output")?
        .trim()
        .parse()
        .map_err(|e| anyhow!("unparsable powershell output: {e}"))
}

fn total_ram_bytes() -> Result<u64> {
    powershell_number("(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory")
}

// PeakPagedMemorySize64 is the Win32 PeakPagefileUsage: peak committed private bytes.
// Working set is not used because Windows pages a process out under pressure.
fn peak_committed_bytes() -> Result<u64> {
    powershell_number(&format!(
        "(Get-Process -Id {}).PeakPagedMemorySize64",
        std::process::id()
    ))
}

fn peak_commit_mb() -> f64 {
    peak_committed_bytes().map_or(-1.0, |b| b as f64 / 1024.0 / 1024.0)
}

// PrivateMemorySize64 is the committed private bytes right now, after the importer freed its DOM.
fn current_commit_mb() -> f64 {
    powershell_number(&format!(
        "(Get-Process -Id {}).PrivateMemorySize64",
        std::process::id()
    ))
    .map_or(-1.0, |b| b as f64 / 1024.0 / 1024.0)
}

fn begin(stage: &str) {
    println!("stage={stage} begin");
}

fn done(stage: &str, elapsed_ms: f64, extra: &str) {
    println!(
        "stage={stage} ms={elapsed_ms:.0} peak_commit_mb={:.0} commit_mb={:.0} {extra}",
        peak_commit_mb(),
        current_commit_mb()
    );
}

fn col_letters(mut index: i32) -> String {
    let mut letters = Vec::new();
    while index > 0 {
        let rest = (index - 1) % 26;
        letters.push((b'A' + rest as u8) as char);
        index = (index - 1) / 26;
    }
    letters.into_iter().rev().collect()
}

fn count_cells(model: &Model) -> (u64, u64) {
    let mut stored = 0u64;
    let mut formulas = 0u64;
    for sheet in &model.workbook.worksheets {
        for (_, _, cell) in sheet.sheet_data.iter() {
            stored += 1;
            if matches!(cell, Cell::CellFormula { .. } | Cell::ArrayFormula(_)) {
                formulas += 1;
            }
        }
    }
    (stored, formulas)
}

fn print_sizes() {
    println!(
        "sizeof cell={} area={} model={} usermodel={}",
        std::mem::size_of::<Cell>(),
        std::mem::size_of::<Area>(),
        std::mem::size_of::<Model<'static>>(),
        std::mem::size_of::<UserModel<'static>>(),
    );
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, edits) = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [file] => (PathBuf::from(file), false),
        [file, "--edits"] => (PathBuf::from(file), true),
        _ => bail!("usage: measure_open <file.xlsx> [--edits]"),
    };
    if !path.is_file() {
        bail!("not a file: {}", path.display());
    }
    let ram = total_ram_bytes().context("reading total RAM")?;
    println!(
        "ram_bytes={ram} ram_gb={:.1}",
        ram as f64 / 1024.0 / 1024.0 / 1024.0
    );
    println!(
        "watchdog_limit_bytes={} watchdog_limit_mb={:.0}",
        ram * 6 / 10,
        ram as f64 * 0.6 / 1024.0 / 1024.0
    );
    print_sizes();
    PEAK.reset_peak_usage();

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Book".to_string());

    begin("a");
    let start = Instant::now();
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let file_mb = bytes.len() as f64 / 1024.0 / 1024.0;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).map_err(|e| anyhow!("{e}"))?;
    let mut parts: Vec<(String, u64)> = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| anyhow!("{e}"))?;
        let entry_name = entry.name().to_string();
        if entry_name.starts_with("xl/worksheets/") && entry_name.ends_with(".xml") {
            let mut part = Vec::new();
            entry.read_to_end(&mut part).map_err(|e| anyhow!("{e}"))?;
            parts.push((entry_name, part.len() as u64));
        }
    }
    parts.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    let parts_mb: f64 = parts.iter().map(|(_, size)| *size as f64).sum::<f64>() / 1024.0 / 1024.0;
    let elapsed = ms(start);
    let summary: Vec<String> = parts
        .iter()
        .map(|(part, size)| format!("{part}={:.0}MB", *size as f64 / 1024.0 / 1024.0))
        .collect();
    done(
        "a",
        elapsed,
        &format!(
            "file_mb={file_mb:.1} sheet_parts_mb={parts_mb:.1} {}",
            summary.join(" ")
        ),
    );
    drop(parts);

    begin("b");
    let start = Instant::now();
    let parsed = read_xlsx_bytes(&bytes, &name, XlsxReader::Fast);
    match &parsed {
        Ok(read) => done(
            "b",
            ms(start),
            &format!(
                "reader=fast ok fallback={}",
                read.fallback().unwrap_or("none")
            ),
        ),
        Err(error) => done("b", ms(start), &format!("reader=fast refused: {error}")),
    }
    drop(parsed);

    begin("c");
    let alloc_before = PEAK.current_usage();
    let start = Instant::now();
    let book = ironcalc::import::load_from_xlsx_bytes(&bytes, &name, LOCALE, TIMEZONE)
        .map_err(|e| anyhow!("{e:?}"))?;
    let model =
        Model::from_workbook(book, LOCALE).map_err(|e| anyhow!("import workbook: {e:?}"))?;
    let mut raw = UserModel::from_model(model);
    let build_ms = ms(start);
    let alloc_after = PEAK.current_usage();
    let (stored, formulas) = count_cells(raw.get_model());
    let delta = alloc_after.saturating_sub(alloc_before) as f64;
    let bytes_per_cell = if stored == 0 {
        -1.0
    } else {
        delta / stored as f64
    };
    done(
        "c",
        build_ms,
        &format!(
            "stored={stored} formulas={formulas} alloc_delta_mb={:.0} bytes_per_cell={bytes_per_cell:.0}",
            delta / 1024.0 / 1024.0,
        ),
    );

    begin("d");
    let start = Instant::now();
    raw.evaluate();
    let recalc_ms = ms(start);
    let mode = raw.get_model().last_recalculation();
    done("d", recalc_ms, &format!("mode={mode:?}"));

    if !edits {
        println!("edits=skipped (pass --edits for the MID file)");
        return Ok(());
    }
    let found = run_raw_probes(&mut raw)?;
    drop(raw);
    drop(bytes);
    run_engine_edits(&path, &found)?;
    Ok(())
}

fn cold_pos() -> Result<CellPos> {
    CellPos::parse_a1("XFD1048576").ok_or_else(|| anyhow!("XFD1048576 does not parse"))
}

struct Found {
    formula_col: i32,
    fill_top: i32,
    bottom: i32,
    formula_text: String,
    rows: u64,
    hot_col: i32,
    hot_row: i32,
    hot_kind: &'static str,
}

fn letters_to_index(letters: &str) -> i32 {
    letters
        .bytes()
        .fold(0i32, |acc, b| acc * 26 + (b - b'A') as i32 + 1)
}

fn absolute_ref(formula: &str) -> Option<(i32, i32)> {
    let bytes = formula.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            let col_start = j;
            while j < bytes.len() && bytes[j].is_ascii_uppercase() {
                j += 1;
            }
            if j > col_start && j < bytes.len() && bytes[j] == b'$' {
                let mut k = j + 1;
                let row_start = k;
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k > row_start {
                    let col = letters_to_index(&formula[col_start..j]);
                    if let Ok(row) = formula[row_start..k].parse::<i32>()
                        && (1..=16384).contains(&col)
                        && (1..=1048576).contains(&row)
                    {
                        return Some((col, row));
                    }
                }
                i = k;
                continue;
            }
        }
        i += 1;
    }
    None
}

fn first_cell_token(formula: &str) -> Option<(i32, i32)> {
    let bytes = formula.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_uppercase() {
                j += 1;
            }
            let mut k = j;
            while k < bytes.len() && bytes[k].is_ascii_digit() {
                k += 1;
            }
            if k > j && (k >= bytes.len() || bytes[k] != b'$') {
                let col = letters_to_index(&formula[i..j]);
                if let Ok(row) = formula[j..k].parse::<i32>()
                    && (1..=16384).contains(&col)
                    && (1..=1048576).contains(&row)
                {
                    return Some((col, row));
                }
            }
            i = k.max(j);
            continue;
        }
        i += 1;
    }
    None
}

fn flip_number(current: &str) -> String {
    let trimmed = current.trim();
    if trimmed.is_empty() {
        return "1".to_string();
    }
    trimmed
        .parse::<f64>()
        .map_or_else(|_| "1".to_string(), |value| (value + 1.0).to_string())
}

fn discover(raw: &UserModel) -> Result<Found> {
    let sheet = raw
        .get_model()
        .workbook
        .worksheet(0)
        .map_err(|e| anyhow!("sheet 0: {e}"))?;
    let mut per_col: std::collections::HashMap<i32, (i32, i32, u64)> =
        std::collections::HashMap::new();
    for (row, col, cell) in sheet.sheet_data.iter() {
        if matches!(cell, Cell::CellFormula { .. } | Cell::ArrayFormula(_)) {
            per_col
                .entry(col)
                .and_modify(|slot| {
                    slot.0 = slot.0.min(row);
                    slot.1 = slot.1.max(row);
                    slot.2 += 1;
                })
                .or_insert((row, row, 1));
        }
    }
    let (formula_col, (mut top, bottom, count)) = per_col
        .into_iter()
        .max_by_key(|(_, (_, _, count))| *count)
        .ok_or_else(|| anyhow!("no formula column found"))?;
    let mut formula_text = String::new();
    for row in top..=bottom {
        let text = raw
            .get_cell_content(0, row, formula_col)
            .unwrap_or_default();
        if text.starts_with('=') {
            formula_text = text;
            top = row;
            break;
        }
    }
    if formula_text.is_empty() {
        bail!(
            "formula column {} has no readable formula",
            col_letters(formula_col)
        );
    }
    let rows = (bottom - top + 1).max(0) as u64;
    let (hot_col, hot_row, hot_kind) = match absolute_ref(&formula_text) {
        Some((col, row)) => (col, row, "absolute ref shared by the column"),
        None => match first_cell_token(&formula_text) {
            Some((col, _)) => (col, top, "same-row input (one dependent row)"),
            None => bail!("cannot parse any cell reference in {formula_text}"),
        },
    };
    println!(
        "discover formula_col={} rows={count} fill_rows={rows} range={}{}:{}{} formula={formula_text:?} hot={}{} ({hot_kind})",
        col_letters(formula_col),
        col_letters(formula_col),
        top,
        col_letters(formula_col),
        bottom,
        col_letters(hot_col),
        hot_row,
    );
    Ok(Found {
        formula_col,
        fill_top: top,
        bottom,
        formula_text,
        rows,
        hot_col,
        hot_row,
        hot_kind,
    })
}

fn run_raw_probes(raw: &mut UserModel) -> Result<Found> {
    let found = discover(raw)?;
    let _ = cold_pos()?;

    begin("f_raw_cold");
    let start = Instant::now();
    raw.set_user_input(0, 1048576, 16384, "1")
        .map_err(|e| anyhow!("cold edit: {e}"))?;
    let cold_edit_ms = ms(start);
    let start = Instant::now();
    raw.evaluate();
    let cold_recalc_ms = ms(start);
    let mode = raw.get_model().last_recalculation();
    done(
        "f_raw_cold",
        cold_edit_ms + cold_recalc_ms,
        &format!(
            "cell=XFD1048576 edit_ms={cold_edit_ms:.0} recalc_ms={cold_recalc_ms:.0} mode={mode:?}"
        ),
    );

    begin("f_raw_hot");
    let before = raw
        .get_cell_content(0, found.hot_row, found.hot_col)
        .unwrap_or_default();
    let start = Instant::now();
    raw.set_user_input(0, found.hot_row, found.hot_col, &flip_number(&before))
        .map_err(|e| anyhow!("hot edit: {e}"))?;
    let hot_edit_ms = ms(start);
    let start = Instant::now();
    raw.evaluate();
    let hot_recalc_ms = ms(start);
    let mode = raw.get_model().last_recalculation();
    done(
        "f_raw_hot",
        hot_edit_ms + hot_recalc_ms,
        &format!(
            "cell={}{} kind='{}' edit_ms={hot_edit_ms:.0} recalc_ms={hot_recalc_ms:.0} mode={mode:?}",
            col_letters(found.hot_col),
            found.hot_row,
            found.hot_kind,
        ),
    );

    begin("e_raw_parse");
    let start = Instant::now();
    let mut rows: Vec<String> = Vec::new();
    for row in found.fill_top..=found.bottom {
        rows.push(
            raw.get_model()
                .extend_to(0, found.fill_top, found.formula_col, row, found.formula_col)
                .map_err(|e| anyhow!("extend: {e}"))?,
        );
    }
    let parse_ms = ms(start);
    done(
        "e_raw_parse",
        parse_ms,
        &format!("shifted_rows={}", rows.len()),
    );
    let tsv = rows.join("\n");

    raw.pause_evaluation();
    raw.set_selected_sheet(0)
        .map_err(|e| anyhow!("select sheet: {e}"))?;
    raw.set_selected_cell(found.fill_top, found.formula_col)
        .map_err(|e| anyhow!("select cell: {e}"))?;
    raw.set_selected_range(
        found.fill_top,
        found.formula_col,
        found.bottom,
        found.formula_col,
    )
    .map_err(|e| anyhow!("select range: {e}"))?;
    let alloc_before = PEAK.current_usage();
    begin("e_raw_paste");
    let start = Instant::now();
    let area = Area {
        sheet: 0,
        row: found.fill_top,
        column: found.formula_col,
        width: 1,
        height: found.rows as i32,
    };
    raw.paste_csv_string(&area, &tsv)
        .map_err(|e| anyhow!("paste: {e}"))?;
    let paste_ms = ms(start);
    let undo_bytes = PEAK.current_usage().saturating_sub(alloc_before);
    done(
        "e_raw_paste",
        paste_ms,
        &format!("undo_growth_mb={:.1}", undo_bytes as f64 / 1024.0 / 1024.0),
    );
    raw.resume_evaluation();
    begin("e_raw_recalc");
    let start = Instant::now();
    raw.evaluate();
    let recalc_ms = ms(start);
    let mode = raw.get_model().last_recalculation();
    done(
        "e_raw_recalc",
        recalc_ms,
        &format!(
            "mode={mode:?} parse_ms={parse_ms:.0} paste_ms={paste_ms:.0} total_ms={:.0}",
            parse_ms + paste_ms + recalc_ms,
        ),
    );
    Ok(found)
}

fn run_engine_edits(path: &Path, found: &Found) -> Result<()> {
    begin("e0");
    let start = Instant::now();
    let opened = open_xlsx_with(path, XlsxReader::Fast)?;
    let open_ms = ms(start);
    let unsupported = opened.unsupported.len();
    let fallback = opened
        .fallback
        .clone()
        .unwrap_or_else(|| "none".to_string());
    done(
        "e0",
        open_ms,
        &format!("app_open unsupported={unsupported} fallback={fallback}"),
    );
    let mut workbook: Workbook = opened.workbook;
    let sheet = SheetId(0);
    let hot_pos = CellPos::new(
        RowIdx::clamped(i64::from(found.hot_row) - 1),
        ColIdx::clamped(i64::from(found.hot_col) - 1),
    );
    let cold_pos = cold_pos()?;

    begin("f_hot");
    let before = workbook.input(sheet, hot_pos);
    let start = Instant::now();
    workbook.set_input(sheet, hot_pos, &flip_number(&before))?;
    done(
        "f_hot",
        ms(start),
        &format!(
            "cell={}{} kind='{}' app_path=set_input",
            col_letters(found.hot_col),
            found.hot_row,
            found.hot_kind,
        ),
    );

    begin("f_cold");
    let start = Instant::now();
    workbook.set_input(sheet, cold_pos, "1")?;
    done("f_cold", ms(start), "cell=XFD1048576 app_path=set_input");

    begin("e");
    let from = CellPos::new(
        RowIdx::clamped(i64::from(found.fill_top) - 1),
        ColIdx::clamped(i64::from(found.formula_col) - 1),
    );
    let range = Range::new(
        from,
        CellPos::new(
            RowIdx::clamped(i64::from(found.bottom) - 1),
            ColIdx::clamped(i64::from(found.formula_col) - 1),
        ),
    );
    let alloc_before = PEAK.current_usage();
    let start = Instant::now();
    workbook.fill_with(sheet, from, &found.formula_text, range)?;
    let total_ms = ms(start);
    let undo_bytes = PEAK.current_usage().saturating_sub(alloc_before);
    let probe = workbook.cell(
        sheet,
        CellPos::new(
            RowIdx::clamped(i64::from(found.fill_top)),
            ColIdx::clamped(i64::from(found.formula_col) - 1),
        ),
    );
    done(
        "e",
        total_ms,
        &format!(
            "app_path=fill_with rows={} undo_growth_mb={:.1} probe={:?}",
            found.rows,
            undo_bytes as f64 / 1024.0 / 1024.0,
            probe.text,
        ),
    );
    Ok(())
}

#[cfg(test)]
mod bin_tests {
    #[test]
    fn absolute_ref_finds_a_shared_input() {
        assert_eq!(super::absolute_ref("=A2*$B$1"), Some((2, 1)));
        assert_eq!(super::absolute_ref("=A2*2"), None);
    }

    #[test]
    fn column_letters_round_trip() {
        assert_eq!(super::col_letters(1), "A");
        assert_eq!(super::col_letters(28), "AB");
    }
}
