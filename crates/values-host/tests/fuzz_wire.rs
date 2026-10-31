// Fuzzing of the decoder of the host's frames: whatever bytes arrive on the pipe, reading
// and decoding frames ends in messages or errors, never a panic, and frames that do decode
// are within the caps.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use zenkai_pdf_host::frame::{read_frame, write_frame};
use zenkai_values_host::wire::{Failure, FailureKind, Message, decode, encode};

fn valid_stream() -> Vec<u8> {
    let rows = vec![
        vec!["a".to_string(), "b".to_string()],
        vec!["c".to_string()],
    ];
    let messages = [
        (Message::Sheet { name: "S".into() }, Vec::new()),
        (
            Message::Block {
                first_row: 1,
                first_col: 2,
            },
            rows,
        ),
        (
            Message::Failed(Failure {
                kind: FailureKind::Panicked,
                detail: "boom".into(),
            }),
            Vec::new(),
        ),
        (Message::Done, Vec::new()),
    ];
    let mut bytes = Vec::new();
    for (message, rows) in messages {
        write_frame(&mut bytes, &encode(&message, &rows).unwrap()).unwrap();
    }
    bytes
}

fn drain(mut bytes: &[u8]) {
    while let Ok(Some(frame)) = read_frame(&mut bytes) {
        if let Ok((_, rows)) = decode(&frame) {
            assert!(rows.iter().all(|row| row.len() <= frame.payload.len()));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn random_bytes_never_panic_the_decoder(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        drain(&bytes);
    }

    #[test]
    fn a_damaged_valid_stream_never_panics_the_decoder(
        at in any::<prop::sample::Index>(),
        how in 0u8..4,
        byte in any::<u8>(),
    ) {
        let mut bytes = valid_stream();
        let at = at.index(bytes.len());
        match how {
            0 => bytes[at] ^= byte | 1,
            1 => bytes.truncate(at),
            2 => { bytes.remove(at); }
            _ => bytes.insert(at, byte),
        }
        drain(&bytes);
    }
}

#[test]
fn the_valid_stream_decodes_to_its_four_messages() {
    let mut bytes = valid_stream().into_iter().collect::<Vec<_>>();
    let mut reader = bytes.as_mut_slice() as &[u8];
    let mut count = 0;
    while let Some(frame) = read_frame(&mut reader).unwrap() {
        decode(&frame).unwrap();
        count += 1;
    }
    assert_eq!(count, 4);
}
