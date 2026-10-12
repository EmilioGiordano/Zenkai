use std::io::{Read, Seek};

use zenkai_types::Range;

use crate::ReadError;

pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
// Excel's own limit; it also bounds how deep any formula's syntax tree can get.
pub const MAX_FORMULA_CHARS: usize = 8_192;
pub const MAX_FORMULA_DEPTH: usize = 256;
pub const MAX_FORMULA_AREA: u64 = 1_000_000;
// Together the array formulas of one sheet may claim this many cells: each claimed cell costs a
// map entry (about 40 bytes), so 4 million is roughly 160 MB.
pub const MAX_ARRAY_CELLS_PER_SHEET: u64 = 4_000_000;

pub fn check_archive<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Result<(), ReadError> {
    if archive.len() > MAX_ENTRIES {
        return Err(ReadError::Unsafe(format!(
            "{} entries in the archive",
            archive.len()
        )));
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Unreadable(e.to_string()))?;
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(ReadError::Unsafe(format!(
                "part {} expands to {} MB",
                entry.name(),
                entry.size() / 1024 / 1024
            )));
        }
        total = total.saturating_add(entry.size());
    }
    if total > MAX_TOTAL_BYTES {
        return Err(ReadError::Unsafe(format!(
            "the workbook expands to {} MB",
            total / 1024 / 1024
        )));
    }
    Ok(())
}

pub fn check_formula(formula: &str, area: Option<&str>) -> Result<(), String> {
    if let Some(range) = area.and_then(Range::parse_a1)
        && range.cell_count() > MAX_FORMULA_AREA
    {
        return Err(format!(
            "a formula covers {} cells, more than the {MAX_FORMULA_AREA} supported",
            range.cell_count()
        ));
    }
    check_formula_text(formula)
}

pub fn check_formula_text(formula: &str) -> Result<(), String> {
    let chars = formula.chars().count();
    if chars > MAX_FORMULA_CHARS {
        return Err(format!(
            "a formula has {chars} characters, more than the {MAX_FORMULA_CHARS} Excel allows"
        ));
    }
    let depth = max_depth(formula);
    if depth > MAX_FORMULA_DEPTH {
        return Err(format!(
            "a formula is nested {depth} levels deep, more than the {MAX_FORMULA_DEPTH} supported"
        ));
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
