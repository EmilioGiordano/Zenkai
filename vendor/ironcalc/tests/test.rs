#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use ironcalc::export::save_to_xlsx;
use ironcalc::import::{load_from_xlsx, load_from_xlsx_bytes};
use ironcalc_base::types::{Color, HorizontalAlignment, VerticalAlignment};
use ironcalc_base::{Model, UserModel, ROW_HEIGHT_FACTOR};
use std::fs;
use std::io::Read;

// This is a functional test.
// We check that the output of example.xlsx is what we expect.
#[test]
fn test_example() {
    let model = load_from_xlsx("tests/example.xlsx", "en", "UTC", "en").unwrap();
    // We should use the API once it is in place
    let workbook = model.workbook;
    let ws = &workbook.worksheets;
    let expected_names = vec![
        "Sheet1".to_string(),
        "Second".to_string(),
        "Sheet4".to_string(),
        "shared".to_string(),
        "Table".to_string(),
        "Sheet2".to_string(),
        "Created fourth".to_string(),
        "Frozen".to_string(),
        "Split".to_string(),
        "Hidden".to_string(),
    ];
    let names: Vec<String> = ws.iter().map(|s| s.name.clone()).collect();

    // One is not not imported and one is hidden
    assert_eq!(expected_names, names);

    assert_eq!(workbook.views[&0].sheet, 7);

    // Test selection:
    // First sheet (Sheet1)
    // E13 and E13:N20
    assert_eq!(ws[0].frozen_rows, 0);
    assert_eq!(ws[0].frozen_columns, 0);
    assert_eq!(ws[0].views[&0].row, 13);
    assert_eq!(ws[0].views[&0].column, 5);
    assert_eq!(ws[0].views[&0].range, [13, 5, 20, 14]);
}

#[test]
fn test_load_from_xlsx_bytes() {
    let file_path = std::path::Path::new("tests/example.xlsx");
    let mut file = fs::File::open(file_path).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    let workbook = load_from_xlsx_bytes(&bytes, "home", "en", "UTC").unwrap();
    assert_eq!(workbook.views[&0].sheet, 7);
}

#[test]
fn no_grid() {
    let model = load_from_xlsx("tests/NoGrid.xlsx", "en", "UTC", "en").unwrap();
    {
        let workbook = &model.workbook;
        let ws = &workbook.worksheets;

        // NoGrid does not show grid lines
        let no_grid_sheet = &ws[0];
        assert_eq!(no_grid_sheet.name, "NoGrid".to_string());
        assert!(!no_grid_sheet.show_grid_lines);

        let sheet2 = &ws[1];
        assert_eq!(no_grid_sheet.name, "NoGrid".to_string());
        assert!(sheet2.show_grid_lines);

        let no_grid_no_headers_sheet = &ws[2];
        assert_eq!(no_grid_sheet.name, "NoGrid".to_string());
        // There is also no headers
        assert!(!no_grid_no_headers_sheet.show_grid_lines);
    }
    {
        // save it and check again
        let temp_file_name = "temp_file_no_grid.xlsx";
        save_to_xlsx(&model, temp_file_name).unwrap();
        let model = load_from_xlsx(temp_file_name, "en", "UTC", "en").unwrap();
        let workbook = &model.workbook;
        let ws = &workbook.worksheets;

        // NoGrid does not show grid lines
        let no_grid_sheet = &ws[0];
        assert_eq!(no_grid_sheet.name, "NoGrid".to_string());
        assert!(!no_grid_sheet.show_grid_lines);

        let sheet2 = &ws[1];
        assert_eq!(no_grid_sheet.name, "NoGrid".to_string());
        assert!(sheet2.show_grid_lines);

        let no_grid_no_headers_sheet = &ws[2];
        assert_eq!(no_grid_sheet.name, "NoGrid".to_string());
        // There is also no headers
        assert!(!no_grid_no_headers_sheet.show_grid_lines);
        fs::remove_file(temp_file_name).unwrap();
    }
}

