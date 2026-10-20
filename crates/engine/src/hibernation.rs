// A workbook with unsaved work, written to disk with its undo history so it can leave
// memory while idle. Only the cells and the history go to the file; the small bookkeeping a
// save needs about the file the workbook came from stays in `Kept`, in memory. The dump
// never outlives the process: a crash is covered by the recovery copy, which holds the same
// work without the history. The file is written one encoded section at a time, so the peak
// next to the workbook is its largest section, not the whole dump.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use ironcalc::base::UserModel;

use crate::error::EngineError;
use crate::file::{Unsupported, remove_temp, temp_path};
use crate::preflight::run_with_engine_stack;
use crate::saved::SavedSlot;
use crate::sheet_settings::Carried;
use crate::workbook::{LANGUAGE, Workbook, evaluated_user_model};

// The largest workbook the app opens is charged about 2.7 GB of memory (38 million cells at
// 70 bytes), and the undo history of styled cells is capped near 1.5 GB (`workbook.rs`);
// a dump is no bigger than the memory it frees. The sum rounded up leaves a margin.
pub const MAX_DUMP_BYTES: u64 = 6 * 1024 * 1024 * 1024;

const MAGIC: [u8; 8] = *b"ZKHIB\0\0\x01";
const LENGTH_BYTES: usize = 8;
const CHECKSUM_BYTES: usize = 4;
const HEADER_BYTES: usize = MAGIC.len() + LENGTH_BYTES + CHECKSUM_BYTES;
const VERIFY_CHUNK_BYTES: usize = 1024 * 1024;

// What a hibernated workbook still holds in memory: the settings its file carried, what the
// file held when it was last read or written, and what the file lost. A few kilobytes
// next to the cells.
#[derive(Clone, Debug)]
pub struct Kept {
    carried: Carried,
    saved: SavedSlot,
    opened: Vec<Unsupported>,
}

impl Workbook {
    // Writes the workbook and its history to `path`. The workbook is untouched: the caller
    // drops it only once this returns, so a failed write leaves it where it was.
    pub fn hibernate(&self, path: &Path) -> Result<Kept, EngineError> {
        // The way back goes through the open budget; a workbook that would be refused must
        // stay in memory, where its edits can still be saved.
        ironcalc::base::open_budget::check_open_budget(&self.model.get_model().workbook)
            .map_err(EngineError::refused)?;
        write_dump(path, MAX_DUMP_BYTES, |sink| {
            let mut failure = None;
            let written = self.model.write_with_history(&mut |chunk| {
                sink.put(chunk).map_err(|error| {
                    let message = error.to_string();
                    failure = Some(error);
                    message
                })
            });
            match (failure, written) {
                (Some(error), _) => Err(error),
                (None, Err(message)) => Err(EngineError::Rejected(message)),
                (None, Ok(())) => Ok(()),
            }
        })?;
        Ok(Kept {
            carried: self.carried.clone(),
            saved: self.saved.clone(),
            opened: self.opened.clone(),
        })
    }

    // The workbook as it was when `hibernate` returned `kept`, with the same history. It is
    // evaluated in full, as after an open, so a volatile function shows its current value.
    pub fn wake(path: &Path, kept: &Kept) -> Result<Workbook, EngineError> {
        let (mut file, length) = open_verified(path, MAX_DUMP_BYTES)?;
        run_with_engine_stack(move || {
            // Read and decoded a section at a time: next to the model, only the largest
            // section is in memory, never the whole dump.
            let model = UserModel::from_reader_with_history(
                &mut (&mut file).take(length),
                length,
                LANGUAGE,
            )
            .map_err(|error| {
                if error.starts_with(ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX) {
                    EngineError::TooLarge(error)
                } else {
                    EngineError::BadDump(error)
                }
            })?;
            Ok(evaluated_user_model(
                model,
                kept.carried.clone(),
                kept.saved.clone(),
                kept.opened.clone(),
            ))
        })
    }
}

fn header_of(length: u64, checksum: u32) -> [u8; HEADER_BYTES] {
    let mut header = [0; HEADER_BYTES];
    header[..MAGIC.len()].copy_from_slice(&MAGIC);
    header[MAGIC.len()..MAGIC.len() + LENGTH_BYTES].copy_from_slice(&length.to_le_bytes());
    header[MAGIC.len() + LENGTH_BYTES..].copy_from_slice(&checksum.to_le_bytes());
    header
}

