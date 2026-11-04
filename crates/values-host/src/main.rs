#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        return ExitCode::FAILURE;
    };
    let mut out = BufWriter::new(std::io::stdout().lock());
    match zenkai_values_host::host::run(&path, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
