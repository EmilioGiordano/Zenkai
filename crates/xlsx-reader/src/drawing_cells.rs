// Whether a drawing names cells that a row or column move on any sheet would shift: a shape
// whose text shows a cell, a picture of cells (the camera tool) or a legacy control linked
// to cells. Excel writes an empty textlink on every shape, so only a value counts.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

// A drawing that is not well-formed may name anything.
pub(crate) fn names_cells(drawing: &[u8]) -> bool {
    let mut reader = Reader::from_reader(drawing);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) if names_a_cell(&element) => {
                return true;
            }
            Ok(Event::Eof) => return false,
            Ok(_) => {}
            Err(_) => return true,
        }
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
    use super::names_cells;

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
            assert!(!names_cells(&drawing(body)), "{body}");
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
            assert!(names_cells(&drawing(body)), "{body}");
        }
        let vml = concat!(
            r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:x="urn:schemas-microsoft-com:office:excel">"#,
            r#"<v:shape><x:ClientData ObjectType="Checkbox"><x:FmlaLink>$B$2</x:FmlaLink>"#,
            r#"</x:ClientData></v:shape></xml>"#
        );
        assert!(names_cells(vml.as_bytes()));
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
        assert!(!names_cells(vml.as_bytes()));
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
            assert!(names_cells(&broken), "{}", String::from_utf8_lossy(&broken));
        }
    }

    // A deep or long drawing is scanned as it streams, with no tree built for it.
    #[test]
    fn deep_and_wide_drawings_are_scanned_without_a_tree() {
        let depth = 100_000;
        let deep = format!(
            "{DRAWING}{}{}</xdr:wsDr>",
            "<a:g>".repeat(depth),
            "</a:g>".repeat(depth)
        );
        assert!(!names_cells(deep.as_bytes()));
        let wide = drawing(&format!(
            r#"{}<xdr:sp textlink="Other!A1"/>"#,
            r#"<xdr:pic macro=""/>"#.repeat(100_000)
        ));
        assert!(names_cells(&wide));
    }
}
