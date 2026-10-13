// A save that starts from the file on disk: every part the workbook did not change is
// copied without inflating it, and only the changed sheets are written again.

use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Seek, Write};
use std::sync::Arc;

use ironcalc::export::get_shared_string_item_xml;
use zenkai_xlsx_reader::{BasePackage, SHARED_STRINGS_PART, Written};
use zip::ZipWriter;

use crate::error::EngineError;
use crate::fingerprint::{DynamicArrays, SheetPrint};
use crate::save::{ModelData, SheetJob, deflated, rejected, sheet_job, write_sheet};
use crate::saved::{Contents, Metadata, SavedFile, Strings};
use crate::sheet_settings::Carried;

pub(crate) struct PatchPlan {
    pub(crate) base: Arc<SavedFile>,
    sheets: Vec<SheetJob>,
    // Whole <si> elements for the strings the model added since the file was written.
    strings: Vec<u8>,
}

// What the written file will hold, for the next save to copy from.
pub(crate) struct Patched {
    pub(crate) plan: PatchPlan,
    pub(crate) contents: Contents,
}

// None when the workbook changed in a way only a full save writes: the sheet list, the
// names or the styles, or a dynamic array in a file that may lack the metadata part.
pub(crate) fn patch_plan(
    base: Arc<SavedFile>,
    book: u64,
    prints: &[SheetPrint],
    data: ModelData<'_>,
    carried: &Carried,
) -> Result<Option<Patched>, EngineError> {
    let saved = &base.contents;
    if saved.book != book || saved.sheets.len() != prints.len() {
        return Ok(None);
    }
    let changed: Vec<usize> = (0..prints.len())
        .filter(|&index| prints[index] != saved.sheets[index])
        .collect();
    let new_arrays = changed
        .iter()
        .any(|&index| prints[index].dynamic_arrays() == DynamicArrays::Present);
    if new_arrays && saved.metadata == Metadata::Unknown {
        return Ok(None);
    }
    let all_strings = &data.book.shared_strings;
    let (strings, written_strings) = match saved.strings {
        _ if changed.is_empty() => (Vec::new(), saved.strings),
        Strings::Aligned(count) => match all_strings.get(count..) {
            Some(added) => (
                added
                    .iter()
                    .map(|text| get_shared_string_item_xml(text))
                    .collect::<String>()
                    .into_bytes(),
                Strings::Aligned(all_strings.len()),
            ),
            None => return Ok(None),
        },
        Strings::Absent if all_strings.is_empty() => (Vec::new(), Strings::Absent),
        Strings::Absent => return Ok(None),
    };
    let sheets = changed
        .iter()
        .map(|&index| {
            let part = saved.worksheets[index].clone();
            sheet_job(data, carried, index, part, index == saved.selected)
        })
        .collect::<Result<_, _>>()?;
    let contents = Contents {
        worksheets: saved.worksheets.clone(),
        strings: written_strings,
        book,
        sheets: prints.to_vec(),
        selected: saved.selected,
        metadata: saved.metadata,
    };
    Ok(Some(Patched {
        plan: PatchPlan {
            base,
            sheets,
            strings,
        },
        contents,
    }))
}

// The relationships of a rewritten sheet point at parts its new XML no longer refers to.
fn sheet_rels(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

fn read_failed(error: impl ToString) -> EngineError {
    rejected(format!("reading the saved file: {}", error.to_string()))
}

// Returns what was written, part by part, for the verification.
pub(crate) fn write_patch<W: Write + Seek>(
    plan: PatchPlan,
    writer: W,
) -> Result<(W, Vec<(String, Written)>), EngineError> {
    let mut base = BasePackage::open(&plan.base.path).map_err(read_failed)?;
    let names = base.part_names().map_err(read_failed)?;
    let rels: HashSet<String> = plan
        .sheets
        .iter()
        .map(|job| sheet_rels(&job.part))
        .collect();
    let chain = if plan.sheets.is_empty() {
        None
    } else {
        base.calc_chain().map_err(read_failed)?
    };
    let (dropped, mut rewritten) = match chain {
        Some(chain) => (chain.dropped, chain.rewritten.into_iter().collect()),
        None => (Vec::new(), HashMap::new()),
    };
    let mut sheets: HashMap<String, SheetJob> = plan
        .sheets
        .into_iter()
        .map(|job| (job.part.clone(), job))
        .collect();
    let mut strings_left = plan.strings;
    let mut zip = ZipWriter::new(writer);
    let mut written = Vec::with_capacity(names.len());
    for (index, name) in names.into_iter().enumerate() {
        if let Some(job) = sheets.remove(&name) {
            write_sheet(&mut zip, job)?;
        } else if rels.contains(&name) || dropped.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
            continue;
        } else if let Some(bytes) = rewritten.remove(&name) {
            zip.start_file(name.as_str(), deflated())
                .map_err(rejected)?;
            zip.write_all(&bytes).map_err(rejected)?;
        } else if name == SHARED_STRINGS_PART && !strings_left.is_empty() {
            zip.start_file(name.as_str(), deflated())
                .map_err(rejected)?;
            let mut buffered = BufWriter::new(&mut zip);
            base.copy_shared_strings(&std::mem::take(&mut strings_left), &mut buffered)
                .map_err(read_failed)?;
            buffered.flush().map_err(rejected)?;
        } else {
            base.copy_raw(index, &mut zip).map_err(read_failed)?;
            written.push((name, Written::Copied));
            continue;
        }
        written.push((name, Written::Generated));
    }
    if !sheets.is_empty() || !strings_left.is_empty() {
        return Err(read_failed("a part is missing from the saved file"));
    }
    Ok((zip.finish().map_err(rejected)?, written))
}

#[cfg(test)]
mod tests {
    use super::sheet_rels;

    #[test]
    fn a_sheet_s_relationships_sit_beside_it() {
        assert_eq!(
            sheet_rels("xl/worksheets/sheet3.xml"),
            "xl/worksheets/_rels/sheet3.xml.rels"
        );
    }
}