#[test]
fn test_save_to_xlsx() {
    let mut model = load_from_xlsx("tests/example.xlsx", "en", "UTC", "en").unwrap();
    model.evaluate();
    let temp_file_name = "temp_file_example.xlsx";
    // test can safe
    save_to_xlsx(&model, temp_file_name).unwrap();
    // test can open
    let model = load_from_xlsx(temp_file_name, "en", "UTC", "en").unwrap();
    let metadata = &model.workbook.metadata;
    assert_eq!(metadata.application, "IronCalc Sheets");
    // FIXME: This will need to be updated once we fix versioning
    assert_eq!(metadata.app_version, "10.0000");

    let workbook = model.workbook;
    let ws = &workbook.worksheets;

    assert_eq!(workbook.views[&0].sheet, 7);

    // Test selection:
    // First sheet (Sheet1)
    // E13 and E13:N20
    assert_eq!(ws[0].frozen_rows, 0);
    assert_eq!(ws[0].frozen_columns, 0);
    assert_eq!(ws[0].views[&0].row, 13);
    assert_eq!(ws[0].views[&0].column, 5);
    assert_eq!(ws[0].views[&0].range, [13, 5, 20, 14]);
    // TODO: can we show it is the 'same' model?
    fs::remove_file(temp_file_name).unwrap();
}

#[test]
fn test_freeze() {
    // freeze has 3 frozen columns and 2 frozen rows
    let model = load_from_xlsx("tests/freeze.xlsx", "en", "UTC", "en")
        .unwrap()
        .workbook;
    assert_eq!(model.worksheets[0].frozen_rows, 2);
    assert_eq!(model.worksheets[0].frozen_columns, 3);
}

#[test]
fn test_split() {
    // We test that a workbook with split panes do not produce frozen rows and columns
    let model = load_from_xlsx("tests/split.xlsx", "en", "UTC", "en")
        .unwrap()
        .workbook;
    assert_eq!(model.worksheets[0].frozen_rows, 0);
    assert_eq!(model.worksheets[0].frozen_columns, 0);
}

