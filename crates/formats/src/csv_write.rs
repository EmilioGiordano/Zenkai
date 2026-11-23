use std::borrow::Cow;
use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use encoding_rs::WINDOWS_1252;

use crate::csv_format::{CsvError, Delimiter, Encoding, LineEnding};

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvFormat {
    pub delimiter: Delimiter,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
}

impl CsvFormat {
    // UTF-8 with BOM so Excel opens it right, and the line ending Excel writes.
    pub fn export(delimiter: Delimiter) -> CsvFormat {
        CsvFormat {
            delimiter,
            encoding: Encoding::Utf8Bom,
            line_ending: LineEnding::Crlf,
        }
    }
}

// Reading recognises UTF-16 only by its BOM, so it is always written; plain UTF-8 and
// Windows-1252 have none.
fn byte_order_mark(encoding: Encoding) -> &'static [u8] {
    match encoding {
        Encoding::Utf8 | Encoding::Windows1252 => b"",
        Encoding::Utf8Bom => UTF8_BOM,
        Encoding::Utf16Le => b"\xFF\xFE",
        Encoding::Utf16Be => b"\xFE\xFF",
    }
}

#[derive(Clone, Copy)]
enum Recode {
    Utf16Le,
    Utf16Be,
    Windows1252,
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn utf16(text: &str, to_bytes: fn(u16) -> [u8; 2]) -> Vec<u8> {
    text.encode_utf16().flat_map(to_bytes).collect()
}

// Turns the UTF-8 the csv writer produces into the file's own encoding. A chunk may end in the
// middle of a character, so the unfinished bytes wait for the next one.
struct Encoded<W> {
    inner: W,
    recode: Option<Recode>,
    pending: Vec<u8>,
}

impl<W: Write> Encoded<W> {
    fn new(mut inner: W, encoding: Encoding) -> io::Result<Encoded<W>> {
        inner.write_all(byte_order_mark(encoding))?;
        let recode = match encoding {
            Encoding::Utf8 | Encoding::Utf8Bom => None,
            Encoding::Utf16Le => Some(Recode::Utf16Le),
            Encoding::Utf16Be => Some(Recode::Utf16Be),
            Encoding::Windows1252 => Some(Recode::Windows1252),
        };
        Ok(Encoded {
            inner,
            recode,
            pending: Vec::new(),
        })
    }

    fn write_recoded(&mut self, recode: Recode, bytes: &[u8]) -> io::Result<()> {
        self.pending.extend_from_slice(bytes);
        let whole = match std::str::from_utf8(&self.pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(invalid_data("the writer produced bytes that are not UTF-8")),
        };
        let unfinished = self.pending.split_off(whole);
        let text = String::from_utf8_lossy(&self.pending);
        let encoded: Cow<[u8]> = match recode {
            Recode::Utf16Le => Cow::Owned(utf16(&text, u16::to_le_bytes)),
            Recode::Utf16Be => Cow::Owned(utf16(&text, u16::to_be_bytes)),
            Recode::Windows1252 => {
                let (bytes, _, unmappable) = WINDOWS_1252.encode(&text);
                if unmappable {
                    return Err(invalid_data(
                        "the text has characters Windows-1252 cannot hold",
                    ));
                }
                bytes
            }
        };
        self.inner.write_all(&encoded)?;
        self.pending = unfinished;
        Ok(())
    }
}

impl<W: Write> Write for Encoded<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.recode {
            None => self.inner.write(buf),
            Some(recode) => self.write_recoded(recode, buf).map(|()| buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            return Err(invalid_data("the output ends in the middle of a character"));
        }
        self.inner.flush()
    }
}

// Writes an export row by row, so its memory never grows with the size of the sheet.
pub struct CsvWriter<W: Write> {
    writer: csv::Writer<SharedOut<Encoded<W>>>,
    out: SharedOut<Encoded<W>>,
    delimiter: u8,
    terminator: &'static [u8],
}

// The csv writer owns its output and has no way to reach it, but long runs of empty
// fields are cheaper written as raw delimiters between its own writes.
struct SharedOut<W>(Rc<RefCell<W>>);

