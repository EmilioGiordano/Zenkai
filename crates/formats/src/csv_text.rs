use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use zenkai_types::ColIdx;

const LONG_NUMBER_DIGITS: usize = 16;
const EXAMPLE_CHARS: usize = 24;

// The columns a CSV import keeps as text, every field exactly as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Stored", into = "Stored")]
pub enum TextColumns {
    // Sorted and without repeats.
    Chosen(Vec<ColIdx>),
    Everything,
}

// How the choice is written in a session file. What it names is made a valid choice on the way
// in, so a file edited by hand cannot unsort the list that `holds` searches.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stored {
    Chosen(Vec<u16>),
    Everything,
}

impl From<Stored> for TextColumns {
    fn from(stored: Stored) -> TextColumns {
        match stored {
            Stored::Everything => TextColumns::Everything,
            Stored::Chosen(columns) => {
                TextColumns::chosen(columns.into_iter().filter_map(ColIdx::new))
            }
        }
    }
}

impl From<TextColumns> for Stored {
    fn from(columns: TextColumns) -> Stored {
        match columns {
            TextColumns::Everything => Stored::Everything,
            TextColumns::Chosen(columns) => {
                Stored::Chosen(columns.into_iter().map(ColIdx::get).collect())
            }
        }
    }
}

impl Default for TextColumns {
    fn default() -> Self {
        TextColumns::Chosen(Vec::new())
    }
}

// Fields past the last column of a sheet are never imported, so they belong to no column.
fn column_of(index: usize) -> Option<ColIdx> {
    u16::try_from(index).ok().and_then(ColIdx::new)
}

impl TextColumns {
    pub fn chosen(columns: impl IntoIterator<Item = ColIdx>) -> TextColumns {
        let mut columns: Vec<ColIdx> = columns.into_iter().collect();
        columns.sort_unstable();
        columns.dedup();
        TextColumns::Chosen(columns)
    }

    pub fn holds(&self, column: ColIdx) -> bool {
        match self {
            TextColumns::Everything => true,
            TextColumns::Chosen(columns) => columns.binary_search(&column).is_ok(),
        }
    }

    // Whether the field at `index` of a row is in a column read as text.
    pub fn holds_field(&self, index: usize) -> bool {
        match self {
            TextColumns::Everything => true,
            TextColumns::Chosen(columns) => {
                !columns.is_empty() && column_of(index).is_some_and(|column| self.holds(column))
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, TextColumns::Chosen(columns) if columns.is_empty())
    }
}

// A field that showed why its column is at risk: its text, cut for display, and its row
// counted from 1 as a sheet counts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Example {
    pub text: String,
    pub row: usize,
}

// A column with values that reading them as numbers would change: the first example of each
// reason it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtRisk {
    pub column: ColIdx,
    pub leading_zero: Option<Example>,
    pub long_number: Option<Example>,
}

struct Risks {
    leading_zero: bool,
    long_number: bool,
}

// Digits only: a sign, a decimal mark or an exponent makes a field a different kind of number,
// which the import keeps as Excel does. Spaces around the digits are dropped by the import
// when it reads a number, so they do not hide the field.
fn risks(field: &str) -> Risks {
    let digits = field.trim();
    let only_digits = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
    Risks {
        leading_zero: only_digits && digits.len() >= 2 && digits.starts_with('0'),
        long_number: only_digits && digits.len() >= LONG_NUMBER_DIGITS,
    }
}

fn example(field: &str, index: usize) -> Example {
    Example {
        text: field.chars().take(EXAMPLE_CHARS).collect(),
        row: index + 1,
    }
}

// Scans every row, so callers run it off the UI thread. Sorted by column.
pub fn find_at_risk(rows: &[Vec<String>]) -> Vec<AtRisk> {
    at_risk_among(rows.iter().enumerate().flat_map(|(index, row)| {
        row.iter()
            .enumerate()
            .map_while(move |(position, field)| Some((index, column_of(position)?, field.as_str())))
    }))
}

// The same over fields given with their row (counted from 0) and column, in row order.
pub fn at_risk_among<'a>(
    fields: impl IntoIterator<Item = (usize, ColIdx, &'a str)>,
) -> Vec<AtRisk> {
    let mut found: BTreeMap<ColIdx, AtRisk> = BTreeMap::new();
    for (index, column, field) in fields {
        let Risks {
            leading_zero,
            long_number,
        } = risks(field);
        if !leading_zero && !long_number {
            continue;
        }
        let at_risk = found.entry(column).or_insert_with(|| AtRisk {
            column,
            leading_zero: None,
            long_number: None,
        });
        if leading_zero && at_risk.leading_zero.is_none() {
            at_risk.leading_zero = Some(example(field, index));
        }
        if long_number && at_risk.long_number.is_none() {
            at_risk.long_number = Some(example(field, index));
        }
    }
    found.into_values().collect()
}

