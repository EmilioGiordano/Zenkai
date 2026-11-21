use std::collections::BTreeSet;

use zenkai_xlsx_reader::start_tag_names;

use crate::error::EngineError;
use crate::extended_rules;
use crate::file::Unsupported;

pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
// Engine parsing and evaluation recurse on the formula tree; a dedicated stack
// this large keeps any formula within MAX_FORMULA_CHARS far from overflowing.
pub const ENGINE_STACK_BYTES: usize = 256 * 1024 * 1024;

fn reject(reason: String) -> EngineError {
    EngineError::Unsafe(reason)
}

// The part is parsed only for these: formulas are checked against the limits, and where an
// extension rule sits decides what it holds.
const PARSED_ELEMENTS: [&str; 6] = [
    "f",
    "formula",
    "formula1",
    "formula2",
    "definedName",
    "conditionalFormattings",
];

// Every XML part is checked, wherever it lives: the engine follows relationship
// targets to any path and also parses formulas outside worksheets (defined names,
// conditional-format rules, validations). Parts that are not XML are left to the engine.
pub fn check_part(bytes: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let mut found = Vec::new();
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(found);
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let names: BTreeSet<&str> = start_tag_names(text.as_bytes())
        .filter_map(|name| std::str::from_utf8(name).ok())
        .collect();
    // Shared strings with phonetic runs, or a sheet without formulas, are judged by their tag
    // names alone, so a large part is never built into a tree for them.
    if !names.iter().any(|name| PARSED_ELEMENTS.contains(name)) {
        let kinds: BTreeSet<Unsupported> = names.into_iter().filter_map(element_kind).collect();
        return Ok(kinds.into_iter().collect());
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
    let data_bar_ids = extended_rules::data_bar_ids(&document);
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        if matches!(
            name,
            "f" | "formula" | "formula1" | "formula2" | "definedName"
        ) {
            check_formula(&node)?;
        }
        let dropped_rule = name == "cfRule" && extended_rules::is_dropped(node, &data_bar_ids);
        let placed = dropped_rule.then_some(Unsupported::ExtendedConditionalFormats);
        if let Some(kind) = element_kind(name).or(placed)
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
        "rPh" => Some(Unsupported::Phonetic),
        _ => None,
    }
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
    use zenkai_xlsx_reader::has_checked_element;

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
        assert!(has_checked_element(b"<c><f>1</f></c>"));
        assert!(has_checked_element(b"<x:c><x:f t='array'>1</x:f></x:c>"));
        assert!(has_checked_element(
            b"<f
 t=\"shared\">1</f>"
        ));
        assert!(has_checked_element(
            b"<definedNames><definedName name=\"a\">1</definedName>"
        ));
        assert!(has_checked_element("<ñ:f>1</ñ:f>".as_bytes()));
        assert!(has_checked_element(b"<f>1</f"));
        assert!(!has_checked_element(
            b"<row r=\"1\"><c r=\"A1\"><v>12</v></c><font/><fill/></row>"
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
                Unsupported::Scenarios,
                Unsupported::CellWatches
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

    // A phonetic run holds the furigana of a text, which IronCalc reads past and a save that
    // writes the strings again loses; the phonetic settings alone show nothing.
    #[test]
    fn phonetic_runs_are_reported_wherever_they_are() {
        let strings = r#"<sst><si><t>x</t><rPh sb="0" eb="1"><t>y</t></rPh></si></sst>"#;
        assert_eq!(kinds(strings), [Unsupported::Phonetic]);
        let inline = sheet(
            r#"<c r="A1" t="inlineStr"><is><t>x</t><rPh sb="0" eb="1"><t>y</t></rPh></is></c>"#,
        );
        assert_eq!(kinds(&inline), [Unsupported::Phonetic]);
        let settings = r#"<worksheet><sheetData/><phoneticPr fontId="0"/></worksheet>"#;
        assert!(kinds(settings).is_empty());
    }

    // Without a formula to check, the part is never parsed, so even a damaged one is judged.
    #[test]
    fn a_part_without_formulas_is_judged_by_its_tag_names() {
        assert_eq!(
            kinds(r#"<sst><si><t>x</t><rPh sb="0" eb="1"><t>y</t>"#),
            [Unsupported::Phonetic]
        );
        assert!(kinds("<sst><si><t>plain</t></si></sst>").is_empty());
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

    // IronCalc reads an expression from the extension list and writes it back; the rest of
    // what it reads there is in `extended_rules`.
    #[test]
    fn reports_extended_rules_ironcalc_does_not_read() {
        assert_eq!(
            kinds(&EXTENDED_RULE.replace("TYPE", "cellIs")),
            [Unsupported::ExtendedConditionalFormats]
        );
        assert!(kinds(&EXTENDED_RULE.replace("TYPE", "expression")).is_empty());
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
