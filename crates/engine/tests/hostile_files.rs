// Test helpers outside #[test] functions; clippy only exempts the functions themselves.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use zenkai_engine::{
    Engine, SaveTarget, Unsupported, XlsxReader, open_xlsx, open_xlsx_with, save_xlsx,
};
use zenkai_types::{CellPos, SheetId};

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

// Copies a workbook and replaces one entry's body, leaving the rest untouched.
fn with_replaced_part(bytes: Vec<u8>, name: &str, body: &[u8]) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let entry_name = entry.name().to_string();
        out.start_file(&entry_name, options).unwrap();
        if entry_name == name {
            out.write_all(body).unwrap();
        } else {
            std::io::copy(&mut entry, &mut out).unwrap();
        }
    }
    out.finish().unwrap().into_inner()
}

// Copies a workbook and appends one more entry, leaving the rest untouched.
fn with_extra_part(bytes: Vec<u8>, name: &str, body: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let entry_name = entry.name().to_string();
        let mut kept = Vec::new();
        entry.read_to_end(&mut kept).unwrap();
        out.start_file(entry_name, options).unwrap();
        out.write_all(&kept).unwrap();
    }
    out.start_file(name, options).unwrap();
    out.write_all(body.as_bytes()).unwrap();
    out.finish().unwrap().into_inner()
}

// The local and central header starts of one entry, found by its name.
fn header_offsets(bytes: &[u8], name: &str) -> (usize, usize) {
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
    (local.unwrap(), central.unwrap())
}

