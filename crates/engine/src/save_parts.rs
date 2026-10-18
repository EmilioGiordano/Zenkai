// Every part a save writes goes through `PartWriter`, which records what each generated
// part is: its XML, checked as it is produced on a thread of its own so the parse
// overlaps deflate, and the compressed bytes handed to the file. The verification compares
// those bytes and inflates them against the XML's checksum, without parsing it again.

use std::cell::RefCell;
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

use zenkai_xlsx_reader::limits::check_ratio;
use zenkai_xlsx_reader::{PartDigest, ReadError, Written, check_xml};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::EngineError;
use crate::save::{SHEET_BUFFER_BYTES, rejected};

// The check trails the writer by at most this many chunks of `SHEET_BUFFER_BYTES`: with
// the chunk the checker reads and the one being filled, 1.5 MB per part being written.
const CHECK_QUEUE_CHUNKS: usize = 4;

const GENERATED_COMPRESSION: CompressionMethod = CompressionMethod::Deflated;

pub(crate) struct PartWriter<W: Write + Seek> {
    zip: ZipWriter<Recorder<W>>,
    ledger: Rc<RefCell<Ledger>>,
    open: Option<OpenPart>,
    written: Vec<(String, Written)>,
}

impl<W: Write + Seek> PartWriter<W> {
    pub(crate) fn new(mut writer: W) -> Result<PartWriter<W>, EngineError> {
        let ledger = Rc::new(RefCell::new(Ledger {
            position: writer.stream_position().map_err(rejected)?,
            recording: None,
            recorded: None,
        }));
        let recorder = Recorder {
            inner: writer,
            ledger: ledger.clone(),
        };
        Ok(PartWriter {
            zip: ZipWriter::new(recorder),
            ledger,
            open: None,
            written: Vec::new(),
        })
    }

    pub(crate) fn start_generated(&mut self, name: &str) -> Result<(), EngineError> {
        let options = FileOptions::default().compression_method(GENERATED_COMPRESSION);
        self.next_entry(|zip| zip.start_file(name, options).map_err(rejected))?;
        self.ledger.borrow_mut().record();
        self.open = Some(OpenPart::new(name)?);
        Ok(())
    }

    pub(crate) fn copy(
        &mut self,
        name: String,
        copy: impl FnOnce(&mut ZipWriter<Recorder<W>>) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        self.next_entry(copy)?;
        self.written.push((name, Written::Copied));
        Ok(())
    }

    // Returns what was written, part by part, for the verification.
    pub(crate) fn finish(mut self) -> Result<(W, Vec<(String, Written)>), EngineError> {
        let recorder = self.next_entry(|zip| zip.finish().map_err(rejected))?;
        Ok((recorder.inner, self.written))
    }

    // zip ends an entry's compressed bytes only when the next entry starts or the archive
    // is finished, so a part's record is complete after `next` runs.
    fn next_entry<T>(
        &mut self,
        next: impl FnOnce(&mut ZipWriter<Recorder<W>>) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let xml = self.open.take().map(OpenPart::finish).transpose()?;
        let value = next(&mut self.zip)?;
        if let Some(xml) = xml {
            let compressed = self.ledger.borrow_mut().recorded.take().ok_or_else(|| {
                EngineError::VerifyFailed(format!(
                    "{}: its compressed bytes were not recorded",
                    xml.name
                ))
            })?;
            let digest = PartDigest {
                compression: GENERATED_COMPRESSION,
                crc32: xml.crc32,
                size: xml.size,
                data_start: compressed.start,
                compressed_size: compressed.size,
                compressed_crc32: compressed.hasher.finalize(),
            };
            self.written.push((xml.name, Written::Generated(digest)));
        }
        Ok(value)
    }
}

impl<W: Write + Seek> Write for PartWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let open = self
            .open
            .as_mut()
            .ok_or_else(|| io::Error::other("no generated part is being written"))?;
        let written = self.zip.write(buf)?;
        let compressed = self.ledger.borrow().recording_size();
        open.push(&buf[..written], compressed);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.zip.flush()
    }
}

struct Ledger {
    position: u64,
    recording: Option<Region>,
    recorded: Option<Region>,
}

struct Region {
    start: u64,
    size: u64,
    hasher: crc32fast::Hasher,
}

impl Ledger {
    fn record(&mut self) {
        self.recording = Some(Region {
            start: self.position,
            size: 0,
            hasher: crc32fast::Hasher::new(),
        });
    }

    fn recording_size(&self) -> u64 {
        self.recording.as_ref().map_or(0, |region| region.size)
    }

    fn wrote(&mut self, bytes: &[u8]) {
        if let Some(region) = &mut self.recording {
            region.hasher.update(bytes);
            region.size += bytes.len() as u64;
        }
        self.position += bytes.len() as u64;
    }

    // zip writes an entry's compressed bytes in one forward run, then seeks back to fill
    // in its local header: the first move ends the region.
    fn moved(&mut self, to: u64) {
        if to != self.position
            && let Some(region) = self.recording.take()
        {
            self.recorded = Some(region);
        }
        self.position = to;
    }
}

pub(crate) struct Recorder<W> {
    inner: W,
    ledger: Rc<RefCell<Ledger>>,
}

impl<W: Write> Write for Recorder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.ledger.borrow_mut().wrote(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Seek> Seek for Recorder<W> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let at = self.inner.seek(to)?;
        self.ledger.borrow_mut().moved(at);
        Ok(at)
    }
}

struct CheckedXml {
    name: String,
    crc32: u32,
    size: u64,
}

