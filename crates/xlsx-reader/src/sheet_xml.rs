use std::borrow::Cow;
use std::collections::HashMap;

use ironcalc::base::expressions::token::{Error, get_error_by_english_name};
use ironcalc::base::expressions::utils::{column_to_number, parse_reference_a1};
use ironcalc::base::types::{ArrayKind, Cell, FormulaValue, Row, SpillValue};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::ReadError;
use crate::limits;
use crate::text::{decode_xlsx_escapes, parse_bool_false};

// Anything the checker in the engine's preflight looks at; inside <sheetData> only <f> is
// expected, so any other one means a file this reader does not handle.
const CHECKED_ELEMENTS: [&[u8]; 8] = [
    b"formula",
    b"formula1",
    b"formula2",
    b"definedName",
    b"hyperlinks",
    b"dataValidations",
    b"autoFilter",
    b"sheetProtection",
];

const DEFAULT_ROW_HEIGHT: f64 = 14.5;

pub(crate) struct ScannedSheet<'a> {
    pub(crate) stub: Option<Vec<u8>>,
    pub(crate) cells: SheetCells<'a>,
}

// Cells in document order. Cells listed in `string_cells` hold a sheet-local string index
// and every formula cell holds an index into `events`; both are resolved once all sheets
// are read, because IronCalc numbers strings and formulas in document order.
#[derive(Default)]
pub(crate) struct SheetCells<'a> {
    pub(crate) rows: Vec<Row>,
    pub(crate) data: Vec<(i32, Vec<(i32, Cell)>)>,
    pub(crate) strings: Vec<String>,
    pub(crate) string_cells: Vec<Slot>,
    pub(crate) events: Vec<FormulaEvent>,
    pub(crate) jobs: Vec<FormulaJob<'a>>,
}

#[derive(Clone, Copy)]
pub(crate) struct Slot {
    pub(crate) row: usize,
    pub(crate) cell: usize,
}

pub(crate) enum FormulaEvent {
    Convert { job: usize },
    SharedAnchor { si: i32, job: usize },
    SharedChild { si: i32 },
}

pub(crate) struct FormulaJob<'a> {
    pub(crate) text: Cow<'a, str>,
    pub(crate) row: i32,
    pub(crate) column: i32,
    pub(crate) array: bool,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn unsupported(what: &str) -> ReadError {
    ReadError::Unsupported(what.to_string())
}

pub(crate) fn scan_worksheet<'a>(
    xml: &'a str,
    sheet_name: &str,
) -> Result<ScannedSheet<'a>, ReadError> {
    check_characters(xml.as_bytes())?;
    let mut reader = Reader::from_str(xml);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut scanner = Scanner {
        reader,
        prefixes: Vec::new(),
        sheet_name,
        strings: HashMap::new(),
        array_cells: HashMap::new(),
        cells: SheetCells::default(),
    };
    let mut depth = 0usize;
    let mut span = None;
    loop {
        let before = position(&scanner.reader);
        match scanner.next()? {
            Event::Start(start) => {
                if depth == 0 {
                    scanner.declare_prefixes(&start)?;
                }
                if depth == 1 && start.local_name().as_ref() == b"sheetData" {
                    if span.is_some() {
                        return Err(unsupported("two sheetData elements"));
                    }
                    scanner.check_element(&start)?;
                    let name = start.name().as_ref().to_vec();
                    scanner.sheet_data()?;
                    span = Some((before, position(&scanner.reader), name));
                } else {
                    depth += 1;
                }
            }
            Event::Empty(start) => {
                if depth == 1 && start.local_name().as_ref() == b"sheetData" {
                    if span.is_some() {
                        return Err(unsupported("two sheetData elements"));
                    }
                    span = Some((before, before, Vec::new()));
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    let Some((start, end, name)) = span else {
        return Err(unreadable("a worksheet has no sheetData"));
    };
    let bytes = xml.as_bytes();
    let (Some(before), Some(data), Some(after)) =
        (bytes.get(..start), bytes.get(start..end), bytes.get(end..))
    else {
        return Err(unreadable("sheetData out of place"));
    };
    if data.contains(&b'\r') {
        return Err(unsupported("a carriage return inside sheetData"));
    }
    let stub = (start != end).then(|| [before, b"<", &name, b"/>", after].concat());
    let mut cells = scanner.cells;
    let mut strings: Vec<(String, usize)> = scanner.strings.into_iter().collect();
    strings.sort_unstable_by_key(|(_, index)| *index);
    cells.strings = strings.into_iter().map(|(text, _)| text).collect();
    Ok(ScannedSheet { stub, cells })
}

fn position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX)
}

// roxmltree, which IronCalc parses with, rejects characters XML forbids and normalizes
// line breaks; quick-xml does neither, so such a part is left to IronCalc. Excel ends the
// XML declaration with CRLF, so carriage returns are refused only inside sheetData.
fn check_characters(bytes: &[u8]) -> Result<(), ReadError> {
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r') {
            return Err(unsupported("a control character"));
        }
        if byte == 0xEF
            && bytes.get(index + 1) == Some(&0xBF)
            && matches!(bytes.get(index + 2), Some(0xBE | 0xBF))
        {
            return Err(unsupported("a noncharacter"));
        }
        index += 1;
    }
    Ok(())
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

