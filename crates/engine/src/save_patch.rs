// A save that starts from the file on disk: every part the workbook did not change is
// copied without inflating it, and only the changed sheets, or their changed rows, are
// written again.

use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Seek, Write};
use std::sync::Arc;

use ironcalc::export::get_shared_string_item_xml;
use zenkai_xlsx_reader::{BasePackage, SHARED_STRINGS_PART, Written};
use zip::ZipWriter;

use crate::error::EngineError;
use crate::fingerprint::{DynamicArrays, SheetPrint};
use crate::save::{ModelData, SheetJob, deflated, rejected, sheet_job, write_sheet};
use crate::save_rows::{RowJob, dirty_blocks, row_job, write_rows};
use crate::saved::{Contents, Metadata, SavedFile, Strings};
use crate::sheet_settings::Carried;

enum SheetPatch {
    Whole(SheetJob),
    Rows(RowJob),
}

impl SheetPatch {
    fn part(&self) -> &str {
        match self {
            SheetPatch::Whole(job) => &job.part,
            SheetPatch::Rows(job) => &job.part,
        }
    }
}

pub(crate) struct PatchPlan {
    pub(crate) base: Arc<SavedFile>,
    sheets: Vec<SheetPatch>,
    // Whole <si> elements for the strings the model added since the file was written,
    // which held `strings_in_file`.
    strings: Vec<u8>,
    strings_in_file: usize,
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
        .filter(|&index| prints[index].changed_since(&saved.sheets[index]))
        .collect();
    let new_arrays = changed
        .iter()
        .any(|&index| prints[index].dynamic_arrays() == DynamicArrays::Present);
    if new_arrays && saved.metadata == Metadata::Unknown {
        return Ok(None);
    }
    let all_strings = &data.book.shared_strings;
    let strings_in_file = match saved.strings {
        Strings::Aligned(count) => count,
        Strings::Absent => 0,
    };
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
        .map(|&index| -> Result<SheetPatch, EngineError> {
            let part = saved.worksheets[index].clone();
            Ok(match dirty_blocks(&saved.sheets[index], &prints[index]) {
                Some(blocks) => SheetPatch::Rows(row_job(data, carried, index, part, blocks)?),
                None => SheetPatch::Whole(sheet_job(
                    data,
                    carried,
                    index,
                    part,
                    index == saved.selected,
                )?),
            })
        })
        .collect::<Result<_, _>>()?;
    let contents = Contents {
        worksheets: saved.worksheets.clone(),
        strings: written_strings,
        styles: saved.styles,
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
            strings_in_file,
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

fn base_unusable(base: &SavedFile) -> impl Fn(&dyn std::fmt::Display) -> EngineError + '_ {
    |reason| EngineError::BaseUnusable {
        path: base.path.clone(),
        reason: reason.to_string(),
    }
}

// A file can change on disk and keep its length and modification time; what the rewritten
// sheets point into is read again before anything is copied.
fn check_facts(base: &SavedFile, package: &mut BasePackage) -> Result<(), EngineError> {
    let unusable = base_unusable(base);
    let facts = package.facts().map_err(|e| unusable(&e))?;
    let saved = &base.contents;
    if facts.worksheets != saved.worksheets || facts.styles != saved.styles {
        return Err(unusable(
            &"its sheets or styles are not the ones it was read with",
        ));
    }
    Ok(())
}

fn check_strings(
    base: &SavedFile,
    package: &mut BasePackage,
    names: &[String],
) -> Result<(), EngineError> {
    let unusable = base_unusable(base);
    let present = names.iter().any(|name| name == SHARED_STRINGS_PART);
    let same = match base.contents.strings {
        Strings::Absent => !present,
        Strings::Aligned(count) => {
            present && package.shared_string_count().map_err(|e| unusable(&e))? == Some(count)
        }
    };
    if !same {
        return Err(unusable(
            &"its shared strings are not the ones it was read with",
        ));
    }
    Ok(())
}

// Returns what was written, part by part, for the verification.
pub(crate) fn write_patch<W: Write + Seek>(
    plan: PatchPlan,
    writer: W,
) -> Result<(W, Vec<(String, Written)>), EngineError> {
    let saved = plan.base.as_ref();
    let read_failed = base_unusable(saved);
    let read_failed = |error: zenkai_xlsx_reader::ReadError| read_failed(&error);
    let copy_failed = base_unusable(saved);
    let copy_failed = |error: std::io::Error| copy_failed(&error);
    let strings_in_file = plan.strings_in_file;
    let mut base = BasePackage::open(&saved.path).map_err(read_failed)?;
    let names = base.part_names().map_err(read_failed)?;
    check_facts(saved, &mut base)?;
    // Appending checks the count as it copies.
    if !plan.sheets.is_empty() && plan.strings.is_empty() {
        check_strings(saved, &mut base, &names)?;
    }
    // A sheet written row by row keeps the part around its rows, and with it what its
    // relationships point at.
    let rels: HashSet<String> = plan
        .sheets
        .iter()
        .filter_map(|job| match job {
            SheetPatch::Whole(job) => Some(sheet_rels(&job.part)),
            SheetPatch::Rows(_) => None,
        })
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
    let mut sheets: HashMap<String, SheetPatch> = plan
        .sheets
        .into_iter()
        .map(|job| (job.part().to_string(), job))
        .collect();
    let mut strings_left = plan.strings;
    let mut zip = ZipWriter::new(writer);
    let mut written = Vec::with_capacity(names.len());
    for (index, name) in names.into_iter().enumerate() {
        if let Some(job) = sheets.remove(&name) {
            match job {
                SheetPatch::Whole(job) => write_sheet(&mut zip, job)?,
                SheetPatch::Rows(job) => {
                    zip.start_file(name.as_str(), deflated())
                        .map_err(rejected)?;
                    write_rows(&mut base, &mut zip, &job).map_err(copy_failed)?;
                }
            }
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
            base.copy_shared_strings(
                strings_in_file,
                &std::mem::take(&mut strings_left),
                &mut buffered,
            )
            .map_err(copy_failed)?;
            buffered.flush().map_err(rejected)?;
        } else {
            base.copy_raw(index, &mut zip).map_err(copy_failed)?;
            written.push((name, Written::Copied));
            continue;
        }
        written.push((name, Written::Generated));
    }
    if !sheets.is_empty() || !strings_left.is_empty() {
        return Err(base_unusable(saved)(&"a part is missing from it"));
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
