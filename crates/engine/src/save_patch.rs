// A save that starts from the file on disk: every part the workbook did not change is
// copied without inflating it, and only the changed sheets are written again.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufWriter, Seek, Write};
use std::sync::Arc;

use ironcalc::export::get_shared_string_item_xml;
use zenkai_xlsx_reader::{BasePackage, SHARED_STRINGS_PART, Written, relationships_part};
use zip::ZipWriter;

use crate::error::EngineError;
use crate::file::Unsupported;
use crate::fingerprint::{DynamicArrays, SheetPrint};
use crate::held::Held;
use crate::save::{ModelData, SheetJob, deflated, rejected, sheet_job, write_sheet};
use crate::saved::{Contents, Metadata, Remains, SavedFile, Strings};
use crate::sheet_elements::Relationships;
use crate::sheet_settings::Carried;
use crate::table_headers::{headers_match, tables_on};

pub(crate) struct PatchPlan {
    pub(crate) base: Arc<SavedFile>,
    sheets: Vec<SheetJob>,
    // Whole <si> elements for the strings the model added since the file was written,
    // which held `strings_in_file`.
    strings: Vec<u8>,
    strings_in_file: usize,
}

// What the written file will hold, for the next save to copy from.
pub(crate) struct Patched {
    pub(crate) plan: PatchPlan,
    pub(crate) contents: Contents,
    // What the rewritten sheets held in the file and are written without.
    pub(crate) dropped: BTreeSet<Unsupported>,
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
    let mut held = saved.held.clone();
    let mut remains = saved.remains.clone();
    let mut dropped = BTreeSet::new();
    let mut sheets = Vec::with_capacity(changed.len());
    for index in changed {
        let part = saved.worksheets[index].clone();
        let in_file = remains.get(index).copied().unwrap_or(Remains::Nothing);
        let in_file = match tables_intact(data, &held, index) {
            true => in_file,
            false => in_file.unlinked(),
        };
        let job = sheet_job(data, carried, index, part, index == saved.selected, in_file)?;
        let written = job
            .settings
            .as_ref()
            .map(|settings| settings.elements.kinds())
            .unwrap_or_default();
        let relationships = job.relationships();
        dropped.extend(held.rewrite(index, written, relationships));
        if let Some(slot) = remains.get_mut(index) {
            *slot = in_file.rewritten(relationships);
        }
        sheets.push(job);
    }
    let contents = Contents {
        worksheets: saved.worksheets.clone(),
        strings: written_strings,
        styles: saved.styles,
        book,
        sheets: prints.to_vec(),
        selected: saved.selected,
        metadata: saved.metadata,
        held,
        remains,
        losses_through: carried.losses(),
    };
    Ok(Some(Patched {
        plan: PatchPlan {
            base,
            sheets,
            strings,
            strings_in_file,
        },
        contents,
        dropped,
    }))
}

// A sheet keeps its relationships only while each of its tables can be checked and its
// header cells still hold the column names the table part holds.
fn tables_intact(data: ModelData<'_>, held: &Held, index: usize) -> bool {
    let Some(sheet) = data.book.worksheets.get(index) else {
        return false;
    };
    // A table part IronCalc did not read cannot be checked.
    if held.links_to(index, Unsupported::Tables) && tables_on(data.book, sheet).is_empty() {
        return false;
    }
    headers_match(data.book, sheet)
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
    // A rewritten sheet written without the elements that point into its relationships
    // part leaves the part out; the parts it pointed at stay, unreferenced.
    let rels: HashSet<String> = plan
        .sheets
        .iter()
        .filter(|job| job.relationships() == Relationships::Dropped)
        .map(|job| relationships_part(&job.part))
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
