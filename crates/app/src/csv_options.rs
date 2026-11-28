use serde::{Deserialize, Serialize};
use zenkai_engine::FormatEdits;
use zenkai_formats::{
    CsvFormat, DateOrder, DecimalMark, Delimiter, Encoding, Guess, LineEnding, ParsedCsv,
    TextColumns, find_at_risk, guess,
};
use zenkai_types::LoggedChange;

use crate::edit_log::EditLog;

// How a CSV was read, so that saving it writes it back the same way. The byte order mark is
// part of the encoding: UTF-8 with BOM and both UTF-16 forms have one, the others do not.
// `text` is the columns read as text, every field as written: a session saved before it
// existed has none, which is how such a file was read then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvOptions {
    pub delimiter: Delimiter,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub decimal_mark: DecimalMark,
    pub date_order: DateOrder,
    #[serde(default)]
    pub text: TextColumns,
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
            text: TextColumns::default(),
        }
    }

    // The file's own delimiter, encoding and line ending, with the number and date reading
    // that was guessed or that the user chose in its place, and no column read as text.
    pub fn from_read(parsed: &ParsedCsv, reading: &Guess) -> CsvOptions {
        CsvOptions {
            delimiter: parsed.delimiter,
            encoding: parsed.encoding,
            line_ending: parsed.line_ending,
            decimal_mark: reading.decimal_mark,
            date_order: reading.date_order,
            text: TextColumns::default(),
        }
    }

    // How a file is read when nothing was stored for it: what the preview would have guessed and
    // chosen by itself, so the same file opens the same way whichever way it was opened.
    pub fn detected(parsed: &ParsedCsv) -> CsvOptions {
        CsvOptions {
            text: TextColumns::chosen(find_at_risk(&parsed.rows).iter().map(|risk| risk.column)),
            ..CsvOptions::from_read(parsed, &guess(parsed))
        }
    }

    pub fn format(&self) -> CsvFormat {
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
    fn chosen_columns(columns: &[u16]) -> TextColumns {
        TextColumns::chosen(
            columns
                .iter()
                .filter_map(|column| zenkai_types::ColIdx::new(*column)),
        )
    }

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
                            text: chosen_columns(&[0, 2]),
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

    #[test]
    fn the_text_choices_survive_the_session_file_each_way() {
        for text in [
            TextColumns::default(),
            chosen_columns(&[0, 5, 16_383]),
            TextColumns::Everything,
        ] {
            let options = CsvOptions {
                text,
                ..CsvOptions::exported_as(Delimiter::Semicolon)
            };
            let binding = CsvBinding::new(options.clone());
            let json = serde_json::to_string(&binding).unwrap();
            let back: CsvBinding = serde_json::from_str(&json).unwrap();
            assert_eq!(back, binding);
            assert_eq!(back.options, Some(options));
        }
    }

    #[test]
    fn a_session_saved_before_the_text_choices_loads_with_none() {
        let options = CsvOptions::exported_as(Delimiter::Comma);
        let mut stored = serde_json::to_value(CsvBinding::new(options.clone())).unwrap();
        let removed = stored["options"]
            .as_object_mut()
            .and_then(|fields| fields.remove("text"));
        assert!(removed.is_some());
        let loaded: CsvBinding = serde_json::from_value(stored).unwrap();
        assert_eq!(loaded.options, Some(options));
        assert!(
            loaded
                .options
                .is_some_and(|options| options.text.is_empty())
        );
    }

    #[test]
    fn a_hand_edited_list_of_columns_is_sorted_and_cleaned_when_it_is_read() {
        let read: TextColumns = serde_json::from_str(r#"{"chosen":[9,2,2,40000,0]}"#).unwrap();
        assert_eq!(read, chosen_columns(&[0, 2, 9]));
        assert!(read.holds_field(9) && read.holds_field(0) && !read.holds_field(3));
    }

    #[test]
    fn a_file_read_with_nothing_stored_has_its_id_columns_as_text() {
        let parsed = zenkai_formats::parse_csv(b"name,code\nAna,00123\n", None).unwrap();
        let options = CsvOptions::detected(&parsed);
        assert_eq!(options.text, chosen_columns(&[1]));
        assert_eq!(options.delimiter, Delimiter::Comma);
    }
}
