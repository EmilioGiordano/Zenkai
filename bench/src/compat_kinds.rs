// One corpus file for each content kind a save drops and warns about. rust_xlsxwriter
// writes the sparklines; it writes none of the others, so they are its output with the
// parts and elements Excel writes for them added by hand, in schema order.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use rust_xlsxwriter::{Sparkline, Workbook};

const SHEET: &str = "xl/worksheets/sheet1.xml";
const SHEET_RELATIONSHIPS: &str = "xl/worksheets/_rels/sheet1.xml.rels";
const WORKBOOK: &str = "xl/workbook.xml";
const WORKBOOK_RELATIONSHIPS: &str = "xl/_rels/workbook.xml.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
// Every element added after the cells goes before the page margins, which follow them.
const PAGE_MARGINS: &str = "<pageMargins";
const SHEET_END: &str = "</worksheet>";
const OFFICE_RELATIONSHIPS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const CONNECTIONS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<connections xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><connection id="1" name="data" type="6" refreshedVersion="6" background="1" saveData="1"><textPr codePage="65001" sourceFile="C:\Users\Public\data.csv" comma="1"><textFields count="2"><textField/><textField/></textFields></textPr></connection></connections>"#;
const CONNECTIONS_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.connections+xml";

// A package as rust_xlsxwriter wrote it, part by part in archive order.
struct Package {
    parts: Vec<(String, Vec<u8>)>,
}

