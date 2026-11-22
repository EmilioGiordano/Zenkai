use std::path::Path;

use anyhow::{Context, Result, anyhow};
use zenkai_engine::{Engine, EngineError, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, RowIdx, SheetId};

use crate::save_losses::{self, Loss};

// The two edits the report measures, applied to the first sheet before the app saves.
#[derive(Clone, Copy)]
pub enum Edit {
    CellValue,
    InsertRow,
}

impl Edit {
    fn apply(self, book: &mut Workbook) -> Result<(), EngineError> {
        match self {
            Edit::CellValue => book.set_input(SheetId(0), CellPos::default(), "edited"),
            Edit::InsertRow => book.insert_rows(SheetId(0), RowIdx::clamped(1), 1),
        }
    }
}

pub enum Outcome {
    Saved {
        dropped: Vec<String>,
        without_warning: Vec<String>,
    },
    EditRejected(String),
    Failed(String),
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Saved { dropped, .. } if dropped.is_empty() => "-".to_string(),
            Outcome::Saved { dropped, .. } => dropped.join(", "),
            Outcome::EditRejected(reason) => format!("edit rejected: {reason}"),
            Outcome::Failed(reason) => format!("error: {reason}"),
        }
    }

    pub fn without_warning(&self) -> &[String] {
        match self {
            Outcome::Saved {
                without_warning, ..
            } => without_warning,
            Outcome::EditRejected(_) | Outcome::Failed(_) => &[],
        }
    }
}

fn labels(losses: &[Loss]) -> Vec<String> {
    losses.iter().map(|loss| loss.label.clone()).collect()
}

// Saves the way the app does: opens a copy of the file, edits it, and saves over that copy
// as the workbook's own file (patch save, with the drop check the save dialog shows).
pub fn app_save(original: &Path, edit: Edit) -> Outcome {
    match run(original, edit) {
        Ok(outcome) => outcome,
        Err(error) => Outcome::Failed(format!("{error:#}")),
    }
}

fn run(original: &Path, edit: Edit) -> Result<Outcome> {
    let dir = tempfile::tempdir().context("creating a temp dir")?;
    let name = original.file_name().context("file without a name")?;
    let copy = dir.path().join(name);
    std::fs::copy(original, &copy).context("copying the file")?;
    let mut book = open_xlsx(&copy)?.workbook;
    if let Err(error) = edit.apply(&mut book) {
        return Ok(Outcome::EditRejected(error.to_string()));
    }
    let snapshot = book.save_snapshot(&copy, SaveTarget::Document)?;
    let warned = snapshot.dropped().to_vec();
    save_xlsx(snapshot)?;
    let before = std::fs::read(original).context("reading the original")?;
    let after = std::fs::read(&copy).context("reading the saved file")?;
    let losses = save_losses::dropped(&before, &after).map_err(|e| anyhow!(e))?;
    Ok(Outcome::Saved {
        dropped: labels(&losses),
        without_warning: losses
            .iter()
            .filter(|loss| !loss.is_warned(&warned))
            .map(|loss| loss.label.clone())
            .collect(),
    })
}
