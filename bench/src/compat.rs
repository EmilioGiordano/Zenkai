use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use calamine::{Data, Reader, Xlsx};
use rust_xlsxwriter::{
    Chart, ChartType, ConditionalFormatCell, ConditionalFormatCellRule, DataValidation,
    DataValidationRule, Format, FormatAlign, Formula, Image, Note, Table, Url, Workbook,
};
use zenkai_engine::{Engine, open_xlsx};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

use crate::coverage::{CASES, SETUP, Want};
use crate::save_losses;

// A 1x1 transparent PNG, so the image fixture needs no file on disk.
const PIXEL_PNG: [u8; 67] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn save(book: &mut Workbook, dir: &Path, name: &str) -> Result<()> {
    book.save(dir.join(name))
        .with_context(|| format!("writing {name}"))
}

fn cached(want: &Want) -> Option<String> {
    match want {
        Want::Number(n) => Some(n.to_string()),
        Want::Text(t) => Some((*t).to_string()),
        Want::Bool(b) => Some(if *b { "TRUE" } else { "FALSE" }.to_string()),
        Want::AnyNumber => None,
    }
}

pub fn generate(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("Functions")?;
    for (cell, value) in SETUP {
        let pos = CellPos::parse_a1(cell).context("setup cell")?;
        let (row, col) = (pos.row.get(), pos.col.get());
        match value.parse::<f64>() {
            Ok(n) => sheet.write_number(row, col, n)?,
            Err(_) => sheet.write_string(row, col, value)?,
        };
    }
    for (row, case) in (0u32..).zip(CASES.iter()) {
        if let Some(result) = cached(&case.want) {
            sheet.write_formula(row, 9, Formula::new(case.formula).set_result(result))?;
        }
    }
    save(&mut book, dir, "functions-with-cached-values.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("Ñandú 東京 Ελληνικά")?;
    for (row, text) in (0u32..).zip([
        "café",
        "naïve",
        "東京",
        "Привет",
        "مرحبا",
        "😀 emoji",
        "a\nb",
    ]) {
        sheet.write_string(row, 0, text)?;
    }
    save(&mut book, dir, "unicode.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_string(0, 16_383, "last column")?;
    sheet.write_string(1_048_575, 0, "last row")?;
    sheet.write_number(1_048_575, 16_383, 42.0)?;
    save(&mut book, dir, "sheet-edges.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let bold = Format::new().set_bold().set_align(FormatAlign::Center);
    sheet.merge_range(0, 0, 0, 3, "Merged title", &bold)?;
    sheet.set_freeze_panes(1, 1)?;
    sheet.set_column_width(1, 30.0)?;
    sheet.set_row_height(2, 40.0)?;
    for row in 1..20u32 {
        sheet.write_number(row, 0, f64::from(row))?;
        sheet.write_formula(
            row,
            1,
            Formula::new(format!("=A{}*2", row + 1)).set_result((row * 2).to_string()),
        )?;
    }
    save(&mut book, dir, "merges-freeze-sizes.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    for row in 0..5u32 {
        sheet.write_number(row, 0, f64::from(row * 3))?;
    }
    let mut chart = Chart::new(ChartType::Column);
    chart.add_series().set_values("Sheet1!$A$1:$A$5");
    sheet.insert_chart(1, 3, &chart)?;
    save(&mut book, dir, "chart.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_string(0, 0, "image below")?;
    sheet.insert_image(1, 0, &Image::new_from_buffer(&PIXEL_PNG)?)?;
    save(&mut book, dir, "image.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 10.0)?;
    sheet.insert_note(0, 0, &Note::new("A reviewer note"))?;
    save(&mut book, dir, "notes.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    for row in 0..10u32 {
        sheet.write_number(row, 0, f64::from(row))?;
    }
    let rule = ConditionalFormatCell::new()
        .set_rule(ConditionalFormatCellRule::GreaterThan(5))
        .set_format(Format::new().set_bold());
    sheet.add_conditional_format(0, 0, 9, 0, &rule)?;
    save(&mut book, dir, "conditional-format.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let validation = DataValidation::new().allow_whole_number(DataValidationRule::Between(1, 10));
    sheet.add_data_validation(0, 0, 9, 0, &validation)?;
    sheet.write_number(0, 0, 5.0)?;
    save(&mut book, dir, "data-validation.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    for row in 1..6u32 {
        sheet.write_number(row, 0, f64::from(row))?;
        sheet.write_number(row, 1, f64::from(row * 10))?;
    }
    sheet.add_table(0, 0, 5, 1, &Table::new())?;
    save(&mut book, dir, "table.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_url(0, 0, Url::new("https://example.com"))?;
    sheet.write_number(1, 0, 7.0)?;
    book.define_name("Seven", "=Sheet1!$A$2")?;
    book.worksheet_from_index(0)?
        .write_formula(2, 0, Formula::new("=Seven*2").set_result("14"))?;
    save(&mut book, dir, "hyperlink-and-defined-name.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("Layout")?;
    sheet.write_string(0, 0, "Region")?;
    sheet.write_string(0, 1, "Total")?;
    sheet.write_string(1, 0, "North")?;
    sheet.write_number(1, 1, 10.0)?;
    sheet.write_string(4, 0, "after blank rows")?;
    sheet.set_row_height(2, 40)?;
    sheet.set_row_hidden(3)?;
    sheet.autofilter(0, 0, 1, 1)?;
    sheet.set_tab_color("#FF0000");
    sheet.set_landscape();
    sheet.set_header("&CQuarterly report");
    sheet.set_zoom(150);
    sheet.set_screen_gridlines(false);
    sheet.protect();
    save(&mut book, dir, "layout-print-protection.xlsx")?;

    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name("Report")?;
    for row in 0..12u32 {
        sheet.write_number(row, 0, f64::from(row))?;
        sheet.write_number(row, 3, f64::from(row * 2))?;
    }
    sheet.group_rows(1, 4)?;
    sheet.group_rows_collapsed(6, 9)?;
    sheet.group_columns(1, 2)?;
    sheet.group_symbols_above(true);
    sheet.set_tab_color("#00B050");
    sheet.set_zoom(80);
    sheet.set_view_page_layout();
    sheet.set_default_row_height(18);
    sheet.set_paper_size(9);
    sheet.set_portrait();
    sheet.set_margins(0.5, 0.5, 0.8, 0.8, 0.3, 0.3);
    sheet.set_print_fit_to_pages(1, 0);
    sheet.set_print_center_horizontally(true);
    sheet.set_print_gridlines(true);
    sheet.set_header("&L&\"Arial,Bold\"Q3 && Q4 <draft>&RPage &P of &N");
    sheet.set_footer("&C&F");
    sheet.set_page_breaks(&[6])?;
    sheet.set_vertical_page_breaks(&[3])?;
    save(&mut book, dir, "page-layout-outline.xlsx")?;

    Ok(())
}

#[derive(Default)]
struct Row {
    file: String,
    opens: String,
    unsupported: String,
    dropped: String,
    cached: String,
    round_trip: String,
    silently_dropped: Vec<String>,
}

fn same_value(cached: &Data, text: &str, number: Option<f64>) -> bool {
    match cached {
        Data::Empty => text.is_empty(),
        Data::Float(f) => number.is_some_and(|n| (n - f).abs() <= 1e-9 * f.abs().max(1.0)),
        Data::Int(i) => number.is_some_and(|n| (n - *i as f64).abs() < 1e-9),
        Data::Bool(b) => text.eq_ignore_ascii_case(if *b { "TRUE" } else { "FALSE" }),
        Data::String(s) => s == text,
        Data::Error(e) => e.to_string() == text,
        Data::DateTime(dt) => number.is_some_and(|n| (n - dt.as_f64()).abs() < 1e-9),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s == text,
    }
}

// Streams the values saved in the file (calamine's cell reader never builds a dense
// range, so sparse sheets cannot blow up) and compares them with Zenkai's.
fn compare_cached(path: &Path, book: &zenkai_engine::Workbook) -> String {
    let mut cached = match calamine::open_workbook::<Xlsx<_>, _>(path) {
        Ok(cached) => cached,
        Err(e) => return format!("error: {e}"),
    };
    let engine_sheets: Vec<String> = book.sheets().into_iter().map(|s| s.name).collect();
    let mut total = 0usize;
    let mut same = 0usize;
    for name in cached.sheet_names() {
        let Some(index) = engine_sheets.iter().position(|n| *n == name) else {
            return format!("error: sheet {name} missing in Zenkai");
        };
        let sheet = SheetId(u32::try_from(index).unwrap_or(u32::MAX));
        let mut reader = match cached.worksheet_cells_reader(&name) {
            Ok(reader) => reader,
            Err(e) => return format!("error: {e}"),
        };
        loop {
            let cell = match reader.next_cell() {
                Ok(Some(cell)) => cell,
                Ok(None) => break,
                Err(e) => return format!("error: {e}"),
            };
            let value = Data::from(cell.get_value().clone());
            if matches!(value, Data::Empty) {
                continue;
            }
            let (row, col) = cell.get_position();
            let pos = CellPos::new(
                RowIdx::clamped(i64::from(row)),
                ColIdx::clamped(i64::from(col)),
            );
            let view = book.cell(sheet, pos);
            total += 1;
            if same_value(&value, &view.text, view.number) {
                same += 1;
            }
        }
    }
    format!("{same}/{total}")
}

fn compare_round_trip(book: &zenkai_engine::Workbook) -> (String, Vec<u8>) {
    let Ok(bytes) = book.to_xlsx() else {
        return ("save failed".to_string(), Vec::new());
    };
    let Ok(reopened) = zenkai_engine::Workbook::from_xlsx_bytes(&bytes, "round-trip") else {
        return ("reopen failed".to_string(), bytes);
    };
    let mut total = 0usize;
    let mut same = 0usize;
    for sheet in book.sheets() {
        for pos in book.filled_cells(sheet.id) {
            total += 1;
            if reopened.input(sheet.id, pos) == book.input(sheet.id, pos)
                && reopened.cell(sheet.id, pos).text == book.cell(sheet.id, pos).text
            {
                same += 1;
            }
        }
    }
    (format!("{same}/{total}"), bytes)
}

fn check(path: &Path) -> Row {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match open_xlsx(path) {
        Ok(opened) => {
            let (round_trip, saved) = compare_round_trip(&opened.workbook);
            let losses = match std::fs::read(path) {
                Ok(original) if !saved.is_empty() => {
                    save_losses::dropped(&original, &saved).map_err(|e| format!("error: {e}"))
                }
                Ok(_) => Err("error: nothing was saved".to_string()),
                Err(e) => Err(format!("error: {e}")),
            };
            let (dropped, silently_dropped) = match losses {
                Ok(losses) if losses.is_empty() => ("-".to_string(), Vec::new()),
                Ok(losses) => (
                    losses
                        .iter()
                        .map(|loss| loss.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    losses
                        .iter()
                        .filter(|loss| !loss.is_warned(&opened.unsupported))
                        .map(|loss| loss.label.clone())
                        .collect(),
                ),
                Err(error) => (error, Vec::new()),
            };
            Row {
                file,
                opens: "yes".to_string(),
                unsupported: if opened.unsupported.is_empty() {
                    "-".to_string()
                } else {
                    opened
                        .unsupported
                        .iter()
                        .map(|u| u.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                dropped,
                cached: compare_cached(path, &opened.workbook),
                round_trip,
                silently_dropped,
            }
        }
        Err(error) => {
            let fallback = zenkai_formats::read_values(path).map_or_else(
                |e| format!("no ({error}; values: {e})"),
                |sheets| format!("read-only values ({} sheets)", sheets.len()),
            );
            Row {
                file,
                opens: fallback,
                unsupported: "-".to_string(),
                dropped: "-".to_string(),
                cached: "-".to_string(),
                round_trip: "-".to_string(),
                silently_dropped: Vec::new(),
            }
        }
    }
}

pub fn report(dir: &Path, out: &Path) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["xlsx", "xlsm", "xlsb", "xls", "ods"].contains(&e.to_ascii_lowercase().as_str())
            })
        })
        .collect();
    files.sort();
    let mut text = String::from(concat!(
        "# Compatibility corpus\n\n",
        "Generated by `zenkai-bench compat fixtures/compat docs/COMPATIBILITY.md`. ",
        "Every file in `fixtures/compat/` is opened with the app's own open path.\n\n",
        "- **Warned on open**: what Zenkai detects and warns about before saving.\n",
        "- **Dropped by save**: what actually disappears from the file Zenkai writes ",
        "(by part name, top-level element or key attribute).\n",
        "- **Dropped without warning**: what of that has no warning; the corpus test fails ",
        "unless it is empty.\n",
        "- **Cached = recalculated**: each value saved in the file (read with calamine) ",
        "against the value Zenkai shows after recalculating.\n",
        "- **Round trip**: save, reopen and compare every filled cell.\n\n",
        "| File | Opens | Warned on open | Dropped by save | Dropped without warning | Cached = recalculated | Round trip |\n",
        "| --- | --- | --- | --- | --- | --- | --- |\n",
    ));
    for path in &files {
        let row = check(path);
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            row.file,
            row.opens,
            row.unsupported,
            row.dropped,
            if row.silently_dropped.is_empty() {
                "-".to_string()
            } else {
                row.silently_dropped.join(", ")
            },
            row.cached,
            row.round_trip
        ));
    }
    std::fs::write(out, text).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check, generate};

    // The corpus must keep opening, matching the cached values and surviving a save.
    #[test]
    fn generated_corpus_opens_matches_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        generate(dir.path()).unwrap();
        let mut checked = 0;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            let row = check(&path);
            assert_eq!(row.opens, "yes", "{}", row.file);
            for counts in [&row.cached, &row.round_trip] {
                let (same, total) = counts
                    .split_once('/')
                    .unwrap_or_else(|| panic!("{}: {counts}", row.file));
                assert_eq!(same, total, "{}: {counts}", row.file);
            }
            assert!(
                !row.dropped.starts_with("error"),
                "{}: {}",
                row.file,
                row.dropped
            );
            assert!(
                row.silently_dropped.is_empty(),
                "{} loses without a warning: {:?}",
                row.file,
                row.silently_dropped
            );
            checked += 1;
        }
        assert!(checked >= 10);
    }
}
