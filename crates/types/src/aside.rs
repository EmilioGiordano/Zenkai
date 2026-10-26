use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const ASIDE_ATTEMPTS: u32 = 100;

// Names tried are `<file>.<extension>`, then `<file>.1.<extension>` and so on, so an earlier
// quarantined copy is never replaced. None once every name is taken.
pub fn free_aside_name(file: &Path, extension: &str) -> Option<PathBuf> {
    let name = file.file_name()?;
    (0..ASIDE_ATTEMPTS)
        .map(|attempt| {
            let mut candidate: OsString = name.to_os_string();
            if attempt > 0 {
                candidate.push(format!(".{attempt}"));
            }
            candidate.push(format!(".{extension}"));
            file.with_file_name(candidate)
        })
        .find(|candidate| !candidate.exists())
}
