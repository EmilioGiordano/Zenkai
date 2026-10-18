// A save that starts from the file on disk: every part the workbook did not change is
// copied without inflating it, and only the changed sheets, or their changed rows, are
// written again.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufWriter, Seek, Write};
use std::sync::Arc;

use ironcalc::export::get_shared_string_item_xml;
use zenkai_xlsx_reader::{
    BasePackage, SHARED_STRINGS_PART, STYLES_PART, StyleAppendix, Written, is_cannot_splice,
    relationships_part,
};
use zip::ZipWriter;

use crate::error::EngineError;
use crate::file::Unsupported;
use crate::fingerprint::{DynamicArrays, SheetPrint};
use crate::held::Held;
use crate::save::{ModelData, SheetJob, deflated, rejected, sheet_job, write_sheet};
use crate::save_rows::{RowJob, dirty_blocks, row_job, write_rows};
use crate::saved::{Contents, Metadata, SavedFile, Strings};
use crate::sheet_elements::Relationships;
use crate::sheet_settings::Carried;
use crate::table_headers::{headers_match, tables_on};

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
    // The styles the edits created, which go at the end of the file's styles part.
    styles: Option<Box<StyleAppendix>>,
}

// What the written file will hold, for the next save to copy from.
pub(crate) struct Patched {
    pub(crate) plan: PatchPlan,
    pub(crate) contents: Contents,
    // What the rewritten sheets held in the file and are written without.
    pub(crate) dropped: BTreeSet<Unsupported>,
}

