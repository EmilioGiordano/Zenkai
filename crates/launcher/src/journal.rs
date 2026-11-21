use std::fs::OpenOptions;
use std::sync::Mutex;

use crate::disk;
use crate::folders::Folders;

const MAX_BYTES: u64 = 256 * 1024;

// This log is the launcher's only way to report, so failing to open it cannot be reported.
pub fn init(folders: &Folders) {
    let path = folders.journal();
    if disk::prepare_update_folder(folders).is_err() {
        return;
    }
    let over_cap = std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_BYTES);
    let Ok(file) = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!over_cap)
        .truncate(over_cap)
        .open(path)
    else {
        return;
    };
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .init();
}
