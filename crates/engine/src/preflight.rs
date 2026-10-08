use zenkai_types::Range;

use crate::error::EngineError;

pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
// Excel limits formulas to 8192 characters and 64 nested functions; parentheses
// deeper than this are not a real workbook and overflow the engine's parser stack.
pub const MAX_FORMULA_DEPTH: usize = 256;
pub const MAX_FORMULA_AREA: u64 = 1_000_000;

fn reject(reason: String) -> EngineError {
    EngineError::InvalidFile(reason)
}

pub fn check_formulas(xml: &[u8]) -> Result<(), EngineError> {
    let mut rest = xml;
    while let Some(start) = find(rest, b"<f") {
        let after = &rest[start + 2..];
        let is_formula_tag = matches!(after.first(), Some(b' ' | b'>' | b'/'));
        if !is_formula_tag {
            rest = after;
            continue;
        }
        let Some(tag_end) = find(after, b">") else {
            return Ok(());
        };
        let tag = &after[..tag_end];
        if let Some(area) = attribute(tag, b"ref=\"")
            && let Some(range) = std::str::from_utf8(area).ok().and_then(Range::parse_a1)
            && range.cell_count() > MAX_FORMULA_AREA
        {
            return Err(reject(format!(
                "a formula covers {} cells, more than the {MAX_FORMULA_AREA} supported",
                range.cell_count()
            )));
        }
        let body = &after[tag_end + 1..];
        let body_end = find(body, b"</f>").unwrap_or(body.len());
        let depth = max_depth(&body[..body_end]);
        if depth > MAX_FORMULA_DEPTH {
            return Err(reject(format!(
                "a formula is nested {depth} levels deep, more than the {MAX_FORMULA_DEPTH} supported"
            )));
        }
        rest = &body[body_end..];
    }
    Ok(())
}

fn attribute<'a>(tag: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let start = find(tag, prefix)? + prefix.len();
    let value = &tag[start..];
    let end = find(value, b"\"")?;
    Some(&value[..end])
}

fn max_depth(formula: &[u8]) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    for byte in formula {
        match byte {
            b'(' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut offset = 0;
    while let Some(pos) = haystack[offset..].iter().position(|b| *b == first) {
        let at = offset + pos;
        if haystack[at..].starts_with(needle) {
            return Some(at);
        }
        offset = at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_deep_nesting() {
        let deep = format!(
            "<c r=\"A1\"><f>{}1{}</f></c>",
            "(".repeat(300),
            ")".repeat(300)
        );
        assert!(check_formulas(deep.as_bytes()).is_err());
        let fine = format!(
            "<c r=\"A1\"><f>{}1{}</f></c>",
            "(".repeat(64),
            ")".repeat(64)
        );
        assert!(check_formulas(fine.as_bytes()).is_ok());
    }

    #[test]
    fn rejects_huge_array_areas() {
        let huge = b"<c r=\"A1\"><f t=\"array\" ref=\"A1:XFD1048576\">1</f></c>";
        assert!(check_formulas(huge).is_err());
        let small = b"<c r=\"A1\"><f t=\"shared\" ref=\"A1:A100\" si=\"0\">B1*2</f></c>";
        assert!(check_formulas(small).is_ok());
    }

    #[test]
    fn ignores_text_that_looks_like_tags() {
        let xml = b"<c><v>((((</v></c><font/><f>SUM(A1)</f>";
        assert!(check_formulas(xml).is_ok());
    }
}
