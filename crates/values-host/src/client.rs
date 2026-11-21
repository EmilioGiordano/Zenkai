use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use thiserror::Error;
use zenkai_formats::{Block, MAX_ALLOCATED_CELLS, MAX_CELL_BYTES, MAX_TEXT_BYTES, SheetValues};
use zenkai_pdf_host::frame::{Frame, read_frame};
use zenkai_pdf_host::process::{kill_and_wait, spawn_piped};

use crate::wire::{Failure, Message, decode};

pub const HOST_FILE: &str = if cfg!(windows) {
    "zenkai-values-host.exe"
} else {
    "zenkai-values-host"
};
const READ_DEADLINE: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(50);
const FRAMES_IN_FLIGHT: usize = 4;
const MAX_SHEETS: usize = 4096;
// The most an honest host sends over the wire: a character can take six bytes as a \u00XX
// escape, and every cell takes about three more for its quotes and comma, empty or not.
const MAX_PAYLOAD_BYTES: usize = 6 * MAX_TEXT_BYTES + 3 * MAX_ALLOCATED_CELLS;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("the file reader was not found next to the program")]
    HostMissing,
    #[error("the file reader could not start: {0}")]
    StartFailed(String),
    #[error("reading the file was cancelled")]
    Cancelled,
    #[error("reading the file took too long and was stopped")]
    TimedOut,
    #[error("the file has more cells or text than the reader allows")]
    TooLarge,
    #[error("the file could not be read: {}", .0.detail)]
    Failed(Failure),
    #[error("the file reader stopped without a result: {0}")]
    HostBroken(String),
}

// Reads a workbook's values by running the reader host as a child process, one process per
// read. Nothing the child does, however it ends, can do more than make the read fail.
#[derive(Clone, Debug)]
pub struct ValuesClient {
    host: Option<PathBuf>,
    environment: Vec<(String, String)>,
    deadline: Duration,
    payload_budget: usize,
}

impl ValuesClient {
    pub fn new(host: Option<PathBuf>) -> ValuesClient {
        ValuesClient {
            host,
            environment: Vec::new(),
            deadline: READ_DEADLINE,
            payload_budget: MAX_PAYLOAD_BYTES,
        }
    }

    pub fn with_environment(mut self, name: &str, value: &str) -> ValuesClient {
        self.environment.push((name.to_string(), value.to_string()));
        self
    }

    pub fn with_deadline(mut self, deadline: Duration) -> ValuesClient {
        self.deadline = deadline;
        self
    }

    pub fn with_payload_budget(mut self, bytes: usize) -> ValuesClient {
        self.payload_budget = bytes;
        self
    }

    pub fn read(&self, path: &Path, cancel: &AtomicBool) -> Result<Vec<SheetValues>, ReadError> {
        let host = self.host.as_deref().ok_or(ReadError::HostMissing)?;
        let arguments = [path.as_os_str().to_os_string()];
        let (child, stdin, stdout) =
            spawn_piped(host, &arguments, &self.environment).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ReadError::HostMissing
                } else {
                    ReadError::StartFailed(error.to_string())
                }
            })?;
        let child = KillOnDrop(child);
        let (frames, inbox) = mpsc::sync_channel(FRAMES_IN_FLIGHT);
        std::thread::Builder::new()
            .name("values-host-reader".to_string())
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                loop {
                    let item = read_frame(&mut stdout).map_err(|error| error.to_string());
                    let last = !matches!(item, Ok(Some(_)));
                    if frames.send(item).is_err() || last {
                        break;
                    }
                }
            })
            .map_err(|error| ReadError::StartFailed(error.to_string()))?;
        let outcome = self.collect(&inbox, cancel);
        // The host exits once its stdin closes, so it stays open for the whole read; if the app
        // dies first, the system closes it and a host stuck in calamine goes with it.
        drop(stdin);
        drop(child);
        // The reader ends by itself once the killed child closes the pipe or the inbox is gone.
        drop(inbox);
        outcome
    }

    fn collect(
        &self,
        inbox: &mpsc::Receiver<Result<Option<Frame>, String>>,
        cancel: &AtomicBool,
    ) -> Result<Vec<SheetValues>, ReadError> {
        let started = Instant::now();
        let mut result = Collected::default();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(ReadError::Cancelled);
            }
            if started.elapsed() > self.deadline {
                return Err(ReadError::TimedOut);
            }
            let frame = match inbox.recv_timeout(POLL) {
                Ok(Ok(Some(frame))) => frame,
                Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(broken("the pipe closed before the result"));
                }
                Ok(Err(error)) => return Err(broken(&error)),
                Err(RecvTimeoutError::Timeout) => continue,
            };
            result.payload_bytes = result
                .payload_bytes
                .saturating_add(frame.head.len() + frame.payload.len());
            if result.payload_bytes > self.payload_budget {
                return Err(ReadError::TooLarge);
            }
            let (message, rows) = decode(&frame).map_err(|error| broken(&error.to_string()))?;
            if let Some(finished) = result.accept(message, rows)? {
                return Ok(finished);
            }
        }
    }
}

