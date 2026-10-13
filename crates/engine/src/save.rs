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

use crate::error::EngineError;
use crate::file::{remove_temp, temp_path};
use crate::preflight::run_with_engine_stack;
use crate::save_sheet::{SheetWriter, prepare};
use crate::sheet_settings::{Carried, SheetSettings};

// IronCalc's rows are written in many small pieces; the settings writer and the deflate
// stream take them in chunks this large.
const SHEET_BUFFER_BYTES: usize = 256 * 1024;

pub(crate) struct SheetJob {
    pub(crate) part: String,
    pub(crate) sheet: Worksheet,
    pub(crate) formulas: Vec<(Node, StaticResult)>,
    pub(crate) settings: Option<SheetSettings>,
    pub(crate) selected: bool,
}

// Everything a save writes, copied out of the model so the file is written without
// holding the workbook.
pub(crate) struct FullPlan {
    parts: Vec<(String, String)>,
    sheets: Vec<SheetJob>,
}

pub struct SaveSnapshot {
    path: PathBuf,
    plan: FullPlan,
}

impl SaveSnapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn rejected(error: impl ToString) -> EngineError {
    EngineError::Rejected(format!("saving the workbook: {}", error.to_string()))
}

fn settings_of(carried: &Carried, sheet: &Worksheet) -> Option<SheetSettings> {
    carried
        .get(sheet.sheet_id)
        .filter(|settings| !settings.is_empty())
        .cloned()
}

pub(crate) fn sheet_job(
    model: &Model<'_>,
    carried: &Carried,
    index: usize,
    part: String,
    selected: bool,
) -> Result<SheetJob, EngineError> {
    let sheet = model
        .workbook
        .worksheets
        .get(index)
        .ok_or_else(|| rejected("a sheet is missing"))?;
    let formulas = model
        .parsed_formulas
        .get(index)
        .ok_or_else(|| rejected("a sheet has no formulas"))?;
    Ok(SheetJob {
        part,
        sheet: sheet.clone(),
        formulas: formulas.clone(),
        settings: settings_of(carried, sheet),
        selected,
    })
}

pub(crate) fn selected_sheet(model: &Model<'_>) -> usize {
    model
        .workbook
        .views
        .get(&0)
        .map_or(0, |view| view.sheet as usize)
}

pub(crate) fn full_plan(model: &Model<'_>, carried: &Carried) -> Result<FullPlan, EngineError> {
    let parts = get_package_parts(model).map_err(|e| rejected(format!("{e:?}")))?;
    let selected = selected_sheet(model);
    let sheets = (0..model.workbook.worksheets.len())
        .map(|index| {
            let part = format!("xl/worksheets/sheet{}.xml", index + 1);
            sheet_job(model, carried, index, part, index == selected)
        })
        .collect::<Result<_, _>>()?;
    Ok(FullPlan { parts, sheets })
}

pub(crate) fn snapshot(
    path: &Path,
    model: &Model<'_>,
    carried: &Carried,
) -> Result<SaveSnapshot, EngineError> {
    Ok(SaveSnapshot {
        path: path.to_path_buf(),
        plan: full_plan(model, carried)?,
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
    plan: FullPlan,
    file: BufWriter<File>,
    temp: &Path,
) -> Result<Vec<(String, Written)>, EngineError> {
    let (file, written) = write_full(plan, file)?;
    let file = file
        .into_inner()
        .map_err(|error| write_error(temp)(error.into_error()))?;
    file.sync_all().map_err(write_error(temp))?;
    Ok(written)
}

// Written to a sibling temp file, read back, then renamed over the target: the original
// is never truncated and survives any failure before the rename.
pub fn save_xlsx(snapshot: SaveSnapshot) -> Result<(), EngineError> {
    let path = snapshot.path;
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
    let checked =
        run_with_engine_stack(|| write_temp(snapshot.plan, file, &temp)).and_then(|written| {
            zenkai_xlsx_reader::check_written(&temp, None, &written)
                .map_err(|e| EngineError::VerifyFailed(e.to_string()))
        });
    if let Err(error) = checked {
        remove_temp(&temp);
        return Err(error);
    }
    if let Err(source) = fs::rename(&temp, &path) {
        remove_temp(&temp);
        return Err(EngineError::Write { path, source });
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
        super::save_xlsx(book().save_snapshot(&path).unwrap()).unwrap();
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
        assert!(super::save_xlsx(book().save_snapshot(&path).unwrap()).is_err());
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn the_snapshot_does_not_see_later_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        let mut book = book();
        let snapshot = book.save_snapshot(&path).unwrap();
        book.set_input(SheetId(0), CellPos::default(), "5").unwrap();
        super::save_xlsx(snapshot).unwrap();
        let reopened = crate::file::open_xlsx(&path).unwrap().workbook;
        assert_eq!(reopened.input(SheetId(0), CellPos::default()), "2");
    }
}
