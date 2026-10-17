// roxmltree parses by recursion, one pair of frames per open element, and so does every
// other parser a part reaches: a file nesting elements deep enough overflows the stack
// and closes the app. This counts the depth iteratively over the bytes, before any parse.

use crate::ReadError;

// Excel writes fewer than 30 levels (charts and extension lists are the deepest). At
// about 5 KB per level in a debug build this stays under 2 MB of stack, so it holds on
// any thread; a 100,000-level part overflowed the 256 MB engine stack.
pub const MAX_XML_DEPTH: usize = 256;

pub(crate) fn is_xml_part(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".xml") || name.ends_with(".rels")
}

#[derive(Clone, Copy)]
enum State {
    Text,
    // After '<', before the byte that says what kind of markup follows.
    Markup,
    Open { quote: u8, slash: bool },
    Close,
    Declaration,
    DeclarationDash,
    Comment { dashes: u8 },
    CData { brackets: u8 },
    Instruction { question: bool },
    Skipped,
}

// Follows the markup rules roxmltree parses by: attribute values end at their own quote
// and hold no '<', comments end at the first "-->", CDATA at "]]>", instructions at "?>".
// Malformed input is left to the parser, which refuses it before it nests any deeper.
pub(crate) struct NestingCheck<'p> {
    part: &'p str,
    depth: usize,
    state: State,
}

