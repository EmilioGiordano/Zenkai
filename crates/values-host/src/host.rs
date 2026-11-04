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
            Err(error) => Message::Failed(send_failure(&error)),
        },
        Err(error) => Message::Failed(Failure::from(&error)),
    };
    write_frame(out, &encode(&last, &[])?)?;
    Ok(())
}

// A row or a sheet name too big for a frame is a workbook the reader cannot carry, which the
// user is told as too large; nothing of the frame was written, so the stream stays valid.
fn send_failure(error: &HostError) -> Failure {
    let kind = match error {
        HostError::Frame(FrameError::PayloadTooLarge(_) | FrameError::HeaderTooLarge(_)) => {
            FailureKind::TooLarge
        }
        _ => FailureKind::Unreadable,
    };
    Failure {
        kind,
        detail: error.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_over_its_limit_is_reported_as_too_large() {
        for error in [
            FrameError::PayloadTooLarge(1),
            FrameError::HeaderTooLarge(1),
        ] {
            assert_eq!(send_failure(&error.into()).kind, FailureKind::TooLarge);
        }
        let other = FrameError::BadMagic;
        assert_eq!(send_failure(&other.into()).kind, FailureKind::Unreadable);
    }

    #[test]
    fn a_sheet_name_over_the_header_limit_ends_in_a_too_large_failure() {
        let sheets = vec![SheetValues {
            name: "n".repeat(zenkai_pdf_host::frame::MAX_HEADER_BYTES + 1),
            blocks: Vec::new(),
        }];
        let error = send_sheets(&sheets, &mut Vec::new()).unwrap_err();
        assert_eq!(send_failure(&error).kind, FailureKind::TooLarge);
    }
}
