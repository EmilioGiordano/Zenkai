// A stand-in for the values host used only by the tests: it misbehaves the way the mode in
// ZENKAI_FAKE_VALUES_HOST says, so the client can be shown to survive each way a child can end.
#![forbid(unsafe_code)]

use std::io::Write;
use std::time::Duration;

use zenkai_pdf_host::frame::{Frame, write_frame};
use zenkai_values_host::wire::{Message, encode};

fn main() {
    let mode = std::env::var("ZENKAI_FAKE_VALUES_HOST").unwrap_or_default();
    if let Some(file) = std::env::var_os("ZENKAI_FAKE_VALUES_HOST_PID_FILE")
        && std::fs::write(file, std::process::id().to_string()).is_err()
    {
        std::process::exit(2);
    }
    let mut out = std::io::stdout().lock();
    match mode.as_str() {
        "abort" => std::process::abort(),
        "exit" => {}
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "hang_after_sheet" => {
            send(&mut out, &Message::Sheet { name: "S".into() });
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "garbage" => {
            out.write_all(b"this is not a frame at all")
                .unwrap_or_default();
        }
        "block_without_sheet" => {
            let message = Message::Block {
                first_row: 0,
                first_col: 0,
            };
            let frame = encode(&message, &[vec!["a".to_string()]]).unwrap_or_else(|_| empty());
            write_frame(&mut out, &frame).unwrap_or_default();
        }
        "flood" => loop {
            send(&mut out, &Message::Sheet { name: "S".into() });
        },
        _ => send(&mut out, &Message::Done),
    }
}

fn empty() -> Frame {
    Frame {
        head: Vec::new(),
        payload: Vec::new(),
    }
}

fn send(out: &mut impl Write, message: &Message) {
    let frame = encode(message, &[]).unwrap_or_else(|_| empty());
    if write_frame(out, &frame).is_err() {
        std::process::exit(1);
    }
}
