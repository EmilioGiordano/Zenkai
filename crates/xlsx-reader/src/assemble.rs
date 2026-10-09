use std::collections::HashMap;

use ironcalc::base::types::{Cell, Workbook};

use crate::ReadError;
use crate::formulas::Converted;
use crate::sheet_xml::{FormulaEvent, SheetCells};

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
    sheets: Vec<SheetCells<'_>>,
    converted: Converted,
) -> Result<(), ReadError> {
    let mut string_index: HashMap<String, i32> =
        HashMap::with_capacity(workbook.shared_strings.len());
    for (position, text) in workbook.shared_strings.iter().enumerate() {
        string_index.entry(text.clone()).or_insert(index(position)?);
    }
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
        for text in cells.strings {
            let next = index(workbook.shared_strings.len())?;
            let global = match string_index.get(&text) {
                Some(global) => *global,
                None => {
                    workbook.shared_strings.push(text.clone());
                    string_index.insert(text, next);
                    next
                }
            };
            local.push(global);
        }
        let (shared_formulas, resolved) =
            resolve_formulas(&cells.events, ids, &converted.distinct)?;

        let mut data = cells.data;
        for slot in cells.string_cells {
            let Some((_, Cell::SharedString { si, .. })) = data
                .get_mut(slot.row)
                .and_then(|(_, row)| row.get_mut(slot.cell))
            else {
                return Err(unsupported("a lost string"));
            };
            *si = *usize::try_from(*si)
                .ok()
                .and_then(|local_index| local.get(local_index))
                .ok_or_else(|| unsupported("a lost string"))?;
        }
        for (_, row) in &mut data {
            for (_, cell) in row {
                if let Cell::CellFormula { f, .. } | Cell::ArrayFormula { f, .. } = cell {
                    *f = *usize::try_from(*f)
                        .ok()
                        .and_then(|event| resolved.get(event))
                        .ok_or_else(|| unsupported("a lost formula"))?;
                }
            }
        }
        worksheet.sheet_data = data
            .into_iter()
            .map(|(row, cells)| (row, cells.into_iter().collect()))
            .collect();
        worksheet.rows = cells.rows;
        worksheet.shared_formulas = shared_formulas;
    }
    Ok(())
}

// IronCalc keeps one list of formulas per sheet and reuses an entry with the same text. A
// shared formula's child met before its anchor gets an empty placeholder; the anchor then
// inserts before it and shifts every later index, which this reader leaves to IronCalc.
fn resolve_formulas(
    events: &[FormulaEvent],
    ids: &[u32],
    distinct: &[String],
) -> Result<(Vec<String>, Vec<i32>), ReadError> {
    let mut formulas: Vec<String> = Vec::new();
    let mut first: HashMap<&str, i32> = HashMap::new();
    let mut shared: HashMap<i32, i32> = HashMap::new();
    let mut resolved = Vec::with_capacity(events.len());
    let text_of = |job: usize| -> Result<&str, ReadError> {
        ids.get(job)
            .and_then(|id| distinct.get(*id as usize))
            .map(String::as_str)
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
