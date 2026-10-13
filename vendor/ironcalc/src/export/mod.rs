#![allow(clippy::unwrap_used)]

mod _rels;
mod conditional_formatting;
mod doc_props;
mod dxfs_styles;
mod escape;
mod shared_strings;
mod styles;
mod styles_util;
mod theme;
mod workbook;
mod workbook_xml_rels;
mod worksheets;
mod xml_constants;

use std::io::BufWriter;
use std::{
    fs,
    io::{Seek, Write},
};

use ironcalc_base::expressions::utils::number_to_column;
use ironcalc_base::types::Workbook;
use ironcalc_base::{get_milliseconds_since_epoch, Model};

pub use self::worksheets::write_worksheet_xml;
use self::xml_constants::XML_DECLARATION;

use crate::error::XlsxError;

#[cfg(test)]
mod test;

fn get_content_types_xml(workbook: &Workbook) -> String {
    // A list of all files in the zip
    let mut content = vec![
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#.to_string(),
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#.to_string(),
        r#"<Default Extension="xml" ContentType="application/xml"/>"#.to_string(),
        r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#.to_string(),
    ];
    for worksheet in 0..workbook.worksheets.len() {
        let sheet = format!(
            r#"<Override PartName="/xl/worksheets/sheet{}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
            worksheet + 1
        );
        content.push(sheet);
    }
    content.push(
        r#"<Override PartName="/xl/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#.to_string(),
    );
    content.extend([
        r#"<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#.to_string(),
        r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#.to_string(),
        r#"<Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>"#.to_string(),
        r#"<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>"#.to_string(),
        r#"<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#.to_string(),
        r#"</Types>"#.to_string(),
    ]);
    format!("{XML_DECLARATION}\n{}", content.join(""))
}

/// Exports a model to an xlsx file
pub fn save_to_xlsx(model: &Model, file_name: &str) -> Result<(), XlsxError> {
    let file_path = std::path::Path::new(&file_name);
    if file_path.exists() {
        return Err(XlsxError::IO(format!("file {file_name} already exists")));
    }
    let file = fs::File::create(file_path).unwrap();
    let writer = BufWriter::new(file);
    save_xlsx_to_writer(model, writer)?;

    Ok(())
}

fn get_selected_sheet(workbook: &Workbook) -> u32 {
    match workbook.views.get(&0) {
        Some(view) => view.sheet,
        _ => 0,
    }
}

/// Every part of the xlsx package but the worksheets, as (path in the zip, content), in
/// the order `save_xlsx_to_writer` writes them. The worksheets come from
/// `write_worksheet_xml`.
pub fn get_package_parts(model: &Model) -> Result<Vec<(String, String)>, XlsxError> {
    let workbook = &model.workbook;
    let milliseconds = get_milliseconds_since_epoch();
    Ok(vec![
        (
            "[Content_Types].xml".to_string(),
            get_content_types_xml(workbook),
        ),
        (
            "docProps/app.xml".to_string(),
            doc_props::get_app_xml(workbook),
        ),
        (
            "docProps/core.xml".to_string(),
            doc_props::get_core_xml(workbook, milliseconds)?,
        ),
        ("_rels/.rels".to_string(), _rels::get_dot_rels(workbook)),
        (
            "xl/sharedStrings.xml".to_string(),
            shared_strings::get_shared_strings_xml(workbook),
        ),
        (
            "xl/styles.xml".to_string(),
            styles::get_styles_xml(workbook),
        ),
        (
            "xl/workbook.xml".to_string(),
            workbook::get_workbook_xml(workbook, get_selected_sheet(workbook)),
        ),
        (
            "xl/metadata.xml".to_string(),
            include_str!("metadata.xml").to_string(),
        ),
        (
            "xl/theme/theme1.xml".to_string(),
            theme::get_theme_xml(&workbook.theme),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_xml_rels::get_workbook_xml_rels(workbook),
        ),
    ])
}

pub fn save_xlsx_to_writer<W: Write + Seek>(model: &Model, writer: W) -> Result<W, XlsxError> {
    let workbook = &model.workbook;
    let selected_sheet = get_selected_sheet(workbook);
    let mut zip = zip::ZipWriter::new(writer);

    let options = zip::write::FileOptions::default();

    let mut directories: Vec<String> = vec![];
    for (name, content) in get_package_parts(model)? {
        if let Some((directory, _)) = name.rsplit_once('/') {
            if !directories.iter().any(|d| d == directory) {
                zip.add_directory(directory, options)?;
                directories.push(directory.to_string());
            }
        }
        zip.start_file(name, options)?;
        zip.write_all(content.as_bytes())?;
    }

    zip.add_directory("xl/worksheets", options)?;
    for (sheet_index, worksheet) in workbook.worksheets.iter().enumerate() {
        let id = sheet_index + 1;
        zip.start_file(format!("xl/worksheets/sheet{id}.xml"), options)?;
        let dimension = model
            .workbook
            .worksheet(sheet_index as u32)
            .unwrap()
            .dimension();
        let column_min_str = number_to_column(dimension.min_column).unwrap();
        let column_max_str = number_to_column(dimension.max_column).unwrap();
        let min_row = dimension.min_row;
        let max_row = dimension.max_row;
        let sheet_dimension_str = &format!("{column_min_str}{min_row}:{column_max_str}{max_row}");
        let is_sheet_selected = selected_sheet as usize == sheet_index;
        // The rows are written in many small pieces; the deflate stream takes them in
        // larger chunks.
        let mut buffered = BufWriter::new(&mut zip);
        worksheets::write_worksheet_xml(
            &mut buffered,
            worksheet,
            &model.parsed_formulas[sheet_index],
            sheet_dimension_str,
            is_sheet_selected,
        )?;
        buffered.flush()?;
    }

    let writer = zip.finish()?;
    Ok(writer)
}

/// Exports a model to an icalc file
pub fn save_to_icalc(model: &Model, file_name: &str) -> Result<(), XlsxError> {
    let file_path = std::path::Path::new(&file_name);
    if file_path.exists() {
        return Err(XlsxError::IO(format!("file {file_name} already exists")));
    }
    let s = bitcode::encode(&model.workbook);
    let mut file = fs::File::create(file_path)?;
    file.write_all(&s)?;

    Ok(())
}
