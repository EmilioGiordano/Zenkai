// Whether a drawing names cells that a row or column move on any sheet would shift: a shape
// whose text shows a cell, a picture of cells (the camera tool) or a legacy control linked
// to cells. Excel writes an empty textlink on every shape, so only a value counts.

use std::io::{self, BufReader, Read};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::nesting::MAX_XML_DEPTH;

// Scans the drawing as it streams, holding a buffered chunk and the event being read, never
// the whole part. A drawing that is not well-formed UTF-8 XML, or nests past the depth the
// reader allows, may name anything. An error is the source failing to deliver the bytes.
pub(crate) fn names_cells(drawing: impl Read) -> io::Result<bool> {
    let mut reader = Reader::from_reader(BufReader::new(Utf8Checked::new(drawing)));
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut event = Vec::new();
    let mut depth = 0usize;
    loop {
        event.clear();
        match reader.read_event_into(&mut event) {
            Ok(Event::Start(element)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH || names_a_cell(&element) {
                    return Ok(true);
                }
            }
            Ok(Event::Empty(element)) if names_a_cell(&element) => return Ok(true),
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => return Ok(false),
            Ok(_) => {}
            Err(quick_xml::Error::Io(error)) if !reader.get_ref().get_ref().invalid => {
                return Err(io::Error::new(error.kind(), error.to_string()));
            }
            Err(_) => return Ok(true),
        }
    }
}

// Passes the bytes through while checking they are UTF-8 with no NUL, which XML forbids, so
// another encoding such as UTF-16 cannot hide its names from the scan.
struct Utf8Checked<R> {
    inner: R,
    // The start of a character split across reads, at most three bytes.
    pending: Vec<u8>,
    invalid: bool,
}

impl<R> Utf8Checked<R> {
    fn new(inner: R) -> Utf8Checked<R> {
        Utf8Checked {
            inner,
            pending: Vec::new(),
            invalid: false,
        }
    }

    fn accepts(&mut self, chunk: &[u8]) -> bool {
        if chunk.contains(&0) {
            return false;
        }
        let mut text = std::mem::take(&mut self.pending);
        text.extend_from_slice(chunk);
        match std::str::from_utf8(&text) {
            Ok(_) => true,
            Err(error) if error.error_len().is_none() => {
                self.pending = text[error.valid_up_to()..].to_vec();
                true
            }
            Err(_) => false,
        }
    }
}

impl<R: Read> Read for Utf8Checked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        let valid = match read {
            0 => self.pending.is_empty(),
            read => self.accepts(&buf[..read]),
        };
        if !valid {
            self.invalid = true;
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(read)
    }
}

