use zenkai_formats::{AtRisk, DateOrder, DecimalMark};
use zenkai_i18n::t;

use super::columns::{Column, Format, general_text};

const NOTE_NAME_CHARACTERS: usize = 20;
const NOTE_ENTRIES: usize = 2;

pub fn format_label(format: Format) -> &'static str {
    match format {
        Format::General => t!("csv.format.general"),
        Format::Text => t!("csv.format.text"),
    }
}

pub fn keep_all_hint(keep_all: bool) -> &'static str {
    if keep_all {
        t!("csv.keep_all.hint_on")
    } else {
        t!("csv.keep_all.hint")
    }
}

pub fn footer_hint(keep_all: bool, has_columns: bool) -> &'static str {
    if !has_columns {
        t!("csv.hint.empty")
    } else if keep_all {
        t!("csv.hint.keep_all")
    } else {
        t!("csv.hint")
    }
}

fn cut(name: &str) -> String {
    if name.chars().count() > NOTE_NAME_CHARACTERS {
        let head: String = name.chars().take(NOTE_NAME_CHARACTERS).collect();
        format!("{head}\u{2026}")
    } else {
        name.to_string()
    }
}

// Why the import keeps a column as text by itself, as the note lists it: every reason it has.
fn short_reasons(risk: &AtRisk) -> String {
    let mut reasons = Vec::new();
    if risk.leading_zero.is_some() {
        reasons.push(t!("csv.note.reason_zero"));
    }
    if risk.long_number.is_some() {
        reasons.push(t!("csv.note.reason_long"));
    }
    reasons.join(", ")
}

pub struct NoteEntry {
    pub column: usize,
    pub text: String,
}

pub enum Note {
    Sentence(&'static str),
    Flagged {
        entries: Vec<NoteEntry>,
        more: usize,
    },
}

pub fn note(columns: &[Column], formats: &[Format], keep_all: bool) -> Note {
    if columns.is_empty() {
        return Note::Sentence(t!("csv.note.empty"));
    }
    if keep_all {
        return Note::Sentence(t!("csv.note.keep_all"));
    }
    let flagged: Vec<(usize, &Column, &AtRisk)> = columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| column.risk.as_ref().map(|risk| (index, column, risk)))
        .collect();
    if flagged.is_empty() {
        return Note::Sentence(t!("csv.note.none"));
    }
    let entries = flagged
        .iter()
        .take(NOTE_ENTRIES)
        .map(|(index, column, risk)| {
            let mut reason = short_reasons(risk);
            if formats.get(*index) == Some(&Format::General) {
                reason = t!("csv.note.now_general", reason = reason);
            }
            let text = match &column.name {
                Some(name) => t!(
                    "csv.note.entry",
                    letter = &column.letter,
                    name = cut(name),
                    reason = reason
                ),
                None => t!(
                    "csv.note.entry_unnamed",
                    letter = &column.letter,
                    reason = reason
                ),
            };
            NoteEntry {
                column: *index,
                text,
            }
        })
        .collect();
    Note::Flagged {
        entries,
        more: flagged.len().saturating_sub(NOTE_ENTRIES),
    }
}

pub fn note_intro(note: &Note) -> &'static str {
    match note {
        Note::Sentence(sentence) => sentence,
        Note::Flagged { .. } => t!("csv.note.intro"),
    }
}

pub struct Panel {
    pub title: String,
    pub reason: String,
    pub warns: bool,
}

fn auto_part(risk: &AtRisk) -> String {
    let zero = risk.leading_zero.as_ref().map(|example| {
        t!(
            "csv.reason.auto_zero",
            example = &example.text,
            row = example.row
        )
    });
    let long = risk.long_number.as_ref().map(|example| {
        t!(
            "csv.reason.auto_long",
            example = &example.text,
            row = example.row
        )
    });
    let reasons = match (zero, long) {
        (Some(first), Some(second)) => t!("csv.reason.and", first = first, second = second),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => String::new(),
    };
    t!("csv.reason.auto", reasons = reasons)
}

