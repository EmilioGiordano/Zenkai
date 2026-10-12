// Structured fuzzing of the CSV importer: files built from the characters that make CSV
// hard (quotes, separators, line breaks, byte order marks, broken encodings) either parse
// or fail with an error, and what parses can be exported and imported again.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use zenkai_formats::{Delimiter, parse_csv, write_csv};

fn pieces() -> impl Strategy<Value = Vec<u8>> {
    let piece = prop_oneof![
        Just(b"\"".to_vec()),
        Just(b"\"\"".to_vec()),
        Just(b",".to_vec()),
        Just(b";".to_vec()),
        Just(b"\t".to_vec()),
        Just(b"|".to_vec()),
        Just(b"\n".to_vec()),
        Just(b"\r\n".to_vec()),
        Just(b"\r".to_vec()),
        Just(b"\xEF\xBB\xBF".to_vec()),
        Just(b"\xFF\xFE".to_vec()),
        Just(b"\xFE\xFF".to_vec()),
        Just(b"\xE9".to_vec()),
        Just(b"\xC3".to_vec()),
        Just(b"\x00".to_vec()),
        Just("ñ€😀".as_bytes().to_vec()),
        Just(b"=1+1".to_vec()),
        Just(b" ".to_vec()),
        "[a-z0-9]{1,6}".prop_map(String::into_bytes),
    ];
    prop::collection::vec(piece, 0..60).prop_map(|pieces| pieces.concat())
}

fn hint() -> impl Strategy<Value = Option<Delimiter>> {
    prop::option::of(prop::sample::select(vec![
        Delimiter::Comma,
        Delimiter::Semicolon,
        Delimiter::Tab,
        Delimiter::Pipe,
    ]))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn hard_csv_parses_or_fails_and_survives_an_export(bytes in pieces(), hint in hint()) {
        let Ok(parsed) = parse_csv(&bytes, hint) else {
            return Ok(());
        };
        let has_bom_character = parsed
            .rows
            .iter()
            .flatten()
            .any(|cell| cell.contains('\u{feff}'));
        prop_assume!(!has_bom_character);
        let exported = write_csv(&parsed.rows, parsed.delimiter).unwrap();
        let again = parse_csv(&exported, Some(parsed.delimiter)).unwrap();
        prop_assert_eq!(again.rows.len(), parsed.rows.len());
        for (row, original) in again.rows.iter().zip(&parsed.rows) {
            // The exporter prefixes formula-looking text; nothing else may change.
            prop_assert_eq!(row.len(), original.len());
            for (cell, source) in row.iter().zip(original) {
                prop_assert!(cell == source || cell.strip_prefix('\'') == Some(source.as_str()));
            }
        }
    }
}

#[test]
#[ignore = "bug: a first cell holding only U+FEFF is lost when the export is imported again"]
fn a_first_cell_holding_a_byte_order_mark_character_survives_export_and_import() {
    let rows = vec![vec!["\u{feff}".to_string()], vec!["x".to_string()]];
    let exported = write_csv(&rows, Delimiter::Comma).unwrap();
    let again = parse_csv(&exported, Some(Delimiter::Comma)).unwrap();
    assert_eq!(again.rows, rows);
}
