// Damage for xlsx files, the way a corrupt or hostile file would be damaged: bytes flipped,
// cut or removed, hostile tokens spliced into parts, parts dropped or renamed. Included by
// `#[path]` into the test files that open xlsx files.
#![allow(dead_code)]

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use proptest::prelude::*;

pub const TOKENS: [&str; 17] = [
    "<f>",
    "</row>",
    r#" r="A0""#,
    r#" r="XFE1048577""#,
    "1048577",
    "-1",
    "9e999",
    "&#0;",
    "]]>",
    "<!ENTITY x SYSTEM 'file:///c:/windows/win.ini'>",
    "<c><f>SUM(A1:XFD1048576)</f></c>",
    "<mergeCell ref=\"A1:A1\"/>",
    "<col min=\"0\" max=\"99999\" width=\"1e9\"/>",
    "<xf numFmtId=\"999\" fontId=\"99\" fillId=\"99\" borderId=\"99\"/>",
    r#" outlineLevel="8" collapsed="yes""#,
    "<rowBreaks><brk id=\"4294967296\" max=\"-1\"/></rowBreaks>",
    "<pageMargins left=\"NaN\"/><pageSetup paperSize=\"0\" r:id=\"rId99\"/>",
];

#[derive(Clone, Debug)]
pub struct Damage {
    part: prop::sample::Index,
    at: prop::sample::Index,
    how: u8,
    token: usize,
    byte: u8,
}

pub fn damage() -> impl Strategy<Value = Damage> {
    (
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
        0u8..7,
        0..TOKENS.len(),
        any::<u8>(),
    )
        .prop_map(|(part, at, how, token, byte)| Damage {
            part,
            at,
            how,
            token,
            byte,
        })
}

pub fn corpus() -> Vec<PathBuf> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat");
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "xlsx"))
        .collect();
    files.sort();
    files
}

pub fn parts_of(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    (0..archive.len())
        .filter_map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            if entry.is_dir() {
                return None;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            Some((entry.name().to_string(), bytes))
        })
        .collect()
}

pub fn zip_of(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name.as_str(), zip::write::FileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

pub fn apply(parts: &mut Vec<(String, Vec<u8>)>, damage: &Damage) {
    let index = damage.part.index(parts.len());
    let bytes = &mut parts[index].1;
    let at = damage.at.index(bytes.len().max(1)).min(bytes.len());
    match damage.how {
        0 if !bytes.is_empty() => {
            let last = bytes.len() - 1;
            bytes[at.min(last)] ^= damage.byte | 1;
        }
        1 => {
            let end = (at + 1 + usize::from(damage.byte)).min(bytes.len());
            bytes.drain(at..end);
        }
        2 => {
            let token = TOKENS[damage.token].as_bytes().to_vec();
            bytes.splice(at..at, token);
        }
        3 => bytes.truncate(at),
        4 => bytes.clear(),
        5 => {
            parts.remove(index);
        }
        _ => parts[index].0 = parts[index].0.to_uppercase(),
    }
}
