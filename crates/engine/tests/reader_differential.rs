// The fast xlsx reader must build exactly the workbook IronCalc's importer builds, or leave
// the file to it. Every file here is read by both and compared field by field: the corpus,
// hand-written sheets for each importer quirk, and damaged copies of the corpus.
// ZENKAI_READER_CORPUS adds files or folders, separated by ';' (the heavy file, the
// benchmark fixtures).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::PathBuf;

use proptest::prelude::*;
use zenkai_engine::{Engine, ReaderComparison, XlsxReader, compare_readers, open_xlsx_with};

#[path = "../../../test-support/xlsx_damage.rs"]
mod xlsx_damage;

use xlsx_damage::{apply, corpus, damage, parts_of, zip_of};

fn extra_corpus() -> Vec<PathBuf> {
    let Ok(list) = std::env::var("ZENKAI_READER_CORPUS") else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in list.split(';').filter(|e| !e.is_empty()) {
        let path = PathBuf::from(entry);
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(&path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "xlsx"))
                .collect();
            found.sort();
            files.extend(found);
        } else {
            files.push(path);
        }
    }
    files
}

fn assert_agree(label: &str, bytes: &[u8], may_decline: bool) {
    match compare_readers(bytes) {
        ReaderComparison::Same | ReaderComparison::BothRefused => {}
        ReaderComparison::FastDeclined(reason) => {
            assert!(may_decline, "{label}: the fast reader declined: {reason}");
        }
        other => panic!("{label}: {other:#?}"),
    }
}

#[test]
fn the_corpus_reads_the_same_with_both_readers() {
    let files: Vec<PathBuf> = corpus().into_iter().chain(extra_corpus()).collect();
    assert!(files.len() >= 10);
    for path in files {
        assert_agree(
            &path.display().to_string(),
            &std::fs::read(&path).unwrap(),
            false,
        );
    }
}