fn names_a_cell(element: &BytesStart) -> bool {
    let name = element.local_name();
    let name = name.as_ref();
    // VML controls name their cells in FmlaLink, FmlaRange, FmlaPict and the like.
    if name.starts_with(b"Fmla") || name == b"cameraTool" {
        return true;
    }
    element
        .attributes()
        .with_checks(true)
        .any(|attribute| match attribute {
            Ok(attribute) => match attribute.key.local_name().as_ref() {
                b"textlink" | b"cellRange" => !attribute.value.trim_ascii().is_empty(),
                b"type" => name == b"fld" && attribute.value.eq_ignore_ascii_case(b"TxLink"),
                _ => false,
            },
            Err(_) => true,
        })
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::names_cells;
    use crate::nesting::MAX_XML_DEPTH;

    fn scan(drawing: &[u8]) -> bool {
        names_cells(drawing).unwrap()
    }

    const DRAWING: &str = concat!(
        r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" "#,
        r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#
    );

    fn drawing(body: &str) -> Vec<u8> {
        format!("{DRAWING}{body}</xdr:wsDr>").into_bytes()
    }

    #[test]
    fn pictures_and_shapes_with_their_own_text_name_no_cell() {
        for body in [
            "",
            r#"<xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:row>2</xdr:row></xdr:from><xdr:pic macro=""/></xdr:twoCellAnchor>"#,
            r#"<xdr:oneCellAnchor><xdr:sp macro="" textlink=""><xdr:txBody><a:p><a:r><a:t>Sheet3!A1</a:t></a:r></a:p></xdr:txBody></xdr:sp></xdr:oneCellAnchor>"#,
            r#"<xdr:sp textlink="  "/><a:fld type="slidenum"/>"#,
        ] {
            assert!(!scan(&drawing(body)), "{body}");
        }
    }

    #[test]
    fn linked_shape_text_camera_pictures_and_controls_name_cells() {
        for body in [
            r#"<xdr:sp macro="" textlink="Sheet3!$A$1"/>"#,
            r#"<xdr:sp textlink="=Data!B2"><xdr:txBody/></xdr:sp>"#,
            r#"<a:p><a:fld id="{1}" type="TxLink"/></a:p>"#,
            r#"<a14:cameraTool xmlns:a14="urn:a14" cellRange="Sheet1!$A$1:$C$5" spid="s1"/>"#,
            r#"<a14:cameraTool xmlns:a14="urn:a14"/>"#,
        ] {
            assert!(scan(&drawing(body)), "{body}");
        }
        let vml = concat!(
            r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:x="urn:schemas-microsoft-com:office:excel">"#,
            r#"<v:shape><x:ClientData ObjectType="Checkbox"><x:FmlaLink>$B$2</x:FmlaLink>"#,
            r#"</x:ClientData></v:shape></xml>"#
        );
        assert!(scan(vml.as_bytes()));
    }

    #[test]
    fn notes_name_only_their_own_cell() {
        let vml = concat!(
            r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" "#,
            r#"xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit">"#,
            r#"<o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shape id="_x0000_s1025" "#,
            r#"style='position:absolute'><x:ClientData ObjectType="Note"><x:MoveWithCells/>"#,
            r#"<x:Anchor>1, 15, 0, 2, 3, 15, 3, 16</x:Anchor><x:Row>0</x:Row><x:Column>0</x:Column>"#,
            r#"</x:ClientData></v:shape></xml>"#
        );
        assert!(!scan(vml.as_bytes()));
    }

    #[test]
    fn a_drawing_that_is_not_well_formed_may_name_cells() {
        for broken in [
            drawing("<xdr:sp></xdr:pic>"),
            drawing(r#"<xdr:sp macro="" macro=""/>"#),
            drawing("<!-- a -- b -->"),
            b"<xdr:wsDr><xdr:sp textlink=".to_vec(),
            drawing(r#"<xdr:sp textlink="A1/>"#),
        ] {
            assert!(scan(&broken), "{}", String::from_utf8_lossy(&broken));
        }
    }

    #[test]
    fn a_drawing_in_another_encoding_may_name_cells() {
        let utf16: Vec<u8> = String::from_utf8(drawing(""))
            .unwrap()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(scan(&utf16));
        let mut latin1 = drawing("<!--Caf-->");
        let after = latin1.windows(3).position(|w| w == b"Caf").unwrap() + 3;
        latin1.insert(after, 0xE9);
        assert!(scan(&latin1));
    }

    // Hands out one byte per read, so every character and tag is split across reads.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some((first, rest)) = self.0.split_first() else {
                return Ok(0);
            };
            buf[0] = *first;
            self.0 = rest;
            Ok(1)
        }
    }

    #[test]
    fn characters_split_across_reads_are_read_whole() {
        let plain = drawing("<!--Café 東京 😀-->");
        assert!(!names_cells(Trickle(&plain)).unwrap());
        let linked = drawing(r#"<xdr:sp textlink="'Café'!A1"/>"#);
        assert!(names_cells(Trickle(&linked)).unwrap());
        let mut cut = drawing("");
        cut.push("é".as_bytes()[0]);
        assert!(names_cells(Trickle(&cut)).unwrap());
    }

    struct Failing;

    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("the entry cannot be inflated"))
        }
    }

    #[test]
    fn a_source_that_fails_is_an_error_not_an_answer() {
        assert!(names_cells(Failing).is_err());
    }

    #[test]
    fn drawings_nested_past_the_reader_depth_may_name_cells() {
        let nested = |depth: usize| format!("{}{}", "<g>".repeat(depth), "</g>".repeat(depth));
        assert!(!scan(nested(MAX_XML_DEPTH).as_bytes()));
        assert!(scan(nested(MAX_XML_DEPTH + 1).as_bytes()));
    }

    #[test]
    fn a_long_drawing_is_scanned_to_its_end() {
        let wide = drawing(&format!(
            r#"{}<xdr:sp textlink="Other!A1"/>"#,
            r#"<xdr:pic macro=""/>"#.repeat(100_000)
        ));
        assert!(scan(&wide));
    }
}
