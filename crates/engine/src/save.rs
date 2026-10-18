use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use ironcalc::base::Model;
use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::types::Worksheet;
use ironcalc::export::{get_package_parts, write_worksheet_xml};
use zenkai_xlsx_reader::Written;
use zip::ZipWriter;
use zip::write::FileOptions;

use crate::carried_moves::{Counts, Restored};
use crate::error::EngineError;
use crate::file::{Unsupported, remove_temp, temp_path};
use crate::fingerprint::{SheetPrint, book_print};
use crate::held::Held;
use crate::preflight::run_with_engine_stack;
use crate::save_patch::{PatchPlan, Patched, patch_plan, write_patch};
use crate::save_sheet::{SheetWriter, prepare};
use crate::saved::{
    Contents, Metadata, Remains, SavedFile, SavedSlot, Stamp, Strings, selected_sheet, sheet_prints,
};
use crate::sheet_copy::copied_sheet;
use crate::sheet_elements::Relationships;
use crate::sheet_settings::{Carried, SheetSettings};
use crate::style_growth::StylesBase;

// IronCalc's rows are written in many small pieces; the settings writer and the deflate
// stream take them in chunks this large.
pub(crate) const SHEET_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    // The workbook's own file: later saves copy the parts they do not change from it.
    Document,
    // A copy elsewhere, such as a recovery file, that leaves the workbook's file as it is.
    Copy,
}

pub(crate) struct SheetJob {
    pub(crate) part: String,
    pub(crate) sheet: Worksheet,
    pub(crate) formulas: Vec<(Node, StaticResult)>,
    pub(crate) settings: Option<SheetSettings>,
    pub(crate) selected: bool,
}

impl SheetJob {
    // Whether the sheet's relationships part goes with it: only then are the elements
    // pointing into it written.
    pub(crate) fn relationships(&self) -> Relationships {
        self.settings
            .as_ref()
            .map_or(Relationships::Dropped, |s| s.elements.relationships())
    }
}

// Everything a full save writes, copied out of the model so the file is written without
// holding the workbook.
pub(crate) struct FullPlan {
    parts: Vec<(String, String)>,
    sheets: Vec<SheetJob>,
}

enum Plan {
    Full(FullPlan),
    Patch(PatchPlan),
}

pub struct SaveSnapshot {
    path: PathBuf,
    plan: Plan,
    slot: SavedSlot,
    // For a save to the workbook's own file, what the file will hold, so the next save
    // compares against what was written and never against edits made after the snapshot.
    adoption: Option<Contents>,
    // What the save warns about that an undo or redo put back; forgotten once it succeeds.
    restored: Restored,
    seen: Counts,
    fate: Fate,
}

impl SaveSnapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    // Relative to the workbook's own file, so a copy elsewhere reports what it lacks too.
    pub fn dropped(&self) -> &[Unsupported] {
        &self.fate.dropped
    }

    pub fn kept(&self) -> &[Unsupported] {
        &self.fate.kept
    }
}

struct Fate {
    dropped: Vec<Unsupported>,
    kept: Vec<Unsupported>,
}

impl Fate {
    fn full(held: BTreeSet<Unsupported>, lost: BTreeSet<Unsupported>) -> Fate {
        Fate {
            dropped: held.union(&lost).copied().collect(),
            kept: Vec::new(),
        }
    }
}

pub(crate) fn rejected(error: impl ToString) -> EngineError {
    EngineError::Rejected(format!("saving the workbook: {}", error.to_string()))
}

// The parts of the model a save copies. They are plain data, so the copy can run on the
// engine's stack: formulas nest deeply, and cloning or hashing them recurses.
#[derive(Clone, Copy)]
pub(crate) struct ModelData<'m> {
    pub(crate) book: &'m ironcalc::base::types::Workbook,
    pub(crate) formulas: &'m [Vec<(Node, StaticResult)>],
}

impl<'m> ModelData<'m> {
    pub(crate) fn of(model: &'m Model<'_>) -> ModelData<'m> {
        ModelData {
            book: &model.workbook,
            formulas: &model.parsed_formulas,
        }
    }
}