// What the engine is given for a field of a Text column: its apostrophe prefix makes the cell
// text, whatever the field looks like. An empty field stays empty.
pub fn text_input(field: &str) -> Cow<'_, str> {
    if field.is_empty() {
        Cow::Borrowed(field)
    } else {
        Cow::Owned(format!("'{field}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(index: u16) -> ColIdx {
        ColIdx::new(index).unwrap()
    }

    fn rows(lines: &[&[&str]]) -> Vec<Vec<String>> {
        lines
            .iter()
            .map(|line| line.iter().map(ToString::to_string).collect())
            .collect()
    }

    #[test]
    fn a_leading_zero_before_more_digits_flags_the_column() {
        for field in ["00123", "0123", "00", "01", " 0123 "] {
            let found = find_at_risk(&rows(&[&["id"], &[field]]));
            assert_eq!(found.len(), 1, "{field}");
            assert_eq!(found[0].column, col(0));
            assert_eq!(found[0].leading_zero.as_ref().unwrap().row, 2, "{field}");
        }
    }

    #[test]
    fn plain_numbers_decimals_exponents_and_signed_values_do_not_flag() {
        for field in [
            "0", "7", "123", "0.5", "00.5", "0,5", "-0012", "+0012", "1E5", "0e5", "12a", "0x10",
            "", "1 000", "$007",
        ] {
            assert_eq!(find_at_risk(&rows(&[&[field]])), [], "{field:?}");
        }
    }

    #[test]
    fn sixteen_digits_flag_and_fifteen_do_not() {
        assert_eq!(find_at_risk(&rows(&[&["123456789012345"]])), []);
        let found = find_at_risk(&rows(&[&["1234567890123456"]]));
        assert_eq!(found.len(), 1);
        assert!(found[0].leading_zero.is_none());
        assert_eq!(
            found[0].long_number,
            Some(Example {
                text: "1234567890123456".to_string(),
                row: 1
            })
        );
        let nineteen = find_at_risk(&rows(&[&["id"], &["9223372036854775807"]]));
        assert_eq!(nineteen[0].long_number.as_ref().unwrap().row, 2);
    }

    #[test]
    fn the_first_example_of_each_reason_is_kept_with_its_row() {
        let found = find_at_risk(&rows(&[
            &["code", "name"],
            &["12", "a"],
            &["007", "b"],
            &["0042", "c"],
            &["12345678901234567890", "d"],
            &["12345678901234567891", "e"],
        ]));
        assert_eq!(found.len(), 1);
        let column = &found[0];
        assert_eq!(column.column, col(0));
        assert_eq!(
            column.leading_zero,
            Some(Example {
                text: "007".to_string(),
                row: 3
            })
        );
        assert_eq!(
            column.long_number,
            Some(Example {
                text: "12345678901234567890".to_string(),
                row: 5
            })
        );
    }

    #[test]
    fn a_field_with_both_reasons_records_both_and_columns_come_sorted() {
        let found = find_at_risk(&rows(&[&["x", "0000000000000001"], &["0123", "y"]]));
        assert_eq!(
            found.iter().map(|c| c.column).collect::<Vec<_>>(),
            [col(0), col(1)]
        );
        assert!(found[1].leading_zero.is_some() && found[1].long_number.is_some());
    }

    #[test]
    fn an_example_is_cut_at_24_characters() {
        let long = "0".repeat(30);
        let found = find_at_risk(&rows(&[&[long.as_str()]]));
        assert_eq!(found[0].leading_zero.as_ref().unwrap().text, "0".repeat(24));
    }

    #[test]
    fn chosen_columns_are_sorted_without_repeats_and_everything_holds_all() {
        let chosen = TextColumns::chosen([col(3), col(1), col(3)]);
        assert_eq!(chosen, TextColumns::Chosen(vec![col(1), col(3)]));
        assert!(chosen.holds(col(1)) && chosen.holds(col(3)));
        assert!(!chosen.holds(col(0)) && !chosen.holds(col(2)));
        assert!(chosen.holds_field(3) && !chosen.holds_field(2) && !chosen.holds_field(70_000));
        assert!(
            TextColumns::Everything.holds(col(0)) && TextColumns::Everything.holds_field(70_000)
        );
        assert!(TextColumns::default().is_empty());
        assert!(!chosen.is_empty() && !TextColumns::Everything.is_empty());
    }

    #[test]
    fn a_text_field_is_given_to_the_engine_with_an_apostrophe_and_an_empty_one_is_not() {
        assert_eq!(text_input("00123"), "'00123");
        assert_eq!(text_input("'a"), "''a");
        assert_eq!(text_input(""), "");
    }

    #[test]
    fn fields_given_with_their_places_are_checked_like_the_rows_of_a_file() {
        let fields = [
            (3, col(2), "00123"),
            (4, col(2), "007"),
            (0, col(5), "x"),
            (6, col(0), "9223372036854775807"),
        ];
        let found = at_risk_among(fields);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].column, col(0));
        assert_eq!(found[0].long_number.as_ref().unwrap().row, 7);
        assert_eq!(found[1].column, col(2));
        assert_eq!(found[1].leading_zero.as_ref().unwrap().row, 4);
    }
}