impl<'p> NestingCheck<'p> {
    pub(crate) fn new(part: &'p str) -> NestingCheck<'p> {
        NestingCheck {
            part,
            depth: 0,
            state: State::Text,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<(), ReadError> {
        for &byte in chunk {
            self.state = match self.state {
                State::Text if byte == b'<' => State::Markup,
                State::Text => State::Text,
                State::Markup => match byte {
                    b'/' => State::Close,
                    b'?' => State::Instruction { question: false },
                    b'!' => State::Declaration,
                    _ => self.in_open_tag(byte, 0, false)?,
                },
                State::Open { quote, slash } => self.in_open_tag(byte, quote, slash)?,
                State::Close if byte == b'>' => {
                    self.depth = self.depth.saturating_sub(1);
                    State::Text
                }
                State::Close => State::Close,
                State::Declaration => match byte {
                    b'-' => State::DeclarationDash,
                    b'[' => State::CData { brackets: 0 },
                    b'>' => State::Text,
                    _ => State::Skipped,
                },
                State::DeclarationDash => match byte {
                    b'-' => State::Comment { dashes: 0 },
                    b'>' => State::Text,
                    _ => State::Skipped,
                },
                State::Comment { dashes } => match byte {
                    b'-' => State::Comment {
                        dashes: (dashes + 1).min(2),
                    },
                    b'>' if dashes >= 2 => State::Text,
                    _ => State::Comment { dashes: 0 },
                },
                State::CData { brackets } => match byte {
                    b']' => State::CData {
                        brackets: (brackets + 1).min(2),
                    },
                    b'>' if brackets >= 2 => State::Text,
                    _ => State::CData { brackets: 0 },
                },
                State::Instruction { question } => match (byte, question) {
                    (b'>', true) => State::Text,
                    (b'?', _) => State::Instruction { question: true },
                    _ => State::Instruction { question: false },
                },
                State::Skipped if byte == b'>' => State::Text,
                State::Skipped => State::Skipped,
            };
        }
        Ok(())
    }

    fn in_open_tag(&mut self, byte: u8, quote: u8, slash: bool) -> Result<State, ReadError> {
        if quote != 0 {
            let quote = if byte == quote { 0 } else { quote };
            return Ok(State::Open {
                quote,
                slash: false,
            });
        }
        match byte {
            b'"' | b'\'' => Ok(State::Open {
                quote: byte,
                slash: false,
            }),
            b'/' => Ok(State::Open {
                quote: 0,
                slash: true,
            }),
            b'>' => {
                if !slash {
                    self.depth += 1;
                    if self.depth > MAX_XML_DEPTH {
                        return Err(ReadError::Unsafe(format!(
                            "part {} nests elements deeper than {MAX_XML_DEPTH} levels",
                            self.part
                        )));
                    }
                }
                Ok(State::Text)
            }
            _ => Ok(State::Open {
                quote: 0,
                slash: false,
            }),
        }
    }
}

pub(crate) fn check_nesting(part: &str, bytes: &[u8]) -> Result<(), ReadError> {
    if !is_xml_part(part) {
        return Ok(());
    }
    NestingCheck::new(part).push(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(levels: usize) -> String {
        format!("{}{}", "<a>".repeat(levels), "</a>".repeat(levels))
    }

    fn refused(xml: &str) -> bool {
        matches!(
            check_nesting("xl/part.xml", xml.as_bytes()),
            Err(ReadError::Unsafe(_))
        )
    }

    #[test]
    fn the_cap_passes_and_one_level_more_is_refused() {
        assert!(!refused(&nested(MAX_XML_DEPTH)));
        assert!(refused(&nested(MAX_XML_DEPTH + 1)));
    }

    #[test]
    fn a_hundred_thousand_levels_are_refused_without_nesting() {
        assert!(refused(&nested(100_000)));
    }

    #[test]
    fn only_xml_parts_are_counted() {
        let deep = nested(MAX_XML_DEPTH + 1);
        assert!(check_nesting("xl/media/image1.png", deep.as_bytes()).is_ok());
        assert!(check_nesting("xl/_rels/workbook.xml.RELS", deep.as_bytes()).is_err());
    }

    #[test]
    fn markup_that_is_not_an_element_does_not_nest() {
        let open = "<a>".repeat(MAX_XML_DEPTH);
        for hidden in [
            "<!-- <b><b><b> -->",
            "<![CDATA[ <b><b><b> ]]>",
            "<?pi <b><b> ?>",
            "<b/>",
            r#"<b c="<d>" />"#,
            "<b c='x' d=\"y\"/>",
        ] {
            assert!(!refused(&format!("{open}{hidden}{hidden}")), "{hidden}");
        }
    }

    #[test]
    fn a_quoted_slash_before_the_tag_end_does_not_hide_an_open_element() {
        let hostile = r#"<a b="/>">"#.repeat(MAX_XML_DEPTH + 1);
        assert!(refused(&hostile));
        let tricky = "<a b='/>'>".repeat(MAX_XML_DEPTH + 1);
        assert!(refused(&tricky));
    }

    #[test]
    fn the_count_does_not_depend_on_where_the_chunks_cut() {
        let xml = format!(
            "<?xml version=\"1.0\"?><!-- x --><r a=\"/>\">{}<![CDATA[<]]></r>",
            nested(MAX_XML_DEPTH - 1)
        );
        let deeper = xml.replace("<r ", "<q><r ");
        for (xml, expected) in [(&xml, false), (&deeper, true)] {
            for cut in 1..xml.len() {
                let mut check = NestingCheck::new("a.xml");
                let bytes = xml.as_bytes();
                let first = check.push(&bytes[..cut]);
                let second = first.and_then(|()| check.push(&bytes[cut..]));
                assert_eq!(second.is_err(), expected, "cut at {cut}");
            }
        }
    }

    #[test]
    fn the_depth_agrees_with_the_parser_on_a_real_document() {
        let xml = r#"<?xml version="1.0"?><worksheet a="1"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>x &gt; y</t></is></c><c r="B1"/></row></sheetData></worksheet>"#;
        let document = roxmltree::Document::parse(xml).unwrap();
        let depth = document
            .descendants()
            .filter(roxmltree::Node::is_element)
            .map(|n| n.ancestors().count() - 1)
            .max()
            .unwrap();
        assert!(!refused(&format!("{}{xml}", "<a>".repeat(MAX_XML_DEPTH - depth))));
        assert!(refused(&format!("{}{xml}", "<a>".repeat(MAX_XML_DEPTH - depth + 1))));
    }
}