// `remains` is what the file the sheet is written into holds of it.
pub(crate) fn sheet_job(
    data: ModelData<'_>,
    carried: &Carried,
    index: usize,
    part: String,
    selected: bool,
    remains: Remains,
) -> Result<SheetJob, EngineError> {
    let sheet = data
        .book
        .worksheets
        .get(index)
        .ok_or_else(|| rejected("a sheet is missing"))?;
    let formulas = data
        .formulas
        .get(index)
        .ok_or_else(|| rejected("a sheet has no formulas"))?;
    let settings = carried
        .written(sheet)
        .map(|settings| settings.left_in(remains));
    let (sheet, formulas) = copied_sheet(sheet, formulas)?;
    Ok(SheetJob {
        part,
        sheet,
        formulas,
        settings,
        selected,
    })
}

// The kinds an undo or redo put back that a whole save does not write.
fn unwritable_restored(data: ModelData<'_>, carried: &Carried) -> BTreeSet<Unsupported> {
    data.book
        .worksheets
        .iter()
        .flat_map(|sheet| carried.unwritable_restored(sheet, Remains::Nothing))
        .collect()
}

fn full_part(index: usize) -> String {
    format!("xl/worksheets/sheet{}.xml", index + 1)
}

fn full_sheets(data: ModelData<'_>, carried: &Carried) -> Result<Vec<SheetJob>, EngineError> {
    let selected = selected_sheet(data.book);
    (0..data.book.worksheets.len())
        .map(|index| {
            let part = full_part(index);
            sheet_job(
                data,
                carried,
                index,
                part,
                index == selected,
                Remains::Nothing,
            )
        })
        .collect()
}

fn package_parts(model: &Model<'_>) -> Result<Vec<(String, String)>, EngineError> {
    get_package_parts(model).map_err(|e| rejected(format!("{e:?}")))
}

pub(crate) fn full_plan(model: &Model<'_>, carried: &Carried) -> Result<FullPlan, EngineError> {
    let data = ModelData::of(model);
    let sheets = run_with_engine_stack(|| full_sheets(data, carried))?;
    Ok(FullPlan {
        parts: package_parts(model)?,
        sheets,
    })
}

fn full_contents(
    data: ModelData<'_>,
    carried: &Carried,
    book: u64,
    sheets: Vec<SheetPrint>,
) -> Result<Contents, EngineError> {
    Ok(Contents {
        worksheets: (0..sheets.len()).map(full_part).collect(),
        remains: vec![Remains::Nothing; sheets.len()],
        held: Held::written_whole(sheets.len()),
        strings: Strings::Aligned(data.book.shared_strings.len()),
        styles: StylesBase::written(&data.book.styles)?,
        book,
        sheets,
        selected: selected_sheet(data.book),
        metadata: Metadata::Written,
        losses_through: carried.losses(),
    })
}

enum Chosen {
    Full(Vec<SheetJob>),
    Patch(PatchPlan),
}

struct Choice {
    chosen: Chosen,
    adoption: Option<Contents>,
    fate: Fate,
}

// `opened` is what the workbook's file held when it was opened, for when the engine knows
// nothing newer about it.
fn choose(
    path: &Path,
    target: SaveTarget,
    data: ModelData<'_>,
    carried: &Carried,
    slot: &SavedSlot,
    opened: &[Unsupported],
) -> Result<Choice, EngineError> {
    let source = slot.current();
    let through = source
        .as_ref()
        .map_or(0, |file| file.contents.losses_through);
    let lost: BTreeSet<Unsupported> = carried.dropped_after(through).collect();
    let held = match &source {
        Some(file) => file.contents.held.all(),
        None => opened.iter().copied().collect(),
    };
    let base = source.filter(|base| base.same_kind(path));
    if base.is_none() && target == SaveTarget::Copy {
        return Ok(Choice {
            chosen: Chosen::Full(full_sheets(data, carried)?),
            adoption: None,
            fate: Fate::full(
                held,
                lost.union(&unwritable_restored(data, carried))
                    .copied()
                    .collect(),
            ),
        });
    }
    let book = book_print(data.book)?;
    let prints = sheet_prints(data, carried)?;
    let patched = match base {
        Some(base) => patch_plan(base, book, &prints, data, carried)?,
        None => None,
    };
    let (chosen, contents, fate) = match patched {
        Some(Patched {
            plan,
            contents,
            dropped,
        }) => {
            let fate = Fate {
                dropped: dropped.union(&lost).copied().collect(),
                kept: contents.held.all().into_iter().collect(),
            };
            (Chosen::Patch(plan), contents, fate)
        }
        None => (
            Chosen::Full(full_sheets(data, carried)?),
            full_contents(data, carried, book, prints)?,
            Fate::full(
                held,
                lost.union(&unwritable_restored(data, carried))
                    .copied()
                    .collect(),
            ),
        ),
    };
    Ok(Choice {
        chosen,
        adoption: (target == SaveTarget::Document).then_some(contents),
        fate,
    })
}