impl<W> Clone for SharedOut<W> {
    fn clone(&self) -> Self {
        SharedOut(Rc::clone(&self.0))
    }
}

impl<W: Write> Write for SharedOut<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

impl<W: Write> CsvWriter<W> {
    pub fn new(out: W, delimiter: Delimiter) -> Result<CsvWriter<W>, CsvError> {
        CsvWriter::in_format(out, CsvFormat::export(delimiter))
    }

    pub fn in_format(out: W, format: CsvFormat) -> Result<CsvWriter<W>, CsvError> {
        let out = SharedOut(Rc::new(RefCell::new(Encoded::new(out, format.encoding)?)));
        let (terminator, bytes): (_, &'static [u8]) = match format.line_ending {
            LineEnding::Crlf => (csv::Terminator::CRLF, b"\r\n"),
            LineEnding::Lf => (csv::Terminator::Any(b'\n'), b"\n"),
            LineEnding::Cr => (csv::Terminator::Any(b'\r'), b"\r"),
        };
        let writer = csv::WriterBuilder::new()
            .delimiter(format.delimiter.byte())
            .terminator(terminator)
            .flexible(true)
            .from_writer(out.clone());
        Ok(CsvWriter {
            writer,
            out,
            delimiter: format.delimiter.byte(),
            terminator: bytes,
        })
    }

    // For text a program other than this one will read: what starts like a formula is
    // prefixed so it is shown, never evaluated.
    pub fn write_field(&mut self, text: &str) -> Result<(), CsvError> {
        Ok(self.writer.write_field(neutralize(text).as_bytes())?)
    }

    // For a field of the file the user opened and did not edit: it stays as it was.
    pub fn write_verbatim_field(&mut self, text: &str) -> Result<(), CsvError> {
        Ok(self.writer.write_field(text.as_bytes())?)
    }

    // Two fields go through the csv writer so it counts the row as more than one empty
    // field (which it would write as `""`); the rest of a long run is raw delimiters.
    pub fn write_empty_fields(&mut self, count: usize) -> Result<(), CsvError> {
        let through_writer = count.min(2);
        for _ in 0..through_writer {
            self.writer.write_field(b"")?;
        }
        let raw = count - through_writer;
        if raw > 0 {
            self.writer.flush()?;
            let mut out = self.out.0.borrow_mut();
            let chunk = [self.delimiter; 8192];
            let mut left = raw;
            while left > 0 {
                let now = left.min(chunk.len());
                out.write_all(&chunk[..now])?;
                left -= now;
            }
        }
        Ok(())
    }

    // A row whose only field is empty is written as `""`, as the csv crate does, so the
    // line is not read back as no row at all.
    pub fn end_row(&mut self) -> Result<(), CsvError> {
        Ok(self.writer.write_record(std::iter::empty::<&[u8]>())?)
    }

    // Empty lines, which the csv writer never writes by itself: rows with no fields. A run is
    // one flush of what the csv writer holds, not one per line.
    pub fn write_blank_lines(&mut self, count: usize) -> Result<(), CsvError> {
        if count == 0 {
            return Ok(());
        }
        self.writer.flush()?;
        let mut out = self.out.0.borrow_mut();
        for _ in 0..count {
            out.write_all(self.terminator)?;
        }
        Ok(())
    }

    pub fn write_row<'a>(
        &mut self,
        fields: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), CsvError> {
        let mut fields = fields.into_iter().peekable();
        if fields.peek().is_none() {
            return self.write_blank_lines(1);
        }
        for field in fields {
            self.write_field(field)?;
        }
        self.end_row()
    }

    // The file ends after the last field written, with no line break, when it is not preceded
    // by `end_row`.
    pub fn finish(mut self) -> Result<(), CsvError> {
        Ok(self.writer.flush()?)
    }
}

// Cells that start like a formula are prefixed so another spreadsheet opening the
// export shows the text instead of evaluating it (CSV formula injection).
fn neutralize(text: &str) -> Cow<'_, str> {
    match text.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') if text.parse::<f64>().is_err() => {
            Cow::Owned(format!("'{text}"))
        }
        _ => Cow::Borrowed(text),
    }
}