// None when the workbook changed in a way only a full save writes: the sheet list, the
// names, a style the file already holds, or a dynamic array in a file that may lack the
// metadata part.
pub(crate) fn patch_plan(
    base: Arc<SavedFile>,
    book: u64,
    prints: &[SheetPrint],
    data: ModelData<'_>,
    carried: &Carried,
) -> Result<Option<Patched>, EngineError> {
    let saved = &base.contents;
    if saved.book != book || saved.sheets.len() != prints.len() {
        tracing::info!("the sheet list or the names changed; the save writes everything");
        return Ok(None);
    }
    if !saved.styles.unchanged(&data.book.styles)? {
        tracing::info!("a style the file holds changed; the save writes everything");
        return Ok(None);
    }
    let changed: Vec<usize> = (0..prints.len())
        .filter(|&index| prints[index].changed_since(&saved.sheets[index]))
        .collect();
    let new_arrays = changed
        .iter()
        .any(|&index| prints[index].dynamic_arrays() == DynamicArrays::Present);
    if new_arrays && saved.metadata == Metadata::Unknown {
        tracing::info!("a changed sheet holds a dynamic array; the save writes everything");
        return Ok(None);
    }
    tracing::info!(
        changed = changed.len(),
        sheets = prints.len(),
        "the save copies the sheets that did not change"
    );
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
            None => {
                tracing::info!("the shared strings shrank; the save writes everything");
                return Ok(None);
            }
        },
        Strings::Absent if all_strings.is_empty() => (Vec::new(), Strings::Absent),
        Strings::Absent => {
            tracing::info!("the file has no shared strings part; the save writes everything");
            return Ok(None);
        }
    };
    let (styles, appendix) = match changed.is_empty() {
        true => (saved.styles, None),
        false => saved.styles.grown(&data.book.styles)?,
    };
    let mut held = saved.held.clone();
    let mut remains = saved.remains.clone();
    let mut dropped = BTreeSet::new();
    // A sheet copied from the file holds what its remains say, which an undo since may
    // have put back into the model without the file holding it.
    let copied = data
        .book
        .worksheets
        .iter()
        .zip(&remains)
        .enumerate()
        .filter(|(index, _)| changed.binary_search(index).is_err());
    for (_, (sheet, &in_file)) in copied {
        dropped.extend(carried.unwritable_restored(sheet, in_file));
    }
    let mut sheets = Vec::with_capacity(changed.len());
    for index in changed {
        let (Some(part), Some(&in_file)) = (saved.worksheets.get(index), remains.get(index)) else {
            return Err(rejected("a changed sheet is not in the saved file"));
        };
        let part = part.clone();
        let intact = tables_intact(data, &held, index);
        // A sheet written row by row keeps everything the file holds around its rows, which
        // is only right while its tables still match their headers.
        let blocks = if intact && !base.unspliceable.contains(&part) {
            dirty_blocks(&saved.sheets[index], &prints[index])
        } else {
            None
        };
        tracing::debug!(
            sheet = index,
            intact,
            dirty_blocks = blocks.as_ref().map(BTreeSet::len),
            "a changed sheet is written"
        );
        if let Some(blocks) = blocks {
            if let Some(sheet) = data.book.worksheets.get(index) {
                dropped.extend(carried.unwritable_restored(sheet, in_file));
            }
            sheets.push(SheetPatch::Rows(row_job(
                data,
                carried,
                index,
                part,
                blocks,
                &prints[index].bounds(),
            )?));
            continue;
        }
        let in_file = match intact {
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
        dropped.extend(held.rewrite(index, written, relationships)?);
        if let Some(sheet) = data.book.worksheets.get(index) {
            dropped.extend(carried.unwritable_restored(sheet, in_file));
        }
        *remains
            .get_mut(index)
            .ok_or_else(|| rejected("a changed sheet is not in the saved file"))? =
            in_file.rewritten(relationships);
        sheets.push(SheetPatch::Whole(job));
    }
    let contents = Contents {
        worksheets: saved.worksheets.clone(),
        strings: written_strings,
        styles,
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
            styles: appendix.map(Box::new),
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
    if facts.worksheets != saved.worksheets || facts.styles != saved.styles.counts {
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

// Returns what was written, part by part, for the verification. A sheet that cannot be
// spliced leaves its part in `refused`.
pub(crate) fn write_patch<W: Write + Seek>(
    plan: PatchPlan,
    writer: W,
    refused: &mut Option<String>,
) -> Result<(W, Vec<(String, Written)>), EngineError> {
    let saved = plan.base.as_ref();
    let read_failed = base_unusable(saved);
    let read_failed = |error: zenkai_xlsx_reader::ReadError| read_failed(&error);
    let copy_failed = base_unusable(saved);
    let copy_failed = |error: std::io::Error| copy_failed(&error);
    let strings_in_file = plan.strings_in_file;
    let checks = tracing::info_span!("save_base_checks").entered();
    let mut base = BasePackage::open(&saved.path).map_err(read_failed)?;
    let names = base.part_names().map_err(read_failed)?;
    check_facts(saved, &mut base)?;
    // Appending checks the count as it copies.
    if !plan.sheets.is_empty() && plan.strings.is_empty() {
        check_strings(saved, &mut base, &names)?;
    }
    // A rewritten sheet written without the elements that point into its relationships
    // part leaves the part out; the parts it pointed at stay, unreferenced. A sheet written
    // row by row keeps the part around its rows, and with it its relationships.
    let rels: HashSet<String> = plan
        .sheets
        .iter()
        .filter_map(|job| match job {
            SheetPatch::Whole(job) if job.relationships() == Relationships::Dropped => {
                Some(relationships_part(&job.part))
            }
            _ => None,
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
    drop(checks);
    let mut sheets: HashMap<String, SheetPatch> = plan
        .sheets
        .into_iter()
        .map(|job| (job.part().to_string(), job))
        .collect();
    let mut strings_left = plan.strings;
    let mut styles_left = plan.styles;
    let mut zip = ZipWriter::new(writer);
    let mut written = Vec::with_capacity(names.len());
    for (index, name) in names.into_iter().enumerate() {
        if let Some(job) = sheets.remove(&name) {
            match job {
                SheetPatch::Whole(job) => {
                    let _span =
                        tracing::debug_span!("save_part", kind = "whole", part = %name).entered();
                    write_sheet(&mut zip, job)?
                }
                SheetPatch::Rows(job) => {
                    let _span =
                        tracing::debug_span!("save_part", kind = "rows", part = %name).entered();
                    zip.start_file(name.as_str(), deflated())
                        .map_err(rejected)?;
                    if let Err(error) = write_rows(&mut base, &mut zip, &job) {
                        if is_cannot_splice(&error) {
                            *refused = Some(name);
                        }
                        return Err(copy_failed(error));
                    }
                }
            }
        } else if rels.contains(&name) || dropped.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
            continue;
        } else if let Some(bytes) = rewritten.remove(&name) {
            let _span = tracing::debug_span!("save_part", kind = "package", part = %name).entered();
            zip.start_file(name.as_str(), deflated())
                .map_err(rejected)?;
            zip.write_all(&bytes).map_err(rejected)?;
        } else if let Some(appendix) = styles_left.take_if(|_| name == STYLES_PART) {
            let _span = tracing::debug_span!("save_part", kind = "styles", part = %name).entered();
            zip.start_file(name.as_str(), deflated())
                .map_err(rejected)?;
            let mut buffered = BufWriter::new(&mut zip);
            base.copy_styles(saved.contents.styles.counts, &appendix, &mut buffered)
                .map_err(copy_failed)?;
            buffered.flush().map_err(rejected)?;
        } else if name == SHARED_STRINGS_PART && !strings_left.is_empty() {
            let _span = tracing::debug_span!("save_part", kind = "strings", part = %name).entered();
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
            let _span = tracing::debug_span!("save_part", kind = "copied", part = %name).entered();
            base.copy_raw(index, &mut zip).map_err(copy_failed)?;
            written.push((name, Written::Copied));
            continue;
        }
        written.push((name, Written::Generated));
    }
    if !sheets.is_empty() || !strings_left.is_empty() || styles_left.is_some() {
        return Err(base_unusable(saved)(&"a part is missing from it"));
    }
    let finished = tracing::info_span!("save_zip_finish").in_scope(|| zip.finish());
    Ok((finished.map_err(rejected)?, written))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ironcalc::base::Model;

    use super::patch_plan;
    use crate::error::EngineError;
    use crate::fingerprint::book_print;
    use crate::held::Held;
    use crate::save::ModelData;
    use crate::saved::{Contents, Metadata, Remains, SavedFile, Stamp, Strings, sheet_prints};
    use crate::sheet_settings::Carried;
    use crate::style_growth::StylesBase;

    #[test]
    fn a_changed_sheet_missing_from_the_saved_parts_fails_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        std::fs::write(&path, b"").unwrap();
        let carried = Carried::default();
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        let sheets = sheet_prints(ModelData::of(&model), &carried).unwrap();
        model.set_user_input(0, 1, 1, "5".to_string()).unwrap();
        model.evaluate();
        let data = ModelData::of(&model);
        let book = book_print(data.book).unwrap();
        let base = SavedFile {
            path: path.clone(),
            stamp: Stamp::of(&path).unwrap(),
            contents: Contents {
                worksheets: Vec::new(),
                strings: Strings::Aligned(data.book.shared_strings.len()),
                styles: StylesBase::written(&data.book.styles).unwrap(),
                book,
                sheets,
                selected: 0,
                metadata: Metadata::Written,
                held: Held::written_whole(1),
                remains: vec![Remains::All],
                losses_through: 0,
            },
            unspliceable: Vec::new(),
        };
        let prints = sheet_prints(data, &carried).unwrap();
        let Err(EngineError::Rejected(reason)) =
            patch_plan(Arc::new(base), book, &prints, data, &carried)
        else {
            panic!("a plan for a sheet the saved file lacks was not rejected");
        };
        assert!(reason.contains("not in the saved file"), "{reason}");
    }
}