struct OpenPart {
    name: String,
    hasher: crc32fast::Hasher,
    size: u64,
    // Checked as written: the compressed bytes lag the XML, so the ratio is never lower
    // than a reader of the finished part measures.
    ratio: Result<(), ReadError>,
    // Sent whole, so the cost of the check follows the bytes, never the number of writes.
    filling: Vec<u8>,
    // None once the check stopped reading: it found the part malformed, and says so
    // when joined.
    sender: Option<SyncSender<Vec<u8>>>,
    check: JoinHandle<Result<(), ReadError>>,
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
            hasher: crc32fast::Hasher::new(),
            size: 0,
            ratio: Ok(()),
            filling: Vec::with_capacity(SHEET_BUFFER_BYTES),
            sender: Some(sender),
            check,
        })
    }

    fn push(&mut self, bytes: &[u8], compressed: u64) {
        self.hasher.update(bytes);
        self.size += bytes.len() as u64;
        if self.ratio.is_ok() {
            self.ratio = check_ratio(&self.name, compressed, self.size);
        }
        if self.sender.is_none() {
            return;
        }
        let mut rest = bytes;
        while !rest.is_empty() {
            let room = SHEET_BUFFER_BYTES - self.filling.len();
            let (now, later) = rest.split_at(room.min(rest.len()));
            self.filling.extend_from_slice(now);
            rest = later;
            if self.filling.len() == SHEET_BUFFER_BYTES {
                self.send();
            }
        }
    }

    fn send(&mut self) {
        let chunk = std::mem::replace(&mut self.filling, Vec::with_capacity(SHEET_BUFFER_BYTES));
        if let Some(sender) = &self.sender
            && sender.send(chunk).is_err()
        {
            self.sender = None;
        }
    }

    // A part that fails here fails as the verification would have, so the next save
    // still writes the whole file.
    fn finish(mut self) -> Result<CheckedXml, EngineError> {
        if !self.filling.is_empty() {
            self.send();
        }
        let OpenPart {
            name,
            hasher,
            size,
            ratio,
            sender,
            check,
            ..
        } = self;
        drop(sender);
        match check.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(EngineError::VerifyFailed(format!("{name}: {error}"))),
            Err(_) => {
                return Err(EngineError::VerifyFailed(format!(
                    "{name}: the XML check stopped"
                )));
            }
        }
        ratio.map_err(|e| EngineError::VerifyFailed(e.to_string()))?;
        Ok(CheckedXml {
            name,
            crc32: hasher.finalize(),
            size,
        })
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

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::{CHECK_QUEUE_CHUNKS, PartWriter};
    use crate::error::EngineError;
    use crate::save::SHEET_BUFFER_BYTES;
    use zenkai_xlsx_reader::Written;

    fn row_xml(bytes: usize) -> Vec<u8> {
        let mut xml = b"<sheetData>".to_vec();
        let row = b"<row r=\"1\"><c r=\"A1\"><v>12345</v></c></row>";
        while xml.len() < bytes {
            xml.extend_from_slice(row);
        }
        xml.extend_from_slice(b"</sheetData>");
        xml
    }

    fn written(parts: &[(&str, &[u8])]) -> Result<Vec<(String, Written)>, EngineError> {
        let mut writer = PartWriter::new(Cursor::new(Vec::new()))?;
        for (name, xml) in parts {
            writer.start_generated(name)?;
            writer.write_all(xml).unwrap();
        }
        writer.finish().map(|(_, written)| written)
    }

    #[test]
    fn a_part_longer_than_the_check_queue_is_recorded() {
        let xml = row_xml((CHECK_QUEUE_CHUNKS + 2) * SHEET_BUFFER_BYTES + 1);
        let written = written(&[("a.xml", &xml), ("b.xml", b"<b/>")]).unwrap();
        let Written::Generated(digest) = written[0].1 else {
            panic!("the part was generated");
        };
        assert_eq!(digest.size, xml.len() as u64);
        assert_eq!(digest.crc32, crc32fast::hash(&xml));
        assert!(digest.compressed_size > 0);
        assert_eq!(written.len(), 2);
    }

    #[test]
    fn a_part_written_in_small_pieces_is_checked_whole() {
        let xml = row_xml(2 * SHEET_BUFFER_BYTES + 3);
        let mut writer = PartWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.start_generated("a.xml").unwrap();
        for piece in xml.chunks(1000) {
            writer.write_all(piece).unwrap();
        }
        let (_, written) = writer.finish().unwrap();
        let Written::Generated(digest) = written[0].1 else {
            panic!("the part was generated");
        };
        assert_eq!(digest.crc32, crc32fast::hash(&xml));
        let mut writer = PartWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.start_generated("a.xml").unwrap();
        for piece in xml[..xml.len() - 1].chunks(1000) {
            writer.write_all(piece).unwrap();
        }
        assert!(writer.finish().is_err());
    }

    #[test]
    fn a_malformed_part_past_the_check_queue_fails_its_save() {
        let mut malformed = b"<a></b>".to_vec();
        malformed.extend_from_slice(&row_xml((CHECK_QUEUE_CHUNKS + 2) * SHEET_BUFFER_BYTES));
        let error = written(&[("a.xml", &malformed)]).unwrap_err();
        assert!(matches!(error, EngineError::VerifyFailed(_)), "{error}");
    }

    #[test]
    fn a_part_past_the_compression_ratio_fails_its_save() {
        let gate = usize::try_from(zenkai_xlsx_reader::limits::RATIO_GATE_BYTES).unwrap();
        let mut flat = b"<a>".to_vec();
        flat.resize(gate + 1, b' ');
        flat.extend_from_slice(b"</a>");
        let error = written(&[("a.xml", &flat)]).unwrap_err();
        assert!(error.to_string().contains("compression limit"), "{error}");
    }
}
