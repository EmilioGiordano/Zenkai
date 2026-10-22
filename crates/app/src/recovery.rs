use std::path::{Path, PathBuf};

use zenkai_engine::{EngineError, SaveSnapshot, save_xlsx};
use zenkai_types::WorkbookId;

pub fn session_directory() -> Option<PathBuf> {
    Some(directory()?.parent()?.to_path_buf())
}

const PREFIX: &str = "autosave-";
const WORKBOOK_EXTENSION: &str = "xlsx";
const TEXT_EXTENSION: &str = "md";
// Not `.xlsx`, so `leftovers` never offers a dump as work to recover: it only holds the undo
// history of a workbook whose work the autosave copy keeps.
const DUMP_EXTENSION: &str = "zkh";

pub fn directory() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("Zenkai").join("recovery"))
}

// Pid plus start time: a reused pid must never pick up a crashed session's file.
fn session_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        format!("{}-{started}", std::process::id())
    })
}

pub fn document_file(directory: &Path, id: WorkbookId) -> PathBuf {
    directory.join(format!(
        "{PREFIX}{}-{}.{WORKBOOK_EXTENSION}",
        session_name(),
        id.0
    ))
}

pub fn text_file(directory: &Path, id: WorkbookId) -> PathBuf {
    directory.join(format!(
        "{PREFIX}{}-{}.{TEXT_EXTENSION}",
        session_name(),
        id.0
    ))
}

pub fn has_recovery_extension(name: &str) -> bool {
    [WORKBOOK_EXTENSION, TEXT_EXTENSION]
        .iter()
        .any(|extension| name.ends_with(&format!(".{extension}")))
}

pub fn is_text(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == TEXT_EXTENSION)
}

pub fn dump_file(directory: &Path, id: WorkbookId) -> PathBuf {
    directory.join(format!(
        "{PREFIX}{}-{}.{DUMP_EXTENSION}",
        session_name(),
        id.0
    ))
}

fn lock_file(directory: &Path) -> PathBuf {
    directory.join(format!("{PREFIX}{}.lock", session_name()))
}

// One lock per session guards the recovery files of all its documents.
fn lock_of(autosave: &Path) -> PathBuf {
    let stem = autosave
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let session = stem
        .rsplit_once('-')
        .map_or(stem.as_str(), |(session, _)| session);
    autosave.with_file_name(format!("{session}.lock"))
}

// Held for the whole session; an autosave whose lock can still be taken belongs to
// a process that is gone, so only those are offered for recovery.
pub struct SessionLock {
    _file: std::fs::File,
}

pub fn lock_session(directory: &Path) -> std::io::Result<SessionLock> {
    std::fs::create_dir_all(directory)?;
    let file = std::fs::File::create(lock_file(directory))?;
    file.try_lock().map_err(std::io::Error::other)?;
    Ok(SessionLock { _file: file })
}

fn owner_is_alive(autosave: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .open(lock_of(autosave))
    else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

fn is_autosave(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(PREFIX) && has_recovery_extension(name))
}

pub fn leftovers(directory: &Path) -> Vec<PathBuf> {
    let own_lock = lock_file(directory);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| lock_of(path) != own_lock && is_autosave(path) && !owner_is_alive(path))
        .collect();
    found.sort();
    found
}

fn is_dump(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(PREFIX) && name.ends_with(&format!(".{DUMP_EXTENSION}"))
        })
}

// The dump a file belongs to: the dump itself, or the temp file the engine writes it through
// (`~$<dump name>.<pid>.tmp`) and leaves behind if the process dies mid-write.
fn dump_of(path: &Path) -> Option<PathBuf> {
    if is_dump(path) {
        return Some(path.to_path_buf());
    }
    let name = path.file_name()?.to_str()?;
    let name = name.strip_prefix("~$")?.strip_suffix(".tmp")?;
    let (dump_name, process) = name.rsplit_once('.')?;
    if process.is_empty() || !process.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let dump = path.with_file_name(dump_name);
    is_dump(&dump).then_some(dump)
}

// A dump of a session that is gone is useless: its memory went with it, and the autosave
// copy next to it is what recovery offers.
pub fn remove_dead_dumps(directory: &Path) {
    let own_lock = lock_file(directory);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for path in entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            dump_of(path).is_some_and(|dump| lock_of(&dump) != own_lock && !owner_is_alive(&dump))
        })
    {
        remove(&path);
    }
}

// Every dump of this session, at the end of it: a hibernated workbook is not loaded again.
pub fn remove_own_dumps(directory: &Path) {
    let own_lock = lock_file(directory);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for path in entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| dump_of(path).is_some_and(|dump| lock_of(&dump) == own_lock))
    {
        remove(&path);
    }
}