fn test_model_has_correct_styles(model: &Model) {
    // A1 is bold
    let style_a1 = model.get_style_for_cell(0, 1, 1).unwrap();
    assert!(style_a1.font.b);
    assert!(!style_a1.font.i);
    assert!(!style_a1.font.u);

    // B1 is Italics
    let style_b1 = model.get_style_for_cell(0, 1, 2).unwrap();
    assert!(style_b1.font.i);
    assert!(!style_b1.font.b);
    assert!(!style_b1.font.u);

    // C1 Underlined
    let style_c1 = model.get_style_for_cell(0, 1, 3).unwrap();
    assert!(style_c1.font.u);
    assert!(!style_c1.font.b);
    assert!(!style_c1.font.i);

    // D1 Bold and Italics
    let style_d1 = model.get_style_for_cell(0, 1, 4).unwrap();
    assert!(style_d1.font.b);
    assert!(style_d1.font.i);
    assert!(!style_d1.font.u);

    // E1 Bold, italics and underlined
    let style_e1 = model.get_style_for_cell(0, 1, 5).unwrap();
    assert!(style_e1.font.b);
    assert!(style_e1.font.i);
    assert!(style_e1.font.u);
    assert!(!style_e1.font.strike);

    // F1 strikethrough
    let style_f1 = model.get_style_for_cell(0, 1, 6).unwrap();
    assert!(style_f1.font.strike);

    // G1 Double underlined just get simple underlined
    let style_g1 = model.get_style_for_cell(0, 1, 7).unwrap();
    assert!(style_g1.font.u);

    // Taken from the xlsx
    let ht_row3 = 68.0;
    let height_row_3 = model.workbook.worksheet(0).unwrap().row_height(3).unwrap();
    assert_eq!(height_row_3, ht_row3 * ROW_HEIGHT_FACTOR);

    // Taken from the xlsx
    let ht_row_5 = 31.0;
    let height_row_5 = model.workbook.worksheet(0).unwrap().row_height(5).unwrap();
    assert_eq!(height_row_5, ht_row_5 * ROW_HEIGHT_FACTOR);

    // Second sheet has alignment
    // Horizontal
    let alignment = model.get_style_for_cell(1, 2, 1).unwrap().alignment;
    assert_eq!(alignment, None);

    let alignment = model
        .get_style_for_cell(1, 3, 1)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.horizontal, HorizontalAlignment::Left);

    let alignment = model
        .get_style_for_cell(1, 4, 1)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.horizontal, HorizontalAlignment::Distributed);

    let alignment = model
        .get_style_for_cell(1, 5, 1)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.horizontal, HorizontalAlignment::Right);

    let alignment = model
        .get_style_for_cell(1, 6, 1)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.horizontal, HorizontalAlignment::Center);

    let alignment = model
        .get_style_for_cell(1, 7, 1)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.horizontal, HorizontalAlignment::Fill);

    let alignment = model
        .get_style_for_cell(1, 8, 1)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.horizontal, HorizontalAlignment::Justify);

    // Vertical
    let alignment = model.get_style_for_cell(1, 2, 2).unwrap().alignment;
    assert_eq!(alignment, None);

    let alignment = model.get_style_for_cell(1, 3, 2).unwrap().alignment;
    assert_eq!(alignment, None);

    let alignment = model
        .get_style_for_cell(1, 4, 2)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.vertical, VerticalAlignment::Top);

    let alignment = model
        .get_style_for_cell(1, 5, 2)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.vertical, VerticalAlignment::Center);

    let alignment = model
        .get_style_for_cell(1, 6, 2)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.vertical, VerticalAlignment::Justify);

    let alignment = model
        .get_style_for_cell(1, 7, 2)
        .unwrap()
        .alignment
        .unwrap();
    assert_eq!(alignment.vertical, VerticalAlignment::Distributed);
}

#[test]
fn test_simple_text() {
    let model = load_from_xlsx("tests/basic_text.xlsx", "en", "UTC", "en").unwrap();

    test_model_has_correct_styles(&model);

    let temp_file_name = "temp_file_test_named_styles.xlsx";
    save_to_xlsx(&model, temp_file_name).unwrap();

    let model = load_from_xlsx(temp_file_name, "en", "UTC", "en").unwrap();
    fs::remove_file(temp_file_name).unwrap();
    test_model_has_correct_styles(&model);
}

#[test]
fn test_defined_names_casing() {
    let test_file_path = "tests/calc_tests/defined_names_for_unit_test.xlsx";
    let loaded_workbook = load_from_xlsx(test_file_path, "en", "UTC", "en")
        .unwrap()
        .workbook;
    let mut model = Model::from_bytes(&bitcode::encode(&loaded_workbook), "en").unwrap();

    let (row, column) = (2, 13); // B13
    let test_cases = [
        ("=named1", "11"),
        ("=NAMED1", "11"),
        ("=NaMeD1", "11"),
        ("=named2", "22"),
        ("=NAMED2", "22"),
        ("=NaMeD2", "22"),
        ("=named3", "33"),
        ("=NAMED3", "33"),
        ("=NaMeD3", "33"),
    ];
    for (formula, expected_value) in test_cases {
        model
            .set_user_input(0, row, column, formula.to_string())
            .unwrap();
        model.evaluate();
        assert_eq!(
            model.get_formatted_cell_value(0, row, column).unwrap(),
            expected_value
        );
    }
}