// The payload length and checksum a header declares, if it is ours and agrees with the
// size of the file it came from.
fn parse_header(
    header: &[u8; HEADER_BYTES],
    file_bytes: u64,
    cap: u64,
) -> Result<(u64, u32), EngineError> {
    if header[..MAGIC.len()] != MAGIC {
        return Err(EngineError::BadDump(
            "it was not written by this version of Zenkai".to_string(),
        ));
    }
    let mut length = [0; LENGTH_BYTES];
    length.copy_from_slice(&header[MAGIC.len()..MAGIC.len() + LENGTH_BYTES]);
    let mut checksum = [0; CHECKSUM_BYTES];
    checksum.copy_from_slice(&header[MAGIC.len() + LENGTH_BYTES..]);
    let declared = u64::from_le_bytes(length);
    if declared > cap || file_bytes != declared.saturating_add(HEADER_BYTES as u64) {
        return Err(EngineError::BadDump(
            "its size does not match what it declares".to_string(),
        ));
    }
    Ok((declared, u32::from_le_bytes(checksum)))
}

// Where `write_dump` puts the payload: straight into the file, counting its length and
// checksum as it goes, and refusing the byte that would pass the cap.
pub(crate) struct Sink {
    file: fs::File,
    hasher: crc32fast::Hasher,
    written: u64,
    cap: u64,
    temp: std::path::PathBuf,
}

impl Sink {
    pub(crate) fn put(&mut self, bytes: &[u8]) -> Result<(), EngineError> {
        let total = self.written.saturating_add(bytes.len() as u64);
        if total > self.cap {
            return Err(EngineError::TooLarge(format!(
                "the workbook is too large to rest on disk (over {} MB)",
                self.cap / 1024 / 1024
            )));
        }
        self.file
            .write_all(bytes)
            .map_err(|source| EngineError::Write {
                path: self.temp.clone(),
                source,
            })?;
        self.hasher.update(bytes);
        self.written = total;
        Ok(())
    }
}