struct Scanner<'a, 's> {
    reader: Reader<&'a [u8]>,
    prefixes: Vec<Vec<u8>>,
    sheet_name: &'s str,
    strings: HashMap<String, usize>,
    array_cells: HashMap<(i32, i32), (i32, i32)>,
    cells: SheetCells<'a>,
}

struct CellXml<'a> {
    values: usize,
    value: Option<Cow<'a, str>>,
    inline: Option<String>,
    formulas: usize,
    formula: Option<FormulaXml<'a>>,
}

struct FormulaXml<'a> {
    kind: Option<String>,
    reference: Option<String>,
    si: Option<String>,
    calculate_always: bool,
    text: Option<Cow<'a, str>>,
}

enum CellArray {
    None,
    Dynamic(i32, i32),
    Cse(i32, i32),
}

impl<'a> Scanner<'a, '_> {
    fn next(&mut self) -> Result<Event<'a>, ReadError> {
        self.reader.read_event().map_err(unreadable)
    }

    fn declare_prefixes(&mut self, start: &BytesStart<'_>) -> Result<(), ReadError> {
        for attribute in start.attributes().with_checks(true) {
            let attribute = attribute.map_err(unreadable)?;
            if let Some(quick_xml::name::PrefixDeclaration::Named(prefix)) =
                attribute.key.as_namespace_binding()
            {
                self.prefixes.push(prefix.to_vec());
            }
        }
        Ok(())
    }

    fn check_name(&self, name: quick_xml::name::QName<'_>) -> Result<(), ReadError> {
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
    fn attributes<const N: usize>(
        &self,
        start: &BytesStart<'_>,
        names: [&[u8]; N],
    ) -> Result<[Option<String>; N], ReadError> {
        self.check_name(start.name())?;
        let mut found: [Option<String>; N] = std::array::from_fn(|_| None);
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
                let text = std::str::from_utf8(value).map_err(unreadable)?;
                found[slot] = Some(text.to_string());
            }
        }
        Ok(found)
    }

    // For every element below a cell except its own <v>, <is> and <f>: a nested <f> would
    // escape the formula limits, since the preflight checks every <f> wherever it is.
    fn check_element(&self, start: &BytesStart<'_>) -> Result<(), ReadError> {
        let name = start.local_name();
        if name.as_ref() == b"f" || CHECKED_ELEMENTS.contains(&name.as_ref()) {
            return Err(unsupported("a checked element inside sheetData"));
        }
        self.attributes(start, [])?;
        Ok(())
    }

    fn check_text(text: &[u8]) -> Result<(), ReadError> {
        if text.windows(3).any(|w| w == b"]]>") {
            return Err(unreadable("']]>' in text"));
        }
        Ok(())
    }

    fn sheet_data(&mut self) -> Result<(), ReadError> {
        loop {
            match self.next()? {
                Event::Start(start) if start.local_name().as_ref() == b"row" => {
                    self.row(&start, false)?;
                }
                Event::Empty(start) if start.local_name().as_ref() == b"row" => {
                    self.row(&start, true)?;
                }
                Event::End(_) => return Ok(()),
                _ => return Err(unsupported("something other than a row in sheetData")),
            }
        }
    }

    fn row(&mut self, start: &BytesStart<'_>, empty: bool) -> Result<(), ReadError> {
        let [r, ht, custom_height, s, custom_format, hidden] = self.attributes(
            start,
            [
                b"r",
                b"ht",
                b"customHeight",
                b"s",
                b"customFormat",
                b"hidden",
            ],
        )?;
        let mut row_index = match r {
            Some(r) => Some(r.parse::<i32>().map_err(unreadable)?),
            None => None,
        };
        let (has_height, height) = match ht {
            Some(ht) => (true, ht.parse::<f64>().unwrap_or(DEFAULT_ROW_HEIGHT)),
            None => (false, DEFAULT_ROW_HEIGHT),
        };
        let custom_height = parse_bool_false(custom_height.as_deref());
        let style = s.map_or(0, |s| s.parse::<i32>().unwrap_or(0));
        let custom_format = parse_bool_false(custom_format.as_deref());
        let hidden = parse_bool_false(hidden.as_deref());
        if let Some(r) = row_index
            && (custom_height || custom_format || style != 0 || has_height || hidden)
        {
            self.cells.rows.push(Row {
                r,
                height,
                s: style,
                custom_height,
                custom_format,
                hidden,
            });
        }
        let slot = self.cells.data.len();
        let mut cells: Vec<(i32, Cell)> = Vec::new();
        if !empty {
            loop {
                match self.next()? {
                    Event::Start(start) if start.local_name().as_ref() == b"c" => {
                        self.cell(&start, false, slot, &mut row_index, &mut cells)?;
                    }
                    Event::Empty(start) if start.local_name().as_ref() == b"c" => {
                        self.cell(&start, true, slot, &mut row_index, &mut cells)?;
                    }
                    Event::End(_) => break,
                    _ => return Err(unsupported("something other than a cell in a row")),
                }
            }
        }
        let row_index = row_index.ok_or_else(|| unreadable("a row without a row index"))?;
        if let Some((last, _)) = self.cells.data.last()
            && *last >= row_index
        {
            return Err(unsupported("rows out of order"));
        }
        self.cells.data.push((row_index, cells));
        Ok(())
    }

    fn cell(
        &mut self,
        start: &BytesStart<'_>,
        empty: bool,
        slot: usize,
        row_index: &mut Option<i32>,
        cells: &mut Vec<(i32, Cell)>,
    ) -> Result<(), ReadError> {
        let [r, s, t, vm, cm] = self.attributes(start, [b"r", b"s", b"t", b"vm", b"cm"])?;
        let cell_ref = r.ok_or_else(|| unreadable("a cell without a reference"))?;
        let reference = parse_reference_a1(&cell_ref)
            .ok_or_else(|| unreadable(format!("invalid cell reference {cell_ref}")))?;
        let (r_index, column_index) = (reference.row, reference.column);
        if row_index.is_none() {
            *row_index = Some(r_index);
        }
        if let Some((last, _)) = cells.last()
            && *last >= column_index
        {
            return Err(unsupported("cells out of order"));
        }
        let content = if empty {
            CellXml {
                values: 0,
                value: None,
                inline: None,
                formulas: 0,
                formula: None,
            }
        } else {
            self.cell_content()?
        };
        let value = if content.values == 1 {
            Some(content.value.unwrap_or(Cow::Borrowed("")))
        } else {
            None
        };
        let cell_type = match t.as_deref() {
            Some(t) => t,
            None if value.is_none() => "empty",
            None => "n",
        };
        let style = s.map_or(0, |s| s.parse::<i32>().unwrap_or(0));

        let mut formula_event = None;
        let mut array = CellArray::None;
        if content.formulas == 1
            && let Some(formula) = content.formula
        {
            let mut kind = formula.kind.as_deref().unwrap_or("normal");
            if kind == "normal" && formula.calculate_always && formula.text.is_none() {
                kind = "hint-volatile";
            }
            let text = formula.text.unwrap_or(Cow::Borrowed(""));
            match kind {
                "shared" => {
                    let si = formula
                        .si
                        .ok_or_else(|| unreadable("a shared formula without si"))?
                        .parse::<i32>()
                        .map_err(unreadable)?;
                    formula_event = Some(match formula.reference {
                        Some(_) => FormulaEvent::SharedAnchor {
                            si,
                            job: self.job(text, &cell_ref, false)?,
                        },
                        None => FormulaEvent::SharedChild { si },
                    });
                }
                "dataTable" => return Err(unreadable("data table formulas")),
                "array" => {
                    let range = formula
                        .reference
                        .ok_or_else(|| unreadable("an array formula without a ref"))?;
                    let (row1, column1, row2, column2) = parse_range(&range)
                        .ok_or_else(|| unreadable(format!("invalid range {range}")))?;
                    if row1 != r_index || column1 != column_index {
                        return Err(unreadable("an array formula outside its anchor"));
                    }
                    let area = (i64::from(row2) - i64::from(row1) + 1)
                        .saturating_mul(i64::from(column2) - i64::from(column1) + 1);
                    if area > limits::MAX_FORMULA_AREA as i64 {
                        return Err(unsupported("an array formula larger than the limit"));
                    }
                    for r in row1..=row2 {
                        for c in column1..=column2 {
                            if r != row1 || c != column1 {
                                self.array_cells.insert((r, c), (r_index, column_index));
                            }
                        }
                    }
                    let (width, height) = (column2 - column1 + 1, row2 - row1 + 1);
                    array = if cm.as_deref() == Some("1") {
                        CellArray::Dynamic(width, height)
                    } else {
                        CellArray::Cse(width, height)
                    };
                    formula_event = Some(FormulaEvent::Convert {
                        job: self.job(text, &cell_ref, true)?,
                    });
                }
                "normal" => {
                    formula_event = Some(FormulaEvent::Convert {
                        job: self.job(text, &cell_ref, false)?,
                    });
                }
                "hint-volatile" => {}
                other => return Err(unreadable(format!("invalid formula type {other}"))),
            }
        }
        let anchor = if self.array_cells.is_empty() {
            None
        } else {
            self.array_cells.get(&(r_index, column_index)).copied()
        };
        let place = Slot {
            row: slot,
            cell: cells.len(),
        };
        let cell = match formula_event {
            Some(event) => {
                let f = i32::try_from(self.cells.events.len()).map_err(unreadable)?;
                self.cells.events.push(event);
                formula_cell(
                    f,
                    style,
                    array,
                    self.formula_value(
                        cell_type,
                        value.as_deref(),
                        vm.as_deref(),
                        &cell_ref,
                        content.inline,
                    ),
                )
            }
            None => self.value_cell(
                cell_type,
                value.as_deref(),
                vm.as_deref(),
                style,
                anchor,
                content.inline,
                place,
            ),
        };
        cells.push((column_index, cell));
        Ok(())
    }

    fn job(&mut self, text: Cow<'a, str>, cell_ref: &str, array: bool) -> Result<usize, ReadError> {
        let (row, column) = formula_context(cell_ref)?;
        self.cells.jobs.push(FormulaJob {
            text,
            row,
            column,
            array,
        });
        Ok(self.cells.jobs.len() - 1)
    }

    fn intern(&mut self, text: String) -> i32 {
        let next = self.strings.len();
        let index = *self.strings.entry(text).or_insert(next);
        i32::try_from(index).unwrap_or(i32::MAX)
    }

    fn string_cell(&mut self, text: String, s: i32, place: Slot) -> Cell {
        self.cells.string_cells.push(place);
        Cell::SharedString {
            si: self.intern(text),
            s,
        }
    }

    fn cell_content(&mut self) -> Result<CellXml<'a>, ReadError> {
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

    fn formula(
        &mut self,
        start: &BytesStart<'_>,
        empty: bool,
    ) -> Result<FormulaXml<'a>, ReadError> {
        let [t, reference, si, ca] = self.attributes(start, [b"t", b"ref", b"si", b"ca"])?;
        let text = if empty { None } else { self.text()? };
        limits::check_formula(text.as_deref().unwrap_or(""), reference.as_deref())
            .map_err(ReadError::Unsafe)?;
        Ok(FormulaXml {
            kind: t,
            reference,
            si,
            calculate_always: ca.as_deref() == Some("1"),
            text,
        })
    }

    // The text of an element with no child elements, as roxmltree's `Node::text` returns it.
    fn text(&mut self) -> Result<Option<Cow<'a, str>>, ReadError> {
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
    fn inline_string(&mut self) -> Result<String, ReadError> {
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

    fn skip(&mut self) -> Result<(), ReadError> {
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

    fn formula_value(
        &self,
        cell_type: &str,
        value: Option<&str>,
        metadata: Option<&str>,
        cell_ref: &str,
        inline: Option<String>,
    ) -> FormulaValue {
        let origin = || format!("{}!{cell_ref}", self.sheet_name);
        match cell_type {
            "b" => FormulaValue::Boolean(value == Some("1")),
            "n" => FormulaValue::Number(value.unwrap_or("0").parse::<f64>().unwrap_or(0.0)),
            "e" => {
                let name = error_name(value, metadata);
                FormulaValue::Error {
                    ei: get_error_by_english_name(name).unwrap_or(Error::ERROR),
                    o: origin(),
                    m: value.unwrap_or("#ERROR!").to_string(),
                }
            }
            "s" | "d" => FormulaValue::Error {
                ei: Error::NIMPL,
                o: origin(),
                m: Error::NIMPL.to_string(),
            },
            "str" => FormulaValue::Text(decode_xlsx_escapes(value.unwrap_or(""))),
            "inlineStr" => FormulaValue::Text(inline.unwrap_or_default()),
            _ => FormulaValue::Error {
                ei: Error::ERROR,
                o: origin(),
                m: Error::ERROR.to_string(),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn value_cell(
        &mut self,
        cell_type: &str,
        value: Option<&str>,
        metadata: Option<&str>,
        style: i32,
        anchor: Option<(i32, i32)>,
        inline: Option<String>,
        place: Slot,
    ) -> Cell {
        let s = style;
        match (cell_type, anchor) {
            ("b", Some(a)) => Cell::SpillCell {
                v: SpillValue::Boolean(value == Some("1")),
                s,
                a,
            },
            ("b", None) => Cell::BooleanCell {
                v: value == Some("1"),
                s,
            },
            ("n", Some(a)) => Cell::SpillCell {
                v: SpillValue::Number(number(value)),
                s,
                a,
            },
            ("n", None) => Cell::NumberCell {
                v: number(value),
                s,
            },
            ("e", anchor) => {
                let ei =
                    get_error_by_english_name(error_name(value, metadata)).unwrap_or(Error::ERROR);
                match anchor {
                    Some(a) => Cell::SpillCell {
                        v: SpillValue::Error(ei),
                        s,
                        a,
                    },
                    None => Cell::ErrorCell { ei, s },
                }
            }
            ("s", _) => Cell::SharedString {
                si: value.unwrap_or("0").parse::<i32>().unwrap_or(0),
                s,
            },
            // IronCalc adds the text to the shared strings even when the cell is a spill.
            ("str", anchor) => {
                let text = decode_xlsx_escapes(value.unwrap_or(""));
                match anchor {
                    Some(a) => {
                        self.intern(text.clone());
                        Cell::SpillCell {
                            v: SpillValue::Text(text),
                            s,
                            a,
                        }
                    }
                    None => self.string_cell(text, s, place),
                }
            }
            ("d", _) => Cell::ErrorCell {
                ei: Error::NIMPL,
                s,
            },
            ("inlineStr", _) => self.string_cell(inline.unwrap_or_default(), s, place),
            ("empty", _) => Cell::EmptyCell { s },
            _ => Cell::ErrorCell {
                ei: Error::ERROR,
                s,
            },
        }
    }
}

fn formula_cell(f: i32, s: i32, array: CellArray, v: FormulaValue) -> Cell {
    match array {
        CellArray::None => Cell::CellFormula { f, s, v },
        CellArray::Dynamic(width, height) => Cell::ArrayFormula {
            f,
            s,
            r: (width, height),
            kind: ArrayKind::Dynamic,
            v,
        },
        CellArray::Cse(width, height) => Cell::ArrayFormula {
            f,
            s,
            r: (width, height),
            kind: ArrayKind::Cse,
            v,
        },
    }
}

fn number(value: Option<&str>) -> f64 {
    value.unwrap_or("0").parse::<f64>().unwrap_or(0.0)
}

// Excel stores #SPILL! and #CALC! as #VALUE! plus value metadata, for older readers.
fn error_name<'v>(value: Option<&'v str>, metadata: Option<&str>) -> &'v str {
    let name = value.unwrap_or("#ERROR!");
    match (name, metadata) {
        ("#VALUE!", Some("1")) => "#CALC!",
        ("#VALUE!", Some("2")) => "#SPILL!",
        _ => name,
    }
}

fn resolve_reference(reference: &quick_xml::events::BytesRef<'_>) -> Result<char, ReadError> {
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

// IronCalc reads a formula's own cell from "<sheet>!<cell>": the leading letters are the
// column and everything after them the row.
fn formula_context(cell_ref: &str) -> Result<(i32, i32), ReadError> {
    let split = cell_ref
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(cell_ref.len());
    let (column, row) = cell_ref.split_at(split);
    let row = row.parse::<i32>().map_err(unreadable)?;
    let column = column_to_number(column).map_err(unreadable)?;
    Ok((row, column))
}

fn parse_range(range: &str) -> Option<(i32, i32, i32, i32)> {
    let parts: Vec<&str> = range.split(':').collect();
    match parts.as_slice() {
        [single] => parse_reference_a1(single).map(|r| (r.row, r.column, r.row, r.column)),
        [left, right] => {
            let (left, right) = (parse_reference_a1(left)?, parse_reference_a1(right)?);
            Some((left.row, left.column, right.row, right.column))
        }
        _ => None,
    }
}