pub fn write_csv(rows: &[Vec<String>], delimiter: Delimiter) -> Result<Vec<u8>, CsvError> {
    let mut bytes = Vec::new();
    let mut writer = CsvWriter::new(&mut bytes, delimiter)?;
    for row in rows {
        writer.write_row(row.iter().map(String::as_str))?;
    }
    writer.finish()?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(delimiter: Delimiter, build: impl FnOnce(&mut CsvWriter<&mut Vec<u8>>)) -> String {
        let mut bytes = Vec::new();
        let mut writer = CsvWriter::new(&mut bytes, delimiter).unwrap();
        build(&mut writer);
        writer.finish().unwrap();
        assert!(bytes.starts_with(UTF8_BOM));
        String::from_utf8(bytes[UTF8_BOM.len()..].to_vec()).unwrap()
    }

    #[test]
    fn quotes_only_fields_holding_the_delimiter_a_quote_or_a_line_break() {
        let text = written(Delimiter::Semicolon, |w| {
            w.write_row(["a;b", "say \"hi\"", "two\nlines", "a,b", "plain"])
                .unwrap();
        });
        assert_eq!(
            text,
            "\"a;b\";\"say \"\"hi\"\"\";\"two\nlines\";a,b;plain\r\n"
        );
    }

    #[test]
    fn separates_fields_with_the_chosen_delimiter() {
        let text = written(Delimiter::Tab, |w| w.write_row(["x", "y", "z"]).unwrap());
        assert_eq!(text, "x\ty\tz\r\n");
        let text = written(Delimiter::Pipe, |w| w.write_row(["x", "y"]).unwrap());
        assert_eq!(text, "x|y\r\n");
    }

    #[test]
    fn neutralizes_text_that_starts_like_a_formula_but_not_numbers() {
        let text = written(Delimiter::Comma, |w| {
            w.write_row(["=1+1", "+A1", "-5", "+3", "@x", "\tx", "ok"])
                .unwrap();
        });
        assert_eq!(text, "'=1+1,'+A1,-5,+3,'@x,'\tx,ok\r\n");
    }

    fn in_format(format: CsvFormat, rows: &[&[&str]]) -> Result<Vec<u8>, CsvError> {
        let mut bytes = Vec::new();
        let mut writer = CsvWriter::in_format(&mut bytes, format)?;
        for row in rows {
            writer.write_row(row.iter().copied())?;
        }
        writer.finish()?;
        Ok(bytes)
    }

    fn format(encoding: Encoding, line_ending: LineEnding) -> CsvFormat {
        CsvFormat {
            delimiter: Delimiter::Semicolon,
            encoding,
            line_ending,
        }
    }

    #[test]
    fn a_verbatim_field_is_written_as_it_is_and_the_same_text_as_a_plain_field_is_neutralized() {
        let texts = ["-", "+34 600", "=x", "@user", "ok"];
        let mut bytes = Vec::new();
        let mut writer =
            CsvWriter::in_format(&mut bytes, format(Encoding::Utf8, LineEnding::Lf)).unwrap();
        for text in texts {
            writer.write_verbatim_field(text).unwrap();
        }
        writer.end_row().unwrap();
        for text in texts {
            writer.write_field(text).unwrap();
        }
        writer.end_row().unwrap();
        writer.finish().unwrap();
        assert_eq!(bytes, b"-;+34 600;=x;@user;ok\n'-;'+34 600;'=x;'@user;ok\n");
    }

    #[test]
    fn writes_each_encoding_with_its_own_byte_order_mark() {
        let rows: &[&[&str]] = &[&["é", "x"]];
        let expected: [(Encoding, &[u8]); 5] = [
            (Encoding::Utf8, b"\xC3\xA9;x\n"),
            (Encoding::Utf8Bom, b"\xEF\xBB\xBF\xC3\xA9;x\n"),
            (Encoding::Windows1252, b"\xE9;x\n"),
            (Encoding::Utf16Le, b"\xFF\xFE\xE9\x00;\x00x\x00\n\x00"),
            (Encoding::Utf16Be, b"\xFE\xFF\x00\xE9\x00;\x00x\x00\n"),
        ];
        for (encoding, bytes) in expected {
            let written = in_format(format(encoding, LineEnding::Lf), rows).unwrap();
            assert_eq!(written, bytes, "{encoding:?}");
        }
    }

    #[test]
    fn writes_the_chosen_line_ending() {
        let rows: &[&[&str]] = &[&["a", "b"], &["c", "d"]];
        let expected: [(LineEnding, &[u8]); 3] = [
            (LineEnding::Crlf, b"a;b\r\nc;d\r\n"),
            (LineEnding::Lf, b"a;b\nc;d\n"),
            (LineEnding::Cr, b"a;b\rc;d\r"),
        ];
        for (line_ending, bytes) in expected {
            let written = in_format(format(Encoding::Utf8, line_ending), rows).unwrap();
            assert_eq!(written, bytes, "{line_ending:?}");
        }
    }

    #[test]
    fn text_windows_1252_cannot_hold_fails_instead_of_being_replaced() {
        let rows: &[&[&str]] = &[&["caf\u{e9}", "\u{65e5}\u{672c}"]];
        let failed = in_format(format(Encoding::Windows1252, LineEnding::Lf), rows);
        assert!(matches!(
            failed,
            Err(CsvError::Write(error)) if error.kind() == io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn a_character_split_between_two_writes_is_encoded_whole() {
        let mut out = Vec::new();
        let mut encoded = Encoded::new(&mut out, Encoding::Utf16Le).unwrap();
        encoded.write_all(&[0xC3]).unwrap();
        encoded.write_all(&[0xA9]).unwrap();
        assert_eq!(out, b"\xFF\xFE\xE9\x00");
    }

    #[test]
    fn bytes_that_are_not_utf8_or_end_inside_a_character_are_an_error_not_a_replacement() {
        let invalid = |error: io::Error| error.kind() == io::ErrorKind::InvalidData;
        let mut out = Vec::new();
        let mut encoded = Encoded::new(&mut out, Encoding::Utf16Le).unwrap();
        assert!(invalid(encoded.write_all(&[b'a', 0xFF, b'b']).unwrap_err()));

        let mut out = Vec::new();
        let mut encoded = Encoded::new(&mut out, Encoding::Windows1252).unwrap();
        encoded.write_all(&[b'a', 0xC3]).unwrap();
        assert!(invalid(encoded.flush().unwrap_err()));
        encoded.write_all(&[0xA9]).unwrap();
        encoded.flush().unwrap();
    }

    #[test]
    fn what_is_written_reads_back_in_the_same_format() {
        use crate::csv_format::parse_csv;

        let rows: Vec<Vec<String>> = vec![
            vec!["name".into(), "caf\u{e9}".into(), "line 1\nline 2".into()],
            vec!["1,5".into(), String::new(), "x".into()],
        ];
        let borrowed: Vec<Vec<&str>> = rows
            .iter()
            .map(|row| row.iter().map(String::as_str).collect())
            .collect();
        let borrowed: Vec<&[&str]> = borrowed.iter().map(Vec::as_slice).collect();
        for delimiter in [
            Delimiter::Comma,
            Delimiter::Semicolon,
            Delimiter::Tab,
            Delimiter::Pipe,
        ] {
            for encoding in [
                Encoding::Utf8,
                Encoding::Utf8Bom,
                Encoding::Utf16Le,
                Encoding::Utf16Be,
                Encoding::Windows1252,
            ] {
                for line_ending in [LineEnding::Crlf, LineEnding::Lf, LineEnding::Cr] {
                    let format = CsvFormat {
                        delimiter,
                        encoding,
                        line_ending,
                    };
                    let bytes = in_format(format, &borrowed).unwrap();
                    let parsed = parse_csv(&bytes, Some(delimiter)).unwrap();
                    assert_eq!(parsed.rows, rows, "{format:?}");
                    assert_eq!(parsed.encoding, encoding, "{format:?}");
                    assert_eq!(parsed.line_ending, line_ending, "{format:?}");
                }
            }
        }
    }

    #[test]
    fn gaps_write_empty_fields_and_lone_empty_fields_stay_a_row() {
        let text = written(Delimiter::Comma, |w| {
            w.write_field("a").unwrap();
            w.write_empty_fields(2).unwrap();
            w.write_field("b").unwrap();
            w.end_row().unwrap();
            w.write_empty_fields(4).unwrap();
            w.end_row().unwrap();
            w.write_empty_fields(1).unwrap();
            w.end_row().unwrap();
            w.end_row().unwrap();
        });
        assert_eq!(text, "a,,,b\r\n,,,\r\n\"\"\r\n\"\"\r\n");
    }

    #[test]
    fn a_row_with_no_fields_is_an_empty_line_and_the_file_can_end_without_a_line_break() {
        let rows: &[&[&str]] = &[&["a", "b"], &[], &["c"]];
        let mut format = format(Encoding::Utf8, LineEnding::Lf);
        format.delimiter = Delimiter::Comma;
        assert_eq!(in_format(format, rows).unwrap(), b"a,b\n\nc\n");

        let mut bytes = Vec::new();
        let mut writer = CsvWriter::in_format(&mut bytes, format).unwrap();
        writer.write_row(["a", "b"]).unwrap();
        writer.write_verbatim_field("c").unwrap();
        writer.finish().unwrap();
        assert_eq!(bytes, b"a,b\nc");
    }

    #[test]
    fn a_long_run_of_empty_fields_equals_the_same_fields_one_by_one() {
        for gap in [0usize, 1, 2, 3, 5, 8191, 8192, 8193, 20_000] {
            for lead in [false, true] {
                let bulk = written(Delimiter::Semicolon, |w| {
                    if lead {
                        w.write_field("a").unwrap();
                    }
                    w.write_empty_fields(gap).unwrap();
                    w.write_field("b").unwrap();
                    w.end_row().unwrap();
                    w.write_empty_fields(gap).unwrap();
                    w.end_row().unwrap();
                });
                let one_by_one = written(Delimiter::Semicolon, |w| {
                    if lead {
                        w.write_field("a").unwrap();
                    }
                    for _ in 0..gap {
                        w.write_field("").unwrap();
                    }
                    w.write_field("b").unwrap();
                    w.end_row().unwrap();
                    for _ in 0..gap {
                        w.write_field("").unwrap();
                    }
                    w.end_row().unwrap();
                });
                assert_eq!(bulk, one_by_one, "gap {gap} lead {lead}");
            }
        }
    }

    // The writer before it streamed: the csv crate given each whole row at once.
    fn whole_rows(rows: &[Vec<String>], delimiter: Delimiter) -> Vec<u8> {
        let mut out = UTF8_BOM.to_vec();
        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter.byte())
            .terminator(csv::Terminator::CRLF)
            .flexible(true)
            .from_writer(&mut out);
        for row in rows {
            writer
                .write_record(row.iter().map(|cell| neutralize(cell).into_owned()))
                .unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        out
    }

    fn streamed_with_gaps(rows: &[Vec<String>], delimiter: Delimiter) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut writer = CsvWriter::new(&mut bytes, delimiter).unwrap();
        for row in rows {
            let mut gap = 0;
            for cell in row {
                if cell.is_empty() {
                    gap += 1;
                } else {
                    writer.write_empty_fields(gap).unwrap();
                    gap = 0;
                    writer.write_field(cell).unwrap();
                }
            }
            writer.write_empty_fields(gap).unwrap();
            writer.end_row().unwrap();
        }
        writer.finish().unwrap();
        bytes
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { cases: 300, ..Default::default() })]

        #[test]
        fn streaming_with_gaps_writes_the_bytes_of_whole_rows(
            rows in proptest::collection::vec(
                proptest::collection::vec(
                    proptest::prop_oneof![
                        proptest::strategy::Just(String::new()),
                        "[a1=+@,;| \t\"\r\n-]{0,4}",
                    ],
                    1..5,
                ),
                0..6,
            ),
            delimiter in proptest::sample::select(vec![
                Delimiter::Comma, Delimiter::Semicolon, Delimiter::Tab, Delimiter::Pipe,
            ]),
        ) {
            proptest::prop_assert_eq!(
                streamed_with_gaps(&rows, delimiter),
                whole_rows(&rows, delimiter)
            );
            proptest::prop_assert_eq!(write_csv(&rows, delimiter).unwrap(), whole_rows(&rows, delimiter));
        }
    }
}