// This test verifies whether exporting the merged cells functionality is happening properly or not.
// It first loads the Excel having the merged cell and exports it to another xlsx and verifies whether merged
// cell node is same in both of the xlsx file or not.
#[test]
fn test_exporting_merged_cells() {
    let temp_file_name = "temp_file_test_export_merged_cells.xlsx";
    let expected_merge_cell_ref = {
        // loading the xlsx file containing merged cells
        let example_file_name = "tests/example.xlsx";
        let mut model = load_from_xlsx(example_file_name, "en", "UTC", "en").unwrap();
        let expected_merge_cell_ref = model
            .workbook
            .worksheets
            .first()
            .unwrap()
            .merge_cells
            .clone();
        // exporting and saving it in another xlsx
        model.evaluate();
        save_to_xlsx(&model, temp_file_name).unwrap();
        expected_merge_cell_ref
    };
    {
        let mut temp_model = load_from_xlsx(temp_file_name, "en", "UTC", "en").unwrap();
        {
            // loading the previous file back and verifying whether
            // merged cells got exported properly or not
            let got_merge_cell_ref = &temp_model
                .workbook
                .worksheets
                .first()
                .unwrap()
                .merge_cells
                .clone();
            assert_eq!(expected_merge_cell_ref, *got_merge_cell_ref);
            fs::remove_file(temp_file_name).unwrap();
        }
        {
            // this block is to verify that if there are no
            // merged cells, exported xml should not have the
            // <mergeCells/> xml node
            temp_model
                .workbook
                .worksheets
                .get_mut(0)
                .unwrap()
                .merge_cells
                .clear();

            save_to_xlsx(&temp_model, temp_file_name).unwrap();
            let temp_model2 = load_from_xlsx(temp_file_name, "en", "UTC", "en").unwrap();
            let got_merge_cell_ref_cnt = &temp_model2
                .workbook
                .worksheets
                .first()
                .unwrap()
                .merge_cells
                .len();
            assert!(*got_merge_cell_ref_cnt == 0);
        }
    }

    fs::remove_file(temp_file_name).unwrap();
}

