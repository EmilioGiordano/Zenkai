// A stand-in for the values host used only by the tests: it misbehaves the way the mode in
// ZENKAI_FAKE_VALUES_HOST says, so the client can be shown to survive each way a child can end.
#![forbid(unsafe_code)]

use std::error::Error;
use std::io::Write;
use std::time::Duration;

use zenkai_pdf_host::frame::write_frame;
use zenkai_values_host::wire::{Message, encode};

fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::var("ZENKAI_FAKE_VALUES_HOST").unwrap_or_default();
    if let Some(file) = std::env::var_os("ZENKAI_FAKE_VALUES_HOST_PID_FILE") {
        std::fs::write(file, std::process::id().to_string())?;
    }
    zenkai_values_host::host::exit_when_stdin_closes()?;
    let mut out = std::io::stdout().lock();
    let sheet = Message::Sheet { name: "S".into() };
    let block = Message::Block {
        first_row: 0,
        first_col: 0,
    };
    match mode.as_str() {
        "abort" => std::process::abort(),
        "exit" => {}
        "hang" => hang(),
        "hang_after_sheet" => {
            send(&mut out, &sheet, &[])?;
            hang();
        }
        "garbage" => out.write_all(b"this is not a frame at all")?,
        "block_without_sheet" => send(&mut out, &block, &[vec!["a".to_string()]])?,
        "big_block" => {
            send(&mut out, &sheet, &[])?;
            send(&mut out, &block, &[vec!["x".repeat(1000)]])?;
            send(&mut out, &Message::Done, &[])?;
        }
        "empty_block" => {
            send(&mut out, &sheet, &[])?;
            send(&mut out, &block, &[])?;
        }
        "flood" => loop {
            send(&mut out, &sheet, &[])?;
        },
        _ => send(&mut out, &Message::Done, &[])?,
    }
    Ok(())
}

fn hang() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn send(
    out: &mut impl Write,
    message: &Message,
    rows: &[Vec<String>],
) -> Result<(), Box<dyn Error>> {
    write_frame(out, &encode(message, rows)?)?;
    Ok(())
}
