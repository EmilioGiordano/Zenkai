// Test helpers outside #[test] functions; clippy only exempts the functions themselves.
#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::time::{Duration, Instant};

use zenkai_engine::{XlsxReader, open_xlsx, open_xlsx_with};

fn workbook_with_sheet(sheet_xml: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/styles.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border/></borders><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs></styleSheet>"#.to_string(),
        ),
        ("xl/worksheets/sheet1.xml", sheet_xml.to_string()),
    ];
    for (name, body) in parts {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn sheet(cell: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1">{cell}</row></sheetData></worksheet>"#
    )
}

fn open_bytes(bytes: Vec<u8>) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.xlsx");
    std::fs::write(&path, bytes).unwrap();
    open_xlsx(&path).map(|_| ()).map_err(|e| e.to_string())
}

fn open_fast(bytes: Vec<u8>) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.xlsx");
    std::fs::write(&path, bytes).unwrap();
    open_xlsx_with(&path, XlsxReader::Fast)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn workbook_with_strings(sheet_xml: &str, strings_xml: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/styles.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border/></borders><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs></styleSheet>"#.to_string(),
        ),
        ("xl/sharedStrings.xml", strings_xml.to_string()),
        ("xl/worksheets/sheet1.xml", sheet_xml.to_string()),
    ];
    for (name, body) in parts {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn shared_strings(count: usize, filler_len: usize) -> String {
    let filler = "A".repeat(filler_len);
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{count}" uniqueCount="{count}">"#
    );
    for _ in 0..count {
        xml.push_str(&format!("<si><t>{filler}</t></si>"));
    }
    xml.push_str("</sst>");
    xml
}