impl Package {
    fn of(book: &mut Workbook) -> Result<Package> {
        let bytes = book.save_to_buffer()?;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        let parts = (0..archive.len())
            .map(|index| -> Result<(String, Vec<u8>)> {
                let mut entry = archive.by_index(index)?;
                let mut content = Vec::new();
                entry.read_to_end(&mut content)?;
                Ok((entry.name().to_string(), content))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Package { parts })
    }

    // Puts `text` right before the first `before` in the part.
    fn insert(&mut self, part: &str, before: &str, text: &str) -> Result<()> {
        let (_, content) = self
            .parts
            .iter_mut()
            .find(|(name, _)| name == part)
            .with_context(|| format!("no part {part}"))?;
        let xml = String::from_utf8(std::mem::take(content))?;
        let at = xml
            .find(before)
            .with_context(|| format!("no {before} in {part}"))?;
        *content = format!("{}{text}{}", &xml[..at], &xml[at..]).into_bytes();
        Ok(())
    }

    // A part declared with its own content type.
    fn add(&mut self, name: &str, content_type: &str, content: &[u8]) -> Result<()> {
        let declared = format!(r#"<Override PartName="/{name}" ContentType="{content_type}"/>"#);
        self.insert(CONTENT_TYPES, "</Types>", &declared)?;
        self.parts.push((name.to_string(), content.to_vec()));
        Ok(())
    }

    fn add_relationships(&mut self, name: &str, relationships: &[(&str, &str, &str)]) {
        let listed: String = relationships
            .iter()
            .map(|relationship| relationship_element(*relationship))
            .collect();
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{listed}</Relationships>"#
        );
        self.parts.push((name.to_string(), xml.into_bytes()));
    }

    fn relate_workbook(&mut self, relationship: (&str, &str, &str)) -> Result<()> {
        self.insert(
            WORKBOOK_RELATIONSHIPS,
            "</Relationships>",
            &relationship_element(relationship),
        )
    }

    fn save(self, path: &Path) -> Result<()> {
        let file =
            std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        let mut writer = zip::ZipWriter::new(file);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in self.parts {
            writer.start_file(name, options)?;
            writer.write_all(&content)?;
        }
        writer.finish()?;
        Ok(())
    }
}

// Id, type under the office relationships namespace, target.
fn relationship_element((id, kind, target): (&str, &str, &str)) -> String {
    format!(r#"<Relationship Id="{id}" Type="{OFFICE_RELATIONSHIPS}/{kind}" Target="{target}"/>"#)
}

// A sheet of numbers, so the corpus test also compares values and the round trip.
fn numbers(name: &str) -> Result<Workbook> {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet().set_name(name)?;
    for row in 0..5u32 {
        sheet.write_number(row, 0, f64::from(row + 1))?;
        sheet.write_number(row, 1, f64::from((row + 1) * 10))?;
    }
    Ok(book)
}

pub fn generate(dir: &Path) -> Result<()> {
    sparklines(dir)?;
    other_sheet_rule(dir)?;
    custom_views(dir)?;
    phonetic(dir)?;
    scenarios(dir)?;
    cell_watches(dir)?;
    workbook_protection(dir)?;
    printer_settings(dir)?;
    connections(dir)?;
    query_table(dir)?;
    custom_xml(dir)
}

fn sparklines(dir: &Path) -> Result<()> {
    let mut book = numbers("Trend")?;
    let sheet = book.worksheet_from_index(0)?;
    sheet.add_sparkline(0, 3, &Sparkline::new().set_range("Trend!$A$1:$A$5"))?;
    book.save(dir.join("sparklines.xlsx"))?;
    Ok(())
}

// Excel writes a rule comparing with another sheet only into the extension list.
fn other_sheet_rule(dir: &Path) -> Result<()> {
    let mut book = numbers("Scores")?;
    book.add_worksheet()
        .set_name("Limits")?
        .write_number(0, 0, 3.0)?;
    let mut package = Package::of(&mut book)?;
    package.insert(
        SHEET,
        SHEET_END,
        concat!(
            r#"<extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}" "#,
            r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
            r#"<x14:conditionalFormattings><x14:conditionalFormatting "#,
            r#"xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main">"#,
            r#"<x14:cfRule type="cellIs" priority="1" operator="greaterThan" "#,
            r#"id="{2B6A1C9E-4F0D-4E8B-9C3A-5D7E8F901234}"><xm:f>Limits!$A$1</xm:f>"#,
            r#"<x14:dxf><font><b/><color rgb="FF9C0006"/></font><fill><patternFill>"#,
            r#"<bgColor rgb="FFFFC7CE"/></patternFill></fill></x14:dxf></x14:cfRule>"#,
            r#"<xm:sqref>A1:A5</xm:sqref></x14:conditionalFormatting>"#,
            r#"</x14:conditionalFormattings></ext></extLst>"#
        ),
    )?;
    package.save(&dir.join("conditional-format-other-sheet.xlsx"))
}

fn custom_views(dir: &Path) -> Result<()> {
    let mut package = Package::of(&mut numbers("Report")?)?;
    package.insert(
        WORKBOOK,
        "</workbook>",
        concat!(
            r#"<customWorkbookViews><customWorkbookView name="Review" "#,
            r#"guid="{6F1A2B3C-4D5E-4F60-8A9B-0C1D2E3F4A5B}" windowWidth="16000" "#,
            r#"windowHeight="9000" activeSheetId="1"/></customWorkbookViews>"#
        ),
    )?;
    package.insert(
        SHEET,
        PAGE_MARGINS,
        concat!(
            r#"<customSheetViews><customSheetView guid="{6F1A2B3C-4D5E-4F60-8A9B-0C1D2E3F4A5B}" "#,
            r#"scale="90"><selection activeCell="B2" sqref="B2"/></customSheetView>"#,
            r#"</customSheetViews>"#
        ),
    )?;
    package.save(&dir.join("custom-views.xlsx"))
}

fn phonetic(dir: &Path) -> Result<()> {
    let mut book = numbers("Names")?;
    book.worksheet_from_index(0)?.write_string(5, 0, "東京")?;
    let mut package = Package::of(&mut book)?;
    package.insert(
        SHEET,
        PAGE_MARGINS,
        r#"<phoneticPr fontId="0" type="Hiragana"/>"#,
    )?;
    package.save(&dir.join("phonetic.xlsx"))
}

fn scenarios(dir: &Path) -> Result<()> {
    let mut package = Package::of(&mut numbers("Plan")?)?;
    package.insert(
        SHEET,
        PAGE_MARGINS,
        concat!(
            r#"<scenarios current="0" show="0"><scenario name="High" count="1" user="Zenkai">"#,
            r#"<inputCells r="A1" val="20"/></scenario><scenario name="Low" count="1" "#,
            r#"user="Zenkai"><inputCells r="A1" val="0"/></scenario></scenarios>"#
        ),
    )?;
    package.save(&dir.join("scenarios.xlsx"))
}

fn cell_watches(dir: &Path) -> Result<()> {
    let mut package = Package::of(&mut numbers("Watched")?)?;
    package.insert(
        SHEET,
        SHEET_END,
        r#"<cellWatches><cellWatch r="B5"/></cellWatches>"#,
    )?;
    package.save(&dir.join("cell-watches.xlsx"))
}

fn workbook_protection(dir: &Path) -> Result<()> {
    let mut package = Package::of(&mut numbers("Locked")?)?;
    package.insert(
        WORKBOOK,
        "<bookViews>",
        r#"<workbookProtection lockStructure="1"/>"#,
    )?;
    package.save(&dir.join("workbook-protection.xlsx"))
}

// A DEVMODEW for "Microsoft Print to PDF", A4 portrait, as Windows stores printer settings.
fn devmode() -> Vec<u8> {
    const SIZE: u16 = 220;
    const NAME_UNITS: usize = 32;
    // DM_ORIENTATION, DM_PAPERSIZE, DM_SCALE and DM_COPIES.
    const FIELDS: u32 = 0x0000_0113;
    const PORTRAIT: u16 = 1;
    const A4: u16 = 9;
    const FULL_SCALE: u16 = 100;
    let mut name: Vec<u16> = "Microsoft Print to PDF".encode_utf16().collect();
    name.resize(NAME_UNITS, 0);
    let mut bytes: Vec<u8> = name.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    for word in [0x0401u16, 0, SIZE, 0] {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(FIELDS.to_le_bytes());
    for word in [PORTRAIT, A4, 0, 0, FULL_SCALE, 1] {
        bytes.extend(word.to_le_bytes());
    }
    bytes.resize(usize::from(SIZE), 0);
    bytes
}

fn printer_settings(dir: &Path) -> Result<()> {
    let mut book = numbers("Printed")?;
    book.worksheet_from_index(0)?.set_paper_size(9);
    let mut package = Package::of(&mut book)?;
    package.insert(SHEET, "paperSize=", r#"r:id="rId1" "#)?;
    package.add_relationships(
        SHEET_RELATIONSHIPS,
        &[(
            "rId1",
            "printerSettings",
            "../printerSettings/printerSettings1.bin",
        )],
    );
    package.add(
        "xl/printerSettings/printerSettings1.bin",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings",
        &devmode(),
    )?;
    package.save(&dir.join("printer-settings.xlsx"))
}

fn add_connections(package: &mut Package) -> Result<()> {
    package.add(
        "xl/connections.xml",
        CONNECTIONS_TYPE,
        CONNECTIONS.as_bytes(),
    )?;
    package.relate_workbook(("rIdZenkai1", "connections", "connections.xml"))
}

fn connections(dir: &Path) -> Result<()> {
    let mut package = Package::of(&mut numbers("Imported")?)?;
    add_connections(&mut package)?;
    package.save(&dir.join("connections.xlsx"))
}

// A text import: the query table fills the range of the sheet's name of the same name.
fn query_table(dir: &Path) -> Result<()> {
    let mut book = numbers("Import")?;
    book.define_name("Import!data", "=Import!$A$1:$B$5")?;
    let mut package = Package::of(&mut book)?;
    add_connections(&mut package)?;
    package.add(
        "xl/queryTables/queryTable1.xml",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.queryTable+xml",
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            "\n",
            r#"<queryTable xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
            r#"name="data" connectionId="1" autoFormatId="16" applyNumberFormats="0" "#,
            r#"applyBorderFormats="0" applyFontFormats="0" applyPatternFormats="0" "#,
            r#"applyAlignmentFormats="0" applyWidthHeightFormats="0"/>"#
        )
        .as_bytes(),
    )?;
    package.add_relationships(
        SHEET_RELATIONSHIPS,
        &[("rId1", "queryTable", "../queryTables/queryTable1.xml")],
    );
    package.save(&dir.join("query-table.xlsx"))
}

fn custom_xml(dir: &Path) -> Result<()> {
    let mut package = Package::of(&mut numbers("Tagged")?)?;
    package.add(
        "customXml/itemProps1.xml",
        "application/vnd.openxmlformats-officedocument.customXmlProperties+xml",
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>"#,
            "\n",
            r#"<ds:datastoreItem ds:itemID="{3C5D7E9F-1A2B-4C3D-8E4F-5A6B7C8D9E0F}" "#,
            r#"xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml">"#,
            r#"<ds:schemaRefs/></ds:datastoreItem>"#
        )
        .as_bytes(),
    )?;
    // The package declares every .xml part without a type of its own as XML.
    package.parts.push((
        "customXml/item1.xml".to_string(),
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            "\n",
            r#"<properties xmlns="urn:zenkai:corpus"><owner>Finance</owner></properties>"#
        )
        .as_bytes()
        .to_vec(),
    ));
    package.add_relationships(
        "customXml/_rels/item1.xml.rels",
        &[("rId1", "customXmlProps", "itemProps1.xml")],
    );
    package.relate_workbook(("rIdZenkai1", "customXml", "../customXml/item1.xml"))?;
    package.save(&dir.join("custom-xml.xlsx"))
}