pub fn write(snapshot: SaveSnapshot) -> Result<(), EngineError> {
    if let Some(parent) = snapshot.path().parent() {
        std::fs::create_dir_all(parent).map_err(|source| EngineError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    save_xlsx(snapshot)
}

// Moves a dead session's copy under this session's name; the copy stays where it was when the
// move fails, since it may be the only one holding the work.
pub fn adopt(from: &Path, to: &Path) -> std::io::Result<()> {
    // A hibernated workbook that falls back to its recovery copy loads this session's own copy,
    // which is already where it belongs.
    if from == to {
        return Ok(());
    }
    std::fs::rename(from, to)?;
    remove_stale_lock(from);
    Ok(())
}

// A lock still held belongs to a running session, this one included, and stays.
fn remove_stale_lock(path: &Path) {
    if !owner_is_alive(path) {
        remove(&lock_of(path));
    }
}

pub fn remove_with_lock(path: &Path) {
    remove(path);
    remove_stale_lock(path);
}

pub fn remove(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(?path, %error, "could not remove a recovery file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_engine::{Engine, SaveTarget, Workbook};
    use zenkai_types::{CellPos, SheetId, WorkbookId};

    #[test]
    fn autosave_round_trips_and_leftovers_skip_our_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), CellPos::default(), "42")
            .unwrap();
        let own = document_file(dir.path(), WorkbookId(0));
        let sibling = document_file(dir.path(), WorkbookId(1));
        write(book.save_snapshot(&own, SaveTarget::Copy).unwrap()).unwrap();
        write(book.save_snapshot(&own, SaveTarget::Copy).unwrap()).unwrap();
        write(book.save_snapshot(&sibling, SaveTarget::Copy).unwrap()).unwrap();
        let other = dir.path().join("autosave-1-1-0.xlsx");
        std::fs::copy(&own, &other).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert_eq!(leftovers(dir.path()), vec![other.clone()]);
        let reopened = zenkai_engine::open_xlsx(&other).unwrap();
        assert_eq!(
            reopened.workbook.cell(SheetId(0), CellPos::default()).text,
            "42"
        );
        remove(&other);
        remove(&other);
        assert!(leftovers(dir.path()).is_empty());

        let _lock = lock_session(dir.path()).unwrap();
        let live = dir.path().join("autosave-2-2-0.xlsx");
        std::fs::copy(&own, &live).unwrap();
        let live_lock = std::fs::File::create(lock_of(&live)).unwrap();
        live_lock.lock().unwrap();
        assert!(
            leftovers(dir.path()).is_empty(),
            "a live session is not a leftover"
        );
        drop(live_lock);
        assert_eq!(leftovers(dir.path()), vec![live.clone()]);
    }

    #[test]
    fn a_text_copy_is_a_leftover_like_a_workbook_copy_and_is_told_apart_by_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = lock_session(dir.path()).unwrap();
        let own = text_file(dir.path(), WorkbookId(2));
        assert!(is_text(&own));
        assert!(!is_text(&document_file(dir.path(), WorkbookId(2))));
        std::fs::write(&own, "mine").unwrap();
        let other = dir.path().join("autosave-9-9-1.md");
        std::fs::write(&other, "left by a crash").unwrap();
        std::fs::write(dir.path().join("notes.md"), "not ours").unwrap();
        assert_eq!(leftovers(dir.path()), vec![other.clone()]);
        remove_with_lock(&other);
        assert!(leftovers(dir.path()).is_empty());
        assert!(own.exists());
    }

    #[test]
    fn dumps_are_never_offered_for_recovery_and_only_a_dead_session_loses_them() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = lock_session(dir.path()).unwrap();
        let own = dump_file(dir.path(), WorkbookId(1));
        let dead = dir.path().join("autosave-1-1-0.zkh");
        let live = dir.path().join("autosave-2-2-0.zkh");
        for dump in [&own, &dead, &live] {
            std::fs::write(dump, b"x").unwrap();
        }
        let live_lock = std::fs::File::create(lock_of(&live)).unwrap();
        live_lock.lock().unwrap();
        assert!(leftovers(dir.path()).is_empty());
        remove_dead_dumps(dir.path());
        assert!(own.exists() && live.exists() && !dead.exists());
        remove_own_dumps(dir.path());
        assert!(!own.exists() && live.exists());
    }

    #[test]
    fn the_temp_file_of_a_dump_goes_with_the_dump() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = lock_session(dir.path()).unwrap();
        let own = dump_file(dir.path(), WorkbookId(1));
        let own_name = own.file_name().unwrap().to_string_lossy().into_owned();
        let own_temp = dir.path().join(format!("~${own_name}.4242.tmp"));
        let dead_temp = dir.path().join("~$autosave-1-1-0.zkh.99.tmp");
        let live_temp = dir.path().join("~$autosave-2-2-0.zkh.99.tmp");
        let stranger = dir.path().join("~$report.xlsx.99.tmp");
        for file in [&own_temp, &dead_temp, &live_temp, &stranger] {
            std::fs::write(file, b"x").unwrap();
        }
        let live_lock = std::fs::File::create(dir.path().join("autosave-2-2.lock")).unwrap();
        live_lock.lock().unwrap();
        remove_dead_dumps(dir.path());
        assert!(!dead_temp.exists());
        assert!(own_temp.exists() && live_temp.exists() && stranger.exists());
        remove_own_dumps(dir.path());
        assert!(!own_temp.exists());
        assert!(live_temp.exists() && stranger.exists());
    }

    #[test]
    fn discarding_a_leftover_never_deletes_the_lock_of_a_live_session() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = lock_session(dir.path()).unwrap();
        let closed = document_file(dir.path(), WorkbookId(3));
        let open = document_file(dir.path(), WorkbookId(4));
        std::fs::write(&closed, b"x").unwrap();
        std::fs::write(&open, b"x").unwrap();
        remove_with_lock(&closed);
        assert!(!closed.exists());
        assert!(owner_is_alive(&open));
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn adopting_moves_the_copy_and_keeps_it_when_the_move_fails() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("autosave-1-1-0.xlsx");
        std::fs::write(&old, b"work").unwrap();
        let target = dir.path().join("autosave-2-2-0.xlsx");
        adopt(&old, &target).unwrap();
        assert!(!old.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"work");

        adopt(&target, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"work");

        let kept = dir.path().join("autosave-3-3-0.xlsx");
        std::fs::write(&kept, b"work").unwrap();
        let impossible = dir.path().join("missing-folder").join("copy.xlsx");
        assert!(adopt(&kept, &impossible).is_err());
        assert!(kept.exists());
    }
}
