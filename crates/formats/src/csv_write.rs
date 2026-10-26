use std::borrow::Cow;
use std::io::Write;

use crate::csv_format::{CsvError, Delimiter};

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

// Writes an export row by row, so its memory never grows with the size of the sheet.
pub struct CsvWriter<W: Write> {
    writer: csv::Writer<W>,
}

impl<W: Write> CsvWriter<W> {
    pub fn new(mut out: W, delimiter: Delimiter) -> Result<CsvWriter<W>, CsvError> {
        out.write_all(UTF8_BOM)?;
        let writer = csv::WriterBuilder::new()
            .delimiter(delimiter.byte())
            .terminator(csv::Terminator::CRLF)
            .flexible(true)
            .from_writer(out);
        Ok(CsvWriter { writer })
    }

    pub fn write_field(&mut self, text: &str) -> Result<(), CsvError> {
        Ok(self.writer.write_field(neutralize(text).as_bytes())?)
    }

    pub fn write_empty_fields(&mut self, count: usize) -> Result<(), CsvError> {
        for _ in 0..count {
            self.writer.write_field(b"")?;
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
        self.writer
            .into_inner()
            .map_err(|error| CsvError::Write(error.into_error()))
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