fn broken(detail: &str) -> ReadError {
    ReadError::HostBroken(detail.to_string())
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        kill_and_wait(&mut self.0);
    }
}

#[derive(Default)]
struct Collected {
    sheets: Vec<SheetValues>,
    allocated: usize,
    text_bytes: usize,
    payload_bytes: usize,
}

impl Collected {
    // Some once the host said it is done. The host's rows are checked again against the reader's
    // caps on allocated cells, bytes per cell and total text; with the sheet cap and the payload
    // budget, a misbehaving host cannot make the app allocate without bound.
    fn accept(
        &mut self,
        message: Message,
        rows: Vec<Vec<String>>,
    ) -> Result<Option<Vec<SheetValues>>, ReadError> {
        match message {
            Message::Sheet { name } => {
                if self.sheets.len() >= MAX_SHEETS {
                    return Err(broken("too many sheets"));
                }
                self.sheets.push(SheetValues {
                    name,
                    blocks: Vec::new(),
                });
            }
            Message::Block {
                first_row,
                first_col,
            } => {
                if rows.is_empty() {
                    return Err(broken("a block without rows"));
                }
                for row in &rows {
                    self.allocated = self.allocated.saturating_add(row.len().max(1));
                    for cell in row {
                        if cell.len() > MAX_CELL_BYTES {
                            return Err(ReadError::TooLarge);
                        }
                        self.text_bytes = self.text_bytes.saturating_add(cell.len());
                    }
                }
                if self.allocated > MAX_ALLOCATED_CELLS || self.text_bytes > MAX_TEXT_BYTES {
                    return Err(ReadError::TooLarge);
                }
                let sheet = self
                    .sheets
                    .last_mut()
                    .ok_or_else(|| broken("a block came before any sheet"))?;
                sheet.blocks.push(Block {
                    first_row,
                    first_col,
                    rows,
                });
            }
            Message::Done => return Ok(Some(std::mem::take(&mut self.sheets))),
            Message::Failed(failure) => return Err(ReadError::Failed(failure)),
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(cell: String) -> (Message, Vec<Vec<String>>) {
        let message = Message::Block {
            first_row: 0,
            first_col: 0,
        };
        (message, vec![vec![cell]])
    }

    fn with_sheet() -> Collected {
        let mut collected = Collected::default();
        let sheet = Message::Sheet { name: "S".into() };
        assert!(collected.accept(sheet, Vec::new()).unwrap().is_none());
        collected
    }

    #[test]
    fn a_cell_over_the_cell_cap_is_too_large() {
        let mut collected = with_sheet();
        let (message, rows) = block("x".repeat(MAX_CELL_BYTES));
        assert!(collected.accept(message, rows).unwrap().is_none());
        let (message, rows) = block("x".repeat(MAX_CELL_BYTES + 1));
        assert!(matches!(
            collected.accept(message, rows),
            Err(ReadError::TooLarge)
        ));
    }

    #[test]
    fn text_past_the_total_cap_is_too_large() {
        let mut collected = with_sheet();
        collected.text_bytes = MAX_TEXT_BYTES - 2;
        let (message, rows) = block("ab".to_string());
        assert!(collected.accept(message, rows).unwrap().is_none());
        let (message, rows) = block("c".to_string());
        assert!(matches!(
            collected.accept(message, rows),
            Err(ReadError::TooLarge)
        ));
    }

    #[test]
    fn text_is_counted_across_blocks_and_sheets() {
        let mut collected = with_sheet();
        for _ in 0..2 {
            let (message, rows) = block("abc".to_string());
            assert!(collected.accept(message, rows).unwrap().is_none());
            let sheet = Message::Sheet { name: "T".into() };
            assert!(collected.accept(sheet, Vec::new()).unwrap().is_none());
        }
        assert_eq!(collected.text_bytes, 6);
    }
}