#[test]
fn test_user_model() {
    let temp_file_name = "temp_file_test_user_model.xlsx";
    let mut model = UserModel::new_empty("my_model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "=1+1").unwrap();

    // test we can use `get_model` to save the model
    save_to_xlsx(model.get_model(), temp_file_name).unwrap();
    fs::remove_file(temp_file_name).unwrap();

    // we can still use the model afterwards
    model.set_rows_height(0, 1, 1, 100.0).unwrap();
}

// This is produced with:
// from openpyxl import Workbook

// # Create new workbook
// wb = Workbook()
// ws = wb.active

// # Write text and formula
// ws['A1'] = 'Hello, World!'
// ws['A2'] = '=1+1'

// ws['B1'] = '=CONCAT("It is", " what it is")'

// # Save
// wb.save('openpyxl_example.xlsx')
#[test]
fn test_pyopenxl_example() {
    let mut model = load_from_xlsx("tests/openpyxl_example.xlsx", "en", "UTC", "en").unwrap();
    model.evaluate();

    let a1 = model.get_formatted_cell_value(0, 1, 1).unwrap();
    assert_eq!(a1, "Hello, World!");

    let a2 = model.get_formatted_cell_value(0, 2, 1).unwrap();
    assert_eq!(a2, "2");

    let b1 = model.get_formatted_cell_value(0, 1, 2).unwrap();
    assert_eq!(b1, "It is what it is");
}

fn assert_eq_ignoring_metadata_and_name(
    workbook1: ironcalc_base::types::Workbook,
    workbook2: ironcalc_base::types::Workbook,
) {
    let mut w2 = workbook2.clone();
    w2.metadata = workbook1.metadata.clone();
    w2.name = workbook1.name.clone();
    assert_eq!(workbook1, w2);
}

#[test]
fn test_dynamic_arrays() {
    let model = load_from_xlsx("tests/dynamic_arrays.xlsx", "en", "UTC", "en").unwrap();
    let temp_file_name = "temp_file_test_dynamic_arrays.xlsx";
    save_to_xlsx(&model, temp_file_name).unwrap();
    let model2 = load_from_xlsx(temp_file_name, "en", "UTC", "en").unwrap();
    fs::remove_file(temp_file_name).unwrap();
    assert_eq_ignoring_metadata_and_name(model.workbook, model2.workbook);
}

#[test]
// This tests the `xl/worksheets/_rels/sheet*` are parsed correctly
// libreoffice sometimes exports .xlsx file with whitespace in the <Relationships> element
fn test_relationship_whitespace_example() {
    let mut model =
        load_from_xlsx("tests/libreoffice_888_example.xlsx", "en", "UTC", "en").unwrap();
    model.evaluate();
}

#[test]
fn test_missing_r_on_row() {
    let mut model = load_from_xlsx("tests/missing_r_on_row.xlsx", "en", "UTC", "en").unwrap();
    model.evaluate();
}

#[test]
// This tests theme color resolution against the workbook's `xl/theme/theme1.xml`
// rather than the hardcoded Office 2013 palette. custom_theme_colors.xlsx ships
// a custom theme where accent6 = #C9211E; B15 uses <fgColor theme="9"/>, which
// must resolve to that red.
fn test_workbook_theme_colors() {
    let model = load_from_xlsx("tests/custom_theme_colors.xlsx", "en", "UTC", "en").unwrap();
    let style_b15 = model.get_style_for_cell(0, 15, 2).unwrap();
    // Theme index 9 = accent6 in OOXML; the custom theme sets accent6 = #C9211E
    assert_eq!(style_b15.fill.color, Color::Theme(9, 0.0));
    assert_eq!(
        style_b15.fill.color.to_rgb(&model.workbook.theme),
        "#C9211E".to_string()
    );
}

#[test]
fn test_comment_and_whitespace_between_rows() {
    let bytes = fs::read("tests/example.xlsx").unwrap();
    let expected = load_from_xlsx_bytes(&bytes, "home", "en", "UTC").unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut body = Vec::new();
        entry.read_to_end(&mut body).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            let text = String::from_utf8(body).unwrap();
            assert!(text.contains("<sheetData>"));
            body = text
                .replacen("<sheetData>", "<sheetData>\n<!-- note -->\n", 1)
                .replace("</row>", "</row>\n<!-- row -->")
                .into_bytes();
        }
        writer
            .start_file(name, zip::write::FileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, &body).unwrap();
    }
    let edited = writer.finish().unwrap().into_inner();
    let model = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    assert_eq!(
        model.worksheets[0].sheet_data,
        expected.worksheets[0].sheet_data
    );
}

#[test]
fn test_number_cell_without_value_is_empty() {
    let bytes = fs::read("tests/example.xlsx").unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut last_row = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut body = Vec::new();
        entry.read_to_end(&mut body).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            let text = String::from_utf8(body).unwrap();
            let row_start = text.rfind("<row r=\"").unwrap() + "<row r=\"".len();
            let row_end = row_start + text[row_start..].find('"').unwrap();
            last_row = text[row_start..row_end].parse::<i32>().unwrap();
            let end = text.rfind("</row>").unwrap();
            body = format!(
                "{}<c r=\"ZY{last_row}\" t=\"n\"/><c r=\"ZZ{last_row}\" t=\"n\"><v></v></c>{}",
                &text[..end],
                &text[end..]
            )
            .into_bytes();
        }
        writer
            .start_file(name, zip::write::FileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, &body).unwrap();
    }
    let edited = writer.finish().unwrap().into_inner();
    let model = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    for column in [701, 702] {
        assert_eq!(
            model.worksheets[0].cell(last_row, column),
            Some(&ironcalc_base::types::Cell::EmptyCell { s: 0 })
        );
    }
}

