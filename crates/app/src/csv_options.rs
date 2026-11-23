use serde::{Deserialize, Serialize};
use zenkai_engine::FormatEdits;
use zenkai_formats::{
    CsvFormat, DateOrder, DecimalMark, Delimiter, Encoding, Guess, LineEnding, ParsedCsv,
};
use zenkai_types::LoggedChange;

use crate::edit_log::EditLog;

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
    pub fn exported_as(delimiter: Delimiter) -> CsvOptions {
        let CsvFormat {
            delimiter,
            encoding,
            line_ending,
        } = CsvFormat::export(delimiter);
        CsvOptions {
            delimiter,
            encoding,
            line_ending,
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
    // Cells that were moved in a way no inserted or deleted row or column explains: sorted,
    // cut and pasted, or too many operations to follow.
    pub moved_cells: bool,
}

impl Losses {
    pub fn any(self) -> bool {
        self.sheets || self.formulas || self.formatting || self.number_formats || self.moved_cells
    }

    // The kinds in `self` that `accepted` does not cover.
    pub fn beyond(self, accepted: Losses) -> Losses {
        Losses {
            sheets: self.sheets && !accepted.sheets,
            formulas: self.formulas && !accepted.formulas,
            formatting: self.formatting && !accepted.formatting,
            number_formats: self.number_formats && !accepted.number_formats,
            moved_cells: self.moved_cells && !accepted.moved_cells,
        }
    }

    pub fn with(self, other: Losses) -> Losses {
        Losses {
            sheets: self.sheets || other.sheets,
            formulas: self.formulas || other.formulas,
            formatting: self.formatting || other.formatting,
            number_formats: self.number_formats || other.number_formats,
            moved_cells: self.moved_cells || other.moved_cells,
        }
    }
}

// A document that is the CSV file itself. `options` is `None` until the file has been read for
// them; a save reads it then, and fails if it cannot, rather than write in a format guessed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvBinding {
    pub options: Option<CsvOptions>,
    // The sheet the file was read into, by the identity its place in the workbook does not
    // change. Only that sheet has fields of the file to write back.
    #[serde(default)]
    pub file_sheet: Option<u32>,
    #[serde(default)]
    pub accepted: Losses,
    #[serde(default)]
    pub introduced: Losses,
    // What was written, cleared or moved since the file was read or last saved, so a save knows
    // which cells are the user's and finds each of the others' own field.
    #[serde(default)]
    pub log: EditLog,
    // The engine's counters as last read. They restart with the workbook, so they are not kept.
    #[serde(skip)]
    pub seen_edits: FormatEdits,
}

impl CsvBinding {
    pub fn new(options: CsvOptions) -> CsvBinding {
        CsvBinding {
            options: Some(options),
            ..CsvBinding::unresolved()
        }
    }

    pub fn with_sheet(self, sheet: Option<u32>) -> CsvBinding {
        CsvBinding {
            file_sheet: sheet,
            ..self
        }
    }

    pub fn unresolved() -> CsvBinding {
        CsvBinding {
            options: None,
            file_sheet: None,
            accepted: Losses::default(),
            introduced: Losses::default(),
            log: EditLog::default(),
            seen_edits: FormatEdits::default(),
        }
    }

    // A batch of edits ended: what the engine counted and logged since the last one.
    pub fn note(&mut self, edits: FormatEdits, changes: Option<Vec<LoggedChange>>) {
        let seen = self.seen_edits;
        self.introduced.number_formats |= edits.numbers > seen.numbers;
        self.introduced.formatting |= edits.other > seen.other;
        self.introduced.moved_cells |= edits.moved > seen.moved;
        match changes {
            Some(changes) => {
                for (offset, change) in (1..).zip(changes) {
                    self.log.push(change, seen.seq + offset);
                }
            }
            None => self.log.lose(edits.seq),
        }
        self.seen_edits = edits;
    }

    // A save made the file what sheet `sheet` was when it read `consumed`. Edits since stay: a
    // change made while the file was written is still to be saved, whether or not a batch
    // had noted it by then.
    pub fn saved(&mut self, consumed: FormatEdits, sheet: u32) {
        self.file_sheet = Some(sheet);
        let seen = self.seen_edits;
        self.introduced.number_formats |= consumed.numbers > seen.numbers;
        self.introduced.formatting |= consumed.other > seen.other;
        if seen.moved <= consumed.moved {
            self.introduced.moved_cells = false;
        }
        self.log.clear_through(consumed.seq);
        self.seen_edits = FormatEdits {
            numbers: seen.numbers.max(consumed.numbers),
            other: seen.other.max(consumed.other),
            moved: seen.moved.max(consumed.moved),
            seq: seen.seq.max(consumed.seq),
        };
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
                        file_sheet: Some(7),
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
                        log: EditLog::default(),
                        seen_edits: FormatEdits::default(),
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
