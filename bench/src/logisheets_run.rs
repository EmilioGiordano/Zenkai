use std::path::Path;
use std::time::Instant;

use anyhow::{Result, anyhow};
use logisheets_rs::{CellInput, EditAction, PayloadsAction, StatusCode, Value, Workbook};

use crate::PEAK;
use crate::fixtures::{self, Fixture};
use crate::measure::{self, CellSnapshot, Metrics, Seen, elapsed_ms, normalize_formula};

fn seen(book: &Workbook, sheet: u32, row: u32, col: u16) -> Seen {
    let value = book
        .get_sheet_by_idx(sheet as usize)
        .and_then(|ws| ws.get_value(row as usize, usize::from(col)));
    match value {
        Ok(Value::Empty) => Seen::Empty,
        Ok(Value::Number(n)) => Seen::Number(n),
        Ok(Value::Bool(b)) => Seen::Bool(b),
        Ok(Value::Str(s)) => Seen::Text(s),
        Ok(Value::Error(e)) => Seen::Error(e),
        Err(e) => Seen::Error(format!("{e:?}")),
    }
}

fn snapshot(book: &Workbook, sheet: u32, row: u32, col: u16) -> CellSnapshot {
    let formula = book
        .get_sheet_by_idx(sheet as usize)
        .and_then(|ws| ws.get_formula(row as usize, usize::from(col)))
        .map(|f| (!f.is_empty()).then(|| normalize_formula(&f)))
        .map_err(|e| format!("{e:?}"));
    CellSnapshot {
        value: seen(book, sheet, row, col),
        formula,
    }
}

fn input(book: &mut Workbook, sheet: u32, row: u32, col: u16, content: &str) -> Result<()> {
    let action = PayloadsAction::new()
        .add_payload(CellInput {
            sheet_idx: sheet as usize,
            row: row as usize,
            col: usize::from(col),
            content: content.to_string(),
        })
        .set_undoable(true);
    let effect = book.handle_action(EditAction::Payloads(action));
    match effect.status {
        StatusCode::Err(_) => Err(anyhow!("rejected: {:?}", effect.error_message)),
        _ => Ok(()),
    }
}

pub fn run(fixture: Fixture, path: &Path) -> Result<Metrics> {
    let bytes = std::fs::read(path)?;
    let mut metrics = Metrics::default();

    PEAK.reset_peak_usage();
    let start = Instant::now();
    let mut book = Workbook::from_file(&bytes, fixture.file_name().to_string())
        .map_err(|e| anyhow!("{e:?}"))?;
    metrics.open_ms = elapsed_ms(start);
    metrics.peak_mb = f64::from(PEAK.peak_usage_as_mb());
    metrics.idle_mb = f64::from(PEAK.current_usage_as_mb());

    let expected = fixtures::expected(fixture);
    if !expected.is_empty() {
        metrics.correct = Some(measure::count_correct(&expected, |c| {
            seen(&book, c.sheet, c.row, c.col)
        }));
    }

    let start = Instant::now();
    let saved = book.save().map_err(|e| anyhow!("{e:?}"))?;
    metrics.save_ms = elapsed_ms(start);

    let reopened =
        Workbook::from_file(&saved, "roundtrip.xlsx".to_string()).map_err(|e| anyhow!("{e:?}"))?;
    metrics.roundtrip = measure::count_same(
        fixture,
        |s, r, c| snapshot(&book, s, r, c),
        |s, r, c| snapshot(&reopened, s, r, c),
    );

    if fixture == Fixture::Formatting {
        metrics.styles = Some(bold_matches(&reopened));
        let merges = reopened.get_sheet_by_idx(0).map_or(0, |ws| {
            ws.get_merged_cells(
                0,
                0,
                fixtures::FORMAT_ROWS as usize,
                usize::from(fixtures::FORMAT_COLS),
            )
            .len()
        });
        metrics.merges = Some((
            merges,
            (fixtures::FORMAT_ROWS / fixtures::MERGE_EVERY) as usize,
        ));
    }

    if let Some(probe) = fixtures::edit_probe(fixture) {
        let start = Instant::now();
        input(&mut book, probe.sheet, probe.row, probe.col, probe.input)?;
        metrics.edit_ms = Some(elapsed_ms(start));
        if !probe.probe_expected.is_nan() {
            let value = seen(&book, probe.sheet, probe.probe_row, probe.probe_col);
            metrics.edit_ok = Some(value == Seen::Number(probe.probe_expected));
        }
    }
    Ok(metrics)
}

fn bold_matches(book: &Workbook) -> (usize, usize) {
    let mut ok = 0;
    let mut total = 0;
    let Ok(ws) = book.get_sheet_by_idx(0) else {
        return (0, 0);
    };
    for row in 0..fixtures::FORMAT_ROWS {
        for col in 0..fixtures::FORMAT_COLS {
            total += 1;
            let bold = ws
                .get_style(row as usize, usize::from(col))
                .is_ok_and(|s| s.font.bold);
            if bold == fixtures::bold_expected(row, col) {
                ok += 1;
            }
        }
    }
    (ok, total)
}

pub fn evaluate_formulas(setup: &[(&str, &str)], formulas: &[&str]) -> Result<Vec<Seen>> {
    let mut book = Workbook::new();
    for (cell, value) in setup {
        let (row, col) = crate::coverage::a1(cell)?;
        input(&mut book, 0, row, col, value)?;
    }
    let mut out = Vec::new();
    for (row, formula) in (0u32..).zip(formulas) {
        let result = input(&mut book, 0, row, 9, formula);
        out.push(match result {
            Ok(()) => seen(&book, 0, row, 9),
            Err(e) => Seen::Error(e.to_string()),
        });
    }
    Ok(out)
}