#[test]
fn test_formula_without_cached_value_is_unevaluated() {
    let last_row = std::cell::Cell::new(0);
    let edited = example_with_part_edited("xl/worksheets/sheet1.xml", |text| {
        let row_start = text.rfind("<row r=\"").unwrap() + "<row r=\"".len();
        let row_end = row_start + text[row_start..].find('"').unwrap();
        let row = text[row_start..row_end].parse::<i32>().unwrap();
        last_row.set(row);
        let end = text.rfind("</row>").unwrap();
        let cells = [
            format!("<c r=\"ZW{row}\"><v>2</v></c>"),
            format!("<c r=\"ZX{row}\"><f>ZW{row}*3</f></c>"),
            format!("<c r=\"ZY{row}\"><f>ZX{row}+1</f><v/></c>"),
            format!("<c r=\"ZZ{row}\" t=\"x\"><f>1+1</f></c>"),
        ];
        format!("{}{}{}", &text[..end], cells.concat(), &text[end..])
    });
    let last_row = last_row.get();
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    let cell = |column| workbook.worksheets[0].cell(last_row, column).cloned();
    for column in [700, 701] {
        assert!(matches!(
            cell(column),
            Some(ironcalc_base::types::Cell::CellFormula {
                v: ironcalc_base::types::FormulaValue::Unevaluated,
                ..
            })
        ));
    }
    assert!(matches!(
        cell(702),
        Some(ironcalc_base::types::Cell::CellFormula {
            v: ironcalc_base::types::FormulaValue::Error(_),
            ..
        })
    ));

    let mut model = Model::from_workbook(workbook, "en").unwrap();
    model.evaluate();
    let value = |column| model.get_formatted_cell_value(0, last_row, column).unwrap();
    assert_eq!(value(700), "6");
    assert_eq!(value(701), "7");
    assert_eq!(value(702), "2");
}