fn warning(risk: &AtRisk) -> String {
    let mut what = Vec::new();
    if let Some(example) = &risk.leading_zero {
        let converted = general_text(&example.text, DecimalMark::Dot, DateOrder::MonthFirst, true);
        what.push(t!(
            "csv.warning.zero",
            example = &example.text,
            converted = converted
        ));
    }
    if let Some(example) = &risk.long_number {
        what.push(t!("csv.warning.long", example = &example.text));
    }
    t!("csv.warning", what = what.join(" "))
}

fn title_of(letter: &str, name: Option<&String>) -> String {
    match name {
        Some(name) => t!("csv.panel.title", letter = letter, name = name),
        None => t!("csv.panel.title_unnamed", letter = letter),
    }
}

pub fn panel(column: Option<&Column>, format: Format, keep_all: bool) -> Panel {
    let Some(column) = column else {
        return Panel {
            title: t!("csv.panel.none").to_string(),
            reason: t!("csv.reason.empty").to_string(),
            warns: false,
        };
    };
    let title = title_of(&column.letter, column.name.as_ref());
    let (reason, warns) = match (&column.risk, format, keep_all) {
        (_, _, true) => (t!("csv.reason.keep_all").to_string(), false),
        (Some(risk), Format::Text, false) => (auto_part(risk), false),
        (Some(risk), Format::General, false) => (warning(risk), true),
        (None, Format::Text, false) => (t!("csv.reason.text_chosen").to_string(), false),
        (None, Format::General, false) => (t!("csv.reason.general").to_string(), false),
    };
    Panel {
        title,
        reason,
        warns,
    }
}

// What a screen reader says of a column header, in the order the header is read.
pub fn header_label(column: &Column, format: Format, keep_all: bool, auto: bool) -> String {
    let format = format_label(format);
    let mut label = match &column.name {
        Some(name) => t!(
            "csv.aria.column_named",
            letter = &column.letter,
            name = name,
            format = format
        ),
        None => t!("csv.aria.column", letter = &column.letter, format = format),
    };
    if auto
        && !keep_all
        && let Some(risk) = &column.risk
    {
        label = t!("csv.aria.auto", label = label, reason = short_reasons(risk));
    }
    label
}

#[cfg(test)]
mod tests {
    use zenkai_formats::find_at_risk;

    use super::super::columns::{columns_of, detected_formats};
    use super::*;

    fn built(lines: &[&[&str]]) -> (Vec<Column>, Vec<Format>) {
        let rows: Vec<Vec<String>> = lines
            .iter()
            .map(|line| line.iter().map(ToString::to_string).collect())
            .collect();
        let count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let columns = columns_of(&rows, count, find_at_risk(&rows));
        let formats = detected_formats(&columns);
        (columns, formats)
    }

    #[test]
    fn the_note_lists_two_flagged_columns_and_counts_the_rest() {
        let (columns, formats) = built(&[
            &[
                "Customer ID",
                "x",
                "Tracking",
                "A very long column name indeed",
                "e",
            ],
            &["00123", "a", "8300123456789012345", "007", "0099"],
        ]);
        let Note::Flagged { entries, more } = note(&columns, &formats, false) else {
            panic!("flagged columns are listed");
        };
        assert_eq!(more, 2);
        assert_eq!(entries[0].column, 0);
        assert_eq!(entries[0].text, "A Customer ID (starts with 0)");
        assert_eq!(entries[1].text, "C Tracking (more than 15 digits)");
    }

    #[test]
    fn a_name_is_cut_at_twenty_characters_and_a_column_without_one_drops_it() {
        let (columns, formats) = built(&[&["Tax registration number", "42"], &["0001", "0002"]]);
        let Note::Flagged { entries, .. } = note(&columns, &formats, false) else {
            panic!("flagged columns are listed");
        };
        assert_eq!(
            entries[0].text,
            "A Tax registration num\u{2026} (starts with 0)"
        );
        assert_eq!(entries[1].text, "B (starts with 0)");
    }

