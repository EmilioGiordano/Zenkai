use std::borrow::Cow;

use quick_xml::events::{BytesStart, Event};

use super::{CHECKED_ELEMENTS, CellXml, FormulaXml, Scanner, unreadable, unsupported};
use crate::ReadError;
use crate::limits;

impl<'a> Scanner<'a, '_> {
    pub(super) fn next(&mut self) -> Result<Event<'a>, ReadError> {
        self.reader.read_event().map_err(unreadable)
    }

    pub(super) fn check_name(&self, name: quick_xml::name::QName<'_>) -> Result<(), ReadError> {
        if !is_plain_name(name.local_name().as_ref()) {
            return Err(unsupported("an unusual XML name"));
        }
        match name.prefix() {
            None => Ok(()),
            Some(prefix) if prefix.as_ref() == b"xmlns" => {
                Err(unsupported("a namespace declared inside sheetData"))
            }
            Some(prefix) if prefix.as_ref() == b"xml" => Ok(()),
            Some(prefix) if self.prefixes.iter().any(|p| p == prefix.as_ref()) => Ok(()),
            Some(_) => Err(unreadable("an undeclared namespace prefix")),
        }
    }

    // Validates the element the way roxmltree would and returns the unprefixed attributes
    // asked for, which are the only ones IronCalc reads.
    pub(super) fn attributes<'e, const N: usize>(
        &self,
        start: &'e BytesStart<'_>,
        names: [&[u8]; N],
    ) -> Result<[Option<Cow<'e, str>>; N], ReadError> {
        self.check_name(start.name())?;
        let mut found: [Option<Cow<'e, str>>; N] = std::array::from_fn(|_| None);
        for attribute in start.attributes().with_checks(true) {
            let attribute = attribute.map_err(unreadable)?;
            if attribute.key.as_ref() == b"xmlns" {
                return Err(unsupported("a namespace declared inside sheetData"));
            }
            self.check_name(attribute.key)?;
            let value = attribute.value.as_ref();
            if value
                .iter()
                .any(|b| matches!(b, b'&' | b'<' | b'\t' | b'\n'))
            {
                return Err(unsupported("an attribute value roxmltree would rewrite"));
            }
            if attribute.key.prefix().is_some() {
                continue;
            }
            if let Some(slot) = names.iter().position(|n| *n == attribute.key.as_ref()) {
                found[slot] = Some(match attribute.value {
                    Cow::Borrowed(bytes) => {
                        Cow::Borrowed(std::str::from_utf8(bytes).map_err(unreadable)?)
                    }
                    Cow::Owned(bytes) => Cow::Owned(String::from_utf8(bytes).map_err(unreadable)?),
                });
            }
        }
        Ok(found)
    }

    // For every element below a cell except its own <v>, <is> and <f>: a nested <f> would
    // escape the formula limits, since the preflight checks every <f> wherever it is.
    pub(super) fn check_element(&self, start: &BytesStart<'_>) -> Result<(), ReadError> {
        let name = start.local_name();
        if name.as_ref() == b"f" || CHECKED_ELEMENTS.contains(&name.as_ref()) {
            return Err(unsupported("a checked element inside sheetData"));
        }
        self.attributes(start, [])?;
        Ok(())
    }

    pub(super) fn check_text(text: &[u8]) -> Result<(), ReadError> {
        if text.windows(3).any(|w| w == b"]]>") {
            return Err(unreadable("']]>' in text"));
        }
        Ok(())
    }

    pub(super) fn cell_content(&mut self) -> Result<CellXml<'a>, ReadError> {
        let mut content = CellXml {
            values: 0,
            value: None,
            inline: None,
            formulas: 0,
            formula: None,
        };
        loop {
            match self.next()? {
                Event::Start(start) => match start.local_name().as_ref() {
                    b"v" => {
                        self.check_element(&start)?;
                        content.values += 1;
                        let text = self.text()?;
                        if content.values == 1 {
                            content.value = text;
                        }
                    }
                    b"f" => {
                        let formula = self.formula(&start, false)?;
                        content.formulas += 1;
                        if content.formulas == 1 {
                            content.formula = Some(formula);
                        }
                    }
                    b"is" => {
                        self.check_element(&start)?;
                        let text = self.inline_string()?;
                        if content.inline.is_none() {
                            content.inline = Some(text);
                        }
                    }
                    _ => {
                        self.check_element(&start)?;
                        self.skip()?;
                    }
                },
                Event::Empty(start) => match start.local_name().as_ref() {
                    b"v" => {
                        self.check_element(&start)?;
                        content.values += 1;
                    }
                    b"f" => {
                        let formula = self.formula(&start, true)?;
                        content.formulas += 1;
                        if content.formulas == 1 {
                            content.formula = Some(formula);
                        }
                    }
                    b"is" => {
                        self.check_element(&start)?;
                        if content.inline.is_none() {
                            content.inline = Some(String::new());
                        }
                    }
                    _ => self.check_element(&start)?,
                },
                Event::Text(text) => Self::check_text(&text)?,
                Event::GeneralRef(reference) => {
                    resolve_reference(&reference)?;
                }
                Event::End(_) => return Ok(content),
                _ => return Err(unsupported("unusual content in a cell")),
            }
        }
    }

    pub(super) fn formula(
        &mut self,
        start: &BytesStart<'_>,
        empty: bool,
    ) -> Result<FormulaXml<'a>, ReadError> {
        let [t, reference, si, ca] = self.attributes(start, [b"t", b"ref", b"si", b"ca"])?;
        let text = if empty { None } else { self.text()? };
        limits::check_formula(text.as_deref().unwrap_or(""), reference.as_deref())
            .map_err(ReadError::Unsafe)?;
        Ok(FormulaXml {
            kind: t.map(Cow::into_owned),
            reference: reference.map(Cow::into_owned),
            si: si.map(Cow::into_owned),
            calculate_always: ca.as_deref() == Some("1"),
            text,
        })
    }

    // The text of an element with no child elements, as roxmltree's `Node::text` returns it.
    pub(super) fn text(&mut self) -> Result<Option<Cow<'a, str>>, ReadError> {
        let mut text: Option<Cow<'a, str>> = None;
        loop {
            match self.next()? {
                Event::Text(piece) => {
                    Self::check_text(&piece)?;
                    let piece = match piece.into_inner() {
                        Cow::Borrowed(bytes) => {
                            Cow::Borrowed(std::str::from_utf8(bytes).map_err(unreadable)?)
                        }
                        Cow::Owned(bytes) => {
                            Cow::Owned(String::from_utf8(bytes).map_err(unreadable)?)
                        }
                    };
                    if piece.is_empty() {
                        continue;
                    }
                    text = Some(match text {
                        None => piece,
                        Some(before) => Cow::Owned(before.into_owned() + &piece),
                    });
                }
                Event::GeneralRef(reference) => {
                    let resolved = resolve_reference(&reference)?;
                    let mut joined = text.map(Cow::into_owned).unwrap_or_default();
                    joined.push(resolved);
                    text = Some(Cow::Owned(joined));
                }
                Event::End(_) => return Ok(text),
                _ => return Err(unsupported("markup inside a text element")),
            }
        }
    }

    // All <t> texts under <is>, phonetic runs included, joined as IronCalc joins them.
    pub(super) fn inline_string(&mut self) -> Result<String, ReadError> {
        let mut joined = String::new();
        let mut depth = 0usize;
        loop {
            match self.next()? {
                Event::Start(start) => {
                    self.check_element(&start)?;
                    if start.local_name().as_ref() == b"t" {
                        if let Some(text) = self.text()? {
                            joined.push_str(&text);
                        }
                    } else {
                        depth += 1;
                    }
                }
                Event::Empty(start) => self.check_element(&start)?,
                Event::Text(text) => Self::check_text(&text)?,
                Event::GeneralRef(reference) => {
                    resolve_reference(&reference)?;
                }
                Event::End(_) => {
                    if depth == 0 {
                        return Ok(joined);
                    }
                    depth -= 1;
                }
                _ => return Err(unsupported("unusual content in an inline string")),
            }
        }
    }

    pub(super) fn skip(&mut self) -> Result<(), ReadError> {
        let mut depth = 0usize;
        loop {
            match self.next()? {
                Event::Start(start) => {
                    self.check_element(&start)?;
                    depth += 1;
                }
                Event::Empty(start) => self.check_element(&start)?,
                Event::Text(text) => Self::check_text(&text)?,
                Event::GeneralRef(reference) => {
                    resolve_reference(&reference)?;
                }
                Event::End(_) => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                _ => return Err(unsupported("unusual content in a cell")),
            }
        }
    }
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

fn is_plain_name(name: &[u8]) -> bool {
    match name.split_first() {
        Some((first, rest)) => {
            (first.is_ascii_alphabetic() || *first == b'_')
                && rest
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        }
        None => false,
    }
}

pub(super) fn resolve_reference(
    reference: &quick_xml::events::BytesRef<'_>,
) -> Result<char, ReadError> {
    if reference.is_char_ref() {
        return match reference.resolve_char_ref() {
            Ok(Some(c)) if is_xml_char(c) => Ok(c),
            _ => Err(unreadable("an invalid character reference")),
        };
    }
    match reference.as_ref() {
        b"lt" => Ok('<'),
        b"gt" => Ok('>'),
        b"amp" => Ok('&'),
        b"apos" => Ok('\''),
        b"quot" => Ok('"'),
        _ => Err(unreadable("an unknown entity")),
    }
}