fn example_with_part_edited(part: &str, edit: impl Fn(String) -> String) -> Vec<u8> {
    let bytes = fs::read("tests/example.xlsx").unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut body = Vec::new();
        entry.read_to_end(&mut body).unwrap();
        if name == part {
            body = edit(String::from_utf8(body).unwrap()).into_bytes();
        }
        writer
            .start_file(name, zip::write::FileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, &body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn test_column_ranges_outside_the_sheet() {
    let edited = example_with_part_edited("xl/worksheets/sheet1.xml", |text| {
        let start = text.find("<cols>").unwrap();
        let end = text.find("</cols>").unwrap();
        format!(
            "{}<cols>{}</cols>{}",
            &text[..start],
            concat!(
                r#"<col min="5" max="3" width="9"/>"#,
                r#"<col min="16385" max="16390" width="9"/>"#,
                r#"<col min="-7" max="0" width="9"/>"#,
                r#"<col min="0" max="2" width="11"/>"#,
                r#"<col min="5" max="5" width="38"/>"#,
                r#"<col min="10" max="20000" width="12"/>"#,
            ),
            &text[end + "</cols>".len()..]
        )
    });
    let model = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    let columns: Vec<(i32, i32, f64)> = model.worksheets[0]
        .cols
        .iter()
        .map(|col| (col.min, col.max, col.width))
        .collect();
    assert_eq!(columns, [(1, 2, 11.0), (5, 5, 38.0), (10, 16_384, 12.0)]);
}

fn example_with_columns(count: usize) -> Vec<u8> {
    example_with_part_edited("xl/worksheets/sheet1.xml", |text| {
        let start = text.find("<cols>").unwrap();
        let end = text.find("</cols>").unwrap();
        format!(
            "{}<cols>{}</cols>{}",
            &text[..start],
            r#"<col min="1" max="16384" width="9"/>"#.repeat(count),
            &text[end + "</cols>".len()..]
        )
    })
}

#[test]
fn test_column_elements_are_bounded_by_the_columns_of_a_sheet() {
    let model = load_from_xlsx_bytes(&example_with_columns(16_384), "home", "en", "UTC").unwrap();
    assert_eq!(model.worksheets[0].cols.len(), 16_384);
    let refused = load_from_xlsx_bytes(&example_with_columns(16_385), "home", "en", "UTC");
    assert!(refused.is_err());
}

#[test]
fn test_whitespace_between_column_elements_does_not_count_against_the_bound() {
    let edited = example_with_part_edited("xl/worksheets/sheet1.xml", |text| {
        let start = text.find("<cols>").unwrap();
        let end = text.find("</cols>").unwrap();
        format!(
            "{}<cols>{}</cols>{}",
            &text[..start],
            "
  <col min=\"1\" max=\"16384\" width=\"9\"/>"
                .repeat(16_384),
            &text[end + "</cols>".len()..]
        )
    });
    let model = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    assert_eq!(model.worksheets[0].cols.len(), 16_384);
}

#[test]
fn test_styles_without_cell_style_formats() {
    let edited = example_with_part_edited("xl/styles.xml", |text| {
        let start = text.find("<cellStyleXfs").unwrap();
        let end = text.find("</cellStyleXfs>").unwrap() + "</cellStyleXfs>".len();
        format!("{}{}", &text[..start], &text[end..])
    });
    let model = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    let default_xf = &ironcalc_base::types::Styles::default().cell_style_xfs[0];
    assert_eq!(model.styles.cell_style_xfs.len(), 4);
    assert!(model
        .styles
        .cell_style_xfs
        .iter()
        .all(|xf| xf == default_xf));
}

#[test]
fn test_saved_style_references_stay_inside_cell_style_formats() {
    let edited = example_with_part_edited("xl/styles.xml", |text| {
        let start = text.find("<cellStyleXfs").unwrap();
        let end = text.find("</cellStyleXfs>").unwrap() + "</cellStyleXfs>".len();
        format!("{}{}", &text[..start], &text[end..])
    });
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    let mut model = Model::from_workbook(workbook, "en").unwrap();
    model.evaluate();
    let saved = ironcalc::export::save_xlsx_to_writer(&model, std::io::Cursor::new(Vec::new()))
        .unwrap()
        .into_inner();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(saved)).unwrap();
    let mut styles = String::new();
    archive
        .by_name("xl/styles.xml")
        .unwrap()
        .read_to_string(&mut styles)
        .unwrap();

    let count_start =
        styles.find("<cellStyleXfs count=\"").unwrap() + "<cellStyleXfs count=\"".len();
    let count: i32 = styles[count_start..]
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut referenced = 0;
    let mut rest = styles.as_str();
    while let Some(at) = rest.find("xfId=\"") {
        rest = &rest[at + "xfId=\"".len()..];
        let id: i32 = rest.split('"').next().unwrap().parse().unwrap();
        assert!(id < count, "xfId {id} is past cellStyleXfs count {count}");
        referenced += 1;
    }
    assert!(referenced > 0);
}

fn example_with_style_list_replaced(list: &str, replacement: &str) -> Vec<u8> {
    example_with_part_edited("xl/styles.xml", |text| {
        let open = format!("<{list}");
        let close = format!("</{list}>");
        let start = text.find(&open).unwrap();
        let end = text.find(&close).unwrap() + close.len();
        format!("{}{}{}", &text[..start], replacement, &text[end..])
    })
}

#[test]
fn test_styles_with_empty_or_missing_lists_use_default_records() {
    for list in ["fonts", "fills", "borders", "cellXfs", "cellStyles"] {
        for replacement in [String::new(), format!("<{list} count=\"0\"/>")] {
            let edited = example_with_style_list_replaced(list, &replacement);
            let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
            let defaults = ironcalc_base::types::Styles::default();
            let styles = &workbook.styles;
            match list {
                "fonts" => assert_eq!(styles.fonts.len(), 1),
                "fills" => assert_eq!(styles.fills, defaults.fills),
                "borders" => assert_eq!(styles.borders, defaults.borders),
                "cellXfs" => assert_eq!(styles.cell_xfs, defaults.cell_xfs),
                _ => assert_eq!(styles.cell_styles.len(), 1),
            }
            let mut model = Model::from_workbook(workbook, "en").unwrap();
            model.evaluate();
            let saved =
                ironcalc::export::save_xlsx_to_writer(&model, std::io::Cursor::new(Vec::new()))
                    .unwrap()
                    .into_inner();
            load_from_xlsx_bytes(&saved, "home", "en", "UTC").unwrap();
        }
    }
}

fn touch_every_style(model: &mut Model) {
    for sheet in 0..model.workbook.worksheets.len() as u32 {
        for row in 1..=40 {
            for column in 1..=20 {
                model.get_style_for_cell(sheet, row, column).unwrap();
            }
        }
        model.set_user_input(sheet, 1, 1, "x".to_string()).unwrap();
    }
}

#[test]
fn test_style_references_past_the_lists_fall_back_to_the_first_record() {
    for list in ["fonts", "fills", "borders"] {
        let edited = example_with_style_list_replaced(list, "");
        let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
        let mut model = Model::from_workbook(workbook, "en").unwrap();
        touch_every_style(&mut model);
    }
    let edited = example_with_part_edited("xl/styles.xml", |text| {
        let start = text.find("<fonts").unwrap();
        let end = text.find("</fonts>").unwrap() + "</fonts>".len();
        let one_font =
            r#"<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>"#;
        let text = format!("{}{}{}", &text[..start], one_font, &text[end..]);
        let at = text.find("<cellXfs").unwrap();
        let (head, tail) = text.split_at(at);
        format!(
            "{head}{}",
            tail.replacen("fontId=\"0\"", "fontId=\"50\"", 1)
        )
    });
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    let mut model = Model::from_workbook(workbook, "en").unwrap();
    touch_every_style(&mut model);
}

#[test]
fn test_cell_row_and_column_styles_past_cell_xfs_are_read_as_default() {
    let edited = example_with_part_edited("xl/worksheets/sheet1.xml", |text| {
        text.replacen(
            "</sheetData>",
            r#"<row r="500" s="77" customFormat="1"><c r="A500" s="99"><v>1</v></c></row></sheetData>"#,
            1,
        )
        .replacen("<col ", "<col style=\"88\" ", 1)
    });
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    let mut model = Model::from_workbook(workbook, "en").unwrap();
    assert_eq!(model.get_cell_style_index(0, 500, 1).unwrap(), 0);
    touch_every_style(&mut model);
    model.set_user_input(0, 500, 1, "y".to_string()).unwrap();
    let saved = ironcalc::export::save_xlsx_to_writer(&model, std::io::Cursor::new(Vec::new()))
        .unwrap()
        .into_inner();
    load_from_xlsx_bytes(&saved, "home", "en", "UTC").unwrap();
}

#[test]
fn test_indented_styles_part_keeps_record_indices() {
    let edited = example_with_part_edited("xl/styles.xml", |_| {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="0">
  </fonts>
  <fills count="1">
    <fill>
      <patternFill patternType="none"/>
    </fill>
  </fills>
  <borders count="1">
    <border/>
  </borders>
  <cellXfs count="2">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0"/>
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0"/>
  </cellXfs>
</styleSheet>"#
            .to_string()
    });
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    assert_eq!(workbook.styles.fonts.len(), 1);
    assert_eq!(workbook.styles.fills.len(), 1);
    assert_eq!(workbook.styles.borders.len(), 1);
    assert_eq!(workbook.styles.cell_xfs.len(), 2);
    assert_eq!(workbook.styles.cell_styles.len(), 1);
}

#[test]
fn test_default_records_for_missing_lists_use_excels_names() {
    let edited = example_with_style_list_replaced("cellStyles", "");
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    assert_eq!(workbook.styles.cell_styles[0].name, "Normal");
    let edited = example_with_style_list_replaced("fonts", "");
    let workbook = load_from_xlsx_bytes(&edited, "home", "en", "UTC").unwrap();
    assert_eq!(workbook.styles.fonts[0].name, "Calibri");
    assert_eq!(workbook.styles.fonts[0].sz, 11);
}
