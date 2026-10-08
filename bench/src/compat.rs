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

fn parts(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    (0..archive.len())
        .map(|i| {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_ascii_lowercase();
            let mut text = String::new();
            if name.ends_with(".xml") {
                std::io::Read::read_to_string(&mut entry, &mut text).map_err(|e| e.to_string())?;
            }
            Ok((name, text))
        })
        .collect()
}

// What is present in the original and absent after Zenkai saves it, by part name or
// by element inside the workbook and worksheet parts.
fn dropped_parts(original: &[u8], saved: &[u8]) -> String {
    let by_name = [
        ("xl/charts/", "charts"),
        ("xl/drawings/", "drawings"),
        ("xl/media/", "images"),
        ("xl/comments", "comments"),
        ("xl/tables/", "tables"),
        ("xl/pivottables/", "pivot tables"),
        ("xl/vbaproject", "macros"),
        ("xl/externallinks/", "external links"),
    ];
    let by_element = [
        ("<conditionalFormatting", "conditional formatting"),
        ("<dataValidations", "data validation"),
        ("<hyperlinks", "hyperlinks"),
        ("<definedName ", "defined names"),
        ("<mergeCell ", "merged cells"),
        ("<pane ", "frozen panes"),
        ("<autoFilter", "autofilter"),
        ("<sheetProtection", "sheet protection"),
        ("<pageMargins", "page margins"),
        ("<pageSetup", "page setup"),
        ("<headerFooter", "header/footer"),
        ("<tabColor", "tab colour"),
        ("zoomScale=", "zoom"),
        ("showGridLines=\"0\"", "hidden gridlines"),
        ("customHeight=\"1\"", "row heights"),
        ("hidden=\"1\"", "hidden rows/columns"),
    ];
    let (before, after) = match (parts(original), parts(saved)) {
        (Ok(before), Ok(after)) => (before, after),
        (Err(e), _) | (_, Err(e)) => return format!("error: {e}"),
    };
    let has_name =
        |set: &[(String, String)], prefix: &str| set.iter().any(|(n, _)| n.starts_with(prefix));
    let has_element =
        |set: &[(String, String)], tag: &str| set.iter().any(|(_, t)| t.contains(tag));
    let mut lost: Vec<&str> = by_name
        .iter()
        .filter(|(prefix, _)| has_name(&before, prefix) && !has_name(&after, prefix))
        .map(|(_, label)| *label)
        .collect();
    lost.extend(
        by_element
            .iter()
            .filter(|(tag, _)| has_element(&before, tag) && !has_element(&after, tag))
            .map(|(_, label)| *label),
    );
    if lost.is_empty() {
        "-".to_string()
    } else {
        lost.join(", ")
    }
}

fn check(path: &Path) -> Row {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match open_xlsx(path) {
        Ok(opened) => {
            let (round_trip, saved) = compare_round_trip(&opened.workbook);
            let dropped = match std::fs::read(path) {
                Ok(original) if !saved.is_empty() => dropped_parts(&original, &saved),
                Ok(_) => "error: nothing was saved".to_string(),
                Err(e) => format!("error: {e}"),
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
        "(by part name or element).\n",
        "- **Cached = recalculated**: each value saved in the file (read with calamine) ",
        "against the value Zenkai shows after recalculating.\n",
        "- **Round trip**: save, reopen and compare every filled cell.\n\n",
        "| File | Opens | Warned on open | Dropped by save | Cached = recalculated | Round trip |\n",
        "| --- | --- | --- | --- | --- | --- |\n",
    ));
    for path in &files {
        let row = check(path);
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            row.file, row.opens, row.unsupported, row.dropped, row.cached, row.round_trip
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
            checked += 1;
        }
        assert!(checked >= 10);
    }
}
