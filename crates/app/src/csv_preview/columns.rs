use std::collections::BTreeMap;

use zenkai_formats::{AtRisk, DateOrder, DecimalMark, number_value, read_field};
use zenkai_types::ColIdx;

pub const SAMPLE_ROWS: usize = 12;

const MIN_WIDTH: usize = 96;
const MAX_WIDTH: usize = 216;
const WIDTH_PER_CHARACTER: usize = 8;
const WIDTH_PADDING: usize = 24;
const CHARACTERS_THAT_FILL_A_COLUMN: usize = (MAX_WIDTH - WIDTH_PADDING) / WIDTH_PER_CHARACTER;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    General,
    Text,
}

impl Format {
    pub fn other(self) -> Format {
        match self {
            Format::General => Format::Text,
            Format::Text => Format::General,
        }
    }
}

// What the preview knows of one column of the file, from its first rows alone.
pub struct Column {
    pub letter: String,
    // Its row 1 value, when that is not a number.
    pub name: Option<String>,
    pub risk: Option<AtRisk>,
    // The raw fields of the sample rows, as the file has them.
    pub sample: Vec<String>,
    // In pixels at the interface scale of 100 percent.
    pub width: f32,
}

pub fn letter_of(index: usize) -> String {
    u16::try_from(index)
        .ok()
        .and_then(ColIdx::new)
        .map_or_else(|| (index + 1).to_string(), ColIdx::letters)
}

fn named_by(first_row: &str) -> Option<String> {
    let name = first_row.trim();
    (!name.is_empty() && number_value(name).is_none()).then(|| name.to_string())
}

// Wide enough for the longest of the sample rows, never moved by what a format shows.
fn width_of(sample: &[String]) -> f32 {
    let longest = sample
        .iter()
        .map(|field| field.chars().take(CHARACTERS_THAT_FILL_A_COLUMN).count())
        .max()
        .unwrap_or(0);
    (longest * WIDTH_PER_CHARACTER + WIDTH_PADDING).clamp(MIN_WIDTH, MAX_WIDTH) as f32
}

pub fn columns_of(rows: &[Vec<String>], count: usize, at_risk: Vec<AtRisk>) -> Vec<Column> {
    let mut risks: BTreeMap<usize, AtRisk> = at_risk
        .into_iter()
        .map(|risk| (risk.column, risk))
        .collect();
    let sample_rows = &rows[..rows.len().min(SAMPLE_ROWS)];
    (0..count)
        .map(|index| {
            let sample: Vec<String> = sample_rows
                .iter()
                .map(|row| row.get(index).cloned().unwrap_or_default())
                .collect();
            Column {
                letter: letter_of(index),
                name: sample.first().and_then(|first| named_by(first)),
                risk: risks.remove(&index),
                width: width_of(&sample),
                sample,
            }
        })
        .collect()
}

// The first column the import would have kept as text by itself, else the first.
pub fn first_selected(columns: &[Column]) -> usize {
    columns
        .iter()
        .position(|column| column.risk.is_some())
        .unwrap_or(0)
}

pub fn detected_formats(columns: &[Column]) -> Vec<Format> {
    columns
        .iter()
        .map(|column| {
            if column.risk.is_some() {
                Format::Text
            } else {
                Format::General
            }
        })
        .collect()
}

// "00123" read as a number is 123, which is what a General column shows of it.
fn without_leading_zeros(text: &str) -> Option<String> {
    let digits = text.trim();
    let zeros = digits.len() >= 2
        && digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    zeros.then(|| match digits.trim_start_matches('0') {
        "" => "0".to_string(),
        rest => rest.to_string(),
    })
}

// What a General column shows of a field: the text the import would read it as, as far as
// that is a cheap call. A number long enough to be rounded shows as written.
pub fn general_text(
    raw: &str,
    decimal_mark: DecimalMark,
    date_order: DateOrder,
    holds_risk: bool,
) -> String {
    let read = read_field(decimal_mark, date_order, raw).unwrap_or_else(|| raw.to_string());
    if holds_risk {
        without_leading_zeros(&read).unwrap_or(read)
    } else {
        read
    }
}