fn write_dump(
    path: &Path,
    cap: u64,
    fill: impl FnOnce(&mut Sink) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let temp = temp_path(path);
    // The temp name carries our process id, so one left by an earlier hibernation of this
    // process is ours to replace.
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(EngineError::Write { path: temp, source }),
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|source| EngineError::Write {
            path: temp.clone(),
            source,
        })?;
    let mut sink = Sink {
        file,
        hasher: crc32fast::Hasher::new(),
        written: 0,
        cap,
        temp: temp.clone(),
    };
    let finished = sink
        .file
        .write_all(&[0; HEADER_BYTES])
        .map_err(|source| EngineError::Write {
            path: temp.clone(),
            source,
        })
        .and_then(|()| fill(&mut sink))
        .and_then(|()| {
            let header = header_of(sink.written, sink.hasher.clone().finalize());
            let patched = sink
                .file
                .seek(SeekFrom::Start(0))
                .and_then(|_| sink.file.write_all(&header))
                .and_then(|()| sink.file.sync_all());
            patched.map_err(|source| EngineError::Write {
                path: temp.clone(),
                source,
            })
        });
    drop(sink);
    if let Err(error) = finished.and_then(|()| verify_file(&temp, cap)) {
        remove_temp(&temp);
        return Err(error);
    }
    if let Err(source) = fs::rename(&temp, path) {
        remove_temp(&temp);
        return Err(EngineError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn short_file() -> EngineError {
    EngineError::BadDump("it is shorter than its header".to_string())
}

// Opens a dump, checks its header and its checksum (reading it in chunks, never whole), and
// leaves the file at the start of the payload, whose length it returns.
fn open_verified(path: &Path, cap: u64) -> Result<(fs::File, u64), EngineError> {
    let read_error = |source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    };
    let mut file = fs::File::open(path).map_err(read_error)?;
    let size = file.metadata().map_err(read_error)?.len();
    let mut header = [0; HEADER_BYTES];
    file.read_exact(&mut header).map_err(|_| short_file())?;
    let (declared, checksum) = parse_header(&header, size, cap)?;
    let mut hasher = crc32fast::Hasher::new();
    let mut chunk = vec![0; VERIFY_CHUNK_BYTES];
    let mut left = declared;
    while left > 0 {
        let take = chunk.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        file.read_exact(&mut chunk[..take]).map_err(read_error)?;
        hasher.update(&chunk[..take]);
        left -= take as u64;
    }
    if hasher.finalize() != checksum {
        return Err(EngineError::BadDump(
            "its contents do not match their checksum".to_string(),
        ));
    }
    file.seek(SeekFrom::Start(HEADER_BYTES as u64))
        .map_err(read_error)?;
    Ok((file, declared))
}

// What a save of the dump checks once it is written.
fn verify_file(path: &Path, cap: u64) -> Result<(), EngineError> {
    open_verified(path, cap)
        .map(|_| ())
        .map_err(|error| match error {
            EngineError::BadDump(reason) => EngineError::VerifyFailed(reason),
            other => other,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_dump(path: &Path, cap: u64) -> Result<Vec<u8>, EngineError> {
        let (mut file, length) = open_verified(path, cap)?;
        let mut payload = Vec::new();
        (&mut file).take(length).read_to_end(&mut payload).unwrap();
        Ok(payload)
    }

    fn write_bytes(path: &Path, payload: &[u8], cap: u64) -> Result<(), EngineError> {
        write_dump(path, cap, |sink| sink.put(payload))
    }

    fn dump_in(dir: &Path, payload: &[u8]) -> std::path::PathBuf {
        let path = dir.join("book.zkh");
        write_bytes(&path, payload, MAX_DUMP_BYTES).unwrap();
        path
    }

    #[test]
    fn a_dump_reads_back_exactly_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dump_in(dir.path(), b"some cells");
        assert_eq!(read_dump(&path, MAX_DUMP_BYTES).unwrap(), b"some cells");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_payload_at_the_cap_is_written_and_one_byte_over_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.zkh");
        write_bytes(&path, &[7; 64], 64).unwrap();
        assert_eq!(read_dump(&path, 64).unwrap().len(), 64);
        let refused = dir.path().join("over.zkh");
        let error = write_bytes(&refused, &[7; 65], 64).unwrap_err();
        assert!(matches!(error, EngineError::TooLarge(_)), "{error}");
        assert!(!refused.exists());
        assert!(matches!(read_dump(&path, 63), Err(EngineError::BadDump(_))));
    }

    #[test]
    fn a_dump_that_cannot_be_trusted_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dump_in(dir.path(), b"some cells to protect");
        let good = fs::read(&path).unwrap();
        let mut variants: Vec<Vec<u8>> = vec![Vec::new(), good[..HEADER_BYTES - 1].to_vec()];
        variants.push(good[..good.len() - 1].to_vec());
        let mut longer = good.clone();
        longer.push(0);
        variants.push(longer);
        for index in 0..good.len() {
            let mut flipped = good.clone();
            flipped[index] ^= 0x01;
            variants.push(flipped);
        }
        for bytes in variants {
            let broken = dir.path().join("broken.zkh");
            fs::write(&broken, &bytes).unwrap();
            assert!(read_dump(&broken, MAX_DUMP_BYTES).is_err());
            assert!(verify_file(&broken, MAX_DUMP_BYTES).is_err());
        }
    }

    #[test]
    fn a_dump_written_in_pieces_reads_back_whole_and_the_cap_counts_every_piece() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.zkh");
        write_dump(&path, 10, |sink| {
            sink.put(b"hello")?;
            sink.put(b"world")
        })
        .unwrap();
        assert_eq!(read_dump(&path, 10).unwrap(), b"helloworld");
        let over = dir.path().join("over.zkh");
        let error = write_dump(&over, 10, |sink| {
            sink.put(b"hello")?;
            sink.put(b"world!")
        })
        .unwrap_err();
        assert!(matches!(error, EngineError::TooLarge(_)), "{error}");
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file stays"
        );
    }

    #[test]
    fn a_temp_file_left_by_this_process_does_not_block_the_next_dump() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.zkh");
        fs::write(temp_path(&path), b"left behind").unwrap();
        write_bytes(&path, b"cells", MAX_DUMP_BYTES).unwrap();
        assert_eq!(read_dump(&path, MAX_DUMP_BYTES).unwrap(), b"cells");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_missing_dump_is_a_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = read_dump(&dir.path().join("none.zkh"), MAX_DUMP_BYTES).unwrap_err();
        assert!(matches!(error, EngineError::Read { .. }), "{error}");
    }
}
