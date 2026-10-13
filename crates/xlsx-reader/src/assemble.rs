use std::collections::HashMap;
use std::sync::Arc;

use ironcalc::base::types::Workbook;

use crate::ReadError;
use crate::formulas::Converted;
use crate::sheet_data::{FormulaEvent, SheetCells, remap_strings, unreadable};

fn unsupported(what: &str) -> ReadError {
    ReadError::Unsupported(what.to_string())
}

fn index(value: usize) -> Result<i32, ReadError> {
    i32::try_from(value).map_err(|_| unsupported("more entries than an index can hold"))
}

// Puts the cells into the workbook IronCalc built from the stub, numbering strings and
// formulas in document order, sheet after sheet, exactly as its importer does.
pub(crate) fn fill(
    workbook: &mut Workbook,
    sheets: Vec<SheetCells>,
    converted: Converted,
) -> Result<(), ReadError> {
    // The workbook's own strings are only borrowed, never cloned: fresh strings are
    // collected per sheet and appended once, after every lookup is done.
    let base = index(workbook.shared_strings.len())?;
    let mut existing: HashMap<&str, i32> = HashMap::with_capacity(workbook.shared_strings.len());
    for (position, text) in workbook.shared_strings.iter().enumerate() {
        existing.entry(text.as_str()).or_insert(index(position)?);
    }
    let mut fresh: HashMap<Arc<str>, i32> = HashMap::new();
    let mut fresh_order: Vec<Arc<str>> = Vec::new();
    for ((worksheet, cells), ids) in workbook
        .worksheets
        .iter_mut()
        .zip(sheets)
        .zip(&converted.per_sheet)
    {
        if !worksheet.sheet_data.is_empty()
            || !worksheet.rows.is_empty()
            || !worksheet.shared_formulas.is_empty()
        {
            return Err(unsupported("cells outside sheetData"));
        }
        let mut local = Vec::with_capacity(cells.strings.len());
        for text in &cells.strings {
            let found: &str = text;
            let global = match existing.get(found) {
                Some(global) => *global,
                None => match fresh.get(found) {
                    Some(global) => *global,
                    None => {
                        let global = base
                            .checked_add(index(fresh_order.len())?)
                            .ok_or_else(|| unreadable("too many strings"))?;
                        let shared: Arc<str> = Arc::from(found);
                        fresh_order.push(shared.clone());
                        fresh.insert(shared, global);
                        global
                    }
                },
            };
            local.push(global);
        }
        let (shared_formulas, resolved) =
            resolve_formulas(&cells.events, ids, &converted.distinct)?;

        let mut data = cells.data;
        remap_strings(&mut data, &cells.string_cells, &local)?;
        for (_, row) in &mut data {
            for (_, cell) in row {
                if let Some(f) = cell.formula_mut() {
                    *f = *usize::try_from(*f)
                        .ok()
                        .and_then(|event| resolved.get(event))
                        .ok_or_else(|| unsupported("a lost formula"))?;
                }
            }
        }
        for (row, cells) in data {
            worksheet.sheet_data.insert_row(row, cells);
        }
        worksheet.rows = cells.rows;
        worksheet.shared_formulas = shared_formulas;
    }
    workbook
        .shared_strings
        .extend(fresh_order.into_iter().map(|text| text.to_string()));
    Ok(())
}

// IronCalc keeps one list of formulas per sheet and reuses an entry with the same text. A
// shared formula's child met before its anchor gets an empty placeholder; the anchor then
// inserts before it and shifts every later index, which this reader leaves to IronCalc.
fn resolve_formulas(
    events: &[FormulaEvent],
    ids: &[u32],
    distinct: &[Arc<str>],
) -> Result<(Vec<String>, Vec<i32>), ReadError> {
    let mut formulas: Vec<String> = Vec::new();
    let mut first: HashMap<&str, i32> = HashMap::new();
    let mut shared: HashMap<i32, i32> = HashMap::new();
    let mut resolved = Vec::with_capacity(events.len());
    let text_of = |job: usize| -> Result<&str, ReadError> {
        ids.get(job)
            .and_then(|id| distinct.get(*id as usize))
            .map(|text| &text[..])
            .ok_or_else(|| unsupported("a lost formula"))
    };
    for event in events {
        let found = match event {
            FormulaEvent::Convert { job } => {
                let text = text_of(*job)?;
                add_once(&mut formulas, &mut first, text)?
            }
            FormulaEvent::SharedAnchor { si, job } => {
                if shared.contains_key(si) {
                    return Err(unsupported("a shared formula anchor after its children"));
                }
                let text = text_of(*job)?;
                let found = add_once(&mut formulas, &mut first, text)?;
                shared.insert(*si, found);
                found
            }
            FormulaEvent::SharedChild { si } => match shared.get(si) {
                Some(found) => *found,
                None => {
                    let placeholder = index(formulas.len())?;
                    formulas.push(String::new());
                    first.entry("").or_insert(placeholder);
                    shared.insert(*si, placeholder);
                    placeholder
                }
            },
        };
        resolved.push(found);
    }
    Ok((formulas, resolved))
}

fn add_once<'t>(
    formulas: &mut Vec<String>,
    first: &mut HashMap<&'t str, i32>,
    text: &'t str,
) -> Result<i32, ReadError> {
    if let Some(found) = first.get(text) {
        return Ok(*found);
    }
    let found = index(formulas.len())?;
    formulas.push(text.to_string());
    first.insert(text, found);
    Ok(found)
}