// Runs under the workbook's read lock; everything here is proportional to the
// workbook in memory, never to the file on disk.
pub(crate) fn snapshot(
    path: &Path,
    target: SaveTarget,
    model: &Model<'_>,
    carried: &Carried,
    slot: &SavedSlot,
    opened: &[Unsupported],
) -> Result<SaveSnapshot, EngineError> {
    let data = ModelData::of(model);
    let Choice {
        chosen,
        adoption,
        fate,
    } = run_with_engine_stack(|| choose(path, target, data, carried, slot, opened))?;
    let plan = match chosen {
        Chosen::Patch(plan) => Plan::Patch(plan),
        Chosen::Full(sheets) => Plan::Full(FullPlan {
            parts: package_parts(model)?,
            sheets,
        }),
    };
    Ok(SaveSnapshot {
        path: path.to_path_buf(),
        plan,
        slot: slot.clone(),
        adoption,
        restored: carried.restored().clone(),
        seen: carried.restored().seen(),
        fate,
    })
}

pub(crate) fn deflated() -> FileOptions {
    FileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

pub(crate) fn write_sheet<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    mut job: SheetJob,
) -> Result<(), EngineError> {
    zip.start_file(job.part.as_str(), deflated())
        .map_err(rejected)?;
    let dimension = prepare(&mut job.sheet, job.settings.as_ref());
    let mut sheet_writer = SheetWriter::new(&mut *zip, job.settings.as_ref());
    let mut buffered = BufWriter::with_capacity(SHEET_BUFFER_BYTES, &mut sheet_writer);
    write_worksheet_xml(
        &mut buffered,
        &job.sheet,
        &job.formulas,
        &dimension,
        job.selected,
    )
    .map_err(rejected)?;
    buffered.flush().map_err(rejected)?;
    drop(buffered);
    sheet_writer.finish().map_err(rejected)?;
    Ok(())
}

// Returns what was written, part by part, for the verification.
pub(crate) fn write_full<W: Write + Seek>(
    plan: FullPlan,
    writer: W,
) -> Result<(W, Vec<(String, Written)>), EngineError> {
    let mut zip = ZipWriter::new(writer);
    let mut written = Vec::with_capacity(plan.parts.len() + plan.sheets.len());
    for (name, content) in plan.parts {
        zip.start_file(name.as_str(), deflated())
            .map_err(rejected)?;
        zip.write_all(content.as_bytes()).map_err(rejected)?;
        written.push((name, Written::Generated));
    }
    for job in plan.sheets {
        written.push((job.part.clone(), Written::Generated));
        write_sheet(&mut zip, job)?;
    }
    Ok((zip.finish().map_err(rejected)?, written))
}

fn write_error(path: &Path) -> impl Fn(io::Error) -> EngineError + '_ {
    move |source| EngineError::Write {
        path: path.to_path_buf(),
        source,
    }
}

fn write_temp(
    plan: Plan,
    file: BufWriter<File>,
    temp: &Path,
    refused: &mut Option<String>,
) -> Result<Vec<(String, Written)>, EngineError> {
    let (file, written) = match plan {
        Plan::Full(plan) => write_full(plan, file)?,
        Plan::Patch(plan) => write_patch(plan, file, refused)?,
    };
    let file = file
        .into_inner()
        .map_err(|error| write_error(temp)(error.into_error()))?;
    file.sync_all().map_err(write_error(temp))?;
    Ok(written)
}

fn unchanged(base: Option<&SavedFile>) -> Result<(), EngineError> {
    match base {
        Some(base) if Stamp::of(&base.path).ok() != Some(base.stamp) => {
            Err(EngineError::ChangedOnDisk(base.path.clone()))
        }
        _ => Ok(()),
    }
}

fn verify(
    temp: &Path,
    base: Option<&SavedFile>,
    written: &[(String, Written)],
) -> Result<(), EngineError> {
    zenkai_xlsx_reader::check_written(temp, base.map(|base| base.path.as_path()), written)
        .map_err(|e| EngineError::VerifyFailed(e.to_string()))
}

