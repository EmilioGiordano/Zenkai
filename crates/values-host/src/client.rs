use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use thiserror::Error;
use zenkai_formats::{Block, MAX_ALLOCATED_CELLS, SheetValues};
use zenkai_pdf_host::frame::{Frame, read_frame};
use zenkai_pdf_host::process::{kill_and_wait, spawn_piped};

use crate::wire::{Failure, Message, decode};

const READ_DEADLINE: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(50);
const FRAMES_IN_FLIGHT: usize = 4;
const MAX_SHEETS: usize = 4096;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("the file reader {0} was not found")]
    HostMissing(String),
    #[error("the file reader could not start: {0}")]
    StartFailed(String),
    #[error("reading the file was cancelled")]
    Cancelled,
    #[error("reading the file took too long and was stopped")]
    TimedOut,
    #[error("the file could not be read: {}", .0.detail)]
    Failed(Failure),
    #[error("the file reader stopped without a result: {0}")]
    HostBroken(String),
}

// Reads a workbook's values by running the reader host as a child process, one process per
// read. Nothing the child does, however it ends, can do more than make the read fail.
#[derive(Clone, Debug)]
pub struct ValuesClient {
    host: PathBuf,
    environment: Vec<(String, String)>,
    deadline: Duration,
}

impl ValuesClient {
    // The host is looked for next to the running exe, or one folder up when that exe is a
    // cargo test binary in `deps`.
    pub fn next_to_exe() -> ValuesClient {
        let folder = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let folder = match folder.file_name() {
            Some(name) if name == "deps" => {
                folder.parent().map(Path::to_path_buf).unwrap_or(folder)
            }
            _ => folder,
        };
        ValuesClient::new(folder.join(format!(
            "zenkai-values-host{}",
            std::env::consts::EXE_SUFFIX
        )))
    }

    pub fn new(host: PathBuf) -> ValuesClient {
        ValuesClient {
            host,
            environment: Vec::new(),
            deadline: READ_DEADLINE,
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

    pub fn read(&self, path: &Path, cancel: &AtomicBool) -> Result<Vec<SheetValues>, ReadError> {
        let arguments = [path.to_string_lossy().into_owned()];
        let (child, stdin, stdout) = spawn_piped(&self.host, &arguments, &self.environment)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ReadError::HostMissing(self.host.display().to_string())
                } else {
                    ReadError::StartFailed(error.to_string())
                }
            })?;
        let child = KillOnDrop(child);
        drop(stdin);
        let (frames, inbox) = mpsc::sync_channel(FRAMES_IN_FLIGHT);
        let reader = std::thread::Builder::new()
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
        drop(child);
        drop(inbox);
        if reader.join().is_err() {
            tracing::warn!("the values host reader thread panicked");
        }
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
}

impl Collected {
    // Some once the host said it is done. Whatever the host sends is held to the same caps as
    // the reader itself, so a misbehaving host cannot make the app allocate without bound.
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
                self.allocated = rows
                    .iter()
                    .fold(self.allocated, |total, row| total.saturating_add(row.len()));
                if self.allocated > MAX_ALLOCATED_CELLS {
                    return Err(broken("more cells than the reader allows"));
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
