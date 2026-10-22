#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

use std::process::ExitCode;

fn main() -> ExitCode {
    match zenkai_pdf_host::host::run_stdio() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
