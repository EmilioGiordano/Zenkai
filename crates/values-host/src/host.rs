use std::io::Write;
use std::path::Path;

use thiserror::Error;
use zenkai_formats::{SheetValues, read_values};
use zenkai_pdf_host::frame::{FrameError, write_frame};

use crate::wire::{Failure, FailureKind, Message, TARGET_ROWS_BYTES, WireError, encode};

#[derive(Debug, Error)]
pub enum HostError {
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

// Reads the workbook and writes it to `out` as frames. A read that fails is reported to the
// client as a Failed frame; a panic inside calamine is caught by `read_values` and arrives the
// same way. Only a failure to write the last frame is an error here.
pub fn run(path: &Path, out: &mut impl Write) -> Result<(), HostError> {
    let last = match read_values(path) {
        Ok(sheets) => match send_sheets(&sheets, out) {
            Ok(()) => Message::Done,
            Err(error) => Message::Failed(Failure {
                kind: FailureKind::Unreadable,
                detail: error.to_string(),
            }),
        },
        Err(error) => Message::Failed(Failure::from(&error)),
    };
    write_frame(out, &encode(&last, &[])?)?;
    Ok(())
}

fn send_sheets(sheets: &[SheetValues], out: &mut impl Write) -> Result<(), HostError> {
    for sheet in sheets {
        let name = Message::Sheet {
            name: sheet.name.clone(),
        };
        write_frame(out, &encode(&name, &[])?)?;
        for block in &sheet.blocks {
            let mut start = 0;
            while start < block.rows.len() {
                let mut end = start;
                let mut bytes = 0;
                while end < block.rows.len() && (end == start || bytes < TARGET_ROWS_BYTES) {
                    bytes += block.rows[end]
                        .iter()
                        .map(|cell| cell.len() + 3)
                        .sum::<usize>();
                    end += 1;
                }
                let message = Message::Block {
                    first_row: block.first_row + start as u32,
                    first_col: block.first_col,
                };
                write_frame(out, &encode(&message, &block.rows[start..end])?)?;
                start = end;
            }
        }
    }
    Ok(())
}
