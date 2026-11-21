use crate::error::EngineError;
use crate::file::Unsupported;
use zenkai_xlsx_reader::{PartRole, has_checked_element};

pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
// Engine parsing and evaluation recurse on the formula tree; a dedicated stack
// this large keeps any formula within MAX_FORMULA_CHARS far from overflowing.
pub const ENGINE_STACK_BYTES: usize = 256 * 1024 * 1024;

fn reject(reason: String) -> EngineError {
    EngineError::Unsafe(reason)
}

// IronCalc reads these rule types from a sheet's extension list; it drops any other.
const EXTENDED_RULES_READ: [&str; 3] = ["dataBar", "iconSet", "expression"];

// Every XML part is checked, wherever it lives: the engine follows relationship
// targets to any path and also parses formulas outside worksheets (defined names,
// conditional-format rules, validations). Parts that are not XML are left to the engine.
pub fn check_part(bytes: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let mut found = Vec::new();
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(found);
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // A part reaches here with its bytes only as a worksheet or once the reader's prefilter
    // found a checked element in it, so the worksheet's wider set is the one to look for.
    if !has_checked_element(text.as_bytes(), PartRole::Worksheet) {
        return Ok(found);
    }
    // The engine reads parts with this same roxmltree, so a part it cannot parse can
    // never feed it a formula; rejecting here would only refuse valid files (VML).
    let document = match roxmltree::Document::parse(text) {
        Ok(document) => document,
        Err(error) => {
            tracing::debug!(%error, "skipping a part that is not well-formed XML");
            return Ok(found);
        }
    };
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        if matches!(
            name,
            "f" | "formula" | "formula1" | "formula2" | "definedName"
        ) {
            check_formula(&node)?;
        }
        if let Some(kind) = element_kind(name).or_else(|| placed_kind(node))
            && !found.contains(&kind)
        {
            found.push(kind);
        }
    }
    Ok(found)
}

// What an element of this name holds, wherever in a part it is found.
pub(crate) fn element_kind(name: &str) -> Option<Unsupported> {
    match name {
        "hyperlinks" => Some(Unsupported::Hyperlinks),
        "dataValidations" => Some(Unsupported::DataValidation),
        "autoFilter" => Some(Unsupported::AutoFilter),
        "sheetProtection" => Some(Unsupported::SheetProtection),
        "sparklineGroups" => Some(Unsupported::Sparklines),
        "slicerList" | "timelineRefs" => Some(Unsupported::Slicers),
        "customSheetViews" | "customWorkbookViews" => Some(Unsupported::CustomViews),
        "scenarios" => Some(Unsupported::Scenarios),
        "cellWatches" => Some(Unsupported::CellWatches),
        "workbookProtection" => Some(Unsupported::WorkbookProtection),
        _ => None,
    }
}

// What an element holds only where it sits, or with the attributes it has.
fn placed_kind(node: roxmltree::Node<'_, '_>) -> Option<Unsupported> {
    let parent = node.parent_element()?;
    match node.tag_name().name() {
        // Shared strings and inline texts hold one for each text; only the sheet's own is
        // looked for, as the reader's prefilter does.
        "phoneticPr" if is_worksheet_root(parent) => Some(Unsupported::Phonetic),
        // The relationship names the printer settings part; the page setup is written
        // back without it.
        "pageSetup" if node.attributes().any(|a| is_relationship_id(&a)) => {
            Some(Unsupported::PrinterSettings)
        }
        "cfRule" if is_dropped_extended_rule(node, parent) => {
            Some(Unsupported::ExtendedConditionalFormats)
        }
        _ => None,
    }
}

fn is_worksheet_root(node: roxmltree::Node<'_, '_>) -> bool {
    node.tag_name().name() == "worksheet" && node.parent().is_some_and(|p| p.is_root())
}

fn is_relationship_id(attribute: &roxmltree::Attribute<'_, '_>) -> bool {
    attribute.name() == "id"
        && attribute
            .namespace()
            .is_some_and(|namespace| namespace.ends_with("/relationships"))
}

// A rule of the sheet's extension list (conditionalFormattings, conditionalFormatting,
// cfRule) of a type IronCalc does not read, such as one comparing with another sheet.
fn is_dropped_extended_rule(
    rule: roxmltree::Node<'_, '_>,
    parent: roxmltree::Node<'_, '_>,
) -> bool {
    let in_extension = parent.tag_name().name() == "conditionalFormatting"
        && parent
            .parent_element()
            .is_some_and(|list| list.tag_name().name() == "conditionalFormattings");
    in_extension
        && !rule
            .attribute("type")
            .is_some_and(|kind| EXTENDED_RULES_READ.contains(&kind))
}

fn check_formula(node: &roxmltree::Node<'_, '_>) -> Result<(), EngineError> {
    zenkai_xlsx_reader::limits::check_formula(
        node.text().unwrap_or_default(),
        node.attribute("ref"),
        node.attribute("t") == Some("shared"),
    )
    .map_err(reject)
}

