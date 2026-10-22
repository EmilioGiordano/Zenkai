use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Mutex;

// The host has no console, so what stops it is written to the log the app writes to:
// %LOCALAPPDATA%\Zenkai\zenkai.log, appended to. Without a usable folder it stays silent.
pub fn init() {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(folder) = path.parent()
        && fs::create_dir_all(folder).is_err()
    {
        return;
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let installed = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .try_init();
    if installed.is_err() {
        // A subscriber is already set (a test); logging goes there.
    }
}

fn log_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("Zenkai").join("zenkai.log"))
}
