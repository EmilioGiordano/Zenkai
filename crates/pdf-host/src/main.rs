#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

use std::process::ExitCode;

fn main() -> ExitCode {
    zenkai_pdf_host::host::init_logging();
    match zenkai_pdf_host::host::run_stdio() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the PDF host stopped");
            ExitCode::FAILURE
        }
    }
}