pub fn run_with_engine_stack<T: Send>(
    job: impl FnOnce() -> Result<T, EngineError> + Send,
) -> Result<T, EngineError> {
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("zenkai-engine".to_string())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn_scoped(scope, job)
            .map_err(|e| {
                EngineError::Rejected(format!("could not start the engine thread: {e}"))
            })?;
        handle.join().unwrap_or_else(|_| {
            Err(EngineError::InvalidFile(
                "the engine stopped on this workbook; the file may be damaged".to_string(),
            ))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(cell: &str) -> String {
        format!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1">{cell}</row></sheetData></worksheet>"#
        )
    }

    #[test]
    fn rejects_deep_nesting_in_any_spelling() {
        let deep = sheet(&format!(
            "<c r=\"A1\"><f>{}1{}</f></c>",
            "(".repeat(300),
            ")".repeat(300)
        ));
        assert!(check_part(deep.as_bytes()).is_err());
        let encoded = sheet(&format!("<c r=\"A1\"><f>{}1</f></c>", "&#40;".repeat(300)));
        assert!(check_part(encoded.as_bytes()).is_err());
        let prefixed = format!(
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData><x:row r="1"><x:c r="A1"><x:f>{}1</x:f></x:c></x:row></x:sheetData></x:worksheet>"#,
            "(".repeat(300)
        );
        assert!(check_part(prefixed.as_bytes()).is_err());
        let fine = sheet(&format!(
            "<c r=\"A1\"><f>{}1{}</f></c>",
            "(".repeat(64),
            ")".repeat(64)
        ));
        assert!(check_part(fine.as_bytes()).is_ok());
    }

    #[test]
    fn rejects_long_operator_chains_and_huge_areas() {
        let long = sheet(&format!("<c r=\"A1\"><f>{}1</f></c>", "1+".repeat(5_000)));
        assert!(check_part(long.as_bytes()).is_err());
        let huge = sheet("<c r=\"A1\"><f\n t='array' ref='A1:XFD1048576'>1</f></c>");
        assert!(check_part(huge.as_bytes()).is_err());
        let small = sheet(r#"<c r="A1"><f t="shared" ref="A1:A100" si="0">B1*2</f></c>"#);
        assert!(check_part(small.as_bytes()).is_ok());
        let whole_column =
            sheet(r#"<c r="A1"><f t="shared" ref="A1:A1048576" si="0">B1*2</f></c>"#);
        assert!(check_part(whole_column.as_bytes()).is_ok());
    }

    #[test]
    fn checks_defined_names_and_skips_binary_parts() {
        let workbook = format!(
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><definedNames><definedName name="Foo">{}1</definedName></definedNames></workbook>"#,
            "1+".repeat(5_000)
        );
        assert!(check_part(workbook.as_bytes()).is_err());
        assert!(check_part(&[0x89, b'P', b'N', b'G', 0xFF, 0x00]).is_ok());
    }

    #[test]
    fn prefilter_finds_formula_tags_in_any_spelling() {
        assert!(has_checked_element(b"<c><f>1</f></c>", PartRole::Worksheet));
        assert!(has_checked_element(
            b"<x:c><x:f t='array'>1</x:f></x:c>",
            PartRole::Worksheet
        ));
        assert!(has_checked_element(
            b"<f
 t=\"shared\">1</f>",
            PartRole::Worksheet
        ));
        assert!(has_checked_element(
            b"<definedNames><definedName name=\"a\">1</definedName>",
            PartRole::Worksheet
        ));
        assert!(has_checked_element(
            "<ñ:f>1</ñ:f>".as_bytes(),
            PartRole::Worksheet
        ));
        assert!(has_checked_element(b"<f>1</f", PartRole::Worksheet));
        assert!(!has_checked_element(
            b"<row r=\"1\"><c r=\"A1\"><v>12</v></c><font/><fill/></row>",
            PartRole::Worksheet
        ));
    }

    #[test]
    fn a_byte_order_mark_does_not_skip_the_check() {
        let deep = format!(
            "\u{feff}{}",
            sheet(&format!("<c r=\"A1\"><f>{}1</f></c>", "(".repeat(300)))
        );
        assert!(check_part(deep.as_bytes()).is_err());
    }

    #[test]
    fn reports_hyperlinks_and_validation() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/><sheetProtection sheet="1"/><autoFilter ref="A1:B2"/><dataValidations count="0"/><hyperlinks/></worksheet>"#;
        let mut found = check_part(xml.as_bytes()).unwrap();
        found.sort();
        assert_eq!(
            found,
            [
                Unsupported::Hyperlinks,
                Unsupported::DataValidation,
                Unsupported::AutoFilter,
                Unsupported::SheetProtection
            ]
        );
    }

    fn kinds(xml: &str) -> Vec<Unsupported> {
        let mut found = check_part(xml.as_bytes()).unwrap();
        found.sort();
        found
    }

    #[test]
    fn reports_sheet_content_a_save_cannot_write() {
        let xml = concat!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
            r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
            r#"<sheetData/><scenarios><scenario name="High" count="1">"#,
            r#"<inputCells r="A1" val="2"/></scenario></scenarios>"#,
            r#"<customSheetViews><customSheetView guid="{00000000-0000-0000-0000-000000000001}"/>"#,
            r#"</customSheetViews><phoneticPr fontId="0"/><pageMargins left="0.7" right="0.7" "#,
            r#"top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup r:id="rId1"/>"#,
            r#"<cellWatches><cellWatch r="A1"/></cellWatches><extLst>"#,
            r#"<ext uri="{05C60535-1F16-4fd2-B633-F4F36F0B64E0}" "#,
            r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
            r#"<x14:sparklineGroups/></ext>"#,
            r#"<ext uri="{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}" "#,
            r#"xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main">"#,
            r#"<x15:slicerList/></ext></extLst></worksheet>"#
        );
        assert_eq!(
            kinds(xml),
            [
                Unsupported::Sparklines,
                Unsupported::Slicers,
                Unsupported::CustomViews,
                Unsupported::Phonetic,
                Unsupported::Scenarios,
                Unsupported::CellWatches,
                Unsupported::PrinterSettings
            ]
        );
    }

    #[test]
    fn reports_workbook_protection_and_custom_views() {
        let xml = concat!(
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
            r#"<workbookProtection lockStructure="1"/><sheets/><customWorkbookViews>"#,
            r#"<customWorkbookView name="v" guid="{00000000-0000-0000-0000-000000000001}" "#,
            r#"windowWidth="100" windowHeight="100" activeSheetId="1"/></customWorkbookViews>"#,
            r#"</workbook>"#
        );
        assert_eq!(
            kinds(xml),
            [Unsupported::CustomViews, Unsupported::WorkbookProtection]
        );
    }

    // Shared strings and cell texts hold phonetic settings for each text; IronCalc reads
    // the text, and only the sheet's own settings are a sheet-level loss.
    #[test]
    fn phonetic_settings_count_only_as_a_child_of_the_sheet() {
        let strings = r#"<sst><si><t>x</t><phoneticPr fontId="0"/></si></sst>"#;
        assert!(kinds(strings).is_empty());
        let inline =
            sheet(r#"<c r="A1" t="inlineStr"><is><t>x</t><phoneticPr fontId="0"/></is></c>"#);
        assert!(kinds(&inline).is_empty());
    }

    #[test]
    fn a_page_setup_without_printer_settings_reports_nothing() {
        let xml = r#"<worksheet><sheetData/><pageSetup paperSize="9" orientation="portrait"/></worksheet>"#;
        assert!(kinds(xml).is_empty());
    }

    const EXTENDED_RULE: &str = concat!(
        r#"<worksheet><sheetData/><extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}" "#,
        r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" "#,
        r#"xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main">"#,
        r#"<x14:conditionalFormattings><x14:conditionalFormatting>"#,
        r#"<x14:cfRule type="TYPE" priority="1" id="{00000000-0000-0000-0000-000000000001}">"#,
        r#"<xm:f>Other!$A$1</xm:f></x14:cfRule><xm:sqref>A1:A3</xm:sqref>"#,
        r#"</x14:conditionalFormatting></x14:conditionalFormattings></ext></extLst></worksheet>"#
    );

    // IronCalc reads data bars, icon sets and expression rules from the extension list and
    // writes them back; any other rule there is dropped.
    #[test]
    fn reports_extended_rules_ironcalc_does_not_read() {
        assert_eq!(
            kinds(&EXTENDED_RULE.replace("TYPE", "cellIs")),
            [Unsupported::ExtendedConditionalFormats]
        );
        for read in ["dataBar", "iconSet", "expression"] {
            assert!(
                kinds(&EXTENDED_RULE.replace("TYPE", read)).is_empty(),
                "{read}"
            );
        }
        let plain = sheet("").replace(
            "</worksheet>",
            concat!(
                r#"<conditionalFormatting sqref="A1"><cfRule type="cellIs" operator="greaterThan">"#,
                r#"<formula>1</formula></cfRule></conditionalFormatting></worksheet>"#
            ),
        );
        assert!(kinds(&plain).is_empty());
    }

    #[test]
    fn engine_stack_survives_the_longest_allowed_chain() {
        let formula = format!(
            "={}1",
            "1+".repeat((zenkai_xlsx_reader::limits::MAX_FORMULA_CHARS - 2) / 2)
        );
        let result = run_with_engine_stack(move || {
            let mut book = crate::Workbook::new_empty()?;
            crate::Engine::set_input(
                &mut book,
                zenkai_types::SheetId(0),
                zenkai_types::CellPos::default(),
                &formula,
            )?;
            Ok(crate::Engine::cell(
                &book,
                zenkai_types::SheetId(0),
                zenkai_types::CellPos::default(),
            )
            .text)
        });
        assert_eq!(result.unwrap(), "4096");
    }

    #[test]
    fn engine_thread_panic_becomes_an_error() {
        let result: Result<(), EngineError> = run_with_engine_stack(|| panic!("simulated"));
        let message = result.unwrap_err().to_string();
        assert!(message.contains("damaged"), "{message}");
    }
}
