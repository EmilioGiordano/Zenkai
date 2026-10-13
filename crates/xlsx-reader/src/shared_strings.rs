// IronCalc's importer reads the shared strings from this path whatever the
// relationships say.
pub const SHARED_STRINGS_PART: &str = "xl/sharedStrings.xml";

// The <sst> start tag must lie within this many bytes of the part's start, both when
// the part is checked on open and when a save rewrites it.
pub(crate) const SST_HEAD_BYTES: usize = 4096;

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
}