// Rewrites the declared uncompressed size of one entry, in its local and central
// headers, leaving the compressed data untouched.
fn patch_declared_size(bytes: &mut Vec<u8>, name: &str, size: u32) {
    let needle = name.as_bytes();
    let (mut local, mut central) = (None, None);
    let mut from = 0;
    while from + needle.len() <= bytes.len() {
        let Some(offset) = bytes[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|at| from + at)
        else {
            break;
        };
        if offset >= 30 && &bytes[offset - 30..offset - 26] == b"PK\x03\x04" {
            assert_eq!(
                &bytes[offset - 4..offset - 2],
                &(needle.len() as u16).to_le_bytes()
            );
            assert_eq!(&bytes[offset - 2..offset], &[0, 0]);
            local = Some(offset - 30);
        }
        if offset >= 46 && &bytes[offset - 46..offset - 42] == b"PK\x01\x02" {
            central = Some(offset - 46);
        }
        from = offset + 1;
    }
    let (local, central) = (local.unwrap(), central.unwrap());
    bytes[local + 22..local + 26].copy_from_slice(&size.to_le_bytes());
    bytes[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(archive.by_name(name).unwrap().size(), u64::from(size));
}

#[test]
fn deeply_nested_formula_is_rejected_not_a_stack_overflow() {
    let cell = format!(
        r#"<c r="A1"><f>{}1{}</f></c>"#,
        "(".repeat(100_000),
        ")".repeat(100_000)
    );
    let error = open_bytes(workbook_with_sheet(&sheet(&cell))).unwrap_err();
    assert!(error.contains("a formula"), "{error}");
}

#[test]
fn long_operator_chain_is_rejected_not_a_stack_overflow() {
    let cell = format!(r#"<c r="A1"><f>{}1</f></c>"#, "1+".repeat(20_000));
    let error = open_bytes(workbook_with_sheet(&sheet(&cell))).unwrap_err();
    assert!(error.contains("characters"), "{error}");
}

#[test]
fn single_quoted_array_ref_on_new_line_is_rejected() {
    let cell = "<c r=\"A1\"><f
 t='array' ref='A1:XFD1048576'>1</f></c>";
    let error = open_bytes(workbook_with_sheet(&sheet(cell))).unwrap_err();
    assert!(error.contains("cells"), "{error}");
}

#[test]
fn whole_sheet_array_formula_is_rejected_quickly() {
    let cell = r#"<c r="A1"><f t="array" ref="A1:XFD1048576">1</f></c>"#;
    let started = Instant::now();
    let error = open_bytes(workbook_with_sheet(&sheet(cell))).unwrap_err();
    assert!(error.contains("cells"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

// IronCalc 0.8.3 panics on this hand-built workbook (no theme or docProps parts);
// the panic must surface as an error instead of closing the app.
#[test]
fn engine_panic_on_incomplete_workbook_becomes_an_error() {
    let cell = r#"<c r="A1"><v>7</v></c><c r="B1"><f>A1*6</f></c>"#;
    let error = open_bytes(workbook_with_sheet(&sheet(cell))).unwrap_err();
    assert!(error.contains("damaged"), "{error}");
}

#[test]
fn worksheet_outside_the_usual_folder_is_still_checked() {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    let body = sheet(&format!(r#"<c r="A1"><f>{}1</f></c>"#, "1+".repeat(20_000)));
    zip.start_file("data/worksheets/s1.xml", options).unwrap();
    zip.write_all(body.as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let error = open_bytes(bytes).unwrap_err();
    assert!(error.contains("characters"), "{error}");
}

#[test]
fn odd_empty_rows_in_a_file_never_reach_the_saved_file() {
    // A real workbook whose sheet XML gets extra cell-less rows a hostile file may hold.
    let mut source = rust_xlsxwriter::Workbook::new();
    source.add_worksheet().write_number(0, 0, 1.0).unwrap();
    let original = source.save_to_buffer().unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(original)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut body = String::new();
        std::io::Read::read_to_string(&mut entry, &mut body).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            body = body.replace(
                "</sheetData>",
                r#"<row r="0" ht="30" customHeight="1"/><row r="7" ht="30" customHeight="1"/><row r="7" ht="40" customHeight="1"/><row r="9" ht="5000" customHeight="1"/><row r="2000000000" ht="30" customHeight="1"/></sheetData>"#,
            );
        }
        out.start_file(name, options).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    let hostile = out.finish().unwrap().into_inner();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rows.xlsx");
    std::fs::write(&path, hostile).unwrap();
    let opened = zenkai_engine::open_xlsx(&path)
        .map_err(|e| e.to_string())
        .unwrap();
    let saved = zenkai_engine::Engine::to_xlsx(&opened.workbook).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(saved)).unwrap();
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    assert!(!sheet.contains(r#"<row r="0""#), "{sheet}");
    assert!(!sheet.contains(r#"<row r="2000000000""#), "{sheet}");
    assert!(!sheet.contains(r#"ht="5000""#), "{sheet}");
    assert_eq!(sheet.matches(r#"<row r="7""#).count(), 1, "{sheet}");
    assert!(!sheet.contains("2000000000"), "{sheet}");
}

#[test]
fn extreme_compression_ratio_is_refused() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let strings = shared_strings(7_000, 10_000);
    assert!(strings.len() > 64 * 1024 * 1024, "bomb too small");
    let started = Instant::now();
    let error = open_fast(workbook_with_strings(&sheet(cell), &strings)).unwrap_err();
    assert!(error.contains("150"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn tiny_zip_with_millions_of_cells_is_refused() {
    let mut rows = String::new();
    for n in 1..=1_000_000 {
        rows.push_str(&format!(
            r#"<row r="{n}"><c r="A{n}" t="s"><v>0</v></c><c r="B{n}" t="s"><v>0</v></c></row>"#
        ));
    }
    let sheet_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
    );
    let bytes = workbook_with_strings(&sheet_xml, &shared_strings(7_000, 10_000));
    assert!(
        bytes.len() < 8 * 1024 * 1024,
        "zip not tiny: {}",
        bytes.len()
    );
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("harm"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(120));
}

#[test]
fn inflated_declared_size_still_opens() {
    let mut source = rust_xlsxwriter::Workbook::new();
    source.add_worksheet().write_number(0, 0, 7.0).unwrap();
    let mut bytes = source.save_to_buffer().unwrap();
    patch_declared_size(&mut bytes, "xl/worksheets/sheet1.xml", 600_000_000);
    open_fast(bytes).expect("a declared size is a hint, not a verdict");
}

#[test]
fn deflated_declared_size_still_refuses() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let mut bytes = workbook_with_strings(&sheet(cell), &shared_strings(7_000, 10_000));
    patch_declared_size(&mut bytes, "xl/sharedStrings.xml", 1_000);
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("150"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn entry_flood_is_refused() {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for n in 0..20_001 {
        zip.start_file(format!("junk{n:05}.dat"), options).unwrap();
        zip.write_all(b"junk").unwrap();
    }
    let bytes = zip.finish().unwrap().into_inner();
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("entries"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn default_open_path_refuses_the_ratio_bomb() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let error = open_bytes(workbook_with_strings(
        &sheet(cell),
        &shared_strings(7_000, 10_000),
    ))
    .unwrap_err();
    assert!(error.contains("150"), "{error}");
}
