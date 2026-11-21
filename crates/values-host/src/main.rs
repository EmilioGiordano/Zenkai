#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    zenkai_pdf_host::logging::init();
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        tracing::error!("the values host was started without a file");
        return ExitCode::FAILURE;
    };
    if let Err(error) = zenkai_values_host::host::exit_when_stdin_closes() {
        tracing::error!(%error, "the values host could not watch its stdin");
        return ExitCode::FAILURE;
    }
    let mut out = BufWriter::new(std::io::stdout().lock());
    match zenkai_values_host::host::run(&path, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the values host stopped");
            ExitCode::FAILURE
        }
    }
}
