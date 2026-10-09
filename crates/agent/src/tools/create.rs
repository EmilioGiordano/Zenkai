use std::fs::OpenOptions;
use std::io::ErrorKind;

use zenkai_engine::{Engine, EngineError, Workbook, run_with_engine_stack, save_xlsx_atomic};
use zenkai_types::SheetId;

use crate::tools::error::ToolError;
use crate::tools::folder::{InsidePath, WorkingFolder};

pub const MAX_NEW_SHEETS: usize = 50;
const WORKBOOK_EXTENSION: &str = "xlsx";
pub const OPENABLE_EXTENSIONS: [&str; 5] = ["xlsx", "xlsm", "xls", "xlsb", "ods"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWorkbook {
    pub folder: WorkingFolder,
    pub path: InsidePath,
    pub sheets: Vec<String>,
}

impl NewWorkbook {
    // A name without an extension gets .xlsx; any other extension is refused rather than
    // writing an .xlsx under a name that says otherwise.
    pub fn new(
        folder: WorkingFolder,
        path: InsidePath,
        sheets: Vec<String>,
    ) -> Result<NewWorkbook, ToolError> {
        let path = match path.extension() {
            None => path.with_extension(WORKBOOK_EXTENSION),
            Some(extension) if extension == WORKBOOK_EXTENSION => path,
            Some(_) => return Err(ToolError::NotNewWorkbookFile(relative_text(&path))),
        };
        if sheets.len() > MAX_NEW_SHEETS {
            return Err(ToolError::TooManySheets {
                count: sheets.len(),
                limit: MAX_NEW_SHEETS,
            });
        }
        Ok(NewWorkbook {
            folder,
            path,
            sheets,
        })
    }
}

pub fn relative_text(path: &InsidePath) -> String {
    path.relative().display().to_string()
}

pub fn check_openable(path: &InsidePath) -> Result<(), ToolError> {
    let openable = path
        .extension()
        .is_some_and(|extension| OPENABLE_EXTENSIONS.contains(&extension.as_str()));
    if !openable {
        return Err(ToolError::NotOpenableFile(relative_text(path)));
    }
    match std::fs::metadata(path.path()) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(ToolError::NotOpenableFile(relative_text(path))),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err(ToolError::FileNotFound(relative_text(path)))
        }
        Err(error) => Err(ToolError::File(error.to_string())),
    }
}

// Built in memory first, so a sheet name the engine refuses leaves no file behind. The name
// is then taken with create_new before the save, so an existing file is never replaced, even
// one that appears after the agent asked.
pub fn create_workbook_file(new: &NewWorkbook) -> Result<Workbook, ToolError> {
    let workbook = build(&new.sheets)?;
    new.folder.create_parents(&new.path)?;
    let target = new.path.path();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| match error.kind() {
            ErrorKind::AlreadyExists => ToolError::AlreadyExists(relative_text(&new.path)),
            _ => ToolError::File(error.to_string()),
        })?;
    if let Err(error) = save_xlsx_atomic(&workbook, target) {
        if let Err(cleanup) = std::fs::remove_file(target) {
            tracing::warn!(%cleanup, path = %target.display(), "could not remove a reserved workbook name");
        }
        return Err(ToolError::Engine(error.to_string()));
    }
    Ok(workbook)
}

fn build(sheets: &[String]) -> Result<Workbook, ToolError> {
    run_with_engine_stack(|| {
        let mut workbook = Workbook::new_empty()?;
        for (index, name) in sheets.iter().enumerate() {
            let sheet = if index == 0 {
                SheetId(0)
            } else {
                workbook.add_sheet()?
            };
            workbook.rename_sheet(sheet, name)?;
        }
        Ok::<Workbook, EngineError>(workbook)
    })
    .map_err(|error| ToolError::Engine(error.to_string()))
}

#[cfg(test)]
mod tests {
    use zenkai_engine::open_xlsx;

    use super::*;

    fn folder() -> (tempfile::TempDir, WorkingFolder) {
        let temp = tempfile::tempdir().unwrap();
        let folder = WorkingFolder::new(temp.path()).unwrap();
        (temp, folder)
    }

    fn new(folder: &WorkingFolder, path: &str, sheets: &[&str]) -> Result<NewWorkbook, ToolError> {
        NewWorkbook::new(
            folder.clone(),
            folder.resolve(path).unwrap(),
            sheets.iter().map(|name| name.to_string()).collect(),
        )
    }

    #[test]
    fn a_new_workbook_is_written_with_its_sheets_in_order() {
        let (_temp, folder) = folder();
        let request = new(&folder, "budget/september", &["Income", "Expenses"]).unwrap();
        let workbook = create_workbook_file(&request).unwrap();
        let names: Vec<String> = workbook.sheets().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Income", "Expenses"]);
        let path = folder.path().join("budget").join("september.xlsx");
        let reopened = open_xlsx(&path).unwrap().workbook;
        let names: Vec<String> = reopened.sheets().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Income", "Expenses"]);
    }

    #[test]
    fn an_existing_file_is_never_replaced() {
        let (_temp, folder) = folder();
        let path = folder.path().join("ventas.xlsx");
        std::fs::write(&path, b"the user's data").unwrap();
        let request = new(&folder, "ventas.xlsx", &[]).unwrap();
        assert!(matches!(
            create_workbook_file(&request),
            Err(ToolError::AlreadyExists(name)) if name == "ventas.xlsx"
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"the user's data");
    }

    #[test]
    fn a_sheet_name_the_engine_refuses_leaves_no_file() {
        let (_temp, folder) = folder();
        let request = new(&folder, "bad.xlsx", &["Ok", "Bad[1]"]).unwrap();
        assert!(matches!(
            create_workbook_file(&request),
            Err(ToolError::Engine(_))
        ));
        assert!(!folder.path().join("bad.xlsx").exists());
    }

    #[test]
    fn only_xlsx_names_and_a_bounded_sheet_count_are_accepted() {
        let (_temp, folder) = folder();
        assert!(matches!(
            new(&folder, "notes.csv", &[]),
            Err(ToolError::NotNewWorkbookFile(_))
        ));
        assert!(new(&folder, "UPPER.XLSX", &[]).is_ok());
        let many: Vec<String> = (0..=MAX_NEW_SHEETS).map(|n| format!("S{n}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(matches!(
            new(&folder, "many.xlsx", &many),
            Err(ToolError::TooManySheets { .. })
        ));
    }

    #[test]
    fn only_existing_spreadsheet_files_can_be_opened() {
        let (_temp, folder) = folder();
        std::fs::write(folder.path().join("data.csv"), b"a,b").unwrap();
        std::fs::create_dir(folder.path().join("folder.xlsx")).unwrap();
        let check = |path: &str| check_openable(&folder.resolve(path).unwrap());
        assert!(matches!(
            check("data.csv"),
            Err(ToolError::NotOpenableFile(_))
        ));
        assert!(matches!(
            check("folder.xlsx"),
            Err(ToolError::NotOpenableFile(_))
        ));
        assert!(matches!(
            check("gone.xlsx"),
            Err(ToolError::FileNotFound(_))
        ));
        let request = new(&folder, "real.xlsx", &[]).unwrap();
        create_workbook_file(&request).unwrap();
        assert!(check("real.xlsx").is_ok());
    }
}
