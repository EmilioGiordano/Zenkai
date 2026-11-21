use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::disk::ERROR_SHARING_VIOLATION;

const DELETE: u32 = 0x0001_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;

// Open without sharing and deleted by Windows when the handle closes, so a launcher that crashes
// or is killed never leaves the lock held.
pub struct Lock {
    _file: File,
}

pub enum Taken {
    Held(Lock),
    Busy,
    Unavailable(io::Error),
}

pub fn take(path: &Path, wait: Duration, poll: Duration) -> Taken {
    if let Some(folder) = path.parent()
        && let Err(error) = fs::create_dir_all(folder)
    {
        return Taken::Unavailable(error);
    }
    let deadline = Instant::now() + wait;
    loop {
        match open(path) {
            Ok(file) => return Taken::Held(Lock { _file: file }),
            Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => {
                if Instant::now() >= deadline {
                    return Taken::Busy;
                }
                sleep(poll);
            }
            Err(error) => return Taken::Unavailable(error),
        }
    }
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .access_mode(GENERIC_WRITE | DELETE)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Taken, take};

    const POLL: Duration = Duration::from_millis(10);

    #[test]
    fn a_second_launcher_waits_and_then_gives_up() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".update").join("lock");
        let Taken::Held(first) = take(&path, Duration::ZERO, POLL) else {
            panic!("the first launcher did not get the lock");
        };
        let started = std::time::Instant::now();
        assert!(matches!(
            take(&path, Duration::from_millis(200), POLL),
            Taken::Busy
        ));
        assert!(started.elapsed() >= Duration::from_millis(200));
        drop(first);
        assert!(matches!(take(&path, Duration::ZERO, POLL), Taken::Held(_)));
    }

    #[test]
    fn the_lock_file_goes_away_with_its_holder() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".update").join("lock");
        let Taken::Held(lock) = take(&path, Duration::ZERO, POLL) else {
            panic!("no lock");
        };
        assert!(path.exists());
        drop(lock);
        assert!(!path.exists());
    }

    #[test]
    fn a_lock_file_left_behind_does_not_block() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".update").join("lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"left by something else").unwrap();
        assert!(matches!(take(&path, Duration::ZERO, POLL), Taken::Held(_)));
    }

    #[test]
    fn a_folder_that_cannot_hold_the_lock_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let blocker = root.path().join(".update");
        std::fs::write(&blocker, b"a file where the folder goes").unwrap();
        assert!(matches!(
            take(&blocker.join("lock"), Duration::from_secs(5), POLL),
            Taken::Unavailable(_)
        ));
    }
}
