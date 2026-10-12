use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter);
    // Release builds run without a console (windows_subsystem), so stdout goes nowhere.
    if cfg!(debug_assertions) {
        subscriber.init();
        return;
    }
    match open_log_file() {
        Ok(file) => subscriber
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        Err(error) => {
            subscriber.init();
            tracing::warn!(%error, "could not open the log file");
        }
    }
}

pub fn log_path() -> Option<PathBuf> {
    Some(crate::recovery::directory()?.parent()?.join("zenkai.log"))
}

fn open_log_file() -> io::Result<File> {
    let path = log_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no local data folder"))?;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    // Several instances may write at once, so the file is appended to and only reset
    // at startup once it passes the cap; renaming it would pull it from under them.
    let over_cap = fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES);
    OpenOptions::new()
        .create(true)
        .write(true)
        .append(!over_cap)
        .truncate(over_cap)
        .open(path)
}

// Release builds have no console, so a panic would otherwise vanish without a trace.
pub fn log_panics() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map_or_else(
            || "unknown location".to_string(),
            |location| format!("{}:{}", location.file(), location.line()),
        );
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-text panic payload".to_string());
        tracing::error!(%location, %message, "panic");
        default_hook(info);
    }));
}
