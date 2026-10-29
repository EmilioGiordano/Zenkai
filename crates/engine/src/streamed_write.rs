use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::error::EngineError;
use crate::file::{create_temp, remove_temp};

struct Counted<W> {
    inner: W,
    bytes: u64,
}

impl<W: Write> Write for Counted<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// `write_atomic` for output too large to hold or re-read whole, such as the CSV of a huge
// sheet. The verify is bounded instead: the temp file must hold exactly the bytes written.
pub fn write_atomic_streaming(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let (file, temp) = create_temp(path)?;
    let mut counted = Counted {
        inner: BufWriter::new(file),
        bytes: 0,
    };
    let written = write(&mut counted).and_then(|()| sync_and_verify(counted, &temp));
    if let Err(error) = written {
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

fn sync_and_verify(counted: Counted<BufWriter<File>>, temp: &Path) -> Result<(), EngineError> {
    let Counted { inner, bytes } = counted;
    let write_error = |source| EngineError::Write {
        path: temp.to_path_buf(),
        source,
    };
    let file = inner
        .into_inner()
        .map_err(|error| write_error(error.into_error()))?;
    file.sync_all().map_err(write_error)?;
    drop(file);
    let on_disk = fs::metadata(temp)
        .map_err(|e| EngineError::VerifyFailed(e.to_string()))?
        .len();
    if on_disk != bytes {
        return Err(EngineError::VerifyFailed(format!(
            "the file on disk holds {on_disk} bytes of the {bytes} written"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn replaces_the_target_with_everything_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        fs::write(&path, b"old").unwrap();
        write_atomic_streaming(&path, |out| {
            for _ in 0..10_000 {
                out.write_all(b"row\r\n")
                    .map_err(|source| EngineError::Write {
                        path: PathBuf::new(),
                        source,
                    })?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"row\r\n".repeat(10_000));
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_failed_write_leaves_the_target_untouched_and_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        fs::write(&path, b"old").unwrap();
        let failed = write_atomic_streaming(&path, |out| {
            out.write_all(&[b'x'; 100_000]).unwrap();
            Err(EngineError::Rejected("stopped halfway".to_string()))
        });
        assert!(matches!(failed, Err(EngineError::Rejected(_))));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_temp_holding_other_bytes_fails_the_verify() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        fs::write(&path, b"old").unwrap();
        let failed = write_atomic_streaming(&path, |out| {
            out.write_all(b"new").unwrap();
            out.flush().unwrap();
            let temp = dir.path().join(&leftovers(dir.path())[0]);
            let mut intruder = fs::OpenOptions::new().append(true).open(&temp).unwrap();
            intruder.write_all(b"extra").unwrap();
            Ok(())
        });
        assert!(matches!(failed, Err(EngineError::VerifyFailed(_))));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_leftover_temp_does_not_block_the_write_and_is_not_touched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        let leftovers = crate::file::Leftovers::plant(dir.path(), "out.csv");
        write_atomic_streaming(&path, |out| {
            out.write_all(b"new").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(leftovers.skipped_all());
        leftovers.assert_untouched();
    }
}
