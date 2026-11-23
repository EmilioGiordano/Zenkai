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

// The kinds of content a CSV cannot hold. A set of them says what a save would lose, what the
// user accepted losing, and what edits have introduced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Losses {
    pub sheets: bool,
    pub formulas: bool,
    pub formatting: bool,
    pub number_formats: bool,
}

impl Losses {
    pub fn any(self) -> bool {
        self.sheets || self.formulas || self.formatting || self.number_formats
    }

    // The kinds in `self` that `accepted` does not cover.
    pub fn beyond(self, accepted: Losses) -> Losses {
        Losses {
            sheets: self.sheets && !accepted.sheets,
            formulas: self.formulas && !accepted.formulas,
            formatting: self.formatting && !accepted.formatting,
            number_formats: self.number_formats && !accepted.number_formats,
        }
    }

    pub fn with(self, other: Losses) -> Losses {
        Losses {
            sheets: self.sheets || other.sheets,
            formulas: self.formulas || other.formulas,
            formatting: self.formatting || other.formatting,
            number_formats: self.number_formats || other.number_formats,
        }
    }
}

// A document that is the CSV file itself. `options` is `None` until the file has been read for
// them; a save reads it then, and fails if it cannot, rather than write in a format guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvBinding {
    pub options: Option<CsvOptions>,
    #[serde(default)]
    pub accepted: Losses,
    // Formatting and number formats changed by edits since the file was read.
    #[serde(default)]
    pub introduced: Losses,
}

impl CsvBinding {
    pub fn new(options: CsvOptions) -> CsvBinding {
        CsvBinding {
            options: Some(options),
            ..CsvBinding::unresolved()
        }
    }

    pub fn unresolved() -> CsvBinding {
        CsvBinding {
            options: None,
            accepted: Losses::default(),
            introduced: Losses::default(),
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
                        options: Some(CsvOptions {
                            delimiter,
                            encoding,
                            line_ending,
                            decimal_mark: DecimalMark::Comma,
                            date_order: DateOrder::DayFirst,
                        }),
                        accepted: Losses {
                            formatting: true,
                            ..Losses::default()
                        },
                        introduced: Losses {
                            number_formats: true,
                            ..Losses::default()
                        },
                    };
                    let json = serde_json::to_string(&binding).unwrap();
                    assert_eq!(serde_json::from_str::<CsvBinding>(&json).unwrap(), binding);
                }
            }
        }
        let unresolved = serde_json::to_string(&CsvBinding::unresolved()).unwrap();
        assert_eq!(
            serde_json::from_str::<CsvBinding>(&unresolved).unwrap(),
            CsvBinding::unresolved()
        );
    }

    #[test]
    fn what_a_save_would_lose_is_asked_only_for_the_kinds_not_yet_accepted() {
        let found = Losses {
            sheets: true,
            formatting: true,
            ..Losses::default()
        };
        let accepted = Losses {
            formatting: true,
            ..Losses::default()
        };
        let new = found.beyond(accepted);
        assert!(new.sheets && !new.formatting);
        assert!(!found.beyond(found).any());
        assert!(!found.beyond(accepted.with(found)).any());
        assert!(accepted.with(found).sheets);
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
