use serde::{Deserialize, Serialize};
use zenkai_formats::{
    CsvFormat, DateOrder, DecimalMark, Delimiter, Encoding, Guess, LineEnding, ParsedCsv,
};

// How a CSV was read, so that saving it writes it back the same way. The byte order mark is
// part of the encoding: UTF-8 with BOM and both UTF-16 forms have one, the others do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvOptions {
    pub delimiter: Delimiter,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub decimal_mark: DecimalMark,
    pub date_order: DateOrder,
}

impl CsvOptions {
    // What an export writes: Excel's own choices.
    pub fn exported_as(delimiter: Delimiter) -> CsvOptions {
        CsvOptions {
            delimiter,
            encoding: Encoding::Utf8Bom,
            line_ending: LineEnding::Crlf,
            decimal_mark: DecimalMark::Dot,
            date_order: DateOrder::MonthFirst,
        }
    }

    // The file's own delimiter, encoding and line ending, with the number and date reading
    // that was guessed or that the user chose in its place.
    pub fn from_read(parsed: &ParsedCsv, reading: &Guess) -> CsvOptions {
        CsvOptions {
            delimiter: parsed.delimiter,
            encoding: parsed.encoding,
            line_ending: parsed.line_ending,
            decimal_mark: reading.decimal_mark,
            date_order: reading.date_order,
        }
    }

    pub fn format(self) -> CsvFormat {
        CsvFormat {
            delimiter: self.delimiter,
            encoding: self.encoding,
            line_ending: self.line_ending,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossAnswer {
    #[default]
    NotAsked,
    // The user chose to keep the CSV format although the workbook holds more than a CSV can.
    KeepCsv,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberFormats {
    #[default]
    AsRead,
    // A number format was edited since the file was read, which a CSV cannot hold.
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvBinding {
    pub options: CsvOptions,
    #[serde(default)]
    pub loss_answer: LossAnswer,
    #[serde(default)]
    pub number_formats: NumberFormats,
}

impl CsvBinding {
    pub fn new(options: CsvOptions) -> CsvBinding {
        CsvBinding {
            options,
            loss_answer: LossAnswer::NotAsked,
            number_formats: NumberFormats::AsRead,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binding_survives_the_session_file() {
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
                    let binding = CsvBinding {
                        options: CsvOptions {
                            delimiter,
                            encoding,
                            line_ending,
                            decimal_mark: DecimalMark::Comma,
                            date_order: DateOrder::DayFirst,
                        },
                        loss_answer: LossAnswer::KeepCsv,
                        number_formats: NumberFormats::Changed,
                    };
                    let json = serde_json::to_string(&binding).unwrap();
                    assert_eq!(serde_json::from_str::<CsvBinding>(&json).unwrap(), binding);
                }
            }
        }
    }

    #[test]
    fn an_export_is_excels_own_choices_with_the_delimiter_it_is_given() {
        let tsv = CsvOptions::exported_as(Delimiter::Tab);
        assert_eq!(tsv.delimiter, Delimiter::Tab);
        assert_eq!(tsv.encoding, Encoding::Utf8Bom);
        assert_eq!(tsv.line_ending, LineEnding::Crlf);
        assert_eq!(tsv.decimal_mark, DecimalMark::Dot);
    }
}