// Rewrites the declared uncompressed size of one entry, in its local and central
// headers, leaving the compressed data untouched.
fn patch_declared_size(bytes: &mut Vec<u8>, name: &str, size: u32) {
    let (local, central) = header_offsets(bytes, name);
    bytes[local + 22..local + 26].copy_from_slice(&size.to_le_bytes());
    bytes[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(archive.by_name(name).unwrap().size(), u64::from(size));
}

// Rewrites the declared COMPRESSED size of one entry, in its local and central
// headers, leaving the compressed data untouched.
fn patch_compressed_size(bytes: &mut Vec<u8>, name: &str, size: u32) {
    let (local, central) = header_offsets(bytes, name);
    bytes[local + 18..local + 22].copy_from_slice(&size.to_le_bytes());
    bytes[central + 20..central + 24].copy_from_slice(&size.to_le_bytes());
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(
        archive.by_name(name).unwrap().compressed_size(),
        u64::from(size)
    );
}

// Rewrites the compression method of one entry, in its local and central
// headers, leaving the data untouched; the entry can no longer be read.
fn patch_compression_method(bytes: &mut [u8], name: &str, method: u16) {
    let (local, central) = header_offsets(bytes, name);
    bytes[local + 8..local + 10].copy_from_slice(&method.to_le_bytes());
    bytes[central + 10..central + 12].copy_from_slice(&method.to_le_bytes());
}

#[test]
fn forged_large_compressed_size_still_refuses_the_ratio_bomb() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let mut bytes = workbook_with_strings(&sheet(cell), &shared_strings(7_000, 10_000));
    patch_compressed_size(&mut bytes, "xl/sharedStrings.xml", 600_000_000);
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("150"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn forged_small_compressed_size_is_an_invalid_file() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let mut bytes = workbook_with_strings(&sheet(cell), &shared_strings(1_000, 1_000));
    patch_compressed_size(&mut bytes, "xl/sharedStrings.xml", 1_000);
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("not a valid xlsx workbook"), "{error}");
    assert!(!error.contains("harm"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn unreadable_first_entry_still_refuses_the_bomb_behind_it() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let mut bytes = workbook_with_strings(&sheet(cell), &shared_strings(7_000, 10_000));
    patch_compression_method(&mut bytes, "[Content_Types].xml", 12);
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("fall back"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

// Deterministic bytes with no XML control characters and no runs for the
// compressor: the ratio gate passes, so only the whole-part cap can fire.
fn incompressible(len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len);
    let mut state: u64 = 0x9E3779B9_7F4A7C15;
    while bytes.len() < len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        for byte in state.to_le_bytes() {
            // No angle brackets: garbage that opens elements would hit the nesting cap first.
            bytes.push(match b' ' + byte % 95 {
                b'<' | b'>' => b'.',
                printable => printable,
            });
            if bytes.len() == len {
                break;
            }
        }
    }
    bytes
}

#[test]
fn dom_part_past_the_whole_cap_is_refused() {
    let cell = r#"<c r="A1"><v>1</v></c>"#;
    let bytes = workbook_with_sheet(&sheet(cell));
    let bytes = with_replaced_part(bytes, "xl/workbook.xml", &incompressible(70_000_000));
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("kept whole"), "{error}");
    assert!(error.contains("harm"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn non_sheet_part_without_checked_elements_past_the_whole_cap_is_refused() {
    let cell = r#"<c r="A1"><v>1</v></c>"#;
    let bytes = workbook_with_sheet(&sheet(cell));
    let bytes = with_replaced_part(bytes, "xl/styles.xml", &incompressible(70_000_000));
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("kept whole"), "{error}");
    assert!(error.contains("harm"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn shared_strings_with_a_forged_small_size_are_capped_on_their_real_size() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let bytes = workbook_with_strings(&sheet(cell), &shared_strings(1, 1));
    let mut bytes = with_replaced_part(bytes, "xl/sharedStrings.xml", &incompressible(70_000_000));
    patch_declared_size(&mut bytes, "xl/sharedStrings.xml", 1_000);
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("kept whole"), "{error}");
    assert!(error.contains("harm"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn a_normal_file_with_ordinary_styles_still_opens() {
    let mut source = rust_xlsxwriter::Workbook::new();
    let bold = rust_xlsxwriter::Format::new().set_bold();
    let sheet = source.add_worksheet();
    sheet
        .write_string_with_format(0, 0, "title", &bold)
        .unwrap();
    sheet.write_number(1, 0, 7.0).unwrap();
    let bytes = source.save_to_buffer().unwrap();
    open_fast(bytes).expect("ordinary styles and strings are far below every cap");
}

#[test]
fn worksheet_tail_past_the_whole_cap_is_refused() {
    let tail = String::from_utf8(incompressible(70_000_000)).unwrap();
    let sheet_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>{tail}</worksheet>"#
    );
    let started = Instant::now();
    let error = open_fast(workbook_with_sheet(&sheet_xml)).unwrap_err();
    assert!(error.contains("kept whole"), "{error}");
    assert!(error.contains("harm"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
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

// A sheet element written back on save, larger than the budget for them: the file opens,
// saves after an edit and opens again.
#[test]
fn large_sheet_elements_save_and_reopen() {
    let mut source = rust_xlsxwriter::Workbook::new();
    source.add_worksheet().write_number(0, 0, 1.0).unwrap();
    let bytes = source.save_to_buffer().unwrap();
    let mut sheet_xml = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(bytes.clone()))
        .unwrap()
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet_xml)
        .unwrap();
    let large = format!(
        r#"<hyperlinks><hyperlink ref="A1" location="{}"/></hyperlinks><pageMargins"#,
        "X".repeat(17 * 1024 * 1024)
    );
    let sheet_xml = sheet_xml.replacen("<pageMargins", &large, 1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("elements.xlsx");
    let bytes = with_replaced_part(bytes, "xl/worksheets/sheet1.xml", sheet_xml.as_bytes());
    std::fs::write(&path, bytes).unwrap();
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.set_input(SheetId(0), CellPos::parse_a1("B2").unwrap(), "2")
        .unwrap();
    save_xlsx(book.save_snapshot(&path, SaveTarget::Document).unwrap()).unwrap();
    open_xlsx(&path).unwrap();
}

// The parsers recurse once per open element, so a part nested past the cap is refused
// before any of them runs, by every reader; 100,000 levels overflowed the engine stack.
#[test]
fn deeply_nested_elements_are_refused_not_a_stack_overflow() {
    let deep = format!(
        r#"<autoFilter ref="A1:B2">{}{}</autoFilter>"#,
        r#"<filterColumn colId="0">"#.repeat(100_000),
        "</filterColumn>".repeat(100_000)
    );
    let sheet_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>{deep}</worksheet>"#
    );
    let styles = format!(
        r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{}{}</styleSheet>"#,
        "<x>".repeat(100_000),
        "</x>".repeat(100_000)
    );
    let with_deep_sheet = workbook_with_sheet(&sheet_xml);
    let with_deep_styles = with_replaced_part(
        workbook_with_sheet(&sheet(r#"<c r="A1"><v>1</v></c>"#)),
        "xl/styles.xml",
        styles.as_bytes(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.xlsx");
    for bytes in [with_deep_sheet, with_deep_styles] {
        std::fs::write(&path, bytes).unwrap();
        for reader in [XlsxReader::Fast, XlsxReader::IronCalc, XlsxReader::Shadow] {
            let error = open_xlsx_with(&path, reader).err().unwrap().to_string();
            assert!(
                error.contains("nests elements deeper"),
                "{reader:?}: {error}"
            );
        }
    }
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

#[test]
fn styles_with_empty_or_missing_lists_open_save_and_reopen() {
    let cell = r#"<c r="A1"><v>7</v></c><c r="B1"><f>A1*6</f></c>"#;
    for list in ["fonts", "fills", "borders", "cellXfs", "cellStyles"] {
        for replacement in [String::new(), format!("<{list} count=\"0\"/>")] {
            let bytes = workbook_with_sheet(&sheet(cell));
            let styles = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{}</styleSheet>"#,
                ["fonts", "fills", "borders", "cellXfs", "cellStyles"]
                    .iter()
                    .map(|name| if *name == list {
                        replacement.clone()
                    } else {
                        style_list(name).to_string()
                    })
                    .collect::<String>()
            );
            let bytes = with_replaced_part(bytes, "xl/styles.xml", styles.as_bytes());
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("styles.xlsx");
            std::fs::write(&path, bytes).unwrap();
            let mut book = open_xlsx(&path).unwrap().workbook;
            book.set_input(SheetId(0), CellPos::parse_a1("C1").unwrap(), "1")
                .unwrap();
            save_xlsx(book.save_snapshot(&path, SaveTarget::Document).unwrap()).unwrap();
            open_xlsx(&path).unwrap();
        }
    }
}

fn style_list(name: &str) -> &'static str {
    match name {
        "fonts" => r#"<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>"#,
        "fills" => r#"<fills count="1"><fill><patternFill patternType="none"/></fill></fills>"#,
        "borders" => r#"<borders count="1"><border/></borders>"#,
        "cellXfs" => {
            r#"<cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs>"#
        }
        _ => {
            r#"<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>"#
        }
    }
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
fn lied_bomb_after_a_declined_sheet_still_refuses() {
    let cell = r#"<c r="A1" t="s"><v>0</v></c>"#;
    let declined = sheet(cell).replace("<sheetData>", "<sheetData><!-- not streamed -->");
    let bytes = workbook_with_strings(&declined, &shared_strings(1, 1));
    let mut bytes = with_extra_part(bytes, "xl/drawings/drawing1.xml", &"A".repeat(70_000_000));
    patch_declared_size(&mut bytes, "xl/drawings/drawing1.xml", 1_000);
    let started = Instant::now();
    let error = open_fast(bytes).unwrap_err();
    assert!(error.contains("harm"), "{error}");
    assert!(error.contains("150"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn whitespace_and_crlf_between_rows_stay_on_the_streaming_reader() {
    for gap in ["\n  ", "\r\n", "\r\n  \r\n"] {
        let mut source = rust_xlsxwriter::Workbook::new();
        let sheet = source.add_worksheet();
        sheet.write_number(0, 0, 7.0).unwrap();
        sheet
            .write_formula(1, 0, rust_xlsxwriter::Formula::new("=A1*6"))
            .unwrap();
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
                body = body
                    .replace("<row ", &format!("{gap}<row "))
                    .replace("</row>", &format!("</row>{gap}"));
                assert!(body.contains(gap), "no rows to separate");
            }
            out.start_file(name, options).unwrap();
            out.write_all(body.as_bytes()).unwrap();
        }
        let bytes = out.finish().unwrap().into_inner();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gaps.xlsx");
        std::fs::write(&path, bytes).unwrap();
        let opened = open_xlsx(&path).unwrap();
        assert!(opened.fallback.is_none(), "gap {gap:?}");
        let cell = |a1: &str| {
            opened
                .workbook
                .cell(
                    zenkai_types::SheetId(0),
                    zenkai_types::CellPos::parse_a1(a1).unwrap(),
                )
                .text
        };
        assert_eq!(cell("A1"), "7", "gap {gap:?}");
        assert_eq!(cell("A2"), "42", "gap {gap:?}");
    }
}

#[test]
fn a_drawing_part_stays_out_of_the_stub() {
    let mut source = rust_xlsxwriter::Workbook::new();
    source.add_worksheet().write_number(0, 0, 7.0).unwrap();
    let original = source.save_to_buffer().unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(original)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    let drawing = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to></xdr:twoCellAnchor></xdr:wsDr>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut body = String::new();
        std::io::Read::read_to_string(&mut entry, &mut body).unwrap();
        if name == "[Content_Types].xml" {
            body = body.replace(
                "</Types>",
                "<Override PartName=\"/xl/drawings/drawing1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/></Types>",
            );
        }
        if name == "xl/worksheets/sheet1.xml" {
            body = body.replace("</worksheet>", "<drawing r:id=\"rId1\"/></worksheet>");
        }
        out.start_file(name, options).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    out.start_file("xl/drawings/drawing1.xml", options).unwrap();
    out.write_all(drawing.as_bytes()).unwrap();
    out.start_file("xl/worksheets/_rels/sheet1.xml.rels", options)
        .unwrap();
    out.write_all(rels.as_bytes()).unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drawing.xlsx");
    std::fs::write(&path, bytes).unwrap();
    let opened = open_xlsx(&path).unwrap();
    assert!(opened.fallback.is_none(), "{:?}", opened.fallback);
    assert!(opened.unsupported.contains(&Unsupported::Images));
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

// cellStyleXfs is optional in the schema; IronCalc indexed its first element unchecked.
#[test]
fn styles_without_cell_style_formats_open_edit_and_save() {
    let mut source = rust_xlsxwriter::Workbook::new();
    source.add_worksheet().write_number(0, 0, 1.0).unwrap();
    let bytes = source.save_to_buffer().unwrap();
    let mut styles = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(bytes.clone()))
        .unwrap()
        .by_name("xl/styles.xml")
        .unwrap()
        .read_to_string(&mut styles)
        .unwrap();
    let start = styles.find("<cellStyleXfs").unwrap();
    let end = styles.find("</cellStyleXfs>").unwrap() + "</cellStyleXfs>".len();
    styles.replace_range(start..end, "");
    let bytes = with_replaced_part(bytes, "xl/styles.xml", styles.as_bytes());
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("styles.xlsx");
        std::fs::write(&path, &bytes).unwrap();
        let mut book = open_xlsx_with(&path, reader).unwrap().workbook;
        let a1 = CellPos::parse_a1("A1").unwrap();
        book.apply_style(
            SheetId(0),
            zenkai_types::Range::parse_a1("A1").unwrap(),
            zenkai_types::StyleChange::Bold(true),
        )
        .unwrap();
        save_xlsx(book.save_snapshot(&path, SaveTarget::Document).unwrap()).unwrap();
        let reopened = open_xlsx(&path).unwrap().workbook;
        assert!(reopened.cell(SheetId(0), a1).style.bold, "{reader:?}");
    }
}
