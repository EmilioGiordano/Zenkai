use std::path::Path;

use serde::{Deserialize, Serialize};
use zenkai_formats::{CsvFormat, DateOrder, Delimiter, Encoding, LineEnding};

use crate::entry::FileSeen;
use crate::files;

// How a CSV was read, so that saving it writes it back the same way. The byte order mark is
// part of the encoding: UTF-8 with BOM and both UTF-16 forms have one, the others do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
                            decimal_comma: true,
                            date_order: DateOrder::DayFirst,
                        },
                        loss_accepted: true,
                    };
                    let json = serde_json::to_string(&binding).unwrap();
                    assert_eq!(serde_json::from_str::<CsvBinding>(&json).unwrap(), binding);
                }
            }
        }
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
