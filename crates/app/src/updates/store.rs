use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("the session directory is unknown")]
    NoDirectory,
    #[error("could not write the file: {0}")]
    Write(#[from] std::io::Error),
    #[error("could not encode the file: {0}")]
    Encode(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("could not read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the file is larger than {0} bytes")]
    TooLarge(u64),
}

// A file past the limit is refused whole, never read in part.
pub fn read_capped(file: &Path, limit: u64) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(ReadError::TooLarge(limit));
    }
    Ok(bytes)
}

// A snapshot older than the one on disk is skipped: the newer one already holds its data.
pub fn write_ordered(
    file: &Path,
    text: &str,
    order: u64,
    last_written: &Mutex<u64>,
) -> Result<(), SaveError> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut last_written = last_written.lock().unwrap_or_else(PoisonError::into_inner);
    if order <= *last_written {
        return Ok(());
    }
    let temp = file.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, file)?;
    *last_written = order;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{ReadError, SaveError, read_capped, write_ordered};

    #[test]
    fn a_file_within_the_limit_is_read_whole() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a.json");
        std::fs::write(&file, b"12345").unwrap();
        assert_eq!(read_capped(&file, 5).unwrap(), b"12345");
    }

    #[test]
    fn a_file_one_byte_over_the_limit_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a.json");
        std::fs::write(&file, b"123456").unwrap();
        assert!(matches!(read_capped(&file, 5), Err(ReadError::TooLarge(5))));
    }

    #[test]
    fn a_missing_file_is_an_io_error_that_says_not_found() {
        let folder = tempfile::tempdir().unwrap();
        let result = read_capped(&folder.path().join("none.json"), 5);
        assert!(
            matches!(result, Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn a_file_is_written_into_a_new_folder() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("session").join("a.json");
        write_ordered(&file, "one", 1, &Mutex::new(0)).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"one");
    }

    #[test]
    fn an_older_snapshot_landing_late_does_not_overwrite_a_newer_one() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a.json");
        let last_written = Mutex::new(0);
        write_ordered(&file, "newer", 2, &last_written).unwrap();
        write_ordered(&file, "older", 1, &last_written).unwrap();
        write_ordered(&file, "same", 2, &last_written).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"newer");
    }

    #[test]
    fn a_failed_write_does_not_move_the_order_forward() {
        let folder = tempfile::tempdir().unwrap();
        let blocker = folder.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let last_written = Mutex::new(0);
        let result = write_ordered(&blocker.join("a.json"), "x", 5, &last_written);
        assert!(matches!(result, Err(SaveError::Write(_))));
        assert_eq!(*last_written.lock().unwrap(), 0);
    }
}