fn adopt(slot: &SavedSlot, path: &Path, contents: Contents) {
    match Stamp::of(path) {
        Ok(stamp) => slot.set(Some(SavedFile {
            path: path.to_path_buf(),
            stamp,
            contents,
            unspliceable: Vec::new(),
        })),
        Err(error) => {
            tracing::warn!(?path, %error, "the next save writes the whole file");
            slot.set(None);
        }
    }
}

// Written to a sibling temp file, read back, then renamed over the target: the original
// is never truncated and survives any failure before the rename. A file a save copies
// from must not change from the snapshot until the rename.
pub fn save_xlsx(snapshot: SaveSnapshot) -> Result<(), EngineError> {
    let SaveSnapshot {
        path,
        plan,
        slot,
        adoption,
        restored,
        seen,
        fate: _,
    } = snapshot;
    let temp = temp_path(&path);
    let created = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp);
    let file = match created {
        Ok(file) => BufWriter::new(file),
        // Another save of the same file owns that temp file.
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            return Err(write_error(&temp)(source));
        }
        Err(source) => {
            remove_temp(&temp);
            return Err(write_error(&temp)(source));
        }
    };
    let held = match &plan {
        Plan::Patch(plan) => Some(plan.base.clone()),
        Plan::Full(_) => None,
    };
    let base = held.as_deref();
    let mut refused = None;
    let checked = unchanged(base)
        .and_then(|()| run_with_engine_stack(|| write_temp(plan, file, &temp, &mut refused)))
        .and_then(|written| verify(&temp, base, &written))
        .and_then(|()| unchanged(base));
    if let Err(error) = checked {
        remove_temp(&temp);
        // A file that cannot be copied from must not keep every later save from working;
        // a temp file that cannot be created or finished says nothing about it. A failed
        // part copy reads and writes at once, so it counts as the file's.
        let base_failed = matches!(
            error,
            EngineError::ChangedOnDisk(_)
                | EngineError::BaseUnusable { .. }
                | EngineError::VerifyFailed(_)
        );
        if let (Some(held), Some(part)) = (&held, refused) {
            tracing::warn!(%error, %part, "a sheet cannot be spliced; the next save writes it whole");
            slot.refuse_splice(held, part);
        } else if base.is_some() && base_failed {
            tracing::warn!(%error, "copying from the saved file failed; the next save writes it whole");
            slot.set(None);
        }
        return Err(error);
    }
    if let Err(source) = fs::rename(&temp, &path) {
        remove_temp(&temp);
        return Err(EngineError::Write { path, source });
    }
    if let Some(contents) = adoption {
        adopt(&slot, &path, contents);
        restored.forget(&seen);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use zenkai_types::{CellPos, SheetId};

    use crate::workbook::{Engine, Workbook};

    fn book() -> Workbook {
        let mut book = Workbook::new_empty().unwrap();
        let rows = vec![vec![
            "2".to_string(),
            "=A1*3".to_string(),
            "text".to_string(),
        ]];
        book.set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        book
    }

    #[test]
    fn a_saved_file_replaces_the_target_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        std::fs::write(&path, b"old contents").unwrap();
        super::save_xlsx(
            book()
                .save_snapshot(&path, super::SaveTarget::Document)
                .unwrap(),
        )
        .unwrap();
        let reopened = crate::file::open_xlsx(&path).unwrap().workbook;
        let b1 = CellPos::parse_a1("B1").unwrap();
        assert_eq!(reopened.cell(SheetId(0), b1).text, "6");
        assert_eq!(reopened.input(SheetId(0), b1), "=A1*3");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "the temp file is gone");
    }

    #[test]
    fn a_failed_save_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("book.xlsx");
        assert!(
            super::save_xlsx(
                book()
                    .save_snapshot(&path, super::SaveTarget::Document)
                    .unwrap()
            )
            .is_err()
        );
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn the_snapshot_does_not_see_later_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        let mut book = book();
        let snapshot = book
            .save_snapshot(&path, super::SaveTarget::Document)
            .unwrap();
        book.set_input(SheetId(0), CellPos::default(), "5").unwrap();
        super::save_xlsx(snapshot).unwrap();
        let reopened = crate::file::open_xlsx(&path).unwrap().workbook;
        assert_eq!(reopened.input(SheetId(0), CellPos::default()), "2");
    }
}
