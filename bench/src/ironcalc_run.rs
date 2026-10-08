use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use anyhow::{Result, anyhow};
use ironcalc::base::Model;
use ironcalc::base::cell::CellValue;
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::{load_from_xlsx, load_from_xlsx_bytes};

use crate::PEAK;
use crate::fixtures::{self, Fixture};
use crate::measure::{self, CellSnapshot, Metrics, Seen, elapsed_ms, normalize_formula};

const LANG: &str = "en";

fn seen(model: &Model, sheet: u32, row: u32, col: u16) -> Seen {
    let row = row as i32 + 1;
    let col = i32::from(col) + 1;
    match model.get_cell_value_by_index(sheet, row, col) {
        Ok(CellValue::None) => Seen::Empty,
        Ok(CellValue::Number(n)) => Seen::Number(n),
        Ok(CellValue::Boolean(b)) => Seen::Bool(b),
        Ok(CellValue::String(s)) if s.starts_with('#') => Seen::Error(s),
        Ok(CellValue::String(s)) => Seen::Text(s),
        Err(e) => Seen::Error(e),
    }
}

fn snapshot(model: &Model, sheet: u32, row: u32, col: u16) -> CellSnapshot {
    let formula = model
        .get_cell_formula(sheet, row as i32 + 1, i32::from(col) + 1)
        .map(|f| f.map(|f| normalize_formula(&f)));
    CellSnapshot {
        value: seen(model, sheet, row, col),
        formula,
    }
}

pub fn run(fixture: Fixture, path: &Path) -> Result<Metrics> {
    let path_str = path.to_str().ok_or_else(|| anyhow!("non UTF-8 path"))?;
    let mut metrics = Metrics::default();

    PEAK.reset_peak_usage();
    let start = Instant::now();
    let mut model = load_from_xlsx(path_str, "en", "UTC", LANG).map_err(|e| anyhow!("{e:?}"))?;
    metrics.open_ms = elapsed_ms(start);

    let start = Instant::now();
    model.evaluate();
    metrics.recalc_ms = Some(elapsed_ms(start));
    metrics.peak_mb = f64::from(PEAK.peak_usage_as_mb());
    metrics.idle_mb = f64::from(PEAK.current_usage_as_mb());

    let expected = fixtures::expected(fixture);
    if !expected.is_empty() {
        metrics.correct = Some(measure::count_correct(&expected, |c| {
            seen(&model, c.sheet, c.row, c.col)
        }));
    }

    let start = Instant::now();
    let bytes = save_xlsx_to_writer(&model, Cursor::new(Vec::new()))
        .map_err(|e| anyhow!("{e:?}"))?
        .into_inner();
    metrics.save_ms = elapsed_ms(start);

    let reopened_book =
        load_from_xlsx_bytes(&bytes, "roundtrip", "en", "UTC").map_err(|e| anyhow!("{e:?}"))?;
    let mut reopened = Model::from_workbook(reopened_book, LANG).map_err(|e| anyhow!(e))?;
    reopened.evaluate();
    metrics.roundtrip = measure::count_same(
        fixture,
        |s, r, c| snapshot(&model, s, r, c),
        |s, r, c| snapshot(&reopened, s, r, c),
    );

    if fixture == Fixture::Formatting {
        metrics.styles = Some(bold_matches(&reopened));
        let merges = reopened
            .workbook
            .worksheets
            .first()
            .map_or(0, |ws| ws.merge_cells.len());
        metrics.merges = Some((
            merges,
            (fixtures::FORMAT_ROWS / fixtures::MERGE_EVERY) as usize,
        ));
    }

    if let Some(probe) = fixtures::edit_probe(fixture) {
        let start = Instant::now();
        model
            .set_user_input(
                probe.sheet,
                probe.row as i32 + 1,
                i32::from(probe.col) + 1,
                probe.input.to_string(),
            )
            .map_err(|e| anyhow!(e))?;
        model.evaluate();
        metrics.edit_ms = Some(elapsed_ms(start));
        if !probe.probe_expected.is_nan() {
            let value = seen(&model, probe.sheet, probe.probe_row, probe.probe_col);
            metrics.edit_ok = Some(value == Seen::Number(probe.probe_expected));
        }
    }
    Ok(metrics)
}

fn bold_matches(model: &Model) -> (usize, usize) {
    let mut ok = 0;
    let mut total = 0;
    for row in 0..fixtures::FORMAT_ROWS {
        for col in 0..fixtures::FORMAT_COLS {
            total += 1;
            let bold = model
                .get_style_for_cell(0, row as i32 + 1, i32::from(col) + 1)
                .is_ok_and(|s| s.font.b);
            if bold == fixtures::bold_expected(row, col) {
                ok += 1;
            }
        }
    }
    (ok, total)
}

pub fn evaluate_formulas(setup: &[(&str, &str)], formulas: &[&str]) -> Result<Vec<Seen>> {
    let mut model = Model::new_empty("coverage", "en", "UTC", LANG).map_err(|e| anyhow!(e))?;
    for (cell, value) in setup {
        let (row, col) = crate::coverage::a1(cell)?;
        model
            .set_user_input(0, row as i32 + 1, i32::from(col) + 1, (*value).to_string())
            .map_err(|e| anyhow!(e))?;
    }
    for (row, formula) in (0u32..).zip(formulas) {
        model
            .set_user_input(0, row as i32 + 1, 10, (*formula).to_string())
            .map_err(|e| anyhow!(e))?;
    }
    model.evaluate();
    Ok((0u32..)
        .zip(formulas)
        .map(|(row, _)| seen(&model, 0, row, 9))
        .collect())
}
