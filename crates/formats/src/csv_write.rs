use std::borrow::Cow;
use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use crate::csv_format::{CsvError, Delimiter};

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

// Writes an export row by row, so its memory never grows with the size of the sheet.
pub struct CsvWriter<W: Write> {
    writer: csv::Writer<SharedOut<W>>,
    out: SharedOut<W>,
    delimiter: u8,
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
    pub fn new(mut out: W, delimiter: Delimiter) -> Result<CsvWriter<W>, CsvError> {
        out.write_all(UTF8_BOM)?;
        let out = SharedOut(Rc::new(RefCell::new(out)));
        let writer = csv::WriterBuilder::new()
            .delimiter(delimiter.byte())
            .terminator(csv::Terminator::CRLF)
            .flexible(true)
            .from_writer(out.clone());
        Ok(CsvWriter {
            writer,
            out,
            delimiter: delimiter.byte(),
        })
    }

    pub fn write_field(&mut self, text: &str) -> Result<(), CsvError> {
        Ok(self.writer.write_field(neutralize(text).as_bytes())?)
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

    pub fn write_row<'a>(
        &mut self,
        fields: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), CsvError> {
        for field in fields {
            self.write_field(field)?;
        }
        self.end_row()
    }

    pub fn finish(self) -> Result<W, CsvError> {
        let CsvWriter {
            writer, out: kept, ..
        } = self;
        drop(
            writer
                .into_inner()
                .map_err(|error| CsvError::Write(error.into_error()))?,
        );
        match Rc::try_unwrap(kept.0) {
            Ok(cell) => Ok(cell.into_inner()),
            Err(_) => Err(CsvError::Write(io::Error::other(
                "the output is still shared after the csv writer was dropped",
            ))),
        }
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
    let mut writer = CsvWriter::new(Vec::new(), delimiter)?;
    for row in rows {
        writer.write_row(row.iter().map(String::as_str))?;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(delimiter: Delimiter, build: impl FnOnce(&mut CsvWriter<Vec<u8>>)) -> String {
        let mut writer = CsvWriter::new(Vec::new(), delimiter).unwrap();
        build(&mut writer);
        let bytes = writer.finish().unwrap();
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
        let mut writer = CsvWriter::new(Vec::new(), delimiter).unwrap();
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
        writer.finish().unwrap()
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
                    0..5,
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