pub fn is_number(shown: &str) -> bool {
    number_value(shown).is_some()
}

#[cfg(test)]
mod tests {
    use zenkai_formats::find_at_risk;

    use super::*;

    fn rows(lines: &[&[&str]]) -> Vec<Vec<String>> {
        lines
            .iter()
            .map(|line| line.iter().map(ToString::to_string).collect())
            .collect()
    }

    fn built(lines: &[&[&str]]) -> Vec<Column> {
        let rows = rows(lines);
        let count = rows.iter().map(Vec::len).max().unwrap_or(0);
        columns_of(&rows, count, find_at_risk(&rows))
    }

    #[test]
    fn a_column_is_named_by_its_first_row_unless_that_is_a_number() {
        let columns = built(&[
            &["Customer ID", "2026", "", " Amount "],
            &["00123", "1", "2", "3"],
        ]);
        assert_eq!(columns[0].name.as_deref(), Some("Customer ID"));
        assert_eq!(columns[1].name, None);
        assert_eq!(columns[2].name, None);
        assert_eq!(columns[3].name.as_deref(), Some("Amount"));
        assert_eq!(columns[0].letter, "A");
        assert_eq!(columns[3].letter, "D");
    }

    #[test]
    fn a_width_comes_from_the_longest_raw_text_of_the_sample_and_stays_within_bounds() {
        let long = "x".repeat(300);
        let columns = built(&[
            &["Customer name", "id", &long],
            &["Lucia Fernandez", "1", "y"],
        ]);
        assert_eq!(columns[0].width, 144.0);
        assert_eq!(columns[1].width, 96.0);
        assert_eq!(columns[2].width, 216.0);
    }

    #[test]
    fn the_sample_is_the_first_twelve_rows_and_short_rows_read_as_empty() {
        let mut lines: Vec<Vec<&str>> = vec![vec!["a", "b"]];
        lines.extend((0..20).map(|_| vec!["x"]));
        let rows: Vec<Vec<String>> = lines
            .iter()
            .map(|line| line.iter().map(ToString::to_string).collect())
            .collect();
        let columns = columns_of(&rows, 2, Vec::new());
        assert_eq!(columns[0].sample.len(), SAMPLE_ROWS);
        assert_eq!(columns[1].sample[0], "b");
        assert_eq!(columns[1].sample[1], "");
    }

    #[test]
    fn flagged_columns_start_as_text_and_the_first_of_them_is_selected() {
        let columns = built(&[&["a", "id", "n"], &["x", "007", "5"]]);
        assert_eq!(
            detected_formats(&columns),
            [Format::General, Format::Text, Format::General]
        );
        assert_eq!(first_selected(&columns), 1);
        let none = built(&[&["a", "b"], &["x", "5"]]);
        assert_eq!(first_selected(&none), 0);
    }

    #[test]
    fn a_general_column_shows_the_leading_zeros_dropped_only_where_it_holds_risk() {
        let (dot, month) = (DecimalMark::Dot, DateOrder::MonthFirst);
        assert_eq!(general_text("00123", dot, month, true), "123");
        assert_eq!(general_text("000", dot, month, true), "0");
        assert_eq!(general_text("00123", dot, month, false), "00123");
        assert_eq!(general_text("Ana", dot, month, true), "Ana");
        assert_eq!(
            general_text("8300123456789012345", dot, month, true),
            "8300123456789012345"
        );
        assert_eq!(
            general_text("1,50", DecimalMark::Comma, month, false),
            "1.50"
        );
    }

    #[test]
    fn a_column_past_the_last_letter_has_a_number_for_a_name() {
        assert_eq!(letter_of(0), "A");
        assert_eq!(letter_of(26), "AA");
        assert_eq!(letter_of(20_000), "20001");
    }
}