fn workbook(sheets: &[&str], shared_strings: Option<&str>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    let mut parts = vec![
        (
            "[Content_Types].xml".to_string(),
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels".to_string(),
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/styles.xml".to_string(),
            r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="4" fontId="0" fillId="0" borderId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#.to_string(),
        ),
    ];
    let mut book_sheets = String::new();
    let mut rels = String::new();
    for (index, sheet) in sheets.iter().enumerate() {
        let n = index + 1;
        book_sheets.push_str(&format!(
            r#"<sheet name="Sheet{n}" sheetId="{n}" r:id="rId{n}"/>"#
        ));
        rels.push_str(&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#));
        parts.push((format!("xl/worksheets/sheet{n}.xml"), sheet.to_string()));
    }
    parts.push((
        "xl/workbook.xml".to_string(),
        format!(r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{book_sheets}</sheets><definedNames><definedName name="Rate">Sheet1!$A$1</definedName><definedName name="Local" localSheetId="0">Sheet1!$B$1:$B$3</definedName></definedNames></workbook>"#),
    ));
    parts.push((
        "xl/_rels/workbook.xml.rels".to_string(),
        format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#),
    ));
    if let Some(strings) = shared_strings {
        parts.push((
            "xl/sharedStrings.xml".to_string(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{strings}</sst>"#),
        ));
    }
    for (name, body) in parts {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn sheet(data: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:x14ac="http://schemas.microsoft.com/office/spreadsheetml/2009/9/ac"><dimension ref="A1:D9"/><sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/><selection pane="bottomLeft" activeCell="B3" sqref="B3"/></sheetView></sheetViews><cols><col min="1" max="2" width="12.5" customWidth="1"/></cols><sheetData>{data}</sheetData><mergeCells count="1"><mergeCell ref="C8:D9"/></mergeCells></worksheet>"#
    )
}

const QUIRKS: [(&str, &str); 22] = [
    (
        "numbers",
        r#"<row r="1"><c r="A1"><v>1</v></c><c r="B1"><v>-0</v></c><c r="C1"><v>1e400</v></c><c r="D1"><v> 7 </v></c></row>"#,
    ),
    (
        "types",
        r#"<row r="1"><c r="A1" t="b"><v>1</v></c><c r="B1" t="e"><v>#DIV/0!</v></c><c r="C1" t="e" vm="2"><v>#VALUE!</v></c><c r="D1" t="d"><v>2020-01-01</v></c><c r="E1" t="zz"><v>1</v></c><c r="F1" s="1"/></row>"#,
    ),
    (
        "inline strings",
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>a_x0001_b</t></is></c><c r="B1" t="inlineStr"><is><r><rPr><b/></rPr><t xml:space="preserve">x </t></r><r><t>y</t></r><rPh sb="0" eb="1"><t>z</t></rPh></is></c><c r="C1" t="inlineStr"><is><t>x y</t></is></c><c r="D1" t="inlineStr"/></row>"#,
    ),
    (
        "str values",
        r#"<row r="1"><c r="A1" t="str"><v>a_x0041_</v></c><c r="B1" t="str"><v>hello</v></c><c r="C1" t="s"><v>1</v></c><c r="D1" t="s"><v>x</v></c></row>"#,
    ),
    (
        "entities",
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>&lt;&amp;&gt;&quot;&apos;&#65;&#x42;</t></is></c><c r="B1"><f>IF(A1&lt;&gt;"",1,2)</f><v>1</v></c></row>"#,
    ),
    (
        "formulas",
        r#"<row r="1"><c r="A1"><f>B1+$C$1+SUM(D1:D3)</f><v>3</v></c><c r="B1"><f>Rate*2</f></c><c r="C1"><f>Sheet1!A1&amp;"x"</f><v>s</v></c></row><row r="2"><c r="A2"><f>B2+$C$1+SUM(D2:D4)</f><v>3</v></c><c r="B2"><f>LOG10(100)</f></c><c r="C2" t="str"><f>"t"&amp;"u"</f><v>tu</v></c><c r="D2" t="e"><f>1/0</f><v>#DIV/0!</v></c></row>"#,
    ),
    (
        "swapped ranges",
        r#"<row r="1"><c r="A1"><f>SUM($B$9:B1)</f></c></row><row r="5"><c r="A5"><f>SUM($B$9:B5)</f></c></row><row r="12"><c r="A12"><f>SUM($B$9:B12)</f></c></row>"#,
    ),
    (
        "parse errors",
        r#"<row r="1"><c r="A1"><f>SUM(</f></c><c r="B1"><f>1+</f></c><c r="C1"><f></f></c><c r="D1"><f>Nope!A1</f></c></row><row r="2"><c r="A2"><f>SUM(</f></c><c r="B2"><f>1+</f></c></row>"#,
    ),
    (
        "shared formulas",
        r#"<row r="1"><c r="A1"><f t="shared" ref="A1:A3" si="0">B1*2</f><v>0</v></c><c r="B1"><v>1</v></c></row><row r="2"><c r="A2"><f t="shared" si="0"/><v>0</v></c></row><row r="3"><c r="A3"><f t="shared" si="0"/><v>0</v></c><c r="B3"><f>B2*2</f></c></row>"#,
    ),
    (
        "shared child before anchor",
        r#"<row r="1"><c r="A1"><f t="shared" si="3"/></c><c r="B1"><f t="shared" ref="B1:B2" si="3">A1</f></c></row>"#,
    ),
    (
        "array formulas",
        r#"<row r="1"><c r="A1" cm="1"><f t="array" ref="A1:B2">SEQUENCE(2,2)</f><v>1</v></c><c r="B1" t="str"><v>2</v></c></row><row r="2"><c r="A2" t="b"><v>0</v></c><c r="B2" t="e"><v>#N/A</v></c></row><row r="4"><c r="A4"><f t="array" ref="A4">SUM(B1:B2*2)</f><v>1</v></c></row>"#,
    ),
    (
        "volatile hints",
        r#"<row r="1"><c r="A1"><f ca="1"/><v>3</v></c><c r="B1"><f t="shared" ca="1" ref="B1:B2" si="0">RAND()</f><v>1</v></c></row><row r="2"><c r="B2"><f t="shared" ca="1" si="0"/><v>1</v></c></row>"#,
    ),
    (
        "rows",
        r#"<row r="1" ht="30" customHeight="1"><c r="A1"><v>1</v></c></row><row r="2" s="1" customFormat="true"/><row r="3" hidden="1" x14ac:dyDescent="0.25"/><row r="4" ht="abc"/><row r="6" spans="1:3"><c r="B6"><v>2</v></c></row><row><c r="A7"><v>7</v></c></row>"#,
    ),
    (
        "styles",
        r#"<row r="1"><c r="A1" s="1"><v>1</v></c><c r="B1" s="x"><v>2</v></c><c r="C1" s="-3"><v>3</v></c></row>"#,
    ),
    (
        "several values",
        r#"<row r="1"><c r="A1"><v>1</v><v>2</v></c><c r="B1"><f>1</f><f>2</f></c><c r="C1"><v/></c><c r="D1"><v></v></c><c r="E1"><extLst><ext uri="x"/></extLst><v>5</v></c></row>"#,
    ),
    (
        "data table",
        r#"<row r="1"><c r="A1"><f t="dataTable" ref="A1:B2"/></c></row>"#,
    ),
    (
        "whitespace between rows",
        "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>\n<row r=\"2\"/>",
    ),
    (
        "rows out of order",
        r#"<row r="2"><c r="A2"><v>1</v></c></row><row r="1"><c r="A1"><v>2</v></c></row>"#,
    ),
    (
        "duplicate cells",
        r#"<row r="1"><c r="A1"><v>1</v></c><c r="A1" t="inlineStr"><is><t>x</t></is></c></row>"#,
    ),
    (
        "bad references",
        r#"<row r="1"><c r="a1"><v>1</v></c></row>"#,
    ),
    (
        "cdata",
        r#"<row r="1"><c r="A1"><f><![CDATA[1+2]]></f></c></row>"#,
    ),
    ("empty", ""),
];

#[test]
fn every_importer_quirk_reads_the_same() {
    let strings = Some(
        "<si><t>x</t></si><si><t>hello</t></si><si><r><t>a</t></r><r><t>_x0042_</t></r></si><si><t>x</t></si>",
    );
    for (label, data) in QUIRKS {
        let other = sheet(
            r#"<row r="1"><c r="A1" t="inlineStr"><is><t>hello</t></is></c><c r="B1"><f>Sheet1!A1+1</f></c></row>"#,
        );
        let bytes = workbook(&[&sheet(data), &other], strings);
        assert_agree(label, &bytes, true);
        assert_agree(label, &workbook(&[&sheet(data)], None), true);
    }
}

#[test]
fn the_fast_reader_handles_the_ordinary_quirks_itself() {
    let declined = [
        "shared child before anchor",
        "data table",
        "whitespace between rows",
        "rows out of order",
        "duplicate cells",
        "bad references",
        "cdata",
    ];
    for (label, data) in QUIRKS {
        let bytes = workbook(&[&sheet(data)], None);
        let declined_here = matches!(compare_readers(&bytes), ReaderComparison::FastDeclined(_));
        assert_eq!(declined_here, declined.contains(&label), "{label}");
    }
}

#[test]
fn a_declined_file_falls_back_and_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("child-first.xlsx");
    let data = QUIRKS
        .iter()
        .find(|(label, _)| *label == "shared child before anchor")
        .unwrap()
        .1;
    std::fs::write(&path, workbook(&[&sheet(data)], None)).unwrap();
    let fast = open_xlsx_with(&path, XlsxReader::Fast).unwrap();
    assert!(fast.fallback.is_some());
    let plain = open_xlsx_with(&path, XlsxReader::IronCalc).unwrap();
    assert!(plain.fallback.is_none());
    let b1 = zenkai_types::CellPos::parse_a1("B1").unwrap();
    let first = zenkai_types::SheetId(0);
    assert_eq!(
        fast.workbook.input(first, b1),
        plain.workbook.input(first, b1)
    );
}

#[test]
fn the_fast_reader_refuses_what_the_preflight_refuses() {
    let deep = format!(
        r#"<row r="1"><c r="A1"><f>{}1</f></c></row>"#,
        "(".repeat(300)
    );
    let long = format!(
        r#"<row r="1"><c r="A1"><f>{}1</f></c></row>"#,
        "1+".repeat(5_000)
    );
    let huge = r#"<row r="1"><c r="A1"><f t="array" ref="A1:XFD1048576">1</f></c></row>"#;
    let nested = format!(
        r#"<row r="1"><c r="A1"><extLst><f>{}1</f></extLst></c></row>"#,
        "(".repeat(300)
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.xlsx");
    for data in [deep.as_str(), long.as_str(), huge, nested.as_str()] {
        let bytes = workbook(&[&sheet(data)], None);
        assert_agree(&format!("{data:.60}"), &bytes, true);
        std::fs::write(&path, &bytes).unwrap();
        let refused = open_xlsx_with(&path, XlsxReader::Fast).err().unwrap();
        assert!(
            matches!(refused, zenkai_engine::EngineError::Unsafe(_)),
            "{data:.60}: {refused}"
        );
    }
}

#[test]
fn the_longest_allowed_formula_opens_with_the_fast_reader() {
    let formula = format!("{}1", "1+".repeat((8_192 - 2) / 2));
    let data = format!(r#"<row r="1"><c r="A1"><f>{formula}</f></c></row>"#);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long.xlsx");
    std::fs::write(&path, workbook(&[&sheet(&data)], None)).unwrap();
    let opened = open_xlsx_with(&path, XlsxReader::Fast).unwrap();
    assert!(opened.fallback.is_none());
    let a1 = zenkai_types::CellPos::parse_a1("A1").unwrap();
    assert_eq!(
        opened.workbook.cell(zenkai_types::SheetId(0), a1).text,
        "4096"
    );
}

// Past a few megabytes a sheet is read in pieces; strings, shared formulas and row order
// must come out as if it were read in one go.
fn large_sheet_data(rows: usize, with_array: bool) -> String {
    let mut data = String::new();
    for row in 1..=rows {
        let r = if row % 1_000 == 7 {
            String::new()
        } else {
            format!(" r=\"{row}\"")
        };
        let shared = if row == 1 {
            r#"<f t="shared" ref="D1:D99999" si="0">A1*2</f>"#.to_string()
        } else {
            r#"<f t="shared" si="0"/>"#.to_string()
        };
        data.push_str(&format!(
            r#"<row{r}><c r="A{row}"><v>{row}</v></c><c r="B{row}" t="inlineStr"><is><t>name {}</t></is></c><c r="C{row}" t="str"><v>v{}</v></c><c r="D{row}">{shared}<v>0</v></c><c r="E{row}"><f>A{row}+D{row}*$A$1+{}</f></c></row>"#,
            row % 37,
            row % 11,
            row % 7,
        ));
    }
    if with_array {
        data.push_str(&format!(
            r#"<row r="{}"><c r="A{0}"><f t="array" ref="A{0}:A{1}">SEQUENCE(2)</f></c></row><row r="{1}"><c r="A{1}"><v>2</v></c></row>"#,
            rows + 1,
            rows + 2,
        ));
    }
    data
}

#[test]
fn a_sheet_read_in_pieces_reads_the_same() {
    for with_array in [false, true] {
        let data = large_sheet_data(20_000, with_array);
        assert!(data.len() > 4 * 1024 * 1024 + 100_000, "{}", data.len());
        let bytes = workbook(&[&sheet(&data), &sheet(&large_sheet_data(50, false))], None);
        assert_agree(&format!("pieces, array {with_array}"), &bytes, false);
    }
}

fn damaged_case(file: prop::sample::Index, damages: &[xlsx_damage::Damage]) -> Vec<u8> {
    let files = corpus();
    let mut parts = parts_of(&files[file.index(files.len())]);
    for damage in damages {
        if parts.is_empty() {
            break;
        }
        apply(&mut parts, damage);
    }
    zip_of(&parts)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn damaged_files_read_the_same_or_fall_back(
        file in any::<prop::sample::Index>(),
        damages in prop::collection::vec(damage(), 1..4),
    ) {
        let bytes = damaged_case(file, &damages);
        assert_agree(&format!("{damages:?}"), &bytes, true);
    }
}
