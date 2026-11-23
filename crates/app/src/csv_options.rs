use std::path::Path;

use serde::{Deserialize, Serialize};
use zenkai_formats::{CsvFormat, DateOrder, Delimiter, Encoding, LineEnding};

use crate::entry::FileSeen;
use crate::files;

// How a CSV was read, so that saving it writes it back the same way. The byte order mark is
// part of the encoding: UTF-8 with BOM and both UTF-16 forms have one, the others do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvOptions {
    pub delimiter: Delimiter,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub decimal_comma: bool,
    pub date_order: DateOrder,
}

impl CsvOptions {
    // What an export to `path` writes: Excel's own choices, with the delimiter its extension
    // implies.
    pub fn exported_as(path: &Path) -> CsvOptions {
        CsvOptions {
            delimiter: files::export_delimiter(path),
            encoding: Encoding::Utf8Bom,
            line_ending: LineEnding::Crlf,
            decimal_comma: false,
            date_order: DateOrder::MonthFirst,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvBinding {
    pub options: CsvOptions,
    // The user chose to keep the CSV format although the workbook holds more than a CSV can,
    // so Save does not ask again.
    pub loss_accepted: bool,
}

impl CsvBinding {
    pub fn new(options: CsvOptions) -> CsvBinding {
        CsvBinding {
            options,
            loss_accepted: false,
        }
    }
}

pub struct CsvLoad {
    pub options: CsvOptions,
    pub seen: FileSeen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CsvRecord {
    delimiter: String,
    encoding: String,
    line_ending: String,
    decimal_comma: bool,
    day_first: bool,
    loss_accepted: bool,
}

const DELIMITERS: [Delimiter; 4] = [
    Delimiter::Comma,
    Delimiter::Semicolon,
    Delimiter::Tab,
    Delimiter::Pipe,
];

const ENCODINGS: [Encoding; 5] = [
    Encoding::Utf8,
    Encoding::Utf8Bom,
    Encoding::Utf16Le,
    Encoding::Utf16Be,
    Encoding::Windows1252,
];

const LINE_ENDINGS: [LineEnding; 3] = [LineEnding::Crlf, LineEnding::Lf, LineEnding::Cr];

fn delimiter_code(delimiter: Delimiter) -> &'static str {
    match delimiter {
        Delimiter::Comma => "comma",
        Delimiter::Semicolon => "semicolon",
        Delimiter::Tab => "tab",
        Delimiter::Pipe => "pipe",
    }
}

fn encoding_code(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "utf-8",
        Encoding::Utf8Bom => "utf-8-bom",
        Encoding::Utf16Le => "utf-16le",
        Encoding::Utf16Be => "utf-16be",
        Encoding::Windows1252 => "windows-1252",
    }
}

fn line_ending_code(line_ending: LineEnding) -> &'static str {
    match line_ending {
        LineEnding::Crlf => "crlf",
        LineEnding::Lf => "lf",
        LineEnding::Cr => "cr",
    }
}

impl CsvRecord {
    pub fn of(binding: &CsvBinding) -> CsvRecord {
        let options = binding.options;
        CsvRecord {
            delimiter: delimiter_code(options.delimiter).to_string(),
            encoding: encoding_code(options.encoding).to_string(),
            line_ending: line_ending_code(options.line_ending).to_string(),
            decimal_comma: options.decimal_comma,
            day_first: options.date_order == DateOrder::DayFirst,
            loss_accepted: binding.loss_accepted,
        }
    }

    // A code this version does not know gives nothing: the file is read again as if opened now.
    pub fn binding(&self) -> Option<CsvBinding> {
        let options = CsvOptions {
            delimiter: DELIMITERS
                .into_iter()
                .find(|delimiter| delimiter_code(*delimiter) == self.delimiter)?,
            encoding: ENCODINGS
                .into_iter()
                .find(|encoding| encoding_code(*encoding) == self.encoding)?,
            line_ending: LINE_ENDINGS
                .into_iter()
                .find(|line_ending| line_ending_code(*line_ending) == self.line_ending)?,
            decimal_comma: self.decimal_comma,
            date_order: if self.day_first {
                DateOrder::DayFirst
            } else {
                DateOrder::MonthFirst
            },
        };
        Some(CsvBinding {
            options,
            loss_accepted: self.loss_accepted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_gives_back_the_binding_it_was_made_from() {
        for delimiter in DELIMITERS {
            for encoding in ENCODINGS {
                for line_ending in LINE_ENDINGS {
                    for day_first in [false, true] {
                        let binding = CsvBinding {
                            options: CsvOptions {
                                delimiter,
                                encoding,
                                line_ending,
                                decimal_comma: day_first,
                                date_order: if day_first {
                                    DateOrder::DayFirst
                                } else {
                                    DateOrder::MonthFirst
                                },
                            },
                            loss_accepted: day_first,
                        };
                        let json = serde_json::to_string(&CsvRecord::of(&binding)).unwrap();
                        let record: CsvRecord = serde_json::from_str(&json).unwrap();
                        assert_eq!(record.binding(), Some(binding));
                    }
                }
            }
        }
    }

    #[test]
    fn an_unknown_code_gives_no_binding() {
        let binding = CsvBinding::new(CsvOptions::exported_as(Path::new("a.csv")));
        let mut record = CsvRecord::of(&binding);
        record.encoding = "ebcdic".to_string();
        assert_eq!(record.binding(), None);
    }

    #[test]
    fn exports_follow_the_extension_and_excels_own_choices() {
        let tsv = CsvOptions::exported_as(Path::new("data/a.TSV"));
        assert_eq!(tsv.delimiter, Delimiter::Tab);
        let csv = CsvOptions::exported_as(Path::new("data/a.csv"));
        assert_eq!(csv.delimiter, Delimiter::Comma);
        assert_eq!(csv.encoding, Encoding::Utf8Bom);
        assert_eq!(csv.line_ending, LineEnding::Crlf);
    }
}
