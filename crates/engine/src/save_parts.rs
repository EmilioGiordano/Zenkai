// Every part a save writes goes through `PartWriter`, which checks each generated part's
// XML as it is produced, on a thread of its own so the parse overlaps deflate.

use std::io::{self, BufRead, Read, Seek, Write};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use zenkai_xlsx_reader::{Written, check_xml};
use zip::ZipWriter;

use crate::error::EngineError;
use crate::save::{SHEET_BUFFER_BYTES, deflated, rejected};

// The check trails the writer by at most this many chunks of `SHEET_BUFFER_BYTES`: with
// the chunk the checker reads and the one being sent, 1.5 MB per part being written.
const CHECK_QUEUE_CHUNKS: usize = 4;

pub(crate) struct PartWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    open: Option<OpenPart>,
    written: Vec<(String, Written)>,
}

impl<W: Write + Seek> PartWriter<W> {
    pub(crate) fn new(writer: W) -> PartWriter<W> {
        PartWriter {
            zip: ZipWriter::new(writer),
            open: None,
            written: Vec::new(),
        }
    }

    pub(crate) fn start_generated(&mut self, name: &str) -> Result<(), EngineError> {
        self.next_entry(|zip| zip.start_file(name, deflated()).map_err(rejected))?;
        self.open = Some(OpenPart::new(name)?);
        Ok(())
    }

    pub(crate) fn copy(
        &mut self,
        name: String,
        copy: impl FnOnce(&mut ZipWriter<W>) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        self.next_entry(copy)?;
        self.written.push((name, Written::Copied));
        Ok(())
    }

    // Returns what was written, part by part, for the verification.
    pub(crate) fn finish(mut self) -> Result<(W, Vec<(String, Written)>), EngineError> {
        let writer = self.next_entry(|zip| zip.finish().map_err(rejected))?;
        Ok((writer, self.written))
    }

    fn next_entry<T>(
        &mut self,
        next: impl FnOnce(&mut ZipWriter<W>) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        if let Some(open) = self.open.take() {
            let name = open.finish()?;
            self.written.push((name, Written::Generated));
        }
        next(&mut self.zip)
    }
}

impl<W: Write + Seek> Write for PartWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let open = self
            .open
            .as_mut()
            .ok_or_else(|| io::Error::other("no generated part is being written"))?;
        let written = self.zip.write(buf)?;
        open.push(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.zip.flush()
    }
}

struct OpenPart {
    name: String,
    // None once the check stopped reading: it found the part malformed, and says so
    // when joined.
    sender: Option<SyncSender<Vec<u8>>>,
    check: JoinHandle<Result<(), String>>,
}

impl OpenPart {
    fn new(name: &str) -> Result<OpenPart, EngineError> {
        let (sender, receiver) = mpsc::sync_channel(CHECK_QUEUE_CHUNKS);
        let check = std::thread::Builder::new()
            .name("zenkai-xml-check".to_string())
            .spawn(move || check_xml(Chunks::new(receiver)))
            .map_err(|e| rejected(format!("could not start the XML check: {e}")))?;
        Ok(OpenPart {
            name: name.to_string(),
            sender: Some(sender),
            check,
        })
    }

    fn push(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(SHEET_BUFFER_BYTES) {
            let Some(sender) = &self.sender else {
                return;
            };
            if sender.send(chunk.to_vec()).is_err() {
                self.sender = None;
            }
        }
    }

    // A malformed part fails as the verification would have, so the next save still
    // writes the whole file.
    fn finish(self) -> Result<String, EngineError> {
        let OpenPart {
            name,
            sender,
            check,
        } = self;
        drop(sender);
        match check.join() {
            Ok(Ok(())) => Ok(name),
            Ok(Err(error)) => Err(EngineError::VerifyFailed(format!("{name}: {error}"))),
            Err(_) => Err(EngineError::VerifyFailed(format!(
                "{name}: the XML check stopped"
            ))),
        }
    }
}

struct Chunks {
    receiver: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

impl Chunks {
    fn new(receiver: Receiver<Vec<u8>>) -> Chunks {
        Chunks {
            receiver,
            chunk: Vec::new(),
            at: 0,
        }
    }
}

impl Read for Chunks {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let read = available.len().min(buf.len());
        buf[..read].copy_from_slice(&available[..read]);
        self.consume(read);
        Ok(read)
    }
}

impl BufRead for Chunks {
    // The part ends when the writer drops its sender.
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.at == self.chunk.len() {
            match self.receiver.recv() {
                Ok(next) => {
                    self.chunk = next;
                    self.at = 0;
                }
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.chunk[self.at..])
    }

    fn consume(&mut self, amount: usize) {
        self.at = (self.at + amount).min(self.chunk.len());
    }
}
