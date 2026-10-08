use zenkai_types::Range;

use crate::error::EngineError;

pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
// Excel's own limit; it also bounds how deep any formula's syntax tree can get.
pub const MAX_FORMULA_CHARS: usize = 8_192;
pub const MAX_FORMULA_DEPTH: usize = 256;
pub const MAX_FORMULA_AREA: u64 = 1_000_000;
// Engine parsing and evaluation recurse on the formula tree; a dedicated stack
// this large keeps any formula within MAX_FORMULA_CHARS far from overflowing.
pub const ENGINE_STACK_BYTES: usize = 256 * 1024 * 1024;

fn reject(reason: String) -> EngineError {
    EngineError::InvalidFile(reason)
}

pub fn check_worksheet(xml: &[u8]) -> Result<SheetFeatures, EngineError> {
    let text =
        std::str::from_utf8(xml).map_err(|e| reject(format!("worksheet is not UTF-8: {e}")))?;
    let document = roxmltree::Document::parse(text)
        .map_err(|e| reject(format!("worksheet XML is malformed: {e}")))?;
    let mut features = SheetFeatures::default();
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        match node.tag_name().name() {
            "f" => check_formula(&node)?,
            "conditionalFormatting" => features.conditional_formatting = true,
            "dataValidations" => features.data_validation = true,
            _ => {}
        }
    }
    Ok(features)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SheetFeatures {
    pub conditional_formatting: bool,
    pub data_validation: bool,
}

fn check_formula(node: &roxmltree::Node<'_, '_>) -> Result<(), EngineError> {
    if let Some(area) = node.attribute("ref")
        && let Some(range) = Range::parse_a1(area)
        && range.cell_count() > MAX_FORMULA_AREA
    {
        return Err(reject(format!(
            "a formula covers {} cells, more than the {MAX_FORMULA_AREA} supported",
            range.cell_count()
        )));
    }
    let formula = node.text().unwrap_or_default();
    let chars = formula.chars().count();
    if chars > MAX_FORMULA_CHARS {
        return Err(reject(format!(
            "a formula has {chars} characters, more than the {MAX_FORMULA_CHARS} Excel allows"
        )));
    }
    let depth = max_depth(formula);
    if depth > MAX_FORMULA_DEPTH {
        return Err(reject(format!(
            "a formula is nested {depth} levels deep, more than the {MAX_FORMULA_DEPTH} supported"
        )));
    }
    Ok(())
}

fn max_depth(formula: &str) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    for c in formula.chars() {
        match c {
            '(' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
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
        assert!(check_worksheet(deep.as_bytes()).is_err());
        let encoded = sheet(&format!("<c r=\"A1\"><f>{}1</f></c>", "&#40;".repeat(300)));
        assert!(check_worksheet(encoded.as_bytes()).is_err());
        let prefixed = format!(
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData><x:row r="1"><x:c r="A1"><x:f>{}1</x:f></x:c></x:row></x:sheetData></x:worksheet>"#,
            "(".repeat(300)
        );
        assert!(check_worksheet(prefixed.as_bytes()).is_err());
        let fine = sheet(&format!(
            "<c r=\"A1\"><f>{}1{}</f></c>",
            "(".repeat(64),
            ")".repeat(64)
        ));
        assert!(check_worksheet(fine.as_bytes()).is_ok());
    }

    #[test]
    fn rejects_long_operator_chains_and_huge_areas() {
        let long = sheet(&format!("<c r=\"A1\"><f>{}1</f></c>", "1+".repeat(5_000)));
        assert!(check_worksheet(long.as_bytes()).is_err());
        let huge = sheet("<c r=\"A1\"><f\n t='array' ref='A1:XFD1048576'>1</f></c>");
        assert!(check_worksheet(huge.as_bytes()).is_err());
        let small = sheet(r#"<c r="A1"><f t="shared" ref="A1:A100" si="0">B1*2</f></c>"#);
        assert!(check_worksheet(small.as_bytes()).is_ok());
    }

    #[test]
    fn reports_conditional_formatting_and_validation() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/><conditionalFormatting sqref="A1"/><dataValidations count="0"/></worksheet>"#;
        let features = check_worksheet(xml.as_bytes()).unwrap();
        assert!(features.conditional_formatting && features.data_validation);
    }

    #[test]
    fn engine_stack_survives_the_longest_allowed_chain() {
        let formula = format!("={}1", "1+".repeat((MAX_FORMULA_CHARS - 2) / 2));
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
}
