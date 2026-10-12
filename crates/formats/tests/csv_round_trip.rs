// Spec: a table written as CSV and imported again gives back the same values, for every
// delimiter and every encoding the importer accepts.
#![allow(clippy::unwrap_used)]

use encoding_rs::WINDOWS_1252;
use proptest::prelude::*;
use zenkai_formats::{Delimiter, Encoding, parse_csv, write_csv};

const DELIMITERS: [Delimiter; 4] = [
    Delimiter::Comma,
    Delimiter::Semicolon,
    Delimiter::Tab,
    Delimiter::Pipe,
];

// The first character is never one the exporter prefixes against formula injection.
fn cell(tail: &'static str) -> impl Strategy<Value = String> {
    (
        prop::sample::select(vec!['a', 'Z', '7', 'é', '_', ' ', '"', ',', ';', '|']),
        prop::collection::vec(prop::sample::select(tail.chars().collect::<Vec<_>>()), 0..8),
    )
        .prop_map(|(head, tail)| std::iter::once(head).chain(tail).collect())
}

const ASCII_TAIL: &str = "ab1 ,;|\"\n\r\t'=+-@";
const LATIN_TAIL: &str = "ab1 ,;|\"\né\u{f1}\u{fc}";

fn table(tail: &'static str) -> impl Strategy<Value = Vec<Vec<String>>> {
    (1usize..5, 1usize..6).prop_flat_map(move |(width, height)| {
        prop::collection::vec(prop::collection::vec(cell(tail), width), height)
    })
}

fn delimiter() -> impl Strategy<Value = Delimiter> {
    prop::sample::select(DELIMITERS.to_vec())
}

fn without_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn utf8_with_bom_round_trips_for_every_delimiter(rows in table(ASCII_TAIL), delimiter in delimiter()) {
        let bytes = write_csv(&rows, delimiter).unwrap();
        let parsed = parse_csv(&bytes, Some(delimiter)).unwrap();
        prop_assert_eq!(parsed.encoding, Encoding::Utf8Bom);
        prop_assert_eq!(parsed.rows, rows);
    }

    #[test]
    fn utf8_without_bom_round_trips_for_every_delimiter(rows in table(ASCII_TAIL), delimiter in delimiter()) {
        let bytes = write_csv(&rows, delimiter).unwrap();
        let parsed = parse_csv(without_bom(&bytes), Some(delimiter)).unwrap();
        prop_assert_eq!(parsed.encoding, Encoding::Utf8);
        prop_assert_eq!(parsed.rows, rows);
    }

    #[test]
    fn utf16_round_trips_for_every_delimiter(rows in table(ASCII_TAIL), delimiter in delimiter(), little in any::<bool>()) {
        let text = String::from_utf8(without_bom(&write_csv(&rows, delimiter).unwrap()).to_vec()).unwrap();
        let mut bytes = if little { vec![0xFF, 0xFE] } else { vec![0xFE, 0xFF] };
        for unit in text.encode_utf16() {
            bytes.extend(if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
        }
        let parsed = parse_csv(&bytes, Some(delimiter)).unwrap();
        prop_assert_eq!(parsed.encoding, if little { Encoding::Utf16Le } else { Encoding::Utf16Be });
        prop_assert_eq!(parsed.rows, rows);
    }

    #[test]
    fn windows_1252_round_trips_for_every_delimiter(rows in table(LATIN_TAIL), delimiter in delimiter()) {
        let text = String::from_utf8(without_bom(&write_csv(&rows, delimiter).unwrap()).to_vec()).unwrap();
        let (bytes, _, unmappable) = WINDOWS_1252.encode(&text);
        prop_assert!(!unmappable);
        let parsed = parse_csv(&bytes, Some(delimiter)).unwrap();
        prop_assert_eq!(parsed.rows, rows);
    }

    #[test]
    fn formula_looking_text_is_exported_neutralized_and_never_loses_its_characters(body in "[A-Z][A-Z0-9(),]{0,7}", lead in prop::sample::select(vec!['=', '+', '-', '@'])) {
        let rows = vec![vec![format!("{lead}{body}")]];
        let bytes = write_csv(&rows, Delimiter::Comma).unwrap();
        let parsed = parse_csv(&bytes, Some(Delimiter::Comma)).unwrap();
        prop_assert_eq!(parsed.rows[0][0].clone(), format!("'{lead}{body}"));
    }

    #[test]
    fn detected_delimiter_matches_the_one_written_for_plain_tables(
        rows in (2usize..5, 2usize..5).prop_flat_map(|(width, height)| {
            prop::collection::vec(prop::collection::vec("[a-z0-9]{1,5}", width), height)
        }),
        delimiter in delimiter(),
    ) {
        let bytes = write_csv(&rows, delimiter).unwrap();
        let parsed = parse_csv(&bytes, None).unwrap();
        prop_assert_eq!(parsed.delimiter, delimiter);
        prop_assert_eq!(parsed.rows, rows);
    }
}