    #[test]
    fn a_column_switched_to_general_says_so_in_the_note() {
        let (columns, mut formats) = built(&[&["id"], &["007"]]);
        formats[0] = Format::General;
        let Note::Flagged { entries, .. } = note(&columns, &formats, false) else {
            panic!("flagged columns are listed");
        };
        assert_eq!(entries[0].text, "A id (starts with 0, now General)");
    }

    #[test]
    fn the_note_is_a_sentence_when_nothing_is_flagged_the_file_is_empty_or_everything_is_text() {
        let (columns, formats) = built(&[&["a"], &["5"]]);
        assert!(matches!(
            note(&columns, &formats, false),
            Note::Sentence(text) if text.starts_with("No column needed Text")
        ));
        assert!(matches!(
            note(&[], &[], false),
            Note::Sentence(text) if text.starts_with("This file is empty")
        ));
        assert!(matches!(
            note(&columns, &formats, true),
            Note::Sentence(text) if text.starts_with("Every column opens as text")
        ));
    }

    #[test]
    fn the_panel_gives_the_reason_for_each_state_of_a_column() {
        let (columns, _) = built(&[&["Customer ID", "n"], &["00123", "5"]]);
        let flagged = &columns[0];
        let plain = &columns[1];

        let auto = panel(Some(flagged), Format::Text, false);
        assert_eq!(auto.title, "Column A: Customer ID");
        assert_eq!(
            auto.reason,
            "Set to Text automatically because values start with 0, for example 00123 (row 2)."
        );
        assert!(!auto.warns);

        let general = panel(Some(flagged), Format::General, false);
        assert!(general.warns);
        assert_eq!(
            general.reason,
            "Warning: General turns 00123 into 123 and drops every leading zero. Choose Text to keep the values exactly as written."
        );

        assert_eq!(panel(Some(plain), Format::Text, false).title, "Column B");
        assert!(
            panel(Some(plain), Format::Text, false)
                .reason
                .starts_with("Every value in this column stays")
        );
        assert!(
            panel(Some(plain), Format::General, false)
                .reason
                .starts_with("Numbers, dates and percentages")
        );
        assert!(
            panel(Some(flagged), Format::Text, true)
                .reason
                .starts_with("Keep everything as text is on")
        );
        assert_eq!(panel(None, Format::General, false).title, "No column");
    }

    #[test]
    fn two_reasons_are_joined_and_a_long_number_warns_of_rounding() {
        let (columns, _) = built(&[&["id"], &["007"], &["12345678901234567890"]]);
        let both = panel(Some(&columns[0]), Format::Text, false);
        assert_eq!(
            both.reason,
            "Set to Text automatically because values start with 0, for example 007 (row 2), and values have more than 15 digits, for example 12345678901234567890 (row 3)."
        );
        let warned = panel(Some(&columns[0]), Format::General, false);
        assert!(warned.reason.contains("General turns 007 into 7"));
        assert!(
            warned
                .reason
                .contains("General rounds 12345678901234567890 to 15 digits.")
        );
    }

    #[test]
    fn the_header_is_read_with_its_letter_name_format_and_reason() {
        let (columns, _) = built(&[&["Customer ID", "n"], &["00123", "5"]]);
        assert_eq!(
            header_label(&columns[0], Format::Text, false, true),
            "Column A, Customer ID, Text, set automatically because starts with 0"
        );
        assert_eq!(
            header_label(&columns[1], Format::General, false, false),
            "Column B, General"
        );
        assert_eq!(
            header_label(&columns[0], Format::Text, true, true),
            "Column A, Customer ID, Text"
        );
    }

    #[test]
    fn the_footer_hint_drops_what_the_state_cannot_do() {
        assert!(footer_hint(false, true).contains("Space: switch Text and General"));
        assert_eq!(
            footer_hint(true, true),
            "Left and Right: choose a column. Enter: Open. Esc: Cancel."
        );
        assert_eq!(footer_hint(false, false), "Enter: Open. Esc: Cancel.");
    }
}
