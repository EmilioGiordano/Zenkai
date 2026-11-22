// Structured fuzzing of the drawing scan, which reads parts of a hostile file: damaged
// drawings never panic, an answer never depends on how the bytes arrive, and what is not
// UTF-8 or holds a NUL always names cells.

use std::io::{self, Read};

use proptest::prelude::*;
use proptest::sample::Index;

use super::names_cells;
use crate::nesting::MAX_XML_DEPTH;

const DRAWING: &str = concat!(
    r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" "#,
    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
    r#"<xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:row>2</xdr:row></xdr:from>"#,
    r#"<xdr:pic macro=""/></xdr:twoCellAnchor><xdr:sp macro="" textlink=""><xdr:txBody>"#,
    r#"<a:p><a:r><a:t>Café</a:t></a:r></a:p></xdr:txBody></xdr:sp></xdr:wsDr>"#
);
const NOTE: &str = concat!(
    r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:x="urn:schemas-microsoft-com:office:excel">"#,
    r#"<v:shape style='position:absolute'><x:ClientData ObjectType="Note"><x:Row>0</x:Row>"#,
    r#"<x:Column>0</x:Column></x:ClientData></v:shape></xml>"#
);

// One seed for each path through the scan: no cell named, each kind of hit, and each way a
// drawing fails closed.
fn seeds() -> Vec<Vec<u8>> {
    let deep = format!(
        "{}{}",
        "<g>".repeat(MAX_XML_DEPTH + 1),
        "</g>".repeat(MAX_XML_DEPTH + 1)
    );
    let utf16: Vec<u8> = DRAWING.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut seeds: Vec<Vec<u8>> = [
        DRAWING.to_string(),
        NOTE.to_string(),
        DRAWING.replace(r#"textlink="""#, r#"textlink="Sheet2!A1""#),
        DRAWING.replace("<a:t>", r#"<a:fld type="TxLink"/><a:t>"#),
        NOTE.replace("<x:Row>", "<x:FmlaLink>$B$2</x:FmlaLink><x:Row>"),
        DRAWING.replace("<xdr:pic", r#"<a14:cameraTool cellRange="A1:B2"/><xdr:pic"#),
        format!(r#"<?xml version="1.0" encoding="windows-1252"?>{DRAWING}"#),
        format!("<!DOCTYPE wsDr>{DRAWING}"),
        DRAWING.replace("</xdr:sp>", "</xdr:pic>"),
        deep,
    ]
    .into_iter()
    .map(String::into_bytes)
    .collect();
    seeds.push(utf16);
    let mut latin1 = DRAWING.as_bytes().to_vec();
    latin1.push(0xE9);
    seeds.push(latin1);
    seeds
}

const TOKENS: [&[u8]; 14] = [
    br#"<xdr:sp textlink="A1"/>"#,
    b"</",
    b"<",
    b">",
    b"\"",
    b"<!DOCTYPE x>",
    br#"<?xml version="1.0" encoding="UTF-16"?>"#,
    b"<x:FmlaLink>A1</x:FmlaLink>",
    b"<!--",
    b"-->",
    b"\0",
    b"\xC3",
    "\u{FEFF}".as_bytes(),
    "東".as_bytes(),
];

#[derive(Clone, Debug)]
enum Damage {
    Flip(Index, u8),
    Insert(Index, Index),
    Delete(Index, usize),
    Cut(Index),
}

fn damage() -> impl Strategy<Value = Damage> {
    prop_oneof![
        (any::<Index>(), any::<u8>()).prop_map(|(at, byte)| Damage::Flip(at, byte)),
        (any::<Index>(), any::<Index>()).prop_map(|(at, token)| Damage::Insert(at, token)),
        (any::<Index>(), 1..40usize).prop_map(|(at, length)| Damage::Delete(at, length)),
        any::<Index>().prop_map(Damage::Cut),
    ]
}

fn damaged(mut bytes: Vec<u8>, damages: &[Damage]) -> Vec<u8> {
    for damage in damages {
        let length = bytes.len();
        let at = |index: &Index| index.index(length + 1);
        match damage {
            Damage::Flip(index, byte) if !bytes.is_empty() => {
                let at = index.index(bytes.len());
                bytes[at] = *byte;
            }
            Damage::Flip(..) => {}
            Damage::Insert(index, token) => {
                let at = at(index);
                let token = TOKENS[token.index(TOKENS.len())];
                bytes.splice(at..at, token.iter().copied());
            }
            Damage::Delete(index, length) => {
                let start = at(index);
                let end = (start + length).min(bytes.len());
                bytes.drain(start..end);
            }
            Damage::Cut(index) => {
                let at = at(index);
                bytes.truncate(at);
            }
        }
    }
    bytes
}

// Hands out the bytes in reads of the given sizes, so tags and characters split anywhere.
struct Pieces<'a> {
    bytes: &'a [u8],
    sizes: Vec<usize>,
    next: usize,
}

impl Read for Pieces<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.sizes[self.next % self.sizes.len()];
        self.next += 1;
        let size = size.min(buf.len()).min(self.bytes.len());
        let (given, rest) = self.bytes.split_at(size);
        buf[..size].copy_from_slice(given);
        self.bytes = rest;
        Ok(size)
    }
}

#[test]
fn every_seed_scans() {
    let answers: Vec<bool> = seeds()
        .iter()
        .map(|seed| names_cells(seed.as_slice()).unwrap())
        .collect();
    assert_eq!(answers[..2], [false, false]);
    assert!(answers[2..].iter().all(|names| *names), "{answers:?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn damaged_drawings_are_answered_the_same_however_they_arrive(
        seed in prop::sample::select(seeds()),
        damages in prop::collection::vec(damage(), 0..6),
        sizes in prop::collection::vec(1..64usize, 1..8),
    ) {
        let bytes = damaged(seed, &damages);
        let whole = names_cells(bytes.as_slice()).unwrap();
        let pieces = Pieces { bytes: &bytes, sizes, next: 0 };
        prop_assert_eq!(names_cells(pieces).unwrap(), whole);
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            prop_assert!(whole);
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_scan(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let names = names_cells(bytes.as_slice()).unwrap();
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            prop_assert!(names);
        }
    }
}
