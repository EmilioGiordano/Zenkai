use crate::error::EngineError;
use crate::file::Unsupported;
use zenkai_xlsx_reader::has_checked_element;

pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
// Engine parsing and evaluation recurse on the formula tree; a dedicated stack
// this large keeps any formula within MAX_FORMULA_CHARS far from overflowing.
pub const ENGINE_STACK_BYTES: usize = 256 * 1024 * 1024;

fn reject(reason: String) -> EngineError {
    EngineError::Unsafe(reason)
}

// Every XML part is checked, wherever it lives: the engine follows relationship
// targets to any path and also parses formulas outside worksheets (defined names,
// conditional-format rules, validations). Parts that are not XML are left to the engine.
pub fn check_part(bytes: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let mut found = Vec::new();
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(found);
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !has_checked_element(text.as_bytes()) {
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
        if let Some(kind) = element_kind(name)
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
