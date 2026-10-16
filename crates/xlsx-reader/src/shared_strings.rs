use std::io::{self, Read, Write};

use quick_xml::events::BytesStart;

use crate::layout::StringScan;
use crate::stream::{CHUNK_BYTES, Guard};

// IronCalc's importer reads the shared strings from this path whatever the
// relationships say.
pub const SHARED_STRINGS_PART: &str = "xl/sharedStrings.xml";

// The <sst> start tag must lie within this many bytes of the part's start, both when
// the part is checked on open and when a save rewrites it.
pub(crate) const SST_HEAD_BYTES: usize = 4096;

const CLOSE: &[u8] = b"</sst";

// Past the closing tag only white space is expected; more than this means the part is
// not what the open checked.
const MAX_HELD_BYTES: usize = 64 * 1024;

// Where the <sst ...> start tag of an open root lies, quotes honored.
pub(crate) fn sst_root(head: &[u8]) -> Option<(usize, usize)> {
    let start = (0..head.len()).find(|&at| {
        head[at..].starts_with(b"<sst")
            && matches!(head.get(at + 4), Some(b'>' | b' ' | b'\t' | b'\n' | b'\r'))
    })?;
    let mut quote = None;
    for (offset, &byte) in head[start..].iter().enumerate() {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(open), _) if open == byte => quote = None,
            (None, b'>') if head[start + offset - 1] == b'/' => return None,
            (None, b'>') => return Some((start, start + offset + 1)),
            _ => {}
        }
    }
    None
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

// count and uniqueCount are optional and would be wrong once strings are appended.
fn without_counts(tag: &[u8]) -> io::Result<Vec<u8>> {
    let inner = std::str::from_utf8(&tag[1..tag.len() - 1]).map_err(invalid)?;
    let start = BytesStart::from_content(inner, 3);
    let mut kept = BytesStart::new("sst");
    for attribute in start.attributes() {
        let attribute = attribute.map_err(invalid)?;
        if !matches!(attribute.key.as_ref(), b"count" | b"uniqueCount") {
            kept.push_attribute(attribute);
        }
    }
    let mut rewritten = Vec::with_capacity(tag.len());
    rewritten.push(b'<');
    rewritten.extend_from_slice(&kept);
    rewritten.push(b'>');
    Ok(rewritten)
}

// Holds back the bytes from the last "</sst" on, so the new items go in front of it.
#[derive(Default)]
struct Closing {
    held: Vec<u8>,
}

impl Closing {
    fn push(&mut self, bytes: &[u8], out: &mut impl Write) -> io::Result<()> {
        self.held.extend_from_slice(bytes);
        let keep_from = match self.held.windows(CLOSE.len()).rposition(|w| w == CLOSE) {
            Some(at) => at,
            None => self.held.len().saturating_sub(CLOSE.len() - 1),
        };
        out.write_all(&self.held[..keep_from])?;
        self.held.drain(..keep_from);
        if self.held.starts_with(CLOSE) {
            self.held.retain(|byte| !byte.is_ascii_whitespace());
            if self.held.len() > MAX_HELD_BYTES {
                return Err(invalid("the shared strings part does not end in </sst>"));
            }
        }
        Ok(())
    }

    fn finish(self, appended: &[u8], out: &mut impl Write) -> io::Result<()> {
        if self.held != b"</sst>" {
            return Err(invalid("the shared strings part does not end in </sst>"));
        }
        out.write_all(appended)?;
        out.write_all(&self.held)
    }
}

struct GuardRead<'g, 'b, R>(&'g mut Guard<'b, R>);

impl<R: Read> Read for GuardRead<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read_chunk(buf).map_err(io::Error::other)
    }
}

// Copies the part with `appended` (whole <si> elements) inserted before </sst> when the
// part still holds exactly `expected` items, the count the open checked.
pub(crate) fn copy_appended<R: Read>(
    guard: &mut Guard<'_, R>,
    expected: usize,
    appended: &[u8],
    out: &mut impl Write,
) -> io::Result<()> {
    let mut reader = GuardRead(guard);
    let mut scan = StringScan::default();
    let mut head = Vec::with_capacity(SST_HEAD_BYTES);
    (&mut reader)
        .take(SST_HEAD_BYTES as u64)
        .read_to_end(&mut head)?;
    scan.push(&head);
    let (start, end) =
        sst_root(&head).ok_or_else(|| invalid("the shared strings part has no <sst> root"))?;
    out.write_all(&head[..start])?;
    out.write_all(&without_counts(&head[start..end])?)?;
    let mut closing = Closing::default();
    closing.push(&head[end..], out)?;
    let mut chunk = vec![0u8; CHUNK_BYTES];
    loop {
        match reader.read(&mut chunk)? {
            0 => break,
            read => {
                scan.push(&chunk[..read]);
                closing.push(&chunk[..read], out)?;
            }
        }
    }
    if scan.appendable_items() != Some(expected) {
        return Err(invalid(
            "the shared strings part is not the one the file was read with",
        ));
    }
    closing.finish(appended, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_found_outside_quotes() {
        let head = br#"<?xml version="1.0"?><sst xmlns="m" a="x>y" count="2"><si>"#;
        let (start, end) = sst_root(head).unwrap();
        assert_eq!(&head[start..end], br#"<sst xmlns="m" a="x>y" count="2">"#);
        assert_eq!(sst_root(b"<sst xmlns=\"m\"/>"), None);
        assert_eq!(sst_root(b"<sstx>"), None);
        assert_eq!(sst_root(b"<sst xmlns=\"m"), None);
    }

    #[test]
    fn the_counts_are_dropped_and_the_rest_kept() {
        let tag = br#"<sst xmlns="m" count="5" uniqueCount="2" xml:space='a'>"#;
        assert_eq!(
            without_counts(tag).unwrap(),
            br#"<sst xmlns="m" xml:space="a">"#
        );
    }

    fn closed(pieces: &[&[u8]], appended: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut closing = Closing::default();
        for piece in pieces {
            closing.push(piece, &mut out)?;
        }
        closing.finish(appended, &mut out)?;
        Ok(out)
    }

    #[test]
    fn items_go_before_the_closing_tag_whatever_the_chunks() {
        let body = b"<si><t>a</t></si></sst  >\r\n";
        for size in 1..body.len() {
            let pieces: Vec<&[u8]> = body.chunks(size).collect();
            let out = closed(&pieces, b"<si><t>b</t></si>").unwrap();
            assert_eq!(out, b"<si><t>a</t></si><si><t>b</t></si></sst>", "{size}");
        }
        assert!(closed(&[b"<si/></sst><!-- -->"], b"").is_err());
        assert!(closed(&[b"<si/>"], b"").is_err());
    }

    #[test]
    fn trailing_white_space_is_not_held() {
        let mut out = Vec::new();
        let mut closing = Closing::default();
        closing.push(b"</sst>", &mut out).unwrap();
        for _ in 0..100 {
            closing.push(&[b' '; MAX_HELD_BYTES], &mut out).unwrap();
        }
        closing.finish(b"", &mut out).unwrap();
        assert_eq!(out, b"</sst>");
    }
}
