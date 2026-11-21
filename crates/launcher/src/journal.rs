use std::fs::OpenOptions;
use std::sync::Mutex;

use crate::folders::Folders;

const MAX_BYTES: u64 = 256 * 1024;

// The launcher has no window to report to, so what it does and every error it meets go to
// `.update\launcher.log`, which outlives the cleanup after an update. Without it, it stays silent.
pub fn init(folders: &Folders) {
    let path = folders.journal();
    if std::fs::create_dir_all(folders.update()).is_err() {
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
